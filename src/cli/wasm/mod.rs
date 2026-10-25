use std::path::PathBuf;

use super::filter::CaseArgs;
use super::xref::Selector;
use clap::{Args, Subcommand};
use rxd_core::Result;
use rxd_core::envelope::{RecordMeta, attach_protocol, record};
use rxd_core::perf::Perf;
use rxd_wasm::{Limits, Module};
use serde_json::{Map, Value, json};

/// One command emits one record kind: `types` only types, `functions` only
/// functions, and so on. There is no mixed structural stream.
#[derive(Subcommand)]
pub enum Command {
    /// Enumerate the type section (function signatures) as JSONL.
    #[command(
        after_long_help = "Examples:\nrxd wasm types module.wasm\nOutput: type JSONL with func signatures and type:N coordinates; these are not functions."
    )]
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
    #[command(
        after_long_help = "Examples:\nrxd wasm imports module.wasm -f env\nOutput: import JSONL; function imports have detail.target_at = func:N for xref.\nImports have no defined body to fetch."
    )]
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
    #[command(
        after_long_help = "Examples:\nrxd wasm exports module.wasm -f sum\nOutput: export JSONL; copy detail.target_at (func:N) for exported function FETCH/xref.\nExport names are aliases, not function identities."
    )]
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
    #[command(after_long_help = "Examples:\nrxd wasm functions module.wasm -f sum\nOutput: function JSONL; at is func:N in the combined imported/defined index space.\nNext: rxd wasm get-function module.wasm --at '<function.at>'.\n--name is exact, -f is a substring; --at optionally selects an existing function.", mut_arg("at", |arg| arg.value_name("FUNCTION_AT").help("Optional existing functions.at coordinate, func:N; omit to list functions")))]
    Functions {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        /// Select a function by a coordinate printed by this module's functions listing.
        #[command(flatten)]
        selector: Selector,
        /// Keep only functions whose name-section label matches exactly.
        /// Exact name-section label; not a substring.
        #[arg(long)]
        name: Option<String>,
        ///  substring over the name-section label or `func:<index>`. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Enumerate the table section as JSONL.
    #[command(
        after_long_help = "Examples:\nrxd wasm tables module.wasm\nOutput: table JSONL with table:N, limits and element types; not func:N."
    )]
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
    #[command(
        after_long_help = "Examples:\nrxd wasm memories module.wasm\nOutput: memory JSONL with memory:N and page limits."
    )]
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
    #[command(
        after_long_help = "Examples:\nrxd wasm globals module.wasm -f counter\nOutput: global JSONL with global:N, type, mutability and initializer evidence."
    )]
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
    #[command(
        after_long_help = "Examples:\nrxd wasm data module.wasm\nOutput: data JSONL with data:N, original segment bytes/ranges and active/passive mode."
    )]
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
    #[command(
        after_long_help = "Examples:\nrxd wasm elements module.wasm\nOutput: element JSONL with element:N and explicit function indices/ref expressions.\nDo not infer resolved call_indirect targets from a table inventory."
    )]
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
    #[command(
        after_long_help = "Examples:\nrxd wasm strings module.wasm -f hello\nOutput: string JSONL from data/name sections. These are not proven string-to-code uses.\nTo navigate code, list functions first and use func:N with xref/get-function."
    )]
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
    #[command(after_long_help = "Examples:\nrxd wasm xref module.wasm --at 'func:1'\nrxd wasm xref module.wasm -m 'env.log' --kind function\nRequired: exactly one of --at/--match; --match requires --kind function.\nCopy functions.at or exports/imports.detail.target_at, not a type/table index.\nOutput: reference JSONL; context_at names the caller, detail.target the function.\nNext: rxd wasm get-function module.wasm --at '<reference.context_at>' --raw.\nOnly explicit call/ref.func; indirect calls remain unresolved.", group(clap::ArgGroup::new("anchor").required(true).args(["at", "filter"])), mut_arg("at", |arg| arg.value_name("FUNCTION_AT").help("Copy functions.at or exports/imports.detail.target_at: func:N")))]
    Xref {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        /// Substring over the target function label; never caller names.
        #[arg(short = 'm', long = "match", requires = "kind", conflicts_with = "at")]
        filter: Option<String>,
        /// Only function references are supported; inferred from --at.
        #[arg(long, value_parser = ["function"])]
        kind: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Get one function. Attempts structured C-like
    /// decompilation by default; falls back to the instruction view.
    #[command(name = "get-function", after_long_help = "Examples:\nrxd wasm functions module.wasm -f sum\nrxd wasm get-function module.wasm --at 'func:1' --raw\nRequired: --at copied from functions.at or xref.context_at; use actual func:N.\nImported functions have no defined body. Not a function name or type:N coordinate.\nOutput: one function_view JSON record. Default attempts C-like reconstruction;\nunsupported cases fall back to instructions. --raw forces original instruction evidence.", mut_arg("at", |arg| arg.required(true).value_name("FUNCTION_AT").help("Copy functions.at or xref.context_at: func:N in this module")))]
    Get {
        /// The regular local `.wasm` file to read.
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        /// Force the raw instruction view instead of attempting decompilation.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        options: Options,
    },
}

#[derive(Args)]
pub struct Options {
    /// Atomically write the result to FILE instead of stdout; never overwrite input.
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Write timing/resource JSONL to stderr without changing the result payload.
    #[arg(long)]
    perf: bool,
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
            | Self::Strings { case, .. }
            | Self::Xref { case, .. } => case.sensitive(),
            Self::Get { .. } => false,
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
            | Self::Strings { filter, .. }
            | Self::Xref { filter, .. } => filter.as_deref(),
            Self::Get { .. } => None,
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
}

pub fn execute(command: Command, policy: rxd_core::BudgetPolicy) -> Result<()> {
    let emit_perf = command.perf();
    let mut perf = Perf::new(emit_perf, "analysis", "wasm", command.name());
    let result = execute_inner(command, policy, &mut perf).map_err(super::classify);
    super::finish(perf, emit_perf, result)
}

fn execute_inner(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    perf: &mut Perf,
) -> anyhow::Result<()> {
    let path = command.input();
    let options = command.options();
    let budget = policy.for_file(path)?;
    let limits = Limits::from(budget);
    perf.resource_limits = Some(std::collections::BTreeMap::from([
        ("input_bytes", budget.input_bytes),
        ("metadata_bytes", budget.metadata_bytes),
    ]));
    for (name, limit) in [("work", budget.work), ("instructions", budget.instructions)] {
        if let Some(limit) = limit {
            perf.resource_limits.as_mut().unwrap().insert(name, limit);
        }
    }
    if let Some(output_bytes) = budget.output_bytes {
        perf.resource_limits
            .as_mut()
            .unwrap()
            .insert("output_bytes", output_bytes);
    }
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
    let mut output = rxd_core::output::Output::new(budget.output_bytes);
    let mut records = 0usize;

    // `get` returns exactly one function_view record, not a header/records/summary stream.
    if let Command::Get { selector, raw, .. } = &command {
        let at = selector.resolve()?;
        let position = module.defined_position_at(&at)?;
        let view = perf.measure("decode", || rxd_wasm::get_view(&module, position, *raw))?;
        let mut value = record(
            "function_view",
            RecordMeta {
                at: Some(&module.function_at(position).id),
                start: Some(view.body_file_offset),
                end: Some(view.body_file_offset + view.body_size),
                ..RecordMeta::default()
            },
            {
                let mut fields = json!({
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
                    "mapping": view.mapping,
                    "complete": true,
                })
                .as_object()
                .unwrap()
                .clone();
                // Include original instructions for --raw or reconstruction fallback.
                if *raw || !view.decompiled {
                    fields.insert("instructions".into(), json!(view.instructions));
                    fields.insert("bytecode_hex".into(), json!(view.bytecode_hex));
                }
                fields
            },
        );
        attach_protocol(&mut value, "wasm", command.name());
        append(&mut output, &value)?;
        perf.record("query", query_start);
        perf.counters.records = Some(1);
        perf.counters.output_bytes = Some(output.len());
        perf.measure("write", || {
            super::write_output(&output, options.output.as_deref())
        })?;
        return Ok(());
    }

    // Resolve identity anchors before filtering: a nonexistent coordinate is never
    // disguised as a successful empty listing. The Module owns the checked bytes.
    let selected_function = if let Command::Functions { selector, .. } = &command {
        if selector.at.is_some() {
            let at = selector.resolve()?;
            let index = module.function_index_at(&at)?;
            anyhow::ensure!(
                index >= module.imported_func_count(),
                rxd_core::Error::failure(
                    rxd_core::Failure::NotFound,
                    format!("no defined function row for func:{index}")
                )
            );
            Some(index)
        } else {
            None
        }
    } else {
        None
    };
    let reference_targets = if let Command::Xref {
        selector,
        filter,
        kind,
        ..
    } = &command
    {
        if let Some(filter) = filter {
            anyhow::ensure!(
                kind.as_deref() == Some("function"),
                rxd_core::Error::failure(
                    rxd_core::Failure::Query,
                    "xref --match requires --kind function"
                )
            );
            let needle = rxd_core::filter::Contains::new(filter, command.case_sensitive());
            let mut targets = Vec::new();
            for import in &module.imports {
                if let Some(index) = import.func_index
                    && needle.matches(&format!("{}.{}", import.module, import.name))
                {
                    targets.push(index);
                }
            }
            targets.extend(
                module
                    .functions
                    .iter()
                    .filter(|f| {
                        needle.matches(
                            f.name
                                .as_deref()
                                .unwrap_or(&coordinate("func", f.func_index)),
                        )
                    })
                    .map(|f| f.func_index),
            );
            Some(targets)
        } else {
            let at = selector.resolve()?;
            let index = module.function_index_at(&at)?;
            anyhow::ensure!(
                kind.as_deref().is_none_or(|k| k == "function"),
                rxd_core::Error::failure(
                    rxd_core::Failure::Query,
                    format!(
                        "--kind {} contradicts function coordinate {}",
                        kind.as_deref().unwrap_or(""),
                        at
                    )
                )
            );
            Some(vec![index])
        }
    } else {
        None
    };
    let filter = command.filter();
    match &command {
        Command::Types { .. } => emit_rows(
            type_rows(&module),
            filter,
            command.case_sensitive(),
            &mut output,
            &mut records,
        )?,
        Command::Imports { .. } => emit_rows(
            import_rows(&module),
            filter,
            command.case_sensitive(),
            &mut output,
            &mut records,
        )?,
        Command::Exports { .. } => emit_rows(
            export_rows(&module),
            filter,
            command.case_sensitive(),
            &mut output,
            &mut records,
        )?,
        Command::Functions { name, .. } => emit_rows(
            function_rows(&module, selected_function, name.as_deref()),
            filter,
            command.case_sensitive(),
            &mut output,
            &mut records,
        )?,
        Command::Tables { .. } => emit_rows(
            table_rows(&module),
            filter,
            command.case_sensitive(),
            &mut output,
            &mut records,
        )?,
        Command::Memories { .. } => emit_rows(
            memory_rows(&module),
            filter,
            command.case_sensitive(),
            &mut output,
            &mut records,
        )?,
        Command::Globals { .. } => emit_rows(
            global_rows(&module),
            filter,
            command.case_sensitive(),
            &mut output,
            &mut records,
        )?,
        Command::Data { .. } => emit_rows(
            data_rows(&module),
            filter,
            command.case_sensitive(),
            &mut output,
            &mut records,
        )?,
        Command::Elements { .. } => emit_rows(
            element_rows(&module),
            filter,
            command.case_sensitive(),
            &mut output,
            &mut records,
        )?,
        Command::Strings { .. } => {
            let strings = perf.measure("strings", || rxd_wasm::strings(&module))?;
            emit_rows(
                string_rows(&strings),
                filter,
                command.case_sensitive(),
                &mut output,
                &mut records,
            )?;
        }
        Command::Xref { .. } => {
            // Multi-target content search scans each body once, never once per target.
            let references = perf.measure("xref", || {
                rxd_wasm::refs(&module, reference_targets.as_deref().unwrap())
            })?;
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

    perf.record("query", query_start);
    perf.counters.records = Some(records);
    perf.counters.output_bytes = Some(output.len());
    perf.measure("write", || {
        super::write_output(&output, options.output.as_deref())
    })?;
    Ok(())
}

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

/// A row whose identity is its `func:<index>` coordinate; the module's identity
/// lives in the stream header.
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
                "type_index": import.type_index,
                "target_at": import.func_index.map(|i| coordinate("func", i))
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
            detail: json!({"name":export.name,"kind":export.kind,"index":export.index,"target_at":coordinate(export.kind, export.index)})
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
    output: &mut rxd_core::output::Output,
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
    output: &mut rxd_core::output::Output,
    records: &mut usize,
    value: &Value,
) -> anyhow::Result<()> {
    append(output, value)?;
    *records += 1;
    Ok(())
}

fn append(output: &mut rxd_core::output::Output, value: &Value) -> anyhow::Result<()> {
    output.json_line(value)?;
    Ok(())
}
