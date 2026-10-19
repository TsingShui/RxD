use std::path::PathBuf;

use clap::{Args, Subcommand};
use rxd_core::envelope::{RecordMeta, SCHEMA_VERSION, header, record, summary};
use rxd_core::perf::Perf;
use rxd_core::{Error, Result};
use rxd_wasm::{Limits, Module};
use serde_json::{Value, json};

#[derive(Subcommand)]
pub enum Command {
    /// Enumerate module structure (types, imports, exports, functions, …) as JSONL.
    Outline {
        input: PathBuf,
        /// Keep only functions whose name-section label matches exactly.
        #[arg(long = "func-name")]
        func_name: Option<String>,
        /// Keep only the function at this merged function index.
        #[arg(long = "func")]
        func: Option<u32>,
        #[command(flatten)]
        options: Options,
    },
    /// Get one content-bound function. Attempts structured C-like
    /// decompilation by default; falls back to the instruction view.
    #[command(name = "get-function")]
    Get {
        input: PathBuf,
        /// Coordinate copied from a `function` record: `func:42`.
        #[arg(long)]
        at: String,
        /// Force the raw instruction view instead of attempting decompilation.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        options: Options,
    },
    /// Enumerate decodable UTF-8 strings in data segments and the name section.
    Strings {
        input: PathBuf,
        #[arg(long)]
        contains: Option<String>,
        #[command(flatten)]
        options: Options,
    },
    /// Find defined functions that explicitly reference a target function index.
    Xref {
        input: PathBuf,
        #[arg(long = "func")]
        func: u32,
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
    /// Input byte cap in bytes (1 through 8589934592); 8 GiB when omitted.
    #[arg(long, value_parser = super::input_bytes)]
    max_input_bytes: Option<usize>,
    #[arg(long, default_value_t = 100_000, value_parser = super::positive)]
    max_entities: usize,
    #[arg(long, default_value_t = 16 * 1024 * 1024, value_parser = super::positive)]
    max_work: usize,
    #[arg(long, default_value_t = 1_000_000, value_parser = super::positive)]
    max_instructions: usize,
    #[arg(long, default_value_t = 100_000, value_parser = super::positive)]
    max_records: usize,
    #[arg(long, default_value_t = 32 * 1024 * 1024, value_parser = super::positive)]
    max_output_bytes: usize,
}

impl Options {
    fn limits(&self) -> Limits {
        Limits {
            input_bytes: super::input_limit(self.max_input_bytes),
            entities: self.max_entities,
            work: self.max_work,
            instructions: self.max_instructions,
            records: self.max_records,
            output_bytes: self.max_output_bytes,
        }
    }
}

impl Command {
    fn options(&self) -> &Options {
        match self {
            Self::Outline { options, .. }
            | Self::Get { options, .. }
            | Self::Strings { options, .. }
            | Self::Xref { options, .. } => options,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Outline { .. } => "outline",
            Self::Get { .. } => "get-function",
            Self::Strings { .. } => "strings",
            Self::Xref { .. } => "xref",
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
    let (path, options) = match &command {
        Command::Outline { input, options, .. }
        | Command::Get { input, options, .. }
        | Command::Strings { input, options, .. }
        | Command::Xref { input, options, .. } => (input, options),
    };
    let limits = options.limits();
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

    // `get` returns exactly one JSON record, not a header/records/summary stream.
    if let Command::Get { at, raw, .. } = &command {
        let at = rxd_core::envelope::full_id("wasm", &module.source_id, at);
        let position = module.defined_position_by_id(&at)?;
        let view = perf.measure("decode", || rxd_wasm::get_view(&module, position, *raw))?;
        let mut value = record(
            "function_view",
            RecordMeta {
                at: Some(&at),
                source_id: Some(&module.source_id),
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
        value["backend"] = json!("wasm");
        value["command"] = json!(command.name());
        value["schema_version"] = json!(SCHEMA_VERSION);
        append(&mut output, &value, limits.output_bytes)?;
        perf.record("query", query_start);
        perf.counters.records = Some(1);
        perf.counters.output_bytes = Some(output.len());
        perf.measure("write", || {
            super::write_output(&output, options.output.as_deref())
        })?;
        return Ok(());
    }

    let query = match &command {
        Command::Outline {
            func_name, func, ..
        } => json!({"func_name": func_name, "func": func}),
        Command::Strings { contains, .. } => json!({"contains": contains}),
        Command::Xref { func, .. } => json!({"func": func}),
        Command::Get { .. } => unreachable!(),
    };
    append(
        &mut output,
        &header(
            "wasm",
            command.name(),
            Some(&module.source_id),
            Some(module.file_bytes),
            query,
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

    match &command {
        Command::Outline {
            func_name, func, ..
        } => outline(
            &module,
            func_name.as_deref(),
            *func,
            &mut output,
            &mut records,
            options,
        )?,
        Command::Strings { contains, .. } => {
            let strings = perf.measure("strings", || {
                rxd_wasm::strings(&module, contains.as_deref())
            })?;
            for string in strings {
                emit_record(
                    &mut output,
                    &mut records,
                    options,
                    &record(
                        "string",
                        RecordMeta {
                            source_id: Some(&module.source_id),
                            ..RecordMeta::default()
                        },
                        json!({
                            "origin": string.origin,
                            "segment_index": string.segment_index,
                            "offset": string.offset,
                            "value": string.value,
                            "bytes": string.bytes
                        })
                        .as_object()
                        .unwrap()
                        .clone(),
                    ),
                )?;
            }
        }
        Command::Xref { func, .. } => {
            let references = perf.measure("xref", || rxd_wasm::refs(&module, *func))?;
            for reference in references {
                emit_record(
                    &mut output,
                    &mut records,
                    options,
                    &record(
                        "reference",
                        RecordMeta {
                            source_id: Some(&module.source_id),
                            context_at: Some(&reference.id),
                            ..RecordMeta::default()
                        },
                        json!({
                            "caller_index": reference.caller_index,
                            "caller_name": reference.caller_name,
                            "offset": reference.offset,
                            "opcode": reference.opcode,
                            "target_index": reference.target_index
                        })
                        .as_object()
                        .unwrap()
                        .clone(),
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

fn outline(
    module: &Module,
    func_name: Option<&str>,
    func: Option<u32>,
    output: &mut Vec<u8>,
    records: &mut usize,
    options: &Options,
) -> anyhow::Result<()> {
    let filtered = func_name.is_some() || func.is_some();
    let keep = |function: &rxd_wasm::Function| -> bool {
        if let Some(name) = func_name
            && function.name.as_deref() != Some(name)
        {
            return false;
        }
        if let Some(index) = func
            && function.func_index != index
        {
            return false;
        }
        true
    };

    if !filtered {
        for ty in &module.func_types {
            emit_record(
                output,
                records,
                options,
                &record(
                    "type",
                    RecordMeta {
                        source_id: Some(&module.source_id),
                        ..RecordMeta::default()
                    },
                    json!({"type_index":ty.type_index,"params":ty.params,"results":ty.results})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )?;
        }
        for import in &module.imports {
            emit_record(
                output,
                records,
                options,
                &record(
                    "import",
                    RecordMeta {
                        source_id: Some(&module.source_id),
                        ..RecordMeta::default()
                    },
                    json!({
                        "module": import.module,
                        "name": import.name,
                        "kind": import.kind,
                        "func_index": import.func_index,
                        "type_index": import.type_index
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )?;
        }
        for export in &module.exports {
            emit_record(
                output,
                records,
                options,
                &record(
                    "export",
                    RecordMeta {
                        source_id: Some(&module.source_id),
                        ..RecordMeta::default()
                    },
                    json!({"name":export.name,"kind":export.kind,"index":export.index})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )?;
        }
    }
    for function in &module.functions {
        if !keep(function) {
            continue;
        }
        emit_record(
            output,
            records,
            options,
            &record(
                "function",
                RecordMeta {
                    source_id: Some(&module.source_id),
                    at: Some(&function.id),
                    start: Some(function.body_file_offset),
                    end: Some(function.body_file_offset + function.body_size),
                    ..RecordMeta::default()
                },
                json!({
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
            ),
        )?;
    }
    if !filtered {
        for table in &module.tables {
            emit_record(
                output,
                records,
                options,
                &record(
                    "table",
                    RecordMeta {
                        source_id: Some(&module.source_id),
                        ..RecordMeta::default()
                    },
                    json!({
                        "index": table.index,
                        "element_type": table.element_type,
                        "initial": table.initial,
                        "maximum": table.maximum
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )?;
        }
        for memory in &module.memories {
            emit_record(
                output,
                records,
                options,
                &record(
                    "memory",
                    RecordMeta {
                        source_id: Some(&module.source_id),
                        ..RecordMeta::default()
                    },
                    json!({
                        "index": memory.index,
                        "initial": memory.initial,
                        "maximum": memory.maximum,
                        "memory64":memory.memory64,
                        "shared": memory.shared
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )?;
        }
        for global in &module.globals {
            emit_record(
                output,
                records,
                options,
                &record(
                    "global",
                    RecordMeta {
                        source_id: Some(&module.source_id),
                        ..RecordMeta::default()
                    },
                    json!({
                        "index": global.index,
                        "value_type": global.value_type,
                        "mutable": global.mutable,
                        "name": global.name
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )?;
        }
        for data in &module.data_segments {
            emit_record(
                output,
                records,
                options,
                &record(
                    "data",
                    RecordMeta {
                        source_id: Some(&module.source_id),
                        ..RecordMeta::default()
                    },
                    json!({
                        "index": data.index,
                        "kind": data.kind,
                        "memory_index": data.memory_index,
                        "offset_expr": data.offset_expr,
                        "byte_length": data.byte_length
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )?;
        }
        for element in &module.element_segments {
            emit_record(
                output,
                records,
                options,
                &record(
                    "element",
                    RecordMeta {
                        source_id: Some(&module.source_id),
                        ..RecordMeta::default()
                    },
                    json!({"index":element.index,"kind":element.kind,"count":element.count})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )?;
        }
    }
    Ok(())
}

fn emit_record(
    output: &mut Vec<u8>,
    records: &mut usize,
    options: &Options,
    value: &Value,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        *records < options.max_records,
        "RESOURCE_LIMIT: record budget exceeded"
    );
    append(output, value, options.max_output_bytes)?;
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
