//! Reusable bounded query dispatch used by isolated MCP analysis workers.
//!
//! Backend adapters own analysis and deterministic record assembly. This layer
//! accepts only catalogued typed arguments, routes immutable captured inputs, and
//! lifts their internal serialized records into their declared structured shape.
mod catalog;
#[cfg(feature = "bundled-kuna")]
mod kuna;
#[cfg(feature = "bundled-kuna")]
mod kuna_dto;
mod lossless;
mod reflect;
mod schema;

pub use catalog::{catalog, validate};
use clap::Parser;
use rxd_core::Error;
#[cfg(any(feature = "backend-asar", feature = "backend-html"))]
use rxd_core::perf::Perf;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(any(feature = "backend-asar", feature = "backend-html"))]
use std::path::Path;
use std::{cell::RefCell, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapturedInput {
    pub path: PathBuf,
    pub original_path: PathBuf,
    #[serde(default)]
    pub source_id: Option<String>,
    #[serde(default)]
    pub input_bytes: Option<usize>,
    #[serde(default)]
    pub identities: Vec<SnapshotIdentity>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotIdentity {
    pub path: PathBuf,
    pub source_id: String,
    pub input_bytes: usize,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct QueryOptions {
    pub threads: usize,
    pub inflight_bytes: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryResult {
    pub structured: Value,
    pub artifact: Option<ExtractedArtifact>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtractedArtifact {
    pub bytes: Vec<u8>,
    pub name: String,
    pub source_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryError {
    pub code: String,
    pub message: String,
    pub candidates: Option<Value>,
}

impl QueryError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            candidates: None,
        }
    }
}
impl From<Error> for QueryError {
    fn from(error: Error) -> Self {
        Self::new(error.code, error.message)
    }
}

struct Capture {
    bytes: Vec<u8>,
    limit: usize,
}
thread_local! { static OUTPUT: RefCell<Option<Capture>> = const { RefCell::new(None) }; }

#[cfg(feature = "backend-dex")]
pub(crate) fn output_limit() -> Option<usize> {
    OUTPUT.with(|slot| slot.borrow().as_ref().map(|capture| capture.limit))
}
/// Explicit backend output sink; this never reads or redirects process stdout.
pub(crate) fn capture_write(bytes: &[u8]) -> Option<rxd_core::Result<()>> {
    OUTPUT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let capture = slot.as_mut()?;
        if bytes.len() > capture.limit.saturating_sub(capture.bytes.len()) {
            return Some(Err(Error::new(
                "RESOURCE_LIMIT",
                "query output byte budget exceeded",
            )));
        }
        capture.bytes.extend_from_slice(bytes);
        Some(Ok(()))
    })
}

struct CaptureGuard;
impl Drop for CaptureGuard {
    fn drop(&mut self) {
        OUTPUT.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}

pub fn execute(
    name: &str,
    arguments: Value,
    inputs: &[CapturedInput],
    options: QueryOptions,
) -> Result<QueryResult, QueryError> {
    validate(name, &arguments)?;
    if inputs.is_empty() {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "query requires a captured input",
        ));
    }
    if inputs.len() != 1 {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "this tool requires exactly one captured input",
        ));
    }
    if options.threads == 0 || options.inflight_bytes == 0 {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "worker thread and shared byte budgets must be positive",
        ));
    }
    let input = &inputs[0];
    let mut budget = if std::fs::metadata(&input.path)
        .map_err(Error::from)?
        .is_dir()
    {
        let mut result = rxd_core::Budget::for_input_bytes(0)?;
        for identity in &input.identities {
            let file = rxd_core::Budget::for_input_bytes(identity.input_bytes)?;
            result.output_bytes = result
                .output_bytes
                .checked_add(file.output_bytes)
                .ok_or_else(|| {
                    QueryError::new("RESOURCE_LIMIT", "directory output envelope overflow")
                })?;
        }
        result
    } else {
        rxd_core::BudgetPolicy::InputScaled.for_file(&input.path)?
    };
    // Analysis envelopes scale with input; the service only bounds total in-flight storage.
    budget.output_bytes = budget.output_bytes.min(options.inflight_bytes);
    #[cfg(feature = "bundled-kuna")]
    if name.starts_with("kuna_") {
        return kuna::execute(name, &arguments, &inputs[0], budget);
    }
    #[cfg(feature = "backend-asar")]
    if name == "asar_extract" {
        return extract_asar(&arguments, inputs, budget);
    }
    #[cfg(feature = "backend-html")]
    if name == "html_extract" {
        return extract_html(&arguments, inputs, budget);
    }
    let argv = catalog::argv(name, &arguments, inputs, options.threads)?;
    let cli = crate::cli::Cli::try_parse_from(argv)
        .map_err(|error| QueryError::new("INVALID_QUERY", error.to_string()))?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(options.threads)
        .stack_size(32 * 1024 * 1024)
        .build()
        .map_err(|error| QueryError::new("RESOURCE_LIMIT", error.to_string()))?;
    let (result, bytes) = pool.install(|| {
        OUTPUT.with(|slot| {
            *slot.borrow_mut() = Some(Capture {
                bytes: Vec::new(),
                limit: budget.output_bytes,
            })
        });
        let _guard = CaptureGuard;
        let result = crate::cli::execute(cli);
        let bytes = OUTPUT.with(|slot| slot.borrow_mut().take().unwrap().bytes);
        (result, bytes)
    });
    let status = result.map_err(QueryError::from)?;
    if status != 0 {
        return Err(QueryError::new(
            "ANALYSIS_ERROR",
            format!("analysis returned status {status}"),
        ));
    }
    let mut structured = reply(&bytes)?;
    restore_paths(&mut structured, inputs);
    Ok(QueryResult {
        structured,
        artifact: None,
    })
}

/// Backend extraction needs a destination identity for its receipt, but the shared query
/// returns the bytes directly rather than writing a caller path. Fabricate a name that is
/// distinct from the input so the receipt has a path to normalize and blank out later.
#[cfg(any(feature = "backend-asar", feature = "backend-html"))]
fn synthetic_extraction_destination(input: &Path) -> PathBuf {
    let mut destination = input.with_file_name(".rxd-mcp-extracted");
    if destination == input {
        destination = input.with_file_name(".rxd-mcp-extracted-2");
    }
    destination
}

/// Normalize an extract receipt for typed projection: the projection carries bytes instead
/// of a caller path, so blank the synthetic destination and restore captured original paths.
#[cfg(any(feature = "backend-asar", feature = "backend-html"))]
fn project_extract_receipt(
    receipt: &[u8],
    inputs: &[CapturedInput],
) -> Result<(Value, String), QueryError> {
    let mut structured: Value = serde_json::from_slice(receipt)
        .map_err(|error| QueryError::new("INTERNAL_ERROR", error.to_string()))?;
    structured["detail"]["path"] = Value::Null;
    restore_paths(&mut structured, inputs);
    let source_id = structured["source_id"].as_str().unwrap().to_owned();
    Ok((structured, source_id))
}

#[cfg(feature = "backend-html")]
fn extract_html(
    arguments: &Value,
    inputs: &[CapturedInput],
    budget: rxd_core::Budget,
) -> Result<QueryResult, QueryError> {
    let at = arguments["at"]
        .as_str()
        .ok_or_else(|| {
            QueryError::new(
                "INVALID_QUERY",
                "html_extract requires at from html_entries.at or html_resources.at",
            )
        })?
        .to_owned();
    let content = arguments["content"].as_bool().unwrap_or(false);
    let destination = synthetic_extraction_destination(&inputs[0].path);
    let mut perf = Perf::new(false, "analysis", "html", "extract");
    let response = rxd_html::extract(
        &inputs[0].path,
        &at,
        content,
        &destination,
        budget.into(),
        &mut perf,
    )?;
    let (structured, source_id) = project_extract_receipt(&response.receipt, inputs)?;
    Ok(QueryResult {
        structured,
        artifact: Some(ExtractedArtifact {
            bytes: response.bytes,
            name: at,
            source_id,
        }),
    })
}

#[cfg(feature = "backend-asar")]
fn extract_asar(
    arguments: &Value,
    inputs: &[CapturedInput],
    budget: rxd_core::Budget,
) -> Result<QueryResult, QueryError> {
    let at = arguments["at"]
        .as_str()
        .ok_or_else(|| {
            QueryError::new(
                "INVALID_QUERY",
                "asar_extract requires at from asar_entries.at",
            )
        })?
        .to_owned();
    let destination = synthetic_extraction_destination(&inputs[0].path);
    let mut perf = Perf::new(false, "analysis", "asar", "extract");
    let response = rxd_asar::run(
        rxd_asar::Query::Extract { at: at.clone() },
        rxd_asar::Options {
            input: &inputs[0].path,
            output: Some(&destination),
            limits: rxd_core::Limits {
                input_bytes: rxd_core::MAX_SPAN_BYTES,
                ..budget.into()
            },
            max_entries: budget.entries,
            max_header_bytes: budget.metadata_bytes,
        },
        &mut perf,
    )?;
    let rxd_asar::Response::Extract { bytes, receipt } = response else {
        unreachable!()
    };
    let (structured, source_id) = project_extract_receipt(&receipt, inputs)?;
    Ok(QueryResult {
        structured,
        artifact: Some(ExtractedArtifact {
            bytes,
            name: at,
            source_id,
        }),
    })
}

/// Shape a captured stdout stream: JSONL streams become `{records:[...]}`, a
/// single-object result keeps its own envelope, and source text arrives as `text`.
fn reply(bytes: &[u8]) -> Result<Value, QueryError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| QueryError::new("INTERNAL_ERROR", error.to_string()))?;
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return Ok(json!({"records": []}));
    }
    if trimmed.starts_with('{')
        && let Ok(record) = lossless::parse(trimmed.as_bytes())
        && record.get("backend").is_some()
        && record.get("command").is_some()
    {
        return Ok(record);
    }
    let mut rows = Vec::new();
    for line in bytes
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let Ok(row) = lossless::parse(line) else {
            return Ok(json!({"text": text}));
        };
        rows.push(row);
    }
    Ok(json!({"records": rows}))
}

fn restore_paths(value: &mut Value, inputs: &[CapturedInput]) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "path" | "container" | "input" | "archive")
                    && let Some(path) = value.as_str()
                {
                    for input in inputs {
                        if std::path::Path::new(path) == input.path {
                            *value = json!(input.original_path);
                            break;
                        }
                        if let Ok(relative) = std::path::Path::new(path).strip_prefix(&input.path) {
                            *value = json!(input.original_path.join(relative));
                            break;
                        }
                    }
                } else {
                    restore_paths(value, inputs);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                restore_paths(value, inputs);
            }
        }
        _ => {}
    }
}
