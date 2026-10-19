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

/// Classify a command by what it does, for tool annotations.
fn capability(command: &str) -> &'static str {
    match command {
        "outline" | "entries" | "classes" | "list" | "analyze" | "interface" | "storage"
        | "disassemble" | "basic-blocks" | "cfg" | "metadata" | "manifest" => "ENUMERATE",
        "strings" | "find" | "identifiers" | "calls" | "members" | "locate" => "SEARCH",
        "xref" => "XREF",
        _ => "FETCH",
    }
}

/// Tools whose handler is MCP-specific rather than an argv projection keep a small
/// synthetic overlay; every other tool is exactly its CLI command.
fn input_schema(spec: &reflect::ToolSpec) -> Value {
    let mut schema = reflect::input_schema(spec);
    if spec.name == "asar_extract" {
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

/// Resource ceilings the server imposes on every call, spelled as the CLI flags the
/// backend actually declares.
fn ceilings(spec: &reflect::ToolSpec, b: QueryBudgets) -> Vec<(&'static str, usize)> {
    let mut limits: Vec<(&'static str, usize)> = Vec::new();
    if spec.backend == "dex" {
        if !matches!(spec.name.as_str(), "dex_manifest" | "dex_entries") {
            limits.push(("threads", b.threads));
        }
        return limits;
    }
    limits.push(("max-input-bytes", b.input_bytes));
    limits.push(("max-output-bytes", b.output_bytes));
    match spec.backend.as_str() {
        "jar" => {
            limits.push(("max-class-bytes", b.input_bytes.min(16 * 1024 * 1024)));
            limits.push(("max-expanded-bytes", b.input_bytes));
            limits.push(("max-entries", b.entities));
            limits.push(("max-work", b.work));
        }
        "js" => {
            limits.push(("max-entities", b.entities));
            limits.push(("max-records", b.records));
            limits.push(("max-ast-depth", b.ast_depth));
            if matches!(spec.name.as_str(), "js_outline" | "js_find") {
                limits.push(("threads", b.threads));
            }
            if matches!(spec.name.as_str(), "js_xref" | "js_symbols" | "js_get") {
                limits.push(("max-semantic-nodes", b.semantic_nodes));
            }
        }
        "html" => {
            limits.push(("max-records", b.records));
            limits.push(("max-depth", b.ast_depth));
        }
        "asar" => {
            limits.push(("max-records", b.records));
            limits.push(("max-entries", b.entities));
            limits.push(("max-header-bytes", b.input_bytes.min(8 * 1024 * 1024)));
        }
        "evm" => {
            limits.push(("max-records", b.records));
            if matches!(
                spec.name.as_str(),
                "evm_outline" | "evm_strings" | "evm_xref" | "evm_get_block" | "evm_get_function"
            ) {
                limits.push(("max-work", b.work));
                limits.push(("max-states", b.states));
                limits.push(("max-stack-cells", b.stack_cells));
                limits.push(("max-reconstruction-work", b.reconstruction_work));
                limits.push(("max-values", b.values));
            }
        }
        _ if spec.name == "pyc_decompile" => {
            limits.push(("max-work", b.work));
            limits.push(("max-ast-depth", b.ast_depth.min(128)));
        }
        _ => {
            limits.push(("max-entities", b.entities));
            limits.push(("max-records", b.records));
            limits.push(("max-work", b.work));
            if spec.backend != "pyc" || matches!(spec.name.as_str(), "pyc_xref" | "pyc_get_code") {
                limits.push(("max-instructions", b.instructions));
            }
        }
    }
    limits
}

pub(super) fn argv(
    name: &str,
    arguments: &Value,
    inputs: &[CapturedInput],
    b: QueryBudgets,
) -> Result<Vec<std::ffi::OsString>, QueryError> {
    let spec = spec(name)?;
    let paths: Vec<PathBuf> = inputs.iter().map(|input| input.path.clone()).collect();
    let limits = ceilings(&spec, b);
    let argv = reflect::argv(&spec, arguments, &paths, &limits)?;
    // Machine-readable output is requested inside `reflect::argv`, before any `--`.
    Ok(argv)
}
