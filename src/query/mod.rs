//! Dispatch typed queries against captured inputs and project backend output.
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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapturedInput {
    pub path: PathBuf,
    pub original_path: PathBuf,
    #[serde(default)]
    pub input_bytes: Option<usize>,
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
    #[cfg(feature = "bundled-kuna")]
    if name.starts_with("kuna_") {
        return kuna::execute(name, &arguments, &inputs[0], options.inflight_bytes);
    }
    let argv = catalog::argv(name, &arguments, inputs, options.threads)?;
    let cli = crate::cli::Cli::try_parse_from(argv)
        .map_err(|error| QueryError::new("INVALID_QUERY", error.to_string()))?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(options.threads)
        .stack_size(32 * 1024 * 1024)
        .build()
        .map_err(|error| QueryError::new("RESOURCE_LIMIT", error.to_string()))?;
    let mut captured = rxd_core::output::Output::new(None);
    let result = pool.install(|| {
        let mut output = crate::cli::Output::new(&mut captured, Some(options.inflight_bytes));
        crate::cli::execute(cli, &mut output)
    });
    let bytes = captured.into_bytes();
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

/// Project JSONL into records, JSON into objects and source into text.
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
                if matches!(
                    key.as_str(),
                    "path" | "container" | "input" | "archive" | "target_path"
                ) && let Some(path) = value.as_str()
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
