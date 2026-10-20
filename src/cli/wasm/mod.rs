use std::path::PathBuf;

use super::filter::CaseArgs;
use clap::{Args, Subcommand};
use rxd_core::envelope::{RecordMeta, attach_protocol, header, record, summary};
use rxd_core::perf::Perf;
use rxd_core::{Error, Result};
use rxd_wasm::{Limits, Module};
use serde_json::{Map, Value, json};

/// One command emits one record kind: `types` only types, `functions` only
/// functions, and so on. There is no mixed structural stream.
#[derive(Subcommand)]
pub enum Command {
    /// Enumerate the type section (function signatures) as JSONL.
    Types {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        ///  substring over the rendered signature. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Enumerate the import section as JSONL.
    Imports {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        ///  substring over `module.name`. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Enumerate the export section as JSONL.
    Exports {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        ///  substring over the export name. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Enumerate module-defined functions as JSONL.
    Functions {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        /// Keep only the function at this merged function index.
        #[arg(long = "func")]
        func: Option<u32>,
        /// Keep only functions whose name-section label matches exactly.
        #[arg(long = "func-name")]
        func_name: Option<String>,
        ///  substring over the name-section label or `func:<index>`. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Enumerate the table section as JSONL.
    Tables {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        ///  substring over `table:<index>`. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Enumerate the memory section as JSONL.
    Memories {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        ///  substring over `memory:<index>`. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Enumerate the global section as JSONL.
    Globals {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        ///  substring over the name-section label or `global:<index>`. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Enumerate data segments as JSONL.
    Data {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        ///  substring over `data:<index>`. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Enumerate element segments as JSONL.
    Elements {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        ///  substring over `element:<index>`. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Enumerate decodable UTF-8 strings in data segments and the name section.
    Strings {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        ///  substring over the decoded literal. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Find defined functions that explicitly reference a target function index.
    Xref {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        /// The `at` a `functions` row prints: `func:42`.
        #[arg(long)]
        at: String,
        #[command(flatten)]
        options: Options,
    },
    /// Get one content-bound function. Attempts structured C-like
    /// decompilation by default; falls back to the instruction view.
    #[command(name = "get-function")]
    Get {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        /// The `at` a `functions` row prints: `func:42`.
        #[arg(long)]
        at: String,
        /// Force the raw instruction view instead of attempting decompilation.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        options: Options,
    },
}

#[derive(Args)]
pub struct Options {
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    #[arg(long)]
    perf: bool,
    #[arg(long)]
    debug: bool,
}

impl Command {
    fn input(&self) -> &PathBuf {
        match self {
            Self::Types { input, .. }
            | Self::Imports { input, .. }
            | Self::Exports { input, .. }
            | Self::Functions { input, .. }
            | Self::Tables { input, .. }
            | Self::Memories { input, .. }
            | Self::Globals { input, .. }
            | Self::Data { input, .. }
            | Self::Elements { input, .. }
            | Self::Strings { input, .. }
            | Self::Xref { input, .. }
            | Self::Get { input, .. } => input,
        }
    }

    fn options(&self) -> &Options {
        match self {
            Self::Types { options, .. }
            | Self::Imports { options, .. }
            | Self::Exports { options, .. }
            | Self::Functions { options, .. }
            | Self::Tables { options, .. }
            | Self::Memories { options, .. }
            | Self::Globals { options, .. }
            | Self::Data { options, .. }
            | Self::Elements { options, .. }
            | Self::Strings { options, .. }
            | Self::Xref { options, .. }
            | Self::Get { options, .. } => options,
        }
    }

    /// Whether this command's filter folds case.
    fn case_sensitive(&self) -> bool {
        match self {
            Self::Types { case, .. }
            | Self::Imports { case, .. }
            | Self::Exports { case, .. }
            | Self::Functions { case, .. }
            | Self::Tables { case, .. }
            | Self::Memories { case, .. }
            | Self::Globals { case, .. }
            | Self::Data { case, .. }
            | Self::Elements { case, .. }
            | Self::Strings { case, .. } => case.sensitive(),
            Self::Xref { .. } | Self::Get { .. } => false,
        }
    }

    fn filter(&self) -> Option<&str> {
        match self {
            Self::Types { filter, .. }
            | Self::Imports { filter, .. }
            | Self::Exports { filter, .. }
            | Self::Functions { filter, .. }
            | Self::Tables { filter, .. }
            | Self::Memories { filter, .. }
            | Self::Globals { filter, .. }
            | Self::Data { filter, .. }
            | Self::Elements { filter, .. }
            | Self::Strings { filter, .. } => filter.as_deref(),
            Self::Xref { .. } | Self::Get { .. } => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Types { .. } => "types",
            Self::Imports { .. } => "imports",
            Self::Exports { .. } => "exports",
            Self::Functions { .. } => "functions",
            Self::Tables { .. } => "tables",
            Self::Memories { .. } => "memories",
            Self::Globals { .. } => "globals",
            Self::Data { .. } => "data",
            Self::Elements { .. } => "elements",
            Self::Strings { .. } => "strings",
            Self::Xref { .. } => "xref",
            Self::Get { .. } => "get-function",
        }
    }

    pub fn perf(&self) -> bool {
        self.options().perf
    }

    pub fn debug(&self) -> bool {
        self.options().debug
    }
}

pub fn execute(command: Command) -> Result<()> {
    let emit_perf = command.perf();
    let debug = command.debug();
    let mut perf = Perf::new(emit_perf || debug, "analysis", "wasm", command.name());
    let result = execute_inner(command, &mut perf).map_err(|error| {
        let message = format!("{error:#}");
        let code = [
            "RESOURCE_LIMIT",
            "INVALID_QUERY",
            "INVALID_ID",
            "SOURCE_MISMATCH",
            "INVALID_INPUT",
            "UNSUPPORTED_INPUT",
        ]
        .into_iter()
        .find(|code| message.contains(code))
        .unwrap_or("IO_ERROR");
        Error::coded(code, message)
    });
    super::finish(perf, emit_perf, debug, result)
}

fn execute_inner(command: Command, perf: &mut Perf) -> anyhow::Result<()> {
    let path = command.input();
    let options = command.options();
    // One budget table for every backend; the CLI exposes no knobs for it.
    let limits = Limits::default();
    let module = perf.measure("read_parse", || Module::read(path, limits))?;
    perf.source_id = Some(module.source_id.clone());
    perf.counters.input_bytes = Some(module.file_bytes);
    let entities = module.func_types.len()
        + module.imports.len()
        + module.exports.len()
        + module.functions.len()
        + module.tables.len()
        + module.memories.len()
        + module.globals.len()
        + module.data_segments.len()
        + module.element_segments.len();
    perf.counters.entities = Some(entities);

    let query_start = perf.start_stage();
    let mut output = Vec::new();
    let mut records = 0usize;

    // `get` returns exactly one function_view record, not a header/records/summary stream.
    if let Command::Get { at, raw, .. } = &command {
        let position = module.defined_position_at(at)?;
        let view = perf.measure("decode", || rxd_wasm::get_view(&module, position, *raw))?;
        let mut value = record(
            "function_view",
            RecordMeta {
                at: Some(&module.function_at(position).id),
                start: Some(view.body_file_offset),
                end: Some(view.body_file_offset + view.body_size),
                ..RecordMeta::default()
            },
            json!({
                "representation": view.representation,
                "decompiled": view.decompiled,
                "reason": view.reason,
                "code": view.code,
                "func": {
                    "func_index": view.func_index,
                    "type_index": view.type_index,
                    "name": view.name,
                    "params": view.params,
                    "results": view.results
                },
                "body_file_offset": view.body_file_offset,
                "body_size": view.body_size,
                "locals": view.locals,
                "instructions": view.instructions,
                "mapping": view.mapping,
                "bytecode_hex": view.bytecode_hex,
                "complete": true,
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        attach_protocol(&mut value, "wasm", command.name());
        append(&mut output, &value, limits.output_bytes)?;
        perf.record("query", query_start);
        perf.counters.records = Some(1);
        perf.counters.output_bytes = Some(output.len());
        perf.measure("write", || {
            super::write_output(&output, options.output.as_deref())
        })?;
        return Ok(());
    }

    append(
        &mut output,
        &header(
            "wasm",
            command.name(),
            Some(&module.source_id),
            Some(module.file_bytes),
            command_query(&command),
            json!({
                "analysis": "metadata",
                "imported_funcs": module.imported_func_count(),
                "total_funcs": module.total_functions()
            })
            .as_object()
            .unwrap()
            .clone(),
        ),
        limits.output_bytes,
    )?;

    let filter = command.filter();
    match &command {
        Command::Types { .. } => emit_rows(
            type_rows(&module),
            filter,
            command.case_sensitive(),
            limits,
            &mut output,
            &mut records,
        )?,
        Command::Imports { .. } => emit_rows(
            import_rows(&module),
            filter,
            command.case_sensitive(),
            limits,
            &mut output,
            &mut records,
        )?,
        Command::Exports { .. } => emit_rows(
            export_rows(&module),
            filter,
            command.case_sensitive(),
            limits,
            &mut output,
            &mut records,
        )?,
        Command::Functions {
            func, func_name, ..
        } => emit_rows(
            function_rows(&module, *func, func_name.as_deref()),
            filter,
            command.case_sensitive(),
            limits,
            &mut output,
            &mut records,
        )?,
        Command::Tables { .. } => emit_rows(
            table_rows(&module),
            filter,
            command.case_sensitive(),
            limits,
            &mut output,
            &mut records,
        )?,
        Command::Memories { .. } => emit_rows(
            memory_rows(&module),
            filter,
            command.case_sensitive(),
            limits,
            &mut output,
            &mut records,
        )?,
        Command::Globals { .. } => emit_rows(
            global_rows(&module),
            filter,
            command.case_sensitive(),
            limits,
            &mut output,
            &mut records,
        )?,
        Command::Data { .. } => emit_rows(
            data_rows(&module),
            filter,
            command.case_sensitive(),
            limits,
            &mut output,
            &mut records,
        )?,
        Command::Elements { .. } => emit_rows(
            element_rows(&module),
            filter,
            command.case_sensitive(),
            limits,
            &mut output,
            &mut records,
        )?,
        Command::Strings { .. } => {
            let strings = perf.measure("strings", || rxd_wasm::strings(&module))?;
            emit_rows(
                string_rows(&strings),
                filter,
                command.case_sensitive(),
                limits,
                &mut output,
                &mut records,
            )?;
        }
        Command::Xref { at, .. } => {
            // The coordinate names one function in the index space, which is what call
            // operands carry; it is not the position of the body in the code section.
            let target = module.function_index_at(at)?;
            let references = perf.measure("xref", || rxd_wasm::refs(&module, target))?;
            for reference in references {
                let mut detail = Map::new();
                detail.insert(
                    "target".into(),
                    json!(coordinate("func", reference.target_index)),
                );
                detail.insert("caller_index".into(), json!(reference.caller_index));
                detail.insert("caller_name".into(), json!(reference.caller_name));
                detail.insert("offset".into(), json!(reference.offset));
                detail.insert("opcode".into(), json!(reference.opcode));
                detail.insert("target_index".into(), json!(reference.target_index));
                push_limited(
                    &mut output,
                    &mut records,
                    limits,
                    &rxd_core::envelope::reference(
                        RecordMeta {
                            context_at: Some(&reference.id),
                            ..RecordMeta::default()
                        },
                        detail,
                    ),
                )?;
            }
        }
        Command::Get { .. } => unreachable!(),
    }

    append(
        &mut output,
        &summary(records, true, serde_json::Map::new()),
        limits.output_bytes,
    )?;
    perf.record("query", query_start);
    perf.counters.records = Some(records);
    perf.counters.output_bytes = Some(output.len());
    perf.measure("write", || {
        super::write_output(&output, options.output.as_deref())
    })?;
    Ok(())
}

/// The `query` echo: the selectors this command actually took.
fn command_query(command: &Command) -> Value {
    match command {
        Command::Functions {
            func, func_name, ..
        } => json!({"func": func, "func_name": func_name, "filter": command.filter()}),
        Command::Xref { at, .. } => json!({"at": at}),
        _ => json!({"filter": command.filter()}),
    }
}

/// One catalog row before it becomes an envelope record.
///
/// `label` is the value the row is named by, which is what `-f/--filter`
/// matches: the name-section label, else the kind's own descriptive text, else
/// the row's `at` coordinate.
struct Row {
    kind: &'static str,
    at: Option<String>,
    start: Option<usize>,
    end: Option<usize>,
    label: String,
    detail: Map<String, Value>,
}

/// The copy-only coordinate a row prints. It is the format's own index, so it is
/// self-describing and needs no content hash to be read.
fn coordinate(kind: &str, index: u32) -> String {
    format!("{kind}:{index}")
}

/// A row whose identity is its content-bound function id; `at_of` shortens it to
/// the `func:<index>` coordinate, leaving the hash in the stream header only.
fn function_row(function: &rxd_wasm::Function) -> Row {
    Row {
        kind: "function",
        at: Some(function.id.clone()),
        start: Some(function.body_file_offset),
        end: Some(function.body_file_offset + function.body_size),
        label: function
            .name
            .clone()
            .unwrap_or_else(|| coordinate("func", function.func_index)),
        detail: json!({
            "func_index": function.func_index,
            "type_index": function.type_index,
            "name": function.name,
            "params": function.params,
            "results": function.results,
            "body_file_offset": function.body_file_offset,
            "body_size": function.body_size
        })
        .as_object()
        .unwrap()
        .clone(),
    }
}

fn type_rows(module: &Module) -> Vec<Row> {
    module
        .func_types
        .iter()
        .map(|ty| Row {
            kind: "type",
            at: Some(coordinate("type", ty.type_index)),
            start: None,
            end: None,
            label: format!("({})->({})", ty.params.join(","), ty.results.join(",")),
            detail: json!({
                "type_index": ty.type_index,
                "params": ty.params,
                "results": ty.results
            })
            .as_object()
            .unwrap()
            .clone(),
        })
        .collect()
}

fn import_rows(module: &Module) -> Vec<Row> {
    module
        .imports
        .iter()
        .enumerate()
        .map(|(position, import)| Row {
            kind: "import",
            at: Some(coordinate("import", position as u32)),
            start: None,
            end: None,
            label: format!("{}.{}", import.module, import.name),
            detail: json!({
                "module": import.module,
                "name": import.name,
                "kind": import.kind,
                "func_index": import.func_index,
                "type_index": import.type_index
            })
            .as_object()
            .unwrap()
            .clone(),
        })
        .collect()
}

fn export_rows(module: &Module) -> Vec<Row> {
    module
        .exports
        .iter()
        .enumerate()
        .map(|(position, export)| Row {
            kind: "export",
            at: Some(coordinate("export", position as u32)),
            start: None,
            end: None,
            label: export.name.clone(),
            detail: json!({"name":export.name,"kind":export.kind,"index":export.index})
                .as_object()
                .unwrap()
                .clone(),
        })
        .collect()
}

fn function_rows(module: &Module, func: Option<u32>, func_name: Option<&str>) -> Vec<Row> {
    module
        .functions
        .iter()
        .filter(|function| func.is_none_or(|index| function.func_index == index))
        .filter(|function| func_name.is_none_or(|name| function.name.as_deref() == Some(name)))
        .map(function_row)
        .collect()
}

fn table_rows(module: &Module) -> Vec<Row> {
    module
        .tables
        .iter()
        .map(|table| Row {
            kind: "table",
            at: Some(coordinate("table", table.index)),
            start: None,
            end: None,
            label: coordinate("table", table.index),
            detail: json!({
                "index": table.index,
                "element_type": table.element_type,
                "initial": table.initial,
                "maximum": table.maximum
            })
            .as_object()
            .unwrap()
            .clone(),
        })
        .collect()
}

fn memory_rows(module: &Module) -> Vec<Row> {
    module
        .memories
        .iter()
        .map(|memory| Row {
            kind: "memory",
            at: Some(coordinate("memory", memory.index)),
            start: None,
            end: None,
            label: coordinate("memory", memory.index),
            detail: json!({
                "index": memory.index,
                "initial": memory.initial,
                "maximum": memory.maximum,
                "memory64":memory.memory64,
                "shared": memory.shared
            })
            .as_object()
            .unwrap()
            .clone(),
        })
        .collect()
}

fn global_rows(module: &Module) -> Vec<Row> {
    module
        .globals
        .iter()
        .map(|global| Row {
            kind: "global",
            at: Some(coordinate("global", global.index)),
            start: None,
            end: None,
            label: global
                .name
                .clone()
                .unwrap_or_else(|| coordinate("global", global.index)),
            detail: json!({
                "index": global.index,
                "value_type": global.value_type,
                "mutable": global.mutable,
                "name": global.name
            })
            .as_object()
            .unwrap()
            .clone(),
        })
        .collect()
}

fn data_rows(module: &Module) -> Vec<Row> {
    module
        .data_segments
        .iter()
        .map(|data| Row {
            kind: "data",
            at: Some(coordinate("data", data.index)),
            start: None,
            end: None,
            label: coordinate("data", data.index),
            detail: json!({
                "index": data.index,
                "kind": data.kind,
                "memory_index": data.memory_index,
                "offset_expr": data.offset_expr,
                "byte_length": data.byte_length
            })
            .as_object()
            .unwrap()
            .clone(),
        })
        .collect()
}

fn element_rows(module: &Module) -> Vec<Row> {
    module
        .element_segments
        .iter()
        .map(|element| Row {
            kind: "element",
            at: Some(coordinate("element", element.index)),
            start: None,
            end: None,
            label: coordinate("element", element.index),
            detail: json!({"index":element.index,"kind":element.kind,"count":element.count})
                .as_object()
                .unwrap()
                .clone(),
        })
        .collect()
}

fn string_rows(strings: &[rxd_wasm::StringRecord]) -> Vec<Row> {
    strings
        .iter()
        .enumerate()
        .map(|(index, string)| Row {
            kind: "string",
            // A string has no index space of its own, so its row carries no coordinate.
            at: None,
            start: None,
            end: None,
            label: match (&string.value, &string.bytes) {
                (Some(value), _) => value.clone(),
                (None, Some(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
                (None, None) => String::new(),
            },
            detail: json!({
                // The backend's own identifier for this string in its inventory.
                "index": index,
                "origin": string.origin,
                "segment_index": string.segment_index,
                "offset": string.offset,
                "value": string.value,
                "bytes": string.bytes
            })
            .as_object()
            .unwrap()
            .clone(),
        })
        .collect()
}

fn emit_rows(
    rows: Vec<Row>,
    filter: Option<&str>,
    case_sensitive: bool,
    limits: Limits,
    output: &mut Vec<u8>,
    records: &mut usize,
) -> anyhow::Result<()> {
    let needle = filter.map(|needle| rxd_core::filter::Contains::new(needle, case_sensitive));
    for row in rows {
        if let Some(needle) = &needle
            && !needle.matches(&row.label)
        {
            continue;
        }
        push_limited(
            output,
            records,
            limits,
            &record(
                row.kind,
                RecordMeta {
                    at: row.at.as_deref(),
                    start: row.start,
                    end: row.end,
                    ..RecordMeta::default()
                },
                row.detail,
            ),
        )?;
    }
    Ok(())
}

fn push_limited(
    output: &mut Vec<u8>,
    records: &mut usize,
    limits: Limits,
    value: &Value,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        *records < limits.records,
        "RESOURCE_LIMIT: record budget exceeded"
    );
    append(output, value, limits.output_bytes)?;
    *records += 1;
    Ok(())
}

fn append(output: &mut Vec<u8>, value: &Value, limit: usize) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    anyhow::ensure!(
        bytes.len().saturating_add(1) <= limit.saturating_sub(output.len()),
        "RESOURCE_LIMIT: output byte budget exceeded"
    );
    output.extend_from_slice(&bytes);
    output.push(b'\n');
    Ok(())
}
