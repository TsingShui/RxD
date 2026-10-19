//! Whitelisted typed calls to the unchanged native Kuna producer, worker-only.
use super::kuna_dto::{Decompilation, FunctionInventory, Nullable};
use super::{CapturedInput, QueryBudgets, QueryError, QueryResult, catalog::ToolDefinition};
use crate::mcp::targets::{InputRef, PathInput, TargetInput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const NATIVE_MAX_INSTRUCTIONS: usize = 100_000;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FunctionsArgs {
    input: InputRef,
    limits: Option<FunctionsLimits>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FunctionsLimits {
    #[schemars(range(min = 1))]
    input_bytes: Option<usize>,
    #[schemars(range(min = 1))]
    output_bytes: Option<usize>,
    #[schemars(range(min = 1))]
    records: Option<usize>,
    #[schemars(range(min = 1))]
    entities: Option<usize>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DecompileLimits {
    #[schemars(range(min = 1))]
    input_bytes: Option<usize>,
    #[schemars(range(min = 1))]
    output_bytes: Option<usize>,
    #[schemars(range(min = 1))]
    records: Option<usize>,
    #[schemars(range(min = 1))]
    instructions: Option<usize>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NamedArgs {
    input: InputRef,
    #[schemars(length(min = 1, max = 4096))]
    name: String,
    limits: Option<DecompileLimits>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddressArgs {
    input: InputRef,
    #[schemars(regex(pattern = "^0[xX][0-9a-fA-F]{1,16}$"))]
    address: String,
    limits: Option<DecompileLimits>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
enum DecompileArgs {
    Name(NamedArgs),
    Address(AddressArgs),
}
#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ErrorBody<T> {
    code: String,
    message: String,
    candidates: Nullable<T>,
}
#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NativeError<T> {
    error: ErrorBody<T>,
}
#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
enum NativeOutput<T> {
    Native(T),
    Error(NativeError<T>),
}

fn object_schema<T: JsonSchema>() -> Value {
    let mut schema = schemars::schema_for!(T).to_value();
    schema["type"] = Value::String("object".into());
    schema
}

pub fn catalog() -> Vec<ToolDefinition> {
    let version = rxd_kuna_cli::engine_version();
    vec![
        ToolDefinition {
            name: "kuna_functions".into(),
            backend: "kuna".into(),
            description: format!(
                "Enumerate all native Kuna function entries with aliases, addresses and object locations. Linked Kuna {version} (pinned a219af9); native JSON, no content-bound entity IDs. Use returned address_hex or a unique name for kuna_decompile. Adapter coverage: full enumeration and single-function JSON only; no search/xref/export/external engines."
            ),
            input_schema: object_schema::<FunctionsArgs>(),
            output_schema: object_schema::<NativeOutput<FunctionInventory>>(),
            capabilities: vec!["ENUMERATE".into()],
        },
        ToolDefinition {
            name: "kuna_decompile".into(),
            backend: "kuna".into(),
            description: format!(
                "Fetch one native Kuna function as declared native JSON with code, language, errors, quality, line mappings, variables and types. Linked Kuna {version} (pinned a219af9). Select exactly one unique name or returned address_hex. Names containing commas, surrounding whitespace, a leading '-' or native 0x address syntax require the hex address selector. Instruction budget failures are explicit, never successful truncated source."
            ),
            input_schema: object_schema::<DecompileArgs>(),
            output_schema: object_schema::<NativeOutput<Decompilation>>(),
            capabilities: vec!["FETCH".into()],
        },
    ]
}

pub fn validate(name: &str, arguments: &Value) -> Result<(), QueryError> {
    match name {
        "kuna_functions" => {
            let args: FunctionsArgs = decode(arguments)?;
            validate_input(&args.input)?;
            if let Some(limits) = args.limits {
                positive([
                    limits.input_bytes,
                    limits.output_bytes,
                    limits.records,
                    limits.entities,
                ])?;
            }
        }
        "kuna_decompile" => {
            let args: DecompileArgs = decode(arguments)?;
            let limits = match args {
                DecompileArgs::Name(args) => {
                    validate_input(&args.input)?;
                    if args.name.is_empty()
                        || args.name.len() > 4096
                        || args.name.contains(['\0', ','])
                        || args.name.starts_with('-')
                        || args.name.starts_with("0x")
                        || args.name.starts_with("0X")
                        || args.name.trim() != args.name
                    {
                        return Err(QueryError::new(
                            "INVALID_QUERY",
                            "native name must be one unambiguous non-flag selector; use returned address_hex for special names",
                        ));
                    }
                    args.limits
                }
                DecompileArgs::Address(args) => {
                    validate_input(&args.input)?;
                    let digits = args
                        .address
                        .strip_prefix("0x")
                        .or_else(|| args.address.strip_prefix("0X"));
                    if digits.is_none_or(|digits| {
                        digits.is_empty()
                            || digits.len() > 16
                            || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
                    }) {
                        return Err(QueryError::new(
                            "INVALID_QUERY",
                            "native address must be exact 0x-prefixed hexadecimal (1..16 digits)",
                        ));
                    }
                    args.limits
                }
            };
            if let Some(limits) = limits {
                positive([
                    limits.input_bytes,
                    limits.output_bytes,
                    limits.records,
                    limits.instructions,
                ])?;
            }
        }
        _ => {
            return Err(QueryError::new(
                "INVALID_QUERY",
                "unknown native analysis tool",
            ));
        }
    }
    Ok(())
}

fn positive<const N: usize>(limits: [Option<usize>; N]) -> Result<(), QueryError> {
    if limits.into_iter().flatten().any(|limit| limit == 0) {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "native request limits must be positive",
        ));
    }
    Ok(())
}
fn validate_input(input: &InputRef) -> Result<(), QueryError> {
    let value = match input {
        InputRef::Path(PathInput { path }) => path
            .to_str()
            .ok_or_else(|| QueryError::new("INVALID_QUERY", "native input path must be UTF-8"))?,
        InputRef::Target(TargetInput { target_id }) => target_id,
    };
    if value.is_empty() || value.contains('\0') {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "native input reference must be nonempty and NUL-free",
        ));
    }
    Ok(())
}
fn decode<T: for<'de> Deserialize<'de>>(arguments: &Value) -> Result<T, QueryError> {
    serde_json::from_value(arguments.clone())
        .map_err(|error| QueryError::new("INVALID_QUERY", error.to_string()))
}

pub fn execute(
    name: &str,
    arguments: &Value,
    input: &CapturedInput,
    budgets: QueryBudgets,
) -> Result<QueryResult, QueryError> {
    let input_bytes = input.input_bytes.map_or_else(
        || {
            std::fs::metadata(&input.path)
                .map(|metadata| metadata.len())
                .map_err(|error| QueryError::new("IO_ERROR", error.to_string()))
        },
        |bytes| Ok(bytes as u64),
    )?;
    if input_bytes > budgets.input_bytes as u64 {
        return Err(QueryError::new(
            "RESOURCE_LIMIT",
            "native input byte budget exceeded",
        ));
    }
    let path = input
        .path
        .to_str()
        .ok_or_else(|| QueryError::new("INVALID_QUERY", "native input path must be UTF-8"))?;
    let logical = input.original_path.to_str().ok_or_else(|| {
        QueryError::new("INVALID_QUERY", "native logical input path must be UTF-8")
    })?;
    let instructions = budgets.instructions.min(NATIVE_MAX_INSTRUCTIONS);
    match name {
        "kuna_functions" => {
            let cap = budgets.records.min(budgets.entities);
            let args = vec![
                "functions".into(),
                path.into(),
                "--json".into(),
                "--jobs".into(),
                "1".into(),
                "--option".into(),
                "maxinstruction".into(),
                instructions.to_string(),
                "--limit".into(),
                cap.saturating_add(1).to_string(),
            ];
            let (status, bytes) = run(args, budgets.output_bytes)?;
            let mut document: FunctionInventory = parse(&bytes, status)?;
            document.binary = logical.to_owned();
            restore_error(&mut document.error.0, path, logical);
            if document.count != document.functions.len() as u64 {
                return Err(QueryError::new(
                    "WORKER_ERROR",
                    "native function count differs from emitted records",
                ));
            }
            if document.total > cap as u64 || document.count > cap as u64 {
                return Err(QueryError::new(
                    "RESOURCE_LIMIT",
                    "native function record/entity budget exceeded",
                ));
            }
            if let Some(message) = document.error.0.clone() {
                return failed("ANALYSIS_ERROR", message, document);
            }
            if status != 0 {
                return Err(native_status(status));
            }
            if document.count != document.total {
                return Err(QueryError::new(
                    "WORKER_ERROR",
                    "native enumeration unexpectedly narrowed the requested full scope",
                ));
            }
            answered(document)
        }
        "kuna_decompile" => {
            let selection: DecompileArgs = decode(arguments)?;
            let (target, address) = match selection {
                DecompileArgs::Name(args) => (args.name, false),
                DecompileArgs::Address(args) => (args.address, true),
            };
            let mut args = vec![
                "decompile".into(),
                path.into(),
                target,
                "--json".into(),
                "--option".into(),
                "maxinstruction".into(),
                instructions.to_string(),
                "--option".into(),
                "errortoomanyinstructions".into(),
                "on".into(),
            ];
            if address {
                args.push("--addr".into());
            }
            let (status, bytes) = run(args, budgets.output_bytes)?;
            let mut document: Decompilation = parse(&bytes, status)?;
            document.binary = logical.to_owned();
            restore_error(&mut document.error.0, path, logical);
            for function in &mut document.functions {
                restore_error(&mut function.error.0, path, logical);
            }
            if document.count != document.functions.len() as u64 {
                return Err(QueryError::new(
                    "WORKER_ERROR",
                    "native single-function response has an inconsistent selection",
                ));
            }
            if document.count > 1 {
                return failed(
                    "AMBIGUOUS",
                    "native name selected multiple functions; use returned address_hex".into(),
                    document,
                );
            }
            if document.count > budgets.records as u64 {
                return Err(QueryError::new(
                    "RESOURCE_LIMIT",
                    "native function record budget exceeded",
                ));
            }
            let failure = document.error.0.clone().or_else(|| {
                document
                    .functions
                    .iter()
                    .find_map(|function| function.error.0.clone())
            });
            if let Some(message) = failure {
                let code = if message.contains("Flow exceeded maximum allowable instructions") {
                    "RESOURCE_LIMIT"
                } else if message.contains(" is ambiguous; candidates:") {
                    "AMBIGUOUS"
                } else {
                    "ANALYSIS_ERROR"
                };
                return failed(code, message, document);
            }
            if status != 0 {
                return Err(native_status(status));
            }
            if document.count != 1 {
                return Err(QueryError::new(
                    "WORKER_ERROR",
                    "native selected function produced no record",
                ));
            }
            answered(document)
        }
        _ => Err(QueryError::new(
            "INVALID_QUERY",
            "unknown native analysis tool",
        )),
    }
}
fn run(args: Vec<String>, limit: usize) -> Result<(i32, Vec<u8>), QueryError> {
    crate::cli::kuna::run_mcp_captured(args, limit).map_err(|error| {
        QueryError::new(
            if error.message.contains("byte budget exceeded") {
                "RESOURCE_LIMIT"
            } else {
                "KUNA_ERROR"
            },
            error.message,
        )
    })
}
fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8], status: i32) -> Result<T, QueryError> {
    if bytes.is_empty() && status != 0 {
        return Err(native_status(status));
    }
    serde_json::from_slice(bytes).map_err(|error| {
        QueryError::new(
            "WORKER_ERROR",
            format!("native Kuna emitted no valid declared JSON (status {status}): {error}"),
        )
    })
}
fn native_status(status: i32) -> QueryError {
    QueryError::new(
        "ANALYSIS_ERROR",
        format!("native Kuna failed with status {status} before a complete declared response"),
    )
}
fn restore_error(error: &mut Option<String>, physical: &str, logical: &str) {
    if let Some(error) = error {
        *error = error.replace(physical, logical);
    }
}
fn answered<T: Serialize>(document: T) -> Result<QueryResult, QueryError> {
    Ok(QueryResult {
        structured: serde_json::to_value(NativeOutput::Native(document)).map_err(json_error)?,
        artifact: None,
    })
}
fn failed<T: Serialize>(
    code: &str,
    message: String,
    document: T,
) -> Result<QueryResult, QueryError> {
    let outcome = NativeOutput::Error(NativeError {
        error: ErrorBody {
            code: code.into(),
            message,
            candidates: Nullable(Some(document)),
        },
    });
    let envelope = serde_json::to_value(outcome).map_err(json_error)?;
    Err(serde_json::from_value(envelope["error"].clone()).map_err(json_error)?)
}
fn json_error(error: serde_json::Error) -> QueryError {
    QueryError::new("WORKER_ERROR", error.to_string())
}
