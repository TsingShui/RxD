//! Reflect the clap surface into the typed query catalog.
//!
//! There is exactly one definition of an analysis command: the CLI. Walking the
//! `clap` tree gives every tool's name, description, flags, types and enums, so a
//! renamed or added flag cannot leave the MCP contract behind. Only two things are
//! MCP-specific and therefore live here instead of in clap: captured inputs replace
//! the path positional, and `--output` becomes an artifact channel.

use clap::{Arg, ArgAction, Command};
use serde_json::{Map, Value, json};

/// Backends exposed as typed tools. `skill`, `mcp` and `kuna` are not analysis tools.
const BACKENDS: &[&str] = &[
    "js", "asar", "html", "dex", "jar", "dotnet", "evm", "wasm", "pyc",
];

/// CLI commands with no typed MCP projection: `dex_extract` writes caller paths and
/// supports batch output, which the one-artifact tool contract cannot express.
const CLI_ONLY: &[&str] = &["dex_extract"];

/// CLI flags that describe where a result goes or how a local process runs, not what
/// to analyze. They never appear in a tool schema.
const HIDDEN_LONGS: &[&str] = &["output", "perf", "debug"];

/// Numeric flags keep `integer` in the schema; clap remains the authority on values.
const NUMERIC: &[&str] = &[
    "byte", "depth", "func", "limit", "offset", "release", "threads",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Bool,
    Integer,
    Text,
}

pub(super) struct ArgSpec {
    /// Property name a caller uses: the flag spelling, or the positional's id.
    pub key: String,
    pub long: Option<String>,
    pub help: Option<String>,
    pub kind: Kind,
    pub required: bool,
    pub enum_values: Vec<String>,
}

pub(super) struct ToolSpec {
    pub name: String,
    pub backend: String,
    pub command: String,
    pub description: String,
    pub args: Vec<ArgSpec>,
    /// `--apk`-style commands take an ordered input list instead of one path.
    pub multi_input: bool,
}

impl ToolSpec {
    /// Arguments a caller may set, excluding the captured-input plumbing.
    pub fn exposed(&self) -> impl Iterator<Item = &ArgSpec> {
        self.args.iter()
    }
}

fn kind_of(arg: &Arg) -> Kind {
    if matches!(arg.get_action(), ArgAction::SetTrue) {
        return Kind::Bool;
    }
    let id = arg.get_id().as_str();
    if NUMERIC.contains(&id) || id.starts_with("max_") || id.ends_with("_index") {
        Kind::Integer
    } else {
        Kind::Text
    }
}

fn hidden(arg: &Arg, first_positional: bool) -> bool {
    match arg.get_long() {
        Some(long) => HIDDEN_LONGS.contains(&long) || long.starts_with("max-") || long == "apk",
        // The first positional is always the analyzed path; later ones are selectors.
        None => first_positional,
    }
}

fn arg_spec(arg: &Arg) -> ArgSpec {
    let long = arg.get_long().map(str::to_owned);
    let kind = kind_of(arg);
    ArgSpec {
        key: long
            .as_deref()
            .unwrap_or(arg.get_id().as_str())
            .replace('-', "_"),
        long,
        help: arg.get_help().map(|help| help.to_string()),
        kind,
        required: arg.is_required_set(),
        // A boolean flag is a switch, never a `true`/`false` value enum.
        enum_values: if kind == Kind::Bool {
            Vec::new()
        } else {
            arg.get_possible_values()
                .iter()
                .filter(|value| !value.is_hide_set())
                .map(|value| value.get_name().to_owned())
                .collect()
        },
    }
}

/// `hide_input` is true for a tool's own command (where positional 1 is the analyzed
/// path) and false for a reference-kind subcommand, whose positional is a selector.
fn args(command: &Command, hide_input: bool) -> Vec<ArgSpec> {
    let first = hide_input
        .then(|| {
            command
                .get_positionals()
                .next()
                .map(|arg| arg.get_id().clone())
        })
        .flatten();
    command
        .get_arguments()
        .filter(|arg| {
            let first_positional =
                arg.is_positional() && first.as_ref().is_some_and(|id| id == arg.get_id());
            !hidden(arg, first_positional)
        })
        .map(arg_spec)
        .collect()
}

fn tool(backend: &str, command: &Command) -> ToolSpec {
    let name = format!("{}_{}", backend, command.get_name().replace('-', "_"));
    let multi_input = command
        .get_arguments()
        .any(|arg| arg.get_long() == Some("apk"));
    ToolSpec {
        name,
        backend: backend.to_owned(),
        command: command.get_name().to_owned(),
        description: command
            .get_about()
            .map(|about| about.to_string())
            .or_else(|| command.get_long_about().map(|about| about.to_string()))
            .unwrap_or_default(),
        args: args(command, true),
        multi_input,
    }
}

/// Walk the CLI once and describe every analysis command as a tool.
pub(super) fn tools() -> Vec<ToolSpec> {
    let root = <crate::cli::Cli as clap::CommandFactory>::command();
    let mut tools = Vec::new();
    for backend in root.get_subcommands() {
        if !BACKENDS.contains(&backend.get_name()) {
            continue;
        }
        for command in backend.get_subcommands() {
            if command.get_name() == "help" {
                continue;
            }
            let spec = tool(backend.get_name(), command);
            if CLI_ONLY.contains(&spec.name.as_str()) {
                continue;
            }
            tools.push(spec);
        }
    }
    tools
}

fn property(arg: &ArgSpec) -> Value {
    let mut value = match arg.kind {
        Kind::Bool => json!({"type": "boolean"}),
        Kind::Integer => json!({"type": "integer", "minimum": 0}),
        Kind::Text => json!({"type": "string"}),
    };
    if !arg.enum_values.is_empty() {
        value = json!({"type": "string", "enum": arg.enum_values});
    }
    if let Some(help) = &arg.help {
        value["description"] = Value::from(help.clone());
    }
    value
}

fn input_reference() -> Value {
    json!({"oneOf":[{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"],"additionalProperties":false},
        {"type":"object","properties":{"target_id":{"type":"string","minLength":1}},"required":["target_id"],"additionalProperties":false}]})
}

/// The tool's input schema: captured inputs and the reflected arguments.
///
/// A command's own flags describe the whole surface: `xref` takes `--at`/`--filter`/`--kind`
/// like every other command takes its options, so there is no per-kind selector to project.
pub(super) fn input_schema(spec: &ToolSpec) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    if spec.multi_input {
        properties.insert(
            "inputs".into(),
            json!({"type":"array","minItems":1,"items":input_reference()}),
        );
        required.push("inputs");
    } else {
        properties.insert("input".into(), input_reference());
        required.push("input");
    }
    for arg in spec.exposed() {
        properties.insert(arg.key.clone(), property(arg));
        if arg.required {
            required.push(arg.key.as_str());
        }
    }
    required.sort_unstable();
    required.dedup();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Build the CLI argv for one tool call. `clap` parses and validates the result, so
/// this only has to spell each argument the way the CLI declares it.
pub(super) fn argv(
    spec: &ToolSpec,
    arguments: &Value,
    inputs: &[std::path::PathBuf],
) -> Result<Vec<std::ffi::OsString>, super::QueryError> {
    use std::ffi::OsString;
    let mut argv: Vec<OsString> = vec![
        "rxd".into(),
        spec.backend.clone().into(),
        spec.command.clone().into(),
    ];
    if spec.multi_input {
        for path in inputs {
            argv.push("--apk".into());
            argv.push(path.as_os_str().into());
        }
    } else {
        let Some(path) = inputs.first() else {
            return Err(super::QueryError::new(
                "INVALID_QUERY",
                "query requires a captured input",
            ));
        };
        argv.push(path.as_os_str().into());
    }
    // Parent flags precede a reference-kind subcommand; its own positionals wait.
    let mut positionals: Vec<OsString> = Vec::new();
    for arg in spec.exposed() {
        let Some(value) = arguments.get(&arg.key) else {
            continue;
        };
        match arg.long.as_deref() {
            Some(long) if arg.kind == Kind::Bool => {
                if value.as_bool() == Some(true) {
                    argv.push(format!("--{long}").into());
                }
            }
            Some(long) => argv.push(format!("--{long}={}", value_text(value)).into()),
            None => positionals.push(value_text(value).into()),
        }
    }
    // Typed tools always request machine-readable output when the CLI offers the switch.
    if spec.exposed().any(|arg| arg.key == "json") {
        argv.push("--json".into());
    }
    if !positionals.is_empty() {
        // Selectors are data even when they look like flags.
        argv.push("--".into());
        argv.extend(positionals);
    }
    Ok(argv)
}
