//! Typed query catalog: one entry per analysis command, reflected from the CLI.
//!
//! The CLI is the single source of truth. `reflect` walks `clap` for names,
//! descriptions, flags, types and enums; this module only adds the MCP-specific
//! envelope (captured inputs, artifact output, server resource ceilings) and the
//! output schema.
use super::{CapturedInput, QueryBudgets, QueryError, reflect, schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub backend: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub capabilities: Vec<String>,
}

/// Classify a command by what it does, for tool annotations. The command names are the
/// shared CLI vocabulary, so a listing grouped by backend still classifies the same way.
fn capability(command: &str) -> &'static str {
    match command {
        "entities" | "functions" | "classes" | "methods" | "fields" | "entries" | "types"
        | "imports" | "exports" | "tables" | "memories" | "globals" | "data" | "elements"
        | "codes" | "blocks" | "analyze" | "interface" | "storage" | "disassemble"
        | "basic-blocks" | "cfg" | "metadata" | "manifest" => "ENUMERATE",
        "strings" | "find" | "identifiers" | "calls" | "members" | "locate" => "SEARCH",
        "xref" => "XREF",
        _ => "FETCH",
    }
}

/// Tools whose handler is MCP-specific rather than an argv projection keep a small
/// synthetic overlay; every other tool is exactly its CLI command.
fn input_schema(spec: &reflect::ToolSpec) -> Value {
    let mut schema = reflect::input_schema(spec);
    if matches!(spec.name.as_str(), "asar_extract" | "html_extract") {
        schema["properties"]["include_bytes"] = serde_json::json!({
            "type": "boolean",
            "description": "Return the extracted bytes beside the receipt."
        });
    }
    schema
}

fn specs() -> Vec<reflect::ToolSpec> {
    reflect::tools()
}

fn spec(name: &str) -> Result<reflect::ToolSpec, QueryError> {
    specs()
        .into_iter()
        .find(|spec| spec.name == name)
        .ok_or_else(|| QueryError::new("INVALID_QUERY", format!("unknown analysis tool: {name}")))
}

pub fn catalog() -> Vec<ToolDefinition> {
    let result: Vec<ToolDefinition> = specs()
        .into_iter()
        .map(|spec| ToolDefinition {
            name: spec.name.clone(),
            backend: spec.backend.clone(),
            description: spec.description.clone(),
            input_schema: input_schema(&spec),
            output_schema: schema::output(),
            capabilities: vec![capability(&spec.command).into()],
        })
        .collect();
    #[cfg(feature = "bundled-kuna")]
    let result = {
        let mut result = result;
        result.extend(super::kuna::catalog());
        result
    };
    result
}

/// Reject a caller mistake before spawning a worker. Everything else (types, enums,
/// conflicts, required flags) is the CLI parser's job, so the two surfaces cannot
/// disagree about what is valid.
pub fn validate(name: &str, arguments: &Value) -> Result<(), QueryError> {
    #[cfg(feature = "bundled-kuna")]
    if name.starts_with("kuna_") {
        return super::kuna::validate(name, arguments);
    }
    let spec = spec(name)?;
    let object = arguments
        .as_object()
        .ok_or_else(|| QueryError::new("INVALID_QUERY", "tool arguments must be a JSON object"))?;
    let allowed = input_schema(&spec);
    let properties = allowed["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for key in object.keys() {
        if !properties.contains_key(key) {
            return Err(QueryError::new(
                "INVALID_QUERY",
                format!("unknown argument `{key}` for {name}"),
            ));
        }
    }
    if spec.multi_input {
        if arguments.get("inputs").is_none() {
            return Err(QueryError::new(
                "INVALID_QUERY",
                "exactly one inputs selector is required",
            ));
        }
    } else if arguments.get("input").is_none() {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "exactly one input selector is required",
        ));
    }
    if !spec.subcommands.is_empty() && arguments.get("kind").is_none() {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "a reference kind is required",
        ));
    }
    Ok(())
}

/// Apply the server's thread budget to a command that declares `--threads`. Every other
/// ceiling lives in `rxd_core::Budget` and has no CLI flag, so nothing else is projected:
/// a caller may ask for fewer threads, never for more, and no backend flag is invented.
fn bound_threads(spec: &reflect::ToolSpec, b: QueryBudgets) -> Option<(&'static str, usize)> {
    spec.exposed()
        .any(|arg| arg.key == "threads")
        .then_some(("threads", b.threads))
}

pub(super) fn argv(
    name: &str,
    arguments: &Value,
    inputs: &[CapturedInput],
    b: QueryBudgets,
) -> Result<Vec<std::ffi::OsString>, QueryError> {
    let spec = spec(name)?;
    let paths: Vec<PathBuf> = inputs.iter().map(|input| input.path.clone()).collect();
    let mut arguments = arguments.clone();
    if let Some((key, ceiling)) = bound_threads(&spec, b) {
        let requested = arguments
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok());
        arguments[key] = serde_json::json!(requested.map_or(ceiling, |value| value.min(ceiling)));
    }
    // Machine-readable output is requested inside `reflect::argv`, before any `--`.
    reflect::argv(&spec, &arguments, &paths)
}
