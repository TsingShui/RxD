//! Reflect CLI commands into typed MCP tools, adding only capture/output envelopes.
use super::{CapturedInput, QueryError, reflect, schema};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf, sync::OnceLock};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub backend: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub capabilities: Vec<String>,
}

fn capability(command: &str) -> &'static str {
    match command {
        "entities" | "functions" | "classes" | "methods" | "fields" | "entries" | "resources"
        | "values" | "types" | "modules" | "imports" | "exports" | "tables" | "memories"
        | "globals" | "data" | "elements" | "codes" | "blocks" | "analyze" | "interface"
        | "storage" | "disassemble" | "basic-blocks" | "cfg" | "metadata" | "manifest" => {
            "ENUMERATE"
        }
        "strings" | "find" | "identifiers" | "calls" | "members" | "locate" => "SEARCH",
        "xref" => "XREF",
        _ => "FETCH",
    }
}

/// Synthetic overlays are limited to MCP-specific extraction handlers.
fn input_schema(spec: &reflect::ToolSpec) -> Value {
    let mut schema = reflect::input_schema(spec);
    if matches!(spec.name.as_str(), "asar_extract" | "zip_extract") {
        schema["properties"]["include_bytes"] = serde_json::json!({
            "type": "boolean",
            "description": "Return the extracted bytes beside the receipt."
        });
    }
    schema
}

struct Entry {
    spec: reflect::ToolSpec,
    input_schema: Value,
}
struct Catalog {
    entries: Vec<Entry>,
    index: HashMap<String, usize>,
}
fn reflected() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let entries: Vec<_> = reflect::tools()
            .into_iter()
            .map(|spec| {
                let input_schema = input_schema(&spec);
                Entry { spec, input_schema }
            })
            .collect();
        let index = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| (entry.spec.name.clone(), i))
            .collect();
        Catalog { entries, index }
    })
}
fn entry(name: &str) -> Result<&'static Entry, QueryError> {
    let catalog = reflected();
    catalog
        .index
        .get(name)
        .map(|&i| &catalog.entries[i])
        .ok_or_else(|| QueryError::new("INVALID_QUERY", format!("unknown analysis tool: {name}")))
}

pub fn catalog() -> Vec<ToolDefinition> {
    let result: Vec<ToolDefinition> = reflected()
        .entries
        .iter()
        .map(|entry| ToolDefinition {
            name: entry.spec.name.clone(),
            backend: entry.spec.backend.clone(),
            description: entry.spec.description.clone(),
            input_schema: entry.input_schema.clone(),
            output_schema: schema::output(),
            capabilities: vec![capability(&entry.spec.command).into()],
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

/// Validate typed fields and clap combinations before capture; placeholders stay unopened.
pub fn validate(name: &str, arguments: &Value) -> Result<(), QueryError> {
    #[cfg(feature = "bundled-kuna")]
    if name.starts_with("kuna_") {
        return super::kuna::validate(name, arguments);
    }
    let entry = entry(name)?;
    let spec = &entry.spec;
    let object = arguments
        .as_object()
        .ok_or_else(|| QueryError::new("INVALID_QUERY", "tool arguments must be a JSON object"))?;
    let allowed = &entry.input_schema;
    let properties = allowed["properties"].as_object().expect("tool properties");
    for key in object.keys() {
        if !properties.contains_key(key) {
            return Err(QueryError::new(
                "INVALID_QUERY",
                format!("unknown argument `{key}` for {name}"),
            ));
        }
    }
    for key in allowed["required"].as_array().into_iter().flatten() {
        let key = key.as_str().expect("schema required field");
        if !object.contains_key(key) {
            return Err(QueryError::new(
                "INVALID_QUERY",
                format!("missing required argument `{key}` for {name}"),
            ));
        }
    }
    for (key, value) in object {
        let rule = &properties[key];
        let valid = match rule["type"].as_str() {
            Some("string") => value.is_string(),
            Some("integer") => value.is_u64(),
            Some("boolean") => value.is_boolean(),
            _ => true,
        };
        if !valid
            || value
                .as_u64()
                .zip(rule["minimum"].as_u64())
                .is_some_and(|(value, minimum)| value < minimum)
            || rule["enum"]
                .as_array()
                .is_some_and(|values| !values.contains(value))
        {
            return Err(QueryError::new(
                "INVALID_QUERY",
                format!("invalid `{key}` for {name}: expected {}", rule),
            ));
        }
    }
    let paths = vec![PathBuf::from(".rxd-mcp-validation-input")];
    let argv = reflect::argv(spec, arguments, &paths)?;
    crate::cli::Cli::try_parse_from(argv)
        .map_err(|error| QueryError::new("INVALID_QUERY", error.to_string()))?;
    Ok(())
}

/// Inject the server thread cap only into commands declaring --threads.
fn bound_threads(spec: &reflect::ToolSpec, threads: usize) -> Option<(&'static str, usize)> {
    spec.exposed()
        .any(|arg| arg.key == "threads")
        .then_some(("threads", threads))
}

pub(super) fn argv(
    name: &str,
    arguments: &Value,
    inputs: &[CapturedInput],
    threads: usize,
) -> Result<Vec<std::ffi::OsString>, QueryError> {
    let spec = &entry(name)?.spec;
    let paths: Vec<PathBuf> = inputs.iter().map(|input| input.path.clone()).collect();
    let mut arguments = arguments.clone();
    if let Some((key, ceiling)) = bound_threads(spec, threads) {
        let requested = arguments
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok());
        arguments[key] = serde_json::json!(requested.map_or(ceiling, |value| value.min(ceiling)));
    }
    // Machine-readable output is requested inside `reflect::argv`, before any `--`.
    reflect::argv(spec, &arguments, &paths)
}
