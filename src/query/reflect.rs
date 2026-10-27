//! Reflect clap names, options and help into MCP schemas.
//! Captured inputs and artifact output replace filesystem arguments.

use clap::{Arg, ArgAction, Command};
use serde_json::{Map, Value, json};

/// Built-in backends reflected from clap; Native tools use their own adapter.
const BACKENDS: &[&str] = &["rasc", "jar", "ilspy", "evmole", "wasm", "pyc", "unluac-rs"];

/// The command credits the engine; the record label stays the neutral format name.
fn label(backend: &str) -> &str {
    match backend {
        "unluac-rs" => "lua",
        "evmole" => "evm",
        other => other,
    }
}

/// CLI commands with no typed MCP projection: `rasc_extract` writes caller paths and
/// supports batch output, which the one-artifact tool contract cannot express.
const CLI_ONLY: &[&str] = &["rasc_extract"];

/// Exclude output/process flags and CLI container selectors; MCP uses child targets.
const HIDDEN_LONGS: &[&str] = &["output", "perf", "debug", "json", "help", "entry"];

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
    /// Use the long flag spelling or positional id as the property name.
    pub key: String,
    pub long: Option<String>,
    pub help: Option<String>,
    pub kind: Kind,
    pub required: bool,
    pub enum_values: Vec<String>,
    pub default: Option<Value>,
    pub minimum: u64,
    pub conflicts: Vec<String>,
}

pub(super) struct ToolSpec {
    pub name: String,
    /// Backend record label, which prefixes the tool name.
    pub label: String,
    /// Clap subcommand spelling used to invoke the backend.
    pub backend: String,
    pub command: String,
    pub description: String,
    pub args: Vec<ArgSpec>,
    pub required_groups: Vec<Vec<String>>,
    pub machine_json: bool,
    pub output_required: bool,
}

impl ToolSpec {
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
    if arg.is_hide_set() {
        return true;
    }
    match arg.get_long() {
        Some(long) => HIDDEN_LONGS.contains(&long) || long.starts_with("max-"),
        // The first positional is always the analyzed path; later ones are selectors.
        None => first_positional,
    }
}

fn arg_spec(arg: &Arg, command: &Command) -> ArgSpec {
    let long = arg.get_long().map(str::to_owned);
    let kind = kind_of(arg);
    ArgSpec {
        key: long
            .as_deref()
            .unwrap_or(arg.get_id().as_str())
            .replace('-', "_"),
        long,
        help: arg
            .get_long_help()
            .or_else(|| arg.get_help())
            .map(|help| help.to_string()),
        kind,
        required: arg.is_required_set(),
        minimum: if kind == Kind::Integer {
            // Ask the declared parser whether zero is admitted, rather than guessing
            // from a flag name. This covers positive custom parsers and range parsers.
            let zero = Command::new("value")
                .arg(Arg::new("value").value_parser(arg.get_value_parser().clone()))
                .try_get_matches_from(["value", "0"]);
            u64::from(zero.is_err())
        } else {
            0
        },
        default: arg.get_default_values().first().and_then(|value| {
            let text = value.to_str()?;
            Some(match kind {
                Kind::Bool => Value::Bool(text.parse().ok()?),
                Kind::Integer => Value::from(text.parse::<u64>().ok()?),
                Kind::Text => Value::from(text),
            })
        }),
        conflicts: command
            .get_arg_conflicts_with(arg)
            .into_iter()
            .filter_map(|arg| arg.get_long())
            .filter(|long| !HIDDEN_LONGS.contains(long))
            .map(|long| long.replace('-', "_"))
            .collect(),
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

fn args(command: &Command) -> Vec<ArgSpec> {
    let first = command
        .get_positionals()
        .next()
        .map(|arg| arg.get_id().clone());
    command
        .get_arguments()
        .filter(|arg| {
            let first_positional =
                arg.is_positional() && first.as_ref().is_some_and(|id| id == arg.get_id());
            !hidden(arg, first_positional)
        })
        .map(|arg| arg_spec(arg, command))
        .collect()
}

fn tool(backend: &str, command: &Command) -> ToolSpec {
    let label = label(backend);
    let name = format!("{}_{}", label, command.get_name().replace('-', "_"));
    let args = args(command);
    let required_groups = command
        .get_groups()
        .filter(|group| group.is_required_set())
        .filter_map(|group| {
            let mut group = group.clone();
            if group.is_multiple() {
                return None;
            }
            let keys: Vec<_> = group
                .get_args()
                .filter_map(|id| {
                    command
                        .get_arguments()
                        .find(|arg| arg.get_id() == id)
                        .and_then(|arg| arg.get_long())
                        .map(|long| long.replace('-', "_"))
                })
                .collect();
            (!keys.is_empty()
                && keys
                    .iter()
                    .all(|key| args.iter().any(|arg| &arg.key == key)))
            .then_some(keys)
        })
        .collect();
    let mut description = command
        .get_long_about()
        .or_else(|| command.get_about())
        .map(ToString::to_string)
        .unwrap_or_default();
    if let Some(guide) = command
        .get_after_long_help()
        .or_else(|| command.get_after_help())
    {
        description.push_str("\n\n");
        description.push_str(&guide.to_string());
    }
    description.push_str("\nMCP: input is {path: ABSOLUTE_PATH} or {target_id: ...}; CLI flags become snake_case fields without --. JSON output is automatic; omit json/output/perf.");
    ToolSpec {
        name,
        label: label.to_owned(),
        backend: backend.to_owned(),
        command: command.get_name().to_owned(),
        description,
        args,
        required_groups,
        machine_json: command
            .get_arguments()
            .any(|arg| arg.get_long() == Some("json")),
        output_required: command
            .get_arguments()
            .any(|arg| arg.get_long() == Some("output") && arg.is_required_set()),
    }
}

pub(super) fn tools() -> Vec<ToolSpec> {
    let mut root = <crate::cli::Cli as clap::CommandFactory>::command();
    root.build();
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
        Kind::Integer => json!({"type": "integer", "minimum": arg.minimum}),
        Kind::Text => json!({"type": "string"}),
    };
    if !arg.enum_values.is_empty() {
        value = json!({"type": "string", "enum": arg.enum_values});
    }
    if let Some(default) = &arg.default {
        value["default"] = default.clone();
    }
    if let Some(help) = &arg.help {
        value["description"] = Value::from(help.clone());
    }
    value
}

fn input_reference() -> Value {
    json!({"oneOf":[{"type":"object","properties":{"path":{"type":"string","minLength":1,"description":"Absolute local input path."}},"required":["path"],"additionalProperties":false},
        {"type":"object","properties":{"target_id":{"type":"string","minLength":1}},"required":["target_id"],"additionalProperties":false}]})
}

pub(super) fn input_schema(spec: &ToolSpec) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    properties.insert("input".into(), input_reference());
    required.push("input");
    for arg in spec.exposed() {
        properties.insert(arg.key.clone(), property(arg));
        if arg.required {
            required.push(arg.key.as_str());
        }
    }
    required.sort_unstable();
    required.dedup();
    let mut schema = json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    });
    let mut constraints = Vec::new();
    for group in &spec.required_groups {
        constraints.push(json!({"oneOf": group.iter().map(|key| {
            let other: Vec<_> = group.iter().filter(|other| *other != key)
                .map(|other| json!({"required":[other]})).collect();
            json!({"required":[key], "not":{"anyOf":other}})
        }).collect::<Vec<_>>()}));
    }
    for arg in spec.exposed() {
        for other in &arg.conflicts {
            if &arg.key < other && spec.args.iter().any(|arg| &arg.key == other) {
                constraints.push(json!({"not":{"required":[arg.key, other]}}));
            }
        }
    }
    if !constraints.is_empty() {
        schema["allOf"] = Value::Array(constraints);
    }
    schema
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Build CLI argv; clap validates types and combinations.
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
    let Some(path) = inputs.first() else {
        return Err(super::QueryError::new(
            "INVALID_QUERY",
            "query requires a captured input",
        ));
    };
    argv.push(path.as_os_str().into());
    if spec.output_required {
        // Use a placeholder output only for shared extraction argument validation.
        argv.push("--output=.rxd-mcp-artifact".into());
    }
    // Selectors are data; any later positionals are appended after flags.
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
    if spec.machine_json {
        argv.push("--json".into());
    }
    if !positionals.is_empty() {
        // Selectors are data even when they look like flags.
        argv.push("--".into());
        argv.extend(positionals);
    }
    Ok(argv)
}
