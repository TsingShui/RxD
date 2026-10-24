use std::{collections::BTreeMap, path::PathBuf};

use super::filter::CaseArgs;
use super::xref::Selector;
use clap::{Args, Subcommand};
use rxd_core::envelope::{self, RecordMeta};
use rxd_core::perf::Perf;
use rxd_core::{Budget, Result};
use rxd_dotnet::{Input, Limits};
use serde_json::{Map, Value, json};

#[derive(Subcommand)]
pub enum Command {
    /// List the types an assembly defines as JSONL.
    Classes {
        input: PathBuf,
        /// Exact full type name.
        #[arg(long)]
        class: Option<String>,
        ///  substring over the full type name. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// List the fields an assembly defines as JSONL.
    Fields {
        input: PathBuf,
        /// Exact full type name.
        #[arg(long)]
        class: Option<String>,
        /// Exact field name; not a substring.
        #[arg(long)]
        name: Option<String>,
        ///  substring over the field name. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// List the methods an assembly defines as JSONL.
    Methods {
        input: PathBuf,
        /// Exact full type name.
        #[arg(long)]
        class: Option<String>,
        /// Exact method name; not a substring.
        #[arg(long)]
        name: Option<String>,
        ///  substring over the method name. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Get one method as conservative code or original instructions.
    #[command(name = "get-method")]
    Get {
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        /// Print the original instruction listing instead of reconstructed source.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        options: Options,
    },
    /// Fetch a selected type and every recursively owned nested type.
    #[command(name = "get-class")]
    GetClass {
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        /// Print the original instruction listing instead of reconstructed source.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        options: Options,
    },
    /// Query decoded user-string heap literals without executing input.
    Strings {
        input: PathBuf,
        ///  substring over the decoded literal. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Find exact/semantic member and type operands or decoded user-string loads.
    Xref {
        input: PathBuf,
        /// Exact target: the metadata token of a member or type, such as `0x0a00001f`; it
        /// also covers external members, generic instantiations and `#US` string tokens
        /// (`0x70…`).
        #[arg(long, value_name = "TOKEN", conflicts_with = "filter")]
        at: Option<String>,
        /// Substring of a decoded user-string literal; folded unless `--case sensitive`.
        #[arg(short = 'm', long = "match", value_name = "PATTERN")]
        filter: Option<String>,
        /// Target kind: read off the token's table when it names one; required with
        /// `--match`, which can only search literals.
        #[arg(long, value_enum)]
        kind: Option<Kind>,
        /// Whether `-f` folds case: `insensitive` (default) or `sensitive`.
        #[arg(long, value_enum, default_value_t = crate::cli::filter::CaseMode::Insensitive)]
        case: crate::cli::filter::CaseMode,

        #[command(flatten)]
        options: Options,
    },
}

/// Which kind of target a reference query names.
///
/// The kind is read off a token's table, so it is only needed where the token cannot say:
/// a `--match` content search, and asserting a table that covers two kinds (`MemberRef`).
type Kind = crate::cli::xref::Kind;

#[derive(Args)]
pub struct Options {
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    #[arg(long)]
    perf: bool,
}

impl Options {
    /// The shared budget table, projected onto this backend's limits.
    fn limits(&self, budget: Budget) -> Limits {
        Limits {
            input_bytes: budget.input_bytes,
            work: budget.work,
            instructions: budget.instructions,
            metadata_bytes: budget.metadata_bytes,
            output_bytes: budget.output_bytes,
        }
    }
}

impl Command {
    fn options(&self) -> &Options {
        match self {
            Self::Classes { options, .. }
            | Self::Fields { options, .. }
            | Self::Methods { options, .. }
            | Self::Get { options, .. }
            | Self::GetClass { options, .. }
            | Self::Strings { options, .. }
            | Self::Xref { options, .. } => options,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Classes { .. } => "classes",
            Self::Fields { .. } => "fields",
            Self::Methods { .. } => "methods",
            Self::Get { .. } => "get-method",
            Self::GetClass { .. } => "get-class",
            Self::Strings { .. } => "strings",
            Self::Xref { .. } => "xref",
        }
    }

    pub fn perf(&self) -> bool {
        self.options().perf
    }
}

pub fn execute(command: Command, policy: rxd_core::BudgetPolicy) -> Result<()> {
    let emit_perf = command.perf();
    let mut perf = Perf::new(emit_perf, "analysis", "dotnet", command.name());
    let result = execute_inner(command, policy, &mut perf).map_err(super::classify);
    super::finish(perf, emit_perf, result)
}

fn execute_inner(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    perf: &mut Perf,
) -> anyhow::Result<()> {
    let (path, options) = match &command {
        Command::Classes { input, options, .. }
        | Command::Fields { input, options, .. }
        | Command::Methods { input, options, .. }
        | Command::Get { input, options, .. }
        | Command::GetClass { input, options, .. }
        | Command::Strings { input, options, .. }
        | Command::Xref { input, options, .. } => (input, options),
    };
    let budget = policy.for_file(path)?;
    let limits = options.limits(budget);
    perf.resource_limits = Some(std::collections::BTreeMap::from([
        ("input_bytes", budget.input_bytes),
        ("work", budget.work),
        ("metadata_bytes", budget.metadata_bytes),
        ("output_bytes", budget.output_bytes),
        ("instructions", budget.instructions),
    ]));
    let mut input = perf.measure("read_parse", || Input::read(path, limits))?;
    perf.source_id = Some(input.source_id.clone());
    perf.counters.input_bytes = Some(input.file_bytes);
    perf.counters.entities = Some(input.types.len() + input.fields.len() + input.methods.len());
    let query_start = perf.start_stage();
    let mut output = rxd_core::output::Output::new(budget.output_bytes);
    let mut records = 0;
    // One target, resolved before the scan: the same two faces every backend takes, and a
    // token is the only thing .NET can match on exactly.
    let xref_target = if let Command::Xref {
        at,
        filter,
        kind,
        case,
        ..
    } = &command
    {
        Some(
            dotnet_target(at.as_deref(), filter.as_deref(), *kind, *case)
                .map_err(super::classify)?,
        )
    } else {
        None
    };
    let mut reference_result = match xref_target.as_ref() {
        // A user-string token (0x70 table) has no member identity; it takes the string route.
        Some(Target::Token { token, .. }) if token >> 24 != 0x70 => {
            Some(perf.measure("references", || input.references(*token, true))?)
        }
        _ => None,
    };
    // The header names the reference target: a member identity, or the `#US` literal
    // when the query went through the string route.
    let reference_target = match (&reference_result, &command) {
        (Some(result), _) => match result.target.as_deref() {
            Some(target) => serde_json::to_value(target)?,
            None => Value::Null,
        },
        (None, _) => match xref_target.as_ref() {
            Some(Target::Literal { pattern, exact, .. }) => {
                if *exact {
                    json!({"target_kind": "user_string", "value": pattern})
                } else {
                    json!({"target_kind": "user_string", "match": pattern})
                }
            }
            Some(Target::Token { token, .. }) => {
                let mut target =
                    json!({"target_kind": "user_string", "token": format!("0x{token:08x}")});
                if let Some(record) = user_string(&input, *token) {
                    match (record.value, record.utf16_units) {
                        (Some(text), _) => target["value"] = Value::from(text),
                        (None, Some(units)) => {
                            target["utf16_units"] = serde_json::to_value(units)?;
                        }
                        (None, None) => {}
                    }
                }
                target
            }
            None => Value::Null,
        },
    };
    if let Command::Get { selector, raw, .. } = &command {
        let at = selector.resolve()?;
        // FETCH resolves and decodes only the selected method; unrelated signatures
        // are not parsed on this path.
        let token = input.method_by_id(&at)?.token;
        let body = perf.measure("decode", || input.body_token(token))?;
        let method = input.method_by_id(&at)?;
        let decompilation = if *raw {
            rxd_dotnet::Decompilation {
                representation: "il".into(),
                decompiled: false,
                reason: None,
                warnings: Vec::new(),
                code: None,
                mapping: Vec::new(),
            }
        } else {
            perf.measure("reconstruct", || {
                rxd_dotnet::render(&input, method, &body, limits)
            })?
        };
        // Each metadata-token operand carries the resolved member it denotes, so a
        // fallback to IL does not require a separate --token --semantic round trip.
        let instructions = if *raw {
            annotate_operands(&input, &body.instructions)?
        } else {
            Value::Null
        };
        let reason_detail = decompilation.reason.as_deref().map(|reason| {
            describe_method_reason(
                reason,
                body.instructions.len(),
                body.handlers.len(),
                &unresolved_operands(&instructions),
            )
        });
        let value = fetch_record(
            "method_body",
            command.name(),
            RecordMeta {
                at: Some(&at),
                parent_at: Some(&coordinate(method.declaring_type_token)),
                start: Some(body.code_offset),
                end: Some(body.code_offset + body.code_size),
                ..RecordMeta::default()
            },
            {
                let mut out = json!({"method":method_record(&input, method)?,"body_present":true,"representation":decompilation.representation,"decompiled":decompilation.decompiled,"reason":decompilation.reason,"reason_detail":reason_detail,"warnings":decompilation.warnings,"code":decompilation.code,"mapping":decompilation.mapping,"file_offset":body.file_offset,"code_offset":body.code_offset,"code_size":body.code_size,"work_used":body.work_used,"complete":true})
                    .as_object()
                    .unwrap()
                    .clone();
                if *raw {
                    out.insert("max_stack".into(), json!(body.max_stack));
                    out.insert("init_locals".into(), json!(body.init_locals));
                    out.insert(
                        "local_signature_token".into(),
                        json!(body.local_signature_token),
                    );
                    out.insert("locals".into(), serde_json::to_value(&body.locals)?);
                    out.insert("instructions".into(), instructions);
                    out.insert("handlers".into(), serde_json::to_value(&body.handlers)?);
                    out.insert("il_hex".into(), json!(body.il_hex));
                }
                out
            },
        );
        append(&mut output, &value)?;
    } else if let Command::GetClass { selector, raw, .. } = &command {
        let at = selector.resolve()?;
        let root = perf.measure("prepare", || input.prepare_class(&at))?;
        if *raw {
            input.prepare_class_fields(root)?;
        }
        let class = perf.measure("decode", || input.class_body(root))?;
        let rendered = if *raw {
            rxd_dotnet::ClassDecompilation {
                representation: "il".into(),
                decompiled: false,
                reason: None,
                warnings: Vec::new(),
                code: None,
                mapping: vec![],
            }
        } else {
            perf.measure("reconstruct", || {
                rxd_dotnet::render_class(&input, &class, limits)
            })?
        };
        let mut payload = detail(
            json!({"work_used":class.work_used,"representation":rendered.representation,"decompiled":rendered.decompiled,"reason":rendered.reason,"reason_detail":rendered.reason.as_deref().map(|reason| describe_class_reason(reason, class.methods.len())),"warnings":rendered.warnings,"code":rendered.code,"mapping":rendered.mapping,"complete":true}),
        );
        if *raw {
            let types = class
                .types
                .iter()
                .map(|ty| type_record(&input, ty))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let fields = class
                .fields
                .iter()
                .map(|field| field_record(&input, field))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let properties = class
                .properties
                .iter()
                .map(|property| {
                    relationship_record(
                        &input,
                        "property",
                        &coordinate(property.declaring_type_token),
                        property.token,
                        *property,
                    )
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let events = class
                .events
                .iter()
                .map(|event| {
                    relationship_record(
                        &input,
                        "event",
                        &coordinate(event.declaring_type_token),
                        event.token,
                        *event,
                    )
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let methods = class
                .methods
                .iter()
                .map(|item| class_method_record(&input, item, *raw))
                .collect::<anyhow::Result<Vec<_>>>()?;
            payload.insert("type".into(), type_record(&input, class.root)?);
            for (key, records) in [
                ("types", types),
                ("fields", fields),
                ("properties", properties),
                ("events", events),
                ("methods", methods),
            ] {
                payload.insert(key.into(), Value::Array(records));
            }
        }
        // Nested inventories carry the same entity references as the top level.
        let value = fetch_record(
            "class_body",
            command.name(),
            RecordMeta {
                at: Some(&at),
                parent_at: class.root.declaring_type_token.map(coordinate).as_deref(),
                ..metadata(&input)
            },
            payload,
        );
        append(&mut output, &value)?;
    } else {
        let query = match &command {
            Command::Classes { class, .. } => json!({"class":class}),
            Command::Fields { class, name, .. } => json!({"class":class,"name":name}),
            Command::Methods { class, name, .. } => json!({"class":class,"name":name}),
            Command::Strings { filter, .. } => json!({"filter":filter}),
            Command::Xref {
                at, filter, case, ..
            } => {
                let mut query = json!({
                    "kind": xref_target.as_ref().map(Target::kind).map(Kind::as_str),
                });
                if let Some(at) = at {
                    query["at"] = json!(at);
                }
                if let Some(filter) = filter {
                    query["match"] = json!(filter);
                    query["case"] = json!(if case.is_sensitive() {
                        "sensitive"
                    } else {
                        "insensitive"
                    });
                }
                query
            }
            Command::Get { .. } | Command::GetClass { .. } => unreachable!(),
        };
        append(
            &mut output,
            &envelope::header(
                "dotnet",
                command.name(),
                Some(&input.source_id),
                Some(input.file_bytes),
                query,
                if matches!(command, Command::Xref { .. }) {
                    detail(json!({"target": reference_target}))
                } else {
                    Map::new()
                },
            ),
        )?;
        match &command {
            Command::Classes { .. } | Command::Fields { .. } | Command::Methods { .. } => {
                list(&mut input, &command, options, &mut output, &mut records)?;
            }
            Command::Strings { filter, case, .. } => {
                let uses = perf.measure("strings", || {
                    input.string_uses(filter.as_deref(), case.sensitive())
                })?;
                for group in &uses.groups {
                    let literal = &uses.literals[group.literal_index];
                    // `index` is the backend's own identifier for this string in its
                    // inventory: here, its position among the assembly's literals.
                    let index = group.literal_index;
                    let method = group.method_token.map(|token| {
                        &input.methods[input
                            .methods
                            .binary_search_by_key(&token, |method| method.token)
                            .expect("checked string owner")]
                    });
                    record(&mut output, &mut records, options, &{
                        // The `#US` token is the literal's identity, so it is the row's
                        // `at`: that is what `--at` takes back, and what the
                        // reference rows point at.
                        let token = format!("0x{:08x}", 0x7000_0000u32 | literal.offset);
                        let context = method.map(|method| coordinate(method.token));
                        envelope::record(
                            "string",
                            RecordMeta {
                                at: Some(&token),
                                parent_at: method
                                    .map(|method| coordinate(method.declaring_type_token))
                                    .as_deref(),
                                context_at: context.as_deref(),
                                ..metadata(&input)
                            },
                            detail(
                                json!({"index":index,"value":literal.value,"utf16_units":literal.utf16_units,"status":if method.is_some(){"referenced"}else{"unreferenced"},"il_offsets":group.il_offsets}),
                            ),
                        )
                    })?;
                }
            }
            Command::Xref { .. } => {
                if let Some(result) = reference_result.take() {
                    for reference in result.references {
                        let value = reference_record(
                            &input,
                            &reference.site,
                            reference.target.as_deref(),
                            None,
                        )?;
                        record(&mut output, &mut records, options, &value)?;
                    }
                } else {
                    // A string-route row names the text it loads, so the literal inventory is
                    // built once instead of resolving a token per row. Both anchors print the
                    // same target: a `#US` token and its text are two ways to name one literal,
                    // and the row says which text it was.
                    let literals = Some(literal_values(&input));
                    let sites = match xref_target.as_ref().expect("resolved here") {
                        Target::Literal {
                            pattern,
                            exact,
                            case_sensitive,
                        } => perf.measure("strings", || {
                            input.string_references(pattern, *exact, *case_sensitive)
                        })?,
                        Target::Token { token, .. } => {
                            perf.measure("strings", || input.user_string_references(*token))?
                        }
                    };
                    for site in sites {
                        let literal = literals
                            .as_ref()
                            .and_then(|map| map.get(&(site.target_token & 0x00ff_ffff)));
                        let value =
                            reference_record(&input, &site, None, literal.map(String::as_str))?;
                        record(&mut output, &mut records, options, &value)?;
                    }
                }
            }
            Command::Get { .. } | Command::GetClass { .. } => unreachable!(),
        }
        append(&mut output, &envelope::summary(records, true, Map::new()))?;
    }
    perf.record("query", query_start);
    perf.counters.records = Some(
        if matches!(command, Command::Get { .. } | Command::GetClass { .. }) {
            1
        } else {
            records
        },
    );
    perf.counters.output_bytes = Some(output.len());
    perf.measure("write", || {
        super::write_output(&output, options.output.as_deref())
    })?;
    Ok(())
}

// Single-record FETCH carries protocol identity without a separate header.
fn fetch_record(
    record: &str,
    command: &str,
    meta: RecordMeta<'_>,
    detail: Map<String, Value>,
) -> Value {
    let mut value = envelope::record(record, meta, detail);
    value["backend"] = json!("dotnet");
    value["command"] = json!(command);
    value["schema_version"] = json!(envelope::SCHEMA_VERSION);
    value
}

fn reference_record(
    input: &Input,
    site: &rxd_dotnet::TokenSite,
    target: Option<&rxd_dotnet::SemanticTarget>,
    // The decoded `#US` literal this site loads, when the query went through the string
    // route: a token alone does not say which text the row is about.
    literal: Option<&str>,
) -> anyhow::Result<Value> {
    let method = &input.methods[(site.method_token & 0xffffff) as usize - 1];
    let context = coordinate(method.token);
    let start = site.code_offset + site.il_offset as usize;
    Ok(envelope::reference(
        RecordMeta {
            context_at: Some(&context),
            parent_at: Some(&coordinate(method.declaring_type_token)),
            start: Some(start),
            end: Some(start + site.size as usize),
            ..metadata(input)
        },
        detail(
            json!({"target":literal.map(str::to_owned).or_else(||target.and_then(|target|target.canonical_token).map(coordinate)).unwrap_or_else(||format!("0x{:08x}",site.target_token)),"declaring_type":method.declaring_type,"name":method.name,"il_offset":site.il_offset,"opcode":site.mnemonic,"target_token":site.target_token,"target_kind":if let Some(target)=target{target.target_kind}else if site.target_token >> 24 == 0x70 {"string"}else{"token"},"resolution_status":target.map_or("token_only",|target|target.resolution_status.as_str()),"scope":target.map(|target|&target.scope),"owner_token":target.map(|target|target.owner_token),"original_owner_token":target.map(|target|target.original_owner_token),"owner_name":target.map(|target|&target.owner_name),"target_name":target.map(|target|&target.name),"signature":target.and_then(|target|target.signature.as_ref()),"type_signature":target.and_then(|target|target.type_signature.as_ref()),"owner_arguments":target.map_or(&[][..],|target|target.owner_arguments.as_slice()),"method_arguments":target.map_or(&[][..],|target|target.method_arguments.as_slice())}),
        ),
    ))
}

fn record(
    output: &mut rxd_core::output::Output,
    records: &mut usize,
    _options: &Options,
    value: &Value,
) -> anyhow::Result<()> {
    append(output, value)?;
    *records += 1;
    Ok(())
}

fn append(output: &mut rxd_core::output::Output, value: &Value) -> anyhow::Result<()> {
    if !rxd_core::envelope::is_wrapper(value) {
        output.json_line(value)?;
    }
    Ok(())
}

/// Names of token operands the resolver could not place, deduplicated and truncated so
/// the reason stays readable.
fn unresolved_operands(instructions: &Value) -> Vec<String> {
    let mut names = Vec::new();
    let Some(list) = instructions.as_array() else {
        return names;
    };
    for instruction in list {
        let Some(operand) = instruction.get("operand") else {
            continue;
        };
        if operand.get("kind").and_then(Value::as_str) != Some("token") {
            continue;
        }
        match operand.get("status").and_then(Value::as_str) {
            Some("resolved" | "external") => {}
            _ => {
                // A user-string operand (0x70 table) has no member identity at all, so
                // it is expected to be nameless rather than unresolved.
                if operand.get("value").and_then(Value::as_u64).unwrap_or(0) >> 24 == 0x70 {
                    continue;
                }
                // An unresolved operand has no owner or name; the token is the only
                // thing an Agent can act on.
                let label = match (
                    operand.get("owner").and_then(Value::as_str),
                    operand.get("name").and_then(Value::as_str),
                ) {
                    (Some(owner), Some(name)) => format!("{owner}::{name}"),
                    _ => format!(
                        "token {:#010x}",
                        operand.get("value").and_then(Value::as_u64).unwrap_or(0)
                    ),
                };
                if !names.contains(&label) {
                    names.push(label);
                }
            }
        }
    }
    names.truncate(4);
    names
}

/// Explain, in the record itself, why this body is IL instead of C#.
fn describe_method_reason(
    reason: &str,
    instructions: usize,
    handlers: usize,
    unresolved: &[String],
) -> String {
    let unresolved_detail = if unresolved.is_empty() {
        "every member operand resolves, so the refusal is the engine's own reconstruction limit rather than missing metadata".to_string()
    } else {
        format!(
            "{} unresolved member operand(s): {}",
            unresolved.len(),
            unresolved.join(", ")
        )
    };
    match reason {
        "engine_unavailable" => format!(
            "this build cannot load the pinned C# engine ({}), so only the original IL is available: {instructions} instructions, {unresolved_detail}",
            engine_state(),
        ),
        other => format!(
            "the C# engine rejected this body (`{other}`); the original IL is returned: {instructions} instructions, {handlers} exception handlers, {unresolved_detail}"
        ),
    }
}

fn describe_class_reason(reason: &str, methods: usize) -> String {
    match reason {
        "class_engine_unavailable" => format!(
            "this build cannot load the pinned C# engine ({}); the {methods}-member inventory and original IL are complete, whole-class C# is not available",
            engine_state(),
        ),
        "class_abi_unavailable" => format!(
            "the loaded engine exposes no whole-class entry point; the {methods}-member inventory and original IL are complete. Fetch one member with `get-method --at <method:token>`"
        ),
        other => format!(
            "the C# engine rejected this class (`{other}`); the {methods}-member inventory and original IL are complete and no partial C# is returned. The refusal names no member, so fetch each member with `get-method --at <at>`: a member the engine also refuses reports `il_warning` and still returns its original IL"
        ),
    }
}

/// Decode one `#US` literal by token so a record can carry the text, not just the token.
/// Decoded `#US` literals keyed by their heap offset, for one query.
fn literal_values(input: &Input) -> BTreeMap<u32, String> {
    input
        .strings()
        .map(|records| {
            records
                .into_iter()
                .filter_map(|record| record.value.map(|value| (record.offset, value)))
                .collect()
        })
        .unwrap_or_default()
}

fn user_string(input: &Input, token: u32) -> Option<rxd_dotnet::StringRecord> {
    input
        .strings()
        .ok()?
        .into_iter()
        .find(|record| record.offset == token & 0x00ff_ffff)
}

fn engine_state() -> &'static str {
    if cfg!(feature = "bundled-ilspy") {
        "bundled engine failed to load"
    } else {
        "built without the bundled engine"
    }
}

/// Annotate every metadata-token operand with the member it provably denotes.
fn annotate_operands(
    input: &Input,
    instructions: &[rxd_dotnet::Instruction],
) -> anyhow::Result<Value> {
    let mut value = serde_json::to_value(instructions)?;
    let Some(list) = value.as_array_mut() else {
        return Ok(value);
    };
    let mut resolver = input.target_resolver()?;
    // A `#US` operand has no member identity, but its token is a usable coordinate and
    // the literal is already decoded in the heap, so one lookup answers both questions.
    let literals = if instructions.iter().any(|instruction| {
        matches!(instruction.operand, rxd_dotnet::Operand::Token(token) if token >> 24 == 0x70)
    }) {
        let mut map = BTreeMap::new();
        for record in input.strings()? {
            map.insert(record.offset, record);
        }
        Some(map)
    } else {
        None
    };
    for instruction in list.iter_mut() {
        let Some(operand) = instruction.get_mut("operand") else {
            continue;
        };
        if operand.get("kind").and_then(Value::as_str) != Some("token") {
            continue;
        }
        let Some(token) = operand.get("value").and_then(Value::as_u64) else {
            continue;
        };
        let token = u32::try_from(token).unwrap_or(0);
        if token >> 24 == 0x70 {
            operand["target_at"] = Value::from(format!("0x{token:08x}"));
            if let Some(record) = literals
                .as_ref()
                .and_then(|map| map.get(&(token & 0x00ff_ffff)))
            {
                match (&record.value, &record.utf16_units) {
                    (Some(text), _) => operand["string"] = Value::from(text.clone()),
                    (None, Some(units)) => {
                        operand["utf16_units"] = serde_json::to_value(units)?;
                    }
                    (None, None) => {}
                }
            }
            continue;
        }
        let Some(target) = resolver.target(token)? else {
            continue;
        };
        operand["owner"] = Value::from(target.owner_name.clone());
        operand["name"] = Value::from(target.name.clone());
        operand["status"] = Value::from(target.resolution_status.clone());
        operand["scope"] = serde_json::to_value(&target.scope)?;
        if let Some(canonical) = target.canonical_token {
            operand["target_at"] = Value::from(coordinate(canonical));
        }
    }
    Ok(value)
}

/// Detail-level entity references use the same copy-only coordinate as the outer
/// envelope; a caller never receives a hash it did not ask for. Nested inventories
/// (class bodies, refs rows) carry the same keys, so the rewrite is recursive.
/// The copy-only coordinate of a row: its ECMA-335 metadata token.
fn coordinate(token: u32) -> String {
    format!("0x{token:08x}")
}

fn method_record(input: &Input, method: &rxd_dotnet::Method) -> anyhow::Result<Value> {
    let fields = detail(serde_json::to_value(method)?);
    let record_at = coordinate(method.token);
    Ok(envelope::record(
        "method",
        RecordMeta {
            at: Some(&record_at),
            parent_at: Some(&coordinate(method.declaring_type_token)),
            ..metadata(input)
        },
        fields,
    ))
}

fn type_record(input: &Input, ty: &rxd_dotnet::TypeInfo) -> anyhow::Result<Value> {
    let mut fields = detail(serde_json::to_value(ty)?);
    fields.remove("id");
    // A base type and an interface are tokens; a reader navigates by name. Both stay,
    // because the token is the identity `get-class --at` takes back and a name is not
    // unique across assemblies. An omitted default keeps the row at its old size.
    if let Some(token) = ty.extends_token
        && let Some(name) = input.type_display(token)
        && name != "System.Object"
    {
        fields.insert("super".to_owned(), Value::from(name));
    }
    if !ty.interfaces.is_empty() {
        let names: Vec<Value> = ty
            .interfaces
            .iter()
            .filter_map(|token| input.type_display(*token))
            .map(Value::from)
            .collect();
        let tokens: Vec<Value> = ty.interfaces.iter().map(|t| Value::from(*t)).collect();
        // Names are a projection of the tokens; print them only for the whole list so the
        // two arrays always line up.
        if names.len() == ty.interfaces.len() {
            fields.insert("interfaces".to_owned(), Value::Array(names));
        }
        fields.insert("interface_tokens".to_owned(), Value::Array(tokens));
    }
    Ok(envelope::record(
        "class",
        RecordMeta {
            at: Some(&coordinate(ty.token)),
            parent_at: ty.declaring_type_token.map(coordinate).as_deref(),
            ..metadata(input)
        },
        fields,
    ))
}

fn field_record(input: &Input, field: &rxd_dotnet::Field) -> anyhow::Result<Value> {
    let fields = detail(serde_json::to_value(field)?);
    // Nested references are copy-only coordinates, exactly as in `methods`/`classes`.
    let record_at = coordinate(field.token);
    Ok(envelope::record(
        "field",
        RecordMeta {
            at: Some(&record_at),
            parent_at: Some(&coordinate(field.declaring_type_token)),
            ..metadata(input)
        },
        fields,
    ))
}

fn class_method_record(
    input: &Input,
    item: &rxd_dotnet::ClassMethod<'_>,
    raw: bool,
) -> anyhow::Result<Value> {
    let body = item.body.as_ref();
    let mut fields = match (body, raw) {
        (Some(body), true) => detail(serde_json::to_value(body)?),
        (Some(body), false) => detail(json!({
            "work_used": body.work_used,
            "file_offset": body.file_offset,
            "code_offset": body.code_offset,
            "code_size": body.code_size,
        })),
        (None, true) => detail(
            json!({"work_used":null,"file_offset":null,"code_offset":null,"code_size":null,"max_stack":null,"init_locals":null,"local_signature_token":null,"locals":null,"instructions":null,"handlers":null,"il_hex":null}),
        ),
        (None, false) => {
            detail(json!({"work_used":null,"file_offset":null,"code_offset":null,"code_size":null}))
        }
    };
    fields.insert("method".into(), method_record(input, item.method)?);
    fields.insert("body_present".into(), json!(body.is_some()));
    fields.insert("complete".into(), json!(true));
    fields.insert("representation".into(), json!("il"));
    fields.insert("decompiled".into(), json!(false));
    fields.insert("reason".into(), json!("member_inventory"));
    fields.insert("code".into(), Value::Null);
    fields.insert("mapping".into(), json!([]));
    let record_at = coordinate(item.method.token);
    Ok(envelope::record(
        "method_body",
        RecordMeta {
            at: Some(&record_at),
            parent_at: Some(&coordinate(item.method.declaring_type_token)),
            start: body.map(|body| body.code_offset),
            end: body.map(|body| body.code_offset + body.code_size),
            ..metadata(input)
        },
        fields,
    ))
}

fn relationship_record<T: serde::Serialize>(
    input: &Input,
    kind: &str,
    owner: &str,
    token: u32,
    relationship: &T,
) -> anyhow::Result<Value> {
    // A property or event is not a fetchable unit: no command takes its 0x17/0x14 token,
    // and a caller reaches the behaviour through the accessor methods. So the row prints
    // no `at` - an `at` that no `--at` accepts would promise a round trip that fails - and
    // keeps the token as evidence. The owner is `parent_at`; `context_at` stays null
    // because it would repeat the same owner.
    let _ = token;
    Ok(envelope::record(
        kind,
        RecordMeta {
            parent_at: Some(owner),
            ..metadata(input)
        },
        // Nested references are copy-only coordinates, as in every other listing.
        detail(serde_json::to_value(relationship)?),
    ))
}

/// Per-row metadata: identity is carried once by the stream header, as in DEX and the
/// JVM backend, so a row does not repeat the assembly's content id.
fn metadata(_input: &Input) -> RecordMeta<'_> {
    RecordMeta::default()
}

/// Which member kind a listing prints.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listing {
    Classes,
    Fields,
    Methods,
}

/// Print one member kind, grouped by declaring type in declaration order.
fn list(
    input: &mut Input,
    command: &Command,
    options: &Options,
    output: &mut rxd_core::output::Output,
    records: &mut usize,
) -> anyhow::Result<()> {
    let (listing, class, name, filter, case) = match command {
        Command::Classes {
            class,
            filter,
            case,
            ..
        } => (Listing::Classes, class, None, filter, case),
        Command::Fields {
            class,
            name,
            filter,
            case,
            ..
        } => (Listing::Fields, class, name.as_ref(), filter, case),
        Command::Methods {
            class,
            name,
            filter,
            case,
            ..
        } => (Listing::Methods, class, name.as_ref(), filter, case),
        _ => unreachable!("only listing commands reach here"),
    };
    let pattern = filter
        .as_deref()
        .map(|needle| rxd_core::filter::Contains::new(needle, case.sensitive()));
    // A filter answers "does this record's own name contain the needle?". The declaring
    // type is a separate question that `--class` answers exactly, so a member is never
    // reported for a match on its owner's name (ADR 0014).
    let matches = |name: &str| pattern.as_ref().is_none_or(|pattern| pattern.matches(name));
    // Resolve signatures only for the rows this query can emit. Filtering by name and
    // declaring type never needs them, so unrelated corrupt members stay unread.
    let selected_classes: Option<std::collections::BTreeSet<u32>> = class.as_ref().map(|wanted| {
        input
            .types
            .iter()
            .filter(|ty| ty.full_name.as_ref() == wanted)
            .map(|ty| ty.token)
            .collect()
    });
    let owner_selected = |token: u32| {
        selected_classes
            .as_ref()
            .is_none_or(|selected| selected.contains(&token))
    };
    let mut resolve_fields = Vec::new();
    if listing == Listing::Fields {
        for (index, field) in input.fields.iter().enumerate() {
            if owner_selected(field.declaring_type_token)
                && name.is_none_or(|wanted| wanted == &field.name)
                && matches(&field.name)
            {
                resolve_fields.push(index);
            }
        }
        for index in resolve_fields {
            input.resolve_field(index)?;
        }
    }
    let mut resolve_methods = Vec::new();
    if listing == Listing::Methods {
        for (index, item) in input.methods.iter().enumerate() {
            if owner_selected(item.declaring_type_token)
                && name.is_none_or(|wanted| wanted == &item.name)
                && matches(&item.name)
            {
                resolve_methods.push(index);
            }
        }
        for index in resolve_methods {
            input.resolve_method(index)?;
        }
    }
    let input: &Input = &*input;
    let mut fields_by_type: BTreeMap<u32, Vec<_>> = BTreeMap::new();
    let mut methods_by_type: BTreeMap<u32, Vec<_>> = BTreeMap::new();
    if listing == Listing::Fields {
        for field in &input.fields {
            fields_by_type
                .entry(field.declaring_type_token)
                .or_default()
                .push(field);
        }
    }
    if listing == Listing::Methods {
        for item in &input.methods {
            methods_by_type
                .entry(item.declaring_type_token)
                .or_default()
                .push(item);
        }
    }
    for ty in &input.types {
        if class
            .as_ref()
            .is_some_and(|name| name != ty.full_name.as_ref())
        {
            continue;
        }
        match listing {
            Listing::Classes => {
                if matches(&ty.full_name) {
                    record(output, records, options, &type_record(input, ty)?)?;
                }
            }
            Listing::Fields => {
                for field in fields_by_type.get(&ty.token).into_iter().flatten() {
                    if name.is_some_and(|wanted| wanted != &field.name) || !matches(&field.name) {
                        continue;
                    }
                    record(output, records, options, &field_record(input, field)?)?;
                }
            }
            Listing::Methods => {
                for item in methods_by_type.get(&ty.token).into_iter().flatten() {
                    if name.is_some_and(|wanted| wanted != &item.name) || !matches(&item.name) {
                        continue;
                    }
                    record(output, records, options, &method_record(input, item)?)?;
                }
            }
        }
    }
    Ok(())
}

fn detail(value: Value) -> Map<String, Value> {
    let Value::Object(fields) = value else {
        unreachable!(".NET record detail is a statically defined object")
    };
    fields
}

/// A dotnet coordinate is the ECMA-335 metadata token itself: eight hex digits,
/// optional `0x`. A kind prefix is refused with the value to use instead.
/// The one target a .NET reference query names.
enum Target {
    /// A metadata or `#US` token: an identity, matched exactly.
    Token { token: u32, kind: Kind },
    /// A decoded user-string literal: an exact anchor, or a substring search.
    Literal {
        pattern: String,
        exact: bool,
        case_sensitive: bool,
    },
}

impl Target {
    fn kind(&self) -> Kind {
        match self {
            Self::Token { kind, .. } => *kind,
            Self::Literal { .. } => Kind::String,
        }
    }
}

/// Resolve the reference target from its two faces.
///
/// A token says its own kind through its table, so `--kind` is only an assertion here. That
/// matters because a token is a per-module row number: `0x0600000b` is a different method in
/// every assembly, so a coordinate only means something against the input it came from.
fn dotnet_target(
    at: Option<&str>,
    filter: Option<&str>,
    kind: Option<Kind>,
    case: crate::cli::filter::CaseMode,
) -> anyhow::Result<Target> {
    let faces = usize::from(at.is_some()) + usize::from(filter.is_some());
    anyhow::ensure!(
        faces == 1,
        "INVALID_QUERY: exactly one reference target is required (`--at TOKEN` or `-f \
         PATTERN --kind string`)"
    );
    if let Some(pattern) = filter {
        anyhow::ensure!(
            kind == Some(crate::cli::xref::Kind::String),
            "INVALID_QUERY: a .NET member or type identity is a metadata token, not a name, so \
             `--match` can only search literals: give `--kind string`, or anchor with `--at TOKEN"
        );
        return Ok(Target::Literal {
            pattern: pattern.to_owned(),
            exact: false,
            case_sensitive: case.is_sensitive(),
        });
    }
    let coordinate = at.expect("checked above").to_owned();
    // `--kind string` reads `--at` as a literal, which is the one target that cannot
    // describe itself. A token still wins when it parses as one: `0x0600000b --kind string`
    // is a contradiction to report, not a search for the text `0x0600000b`.
    if kind == Some(crate::cli::xref::Kind::String) {
        return match parse_token(&coordinate) {
            Ok(token) => {
                let inferred = token_kind(token);
                if token >> 24 != 0x70 || inferred != Some(crate::cli::xref::Kind::String) {
                    anyhow::bail!(
                        "INVALID_QUERY: --at {coordinate} is a {} token, but --kind string was given",
                        inferred.map_or("non-string", |kind| kind.as_str())
                    );
                }
                Ok(Target::Token {
                    token,
                    kind: crate::cli::xref::Kind::String,
                })
            }
            Err(_) => Ok(Target::Literal {
                pattern: coordinate,
                exact: true,
                case_sensitive: true,
            }),
        };
    }
    let token = parse_token(&coordinate).map_err(|error| {
        anyhow::anyhow!("{error:#}; pass `--kind string` to read `--at` as an exact literal")
    })?;
    let inferred = token_kind(token);
    if let (Some(given), Some(inferred)) = (kind, inferred)
        && given != inferred
    {
        anyhow::bail!(
            "INVALID_QUERY: --at {coordinate} is a {} token, but --kind {} was given",
            inferred.as_str(),
            given.as_str()
        );
    }
    Ok(Target::Token {
        token,
        kind: kind.or(inferred).unwrap_or(Kind::Method),
    })
}

/// The kind a token's table names, or `None` where the table covers two (`MemberRef`).
fn token_kind(token: u32) -> Option<Kind> {
    match token >> 24 {
        0x70 => Some(Kind::String),
        0x01 | 0x02 | 0x1b => Some(Kind::Type),
        0x04 => Some(Kind::Field),
        0x06 | 0x2b => Some(Kind::Method),
        _ => None,
    }
}

fn parse_token(value: &str) -> anyhow::Result<u32> {
    if let Some((kind, _)) = value.split_once(':') {
        anyhow::bail!(
            "INVALID_QUERY: `{value}` carries a `{kind}:` prefix; a dotnet coordinate is the metadata token alone, for example `{}`",
            value.rsplit(':').next().unwrap_or(value)
        );
    }
    let body = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    anyhow::ensure!(
        body.len() == 8 && body.bytes().all(|b| b.is_ascii_hexdigit()),
        "INVALID_QUERY: expected a metadata token such as `0x0600000b` (eight hex digits, `0x` optional)"
    );
    Ok(u32::from_str_radix(body, 16)?)
}
