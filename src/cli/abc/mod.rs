//! Dynamic Ark ABC/HAP navigation; source records are not source-language classes.
use std::path::{Path, PathBuf};

use super::{
    Output,
    filter::CaseArgs,
    table::{self, Cell},
};
use clap::{Args, Subcommand};
use rxd_ark::{Abc, Coordinate, Input, ReconstructionWarning};
use rxd_core::{Budget, BudgetPolicy, Error, Limits, Result, perf::Perf};
use serde_json::json;

#[derive(Subcommand)]
pub enum Command {
    /// Navigate module records and their methods.
    List {
        #[command(subcommand)]
        command: ListCommand,
    },
    #[command(
        about = "Reconstruct an ABC, record or method; --raw fetches original instructions.",
        after_long_help = "Examples:\n  rxd ark decompile modules.abc\n  rxd ark decompile app.hap --entry ets/modules.abc\n  rxd ark decompile app.hap --at '<method.at>'\n  rxd ark decompile modules.abc --at '<record.at>' --raw\nWithout --at, reconstruct all code-bearing records in the selected ABC. For multiple records or non-scalar module names, exports are namespaced to keep the combined source parseable; cross-module linkage is not recovered.\n--at selects one record or method; --raw fetches original instructions instead of source.\nA record decompiles its module initializer and required definitions with imports and exports.\nMethods are returned as capture factories with {env, hole, receiver, newTarget, locals, home, function} context.\nBest-effort runtime semantics and unrecovered source structure are marked with warning comments at affected operations.\nDynamic ABC versions are admitted by format; uncertain version semantics carry a source warning.\nPinned dynamic opcodes, including legacy and runtime forms, are reconstructed without substituting empty bodies or disassembly."
    )]
    Decompile(DecompileOptions),
    #[command(
        about = "Export the whole container and every ABC record for text search.",
        after_long_help = "Examples:\n  rxd ark export app.hap -o recovered\n  rxd ark export modules.abc -o recovered\n  rxd ark export app.hap -o recovered --entry ets/widgets.abc\n  rg -n -C 3 'Authorization' recovered/decompiled\nOutput: a new directory with extracted/ (original HAP files), decompiled/<abc path>/<record>.js and manifest.jsonl.\nEach record file keeps its original module form and every coded method, including methods the initializer never references.\nmanifest.jsonl maps each record coordinate to its file, name, coded-method count and warning codes; the ABC line carries the full ABC SHA-256.\nOmit --entry to export every .abc entry; --entry restricts the export to one exact path.\nRecords decompile in parallel; each worker holds one parsed copy of its ABC, so memory scales with --threads.\nThe export is atomic: an existing output directory is refused and any failure removes the new directory.\nRecord sources remain best-effort reconstruction; follow warnings at affected operations and never execute them."
    )]
    Export(ExportOptions),
}

#[derive(Subcommand)]
pub enum ListCommand {
    #[command(
        about = "List module records.",
        long_about = "Columns: at, name, methods\nList module records after one # column header.",
        after_long_help = "Examples:\n  rxd ark list record modules.abc\n  rxd ark list record app.hap -f entry\nOutput: one row per file record, including metadata records with no methods.\nCopy at into list method --at. Record names describe modules, not recovered source classes."
    )]
    Record(ListOptions),
    #[command(
        about = "List methods of one record.",
        long_about = "Columns: at, name, code_size, args, vregs\nList methods after one # column header.",
        after_long_help = "Examples:\n  rxd ark list record app.hap\n  rxd ark list method app.hap --at 'record:0x1234'\nRequired: --at copied from list record. code_size is bytecode bytes; args includes VM parameters.\nMissing code metadata is \\N; filtered zero hits are a complete empty result."
    )]
    Method(MethodOptions),
}

#[derive(Args)]
pub struct Options {
    /// Dynamic ABC file or HAP/ZIP containing ABC entries.
    input: PathBuf,
    /// Exact ABC path inside HAP; required when more than one ABC exists.
    #[arg(long)]
    entry: Option<String>,
    /// Atomically write the result to FILE instead of stdout.
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Emit phase timings as JSONL on stderr.
    #[arg(long)]
    perf: bool,
}

#[derive(Args)]
pub struct ListOptions {
    #[command(flatten)]
    options: Options,
    /// Substring over names, folded unless --case sensitive.
    #[arg(short = 'f', long)]
    filter: Option<String>,
    #[command(flatten)]
    case: CaseArgs,
}

#[derive(Args)]
pub struct MethodOptions {
    #[command(flatten)]
    list: ListOptions,
    /// Copy an existing record coordinate from list record.
    #[arg(long)]
    at: String,
}

#[derive(Args)]
pub struct DecompileOptions {
    #[command(flatten)]
    options: Options,
    /// Optional record or method coordinate; omit to reconstruct the selected ABC.
    #[arg(long)]
    at: Option<String>,
    /// Return original instructions instead of reconstructed source.
    #[arg(long)]
    raw: bool,
}

#[derive(Args)]
pub struct ExportOptions {
    /// Dynamic ABC file or HAP/ZIP containing ABC entries.
    input: PathBuf,
    /// New directory receiving extracted files, record sources and manifest.jsonl.
    #[arg(short = 'o', long, value_name = "DIR")]
    output: PathBuf,
    /// Exact ABC path inside HAP; omit to export every ABC entry.
    #[arg(long)]
    entry: Option<String>,
    /// Parallel record workers; each worker parses its own copy of the ABC.
    #[arg(long, default_value_t = super::default_threads(), value_parser = super::positive)]
    threads: usize,
    /// Emit phase timings as JSONL on stderr.
    #[arg(long)]
    perf: bool,
}

impl Command {
    pub fn perf(&self) -> bool {
        match self {
            Self::Decompile(options) => options.options.perf,
            Self::Export(options) => options.perf,
            Self::List {
                command: ListCommand::Record(list),
            } => list.options.perf,
            Self::List {
                command: ListCommand::Method(methods),
            } => methods.list.options.perf,
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::List {
                command: ListCommand::Record(_),
            } => "list-record",
            Self::List {
                command: ListCommand::Method(_),
            } => "list-method",
            Self::Decompile(_) => "decompile",
            Self::Export(_) => "export",
        }
    }
    fn input(&self) -> &PathBuf {
        match self {
            Self::Decompile(options) => &options.options.input,
            Self::Export(options) => &options.input,
            Self::List {
                command: ListCommand::Record(list),
            } => &list.options.input,
            Self::List {
                command: ListCommand::Method(methods),
            } => &methods.list.options.input,
        }
    }
    fn entry(&self) -> Option<&str> {
        match self {
            Self::Decompile(options) => options.options.entry.as_deref(),
            Self::Export(options) => options.entry.as_deref(),
            Self::List {
                command: ListCommand::Record(list),
            } => list.options.entry.as_deref(),
            Self::List {
                command: ListCommand::Method(methods),
            } => methods.list.options.entry.as_deref(),
        }
    }
    fn output_file(&self) -> Option<&PathBuf> {
        match self {
            Self::Decompile(options) => options.options.output.as_ref(),
            Self::Export(_) => None,
            Self::List {
                command: ListCommand::Record(list),
            } => list.options.output.as_ref(),
            Self::List {
                command: ListCommand::Method(methods),
            } => methods.list.options.output.as_ref(),
        }
    }
}

fn append(output: &mut Vec<u8>, cells: &[Cell<'_>], budget: Budget) -> Result<()> {
    let line = table::line(cells).map_err(super::classify)?;
    let size = output
        .len()
        .checked_add(line.len())
        .filter(|size| budget.output_bytes.is_none_or(|limit| *size <= limit))
        .ok_or_else(|| Error::new("RESOURCE_LIMIT", "ABC output byte budget exceeded"))?;
    output
        .try_reserve(size - output.len())
        .map_err(|_| Error::new("RESOURCE_LIMIT", "ABC output allocation failed"))?;
    output.extend_from_slice(line.as_bytes());
    Ok(())
}

pub fn execute(command: Command, policy: BudgetPolicy, sink: &mut Output<'_>) -> Result<()> {
    let enabled = command.perf();
    let mut perf = Perf::new(enabled, "analysis", "abc", command.name());
    if let Command::Export(options) = &command {
        let result = export(options, policy, &mut perf, sink);
        return super::finish(perf, enabled, result);
    }
    let input_path = command.input().clone();
    let entry = command.entry().map(str::to_owned);
    let output_path = command.output_file().cloned();
    let result = (|| {
        if let Some(output) = &output_path
            && let (Ok(input), Ok(output)) = (input_path.canonicalize(), output.canonicalize())
            && input == output
        {
            return Err(Error::new(
                "INVALID_QUERY",
                "output must not replace the input ABC/HAP",
            ));
        }
        let input = Input::read(&input_path, entry.as_deref(), policy, &mut perf)?;
        perf.limits = Some(Limits::from(input.budget));
        let abc = perf.measure("parse", || Abc::parse(&input.bytes, input.budget))?;
        perf.source_id = Some(abc.source_id.clone());
        perf.counters.input_bytes = Some(input.bytes.len());
        let mut bytes = match &command {
            Command::List {
                command: ListCommand::Record(_),
            } => table::header(&["at", "name", "methods"]).into_bytes(),
            Command::List {
                command: ListCommand::Method(_),
            } => table::header(&["at", "name", "code_size", "args", "vregs"]).into_bytes(),
            Command::Decompile(_) => Vec::new(),
            Command::Export(_) => unreachable!("export is handled before input selection"),
        };
        if input
            .budget
            .output_bytes
            .is_some_and(|limit| bytes.len() > limit)
        {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "ABC output byte budget exceeded",
            ));
        }
        perf.measure("query", || -> Result<()> {
            match &command {
                Command::Decompile(options) => {
                    let at = options.at.as_deref().map(Coordinate::parse).transpose()?;
                    bytes = if options.raw {
                        if let Some(at) = at {
                            abc.raw_target(at)?
                        } else {
                            abc.raw_all()?
                        }
                    } else {
                        let reconstruction = if let Some(at) = at {
                            abc.decompile_target(at)?
                        } else {
                            abc.decompile_all()?
                        };
                        for warning in reconstruction.warnings {
                            match warning {
                                // The warning is already attached to each operation in source.
                                ReconstructionWarning::IteratorRuntimeSemantics
                                | ReconstructionWarning::PropertyEnumeration
                                | ReconstructionWarning::ArraySpread
                                | ReconstructionWarning::SpreadCall
                                | ReconstructionWarning::ClassReconstruction
                                | ReconstructionWarning::GeneratorReconstruction
                                | ReconstructionWarning::ControlFlowReconstruction
                                | ReconstructionWarning::PrivateProperties
                                | ReconstructionWarning::RuntimeBindings
                                | ReconstructionWarning::JavaScriptIntrinsics
                                | ReconstructionWarning::AssumedBranchSemantics
                                | ReconstructionWarning::SendableSemantics
                                | ReconstructionWarning::LazyModuleTiming
                                | ReconstructionWarning::AsyncFrameApproximation
                                | ReconstructionWarning::ExceptionTypeSemantics
                                | ReconstructionWarning::CoroutineAliasing
                                | ReconstructionWarning::FrameConventions
                                | ReconstructionWarning::IteratorOperations
                                | ReconstructionWarning::SendableLocalModule
                                | ReconstructionWarning::VersionSemantics
                                | ReconstructionWarning::WholeAbcModules
                                | ReconstructionWarning::ModuleNameEncoding
                                | ReconstructionWarning::ModuleNameSnapshot => {}
                            }
                        }
                        reconstruction.source
                    };
                }
                Command::List {
                    command: ListCommand::Record(list),
                } => {
                    let filter = list.filter.as_deref().map(|needle| {
                        rxd_core::filter::Contains::new(needle, list.case.sensitive())
                    });
                    for offset in abc.record_offsets() {
                        let record = abc.record(offset)?;
                        if filter
                            .as_ref()
                            .is_some_and(|filter| !filter.matches(&record.name.text))
                        {
                            continue;
                        }
                        append(
                            &mut bytes,
                            &[
                                Cell::Atom(&format!("record:0x{offset:x}")),
                                record
                                    .name
                                    .scalar_text()
                                    .map_or(Cell::Atom(&record.name.quoted()), Cell::Text),
                                Cell::Atom(&record.methods.to_string()),
                            ],
                            input.budget,
                        )?;
                    }
                }
                Command::List {
                    command: ListCommand::Method(methods),
                } => {
                    let Coordinate::Record(offset) = Coordinate::parse(&methods.at)? else {
                        return Err(Error::new(
                            "INVALID_QUERY",
                            "list method --at requires a record coordinate",
                        ));
                    };
                    let record = abc.record(offset)?;
                    let filter = methods.list.filter.as_deref().map(|needle| {
                        rxd_core::filter::Contains::new(needle, methods.list.case.sensitive())
                    });
                    for method in &abc.members(&record)?.methods {
                        if filter
                            .as_ref()
                            .is_some_and(|filter| !filter.matches(&method.name.text))
                        {
                            continue;
                        }
                        let code = method.code_offset.map(|_| abc.code(method)).transpose()?;
                        let size = code.as_ref().map(|code| code.bytes.len().to_string());
                        let args = code.as_ref().map(|code| code.args.to_string());
                        let vregs = code.as_ref().map(|code| code.vregs.to_string());
                        append(
                            &mut bytes,
                            &[
                                Cell::Atom(&format!("method:0x{:x}", method.offset)),
                                method
                                    .name
                                    .scalar_text()
                                    .map_or(Cell::Atom(&method.name.quoted()), Cell::Text),
                                size.as_deref().map_or(Cell::Null, Cell::Atom),
                                args.as_deref().map_or(Cell::Null, Cell::Atom),
                                vregs.as_deref().map_or(Cell::Null, Cell::Atom),
                            ],
                            input.budget,
                        )?;
                    }
                }
                Command::Export(_) => unreachable!("export is handled before input selection"),
            }
            Ok(())
        })?;
        perf.counters.output_bytes = Some(bytes.len());
        perf.measure("write", || {
            super::write_output(&bytes, output_path.as_deref(), sink)
        })
    })();
    super::finish(perf, enabled, result)
}

struct AbcEntry {
    relative: String,
    path: PathBuf,
}

fn budget_for(policy: BudgetPolicy, bytes: usize) -> Result<Budget> {
    match policy {
        BudgetPolicy::InputScaled => Budget::for_input_bytes(bytes),
        BudgetPolicy::Fixed(budget) => Ok(budget),
    }
}

fn read_head(path: &Path) -> Result<[u8; 4]> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut head = [0u8; 4];
    file.read_exact(&mut head).map_err(|_| {
        Error::new(
            "INVALID_INPUT",
            "input is shorter than a HAP archive or ABC header",
        )
    })?;
    Ok(head)
}

fn collect_abc_files(root: &Path, relative: &Path, files: &mut Vec<String>) -> Result<()> {
    let mut entries: Vec<_> =
        std::fs::read_dir(root.join(relative))?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let kind = entry.file_type()?;
        let path = relative.join(entry.file_name());
        if kind.is_dir() {
            collect_abc_files(root, &path, files)?;
        } else if kind.is_file() {
            if path.extension().is_some_and(|extension| extension == "abc") {
                let text = path.to_str().ok_or_else(|| {
                    Error::new("UNSUPPORTED_INPUT", "ABC entry path is not UTF-8")
                })?;
                files.push(text.replace('\\', "/"));
            }
        } else {
            return Err(Error::new(
                "UNSUPPORTED_INPUT",
                "extracted entry is not a regular file or directory",
            ));
        }
    }
    Ok(())
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    std::io::Write::write_all(&mut file, bytes)?;
    Ok(())
}

fn push_json(output: &mut Vec<u8>, value: &serde_json::Value) -> Result<()> {
    serde_json::to_writer(&mut *output, value)
        .map_err(|error| Error::new("INTERNAL_ERROR", error.to_string()))?;
    output.push(b'\n');
    Ok(())
}

fn units_hex(units: &[u16]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(units.len() * 4);
    for unit in units {
        write!(text, "{unit:04x}").expect("string write");
    }
    text
}

struct ExportedRecord {
    at: String,
    name: String,
    name_utf16: Option<String>,
    methods: usize,
    warnings: Vec<&'static str>,
    output: String,
    bytes: usize,
}

fn export_record(
    abc: &Abc<'_>,
    offset: u32,
    directory: &str,
    output: &Path,
) -> Result<Option<ExportedRecord>> {
    let record = abc.record(offset)?;
    let members = abc.members(&record)?;
    let methods = members
        .methods
        .iter()
        .filter(|method| method.code_offset.is_some())
        .count();
    if methods == 0 {
        return Ok(None);
    }
    let reconstruction = abc.decompile_record_all(&record)?;
    let output_path = format!("{directory}/0x{offset:08x}.js");
    write_file(&output.join(&output_path), &reconstruction.source)?;
    let scalar = record.name.scalar_text();
    Ok(Some(ExportedRecord {
        at: format!("record:0x{offset:x}"),
        name: scalar.unwrap_or(&record.name.text).to_owned(),
        name_utf16: scalar.is_none().then(|| units_hex(&record.name.units)),
        methods,
        warnings: reconstruction
            .warnings
            .iter()
            .map(|warning| warning.code())
            .collect(),
        output: output_path,
        bytes: reconstruction.source.len(),
    }))
}

fn parallel_records<'a>(
    pool: &rayon::ThreadPool,
    bytes: &'a [u8],
    budget: Budget,
    offsets: &[u32],
    directory: &str,
    output: &Path,
) -> Result<Vec<Option<ExportedRecord>>> {
    use rayon::prelude::*;
    pool.install(|| {
        offsets
            .par_iter()
            .map_init(
                || None::<Abc<'a>>,
                |worker, offset| -> Result<Option<ExportedRecord>> {
                    if worker.is_none() {
                        *worker = Some(Abc::parse(bytes, budget)?);
                    }
                    let abc = worker.as_ref().expect("worker ABC is initialized");
                    export_record(abc, *offset, directory, output)
                },
            )
            .collect()
    })
}

fn export(
    options: &ExportOptions,
    policy: BudgetPolicy,
    perf: &mut Perf,
    sink: &mut Output<'_>,
) -> Result<()> {
    let destination = &options.output;
    if destination.symlink_metadata().is_ok() {
        return Err(Error::new(
            "INVALID_QUERY",
            "output directory must not already exist",
        ));
    }
    std::fs::create_dir(destination)?;
    let result = export_into(options, policy, perf).and_then(|receipt| {
        perf.counters.output_bytes = Some(receipt.len());
        perf.measure("write", || super::write_output(&receipt, None, sink))
    });
    if result.is_err() {
        let _ = std::fs::remove_dir_all(destination);
    }
    result
}

fn export_into(options: &ExportOptions, policy: BudgetPolicy, perf: &mut Perf) -> Result<Vec<u8>> {
    let metadata = std::fs::metadata(&options.input)?;
    if !metadata.is_file() {
        return Err(Error::new("INVALID_QUERY", "expected a regular local file"));
    }
    let pool = if options.threads > 1 {
        Some(
            rayon::ThreadPoolBuilder::new()
                .num_threads(options.threads)
                .build()
                .map_err(|error| {
                    Error::new(
                        "RESOURCE_LIMIT",
                        format!("cannot start {} export workers: {error}", options.threads),
                    )
                })?,
        )
    } else {
        None
    };
    let container = read_head(&options.input)?.starts_with(b"PK");
    if !container && options.entry.is_some() {
        return Err(Error::new(
            "INVALID_QUERY",
            "--entry requires a HAP/ZIP container",
        ));
    }
    let output = &options.output;
    let mut entries = Vec::new();
    let (mut extracted_files, mut extracted_bytes) = (0usize, 0u64);
    if container {
        let budget = policy.for_file(&options.input)?;
        perf.limits = Some(Limits::from(budget));
        let root = output.join("extracted");
        let unpacked = rxd_zip::unpack(&options.input, &root, budget, perf)?;
        extracted_files = unpacked.files;
        extracted_bytes = unpacked.bytes;
        let mut files = Vec::new();
        collect_abc_files(&root, Path::new(""), &mut files)?;
        if let Some(entry) = options.entry.as_deref() {
            if !entry.ends_with(".abc") {
                return Err(Error::new(
                    "INVALID_QUERY",
                    "--entry must select a .abc path",
                ));
            }
            if !files.iter().any(|path| path == entry) {
                return Err(Error::new(
                    "NOT_FOUND",
                    format!("container has no .abc entry {entry:?}"),
                ));
            }
            files.retain(|path| path == entry);
        }
        if files.is_empty() {
            return Err(Error::new("NOT_FOUND", "container has no .abc entry"));
        }
        for relative in files {
            entries.push(AbcEntry {
                path: root.join(&relative),
                relative,
            });
        }
    } else {
        let name = options
            .input
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| Error::new("UNSUPPORTED_INPUT", "input file name is not UTF-8"))?;
        entries.push(AbcEntry {
            path: options.input.clone(),
            relative: name.to_owned(),
        });
    }
    let mut manifest = Vec::new();
    let (mut records, mut methods, mut files, mut source_bytes, mut input_bytes): (
        usize,
        usize,
        usize,
        usize,
        usize,
    ) = (0, 0, 0, 0, 0);
    for entry in &entries {
        let length = std::fs::metadata(&entry.path)?.len();
        let length = usize::try_from(length)
            .map_err(|_| Error::new("RESOURCE_LIMIT", "ABC length is not addressable"))?;
        let budget = budget_for(policy, length)?;
        perf.limits = Some(Limits::from(budget));
        let bytes = perf.measure("read", || {
            rxd_core::read_bytes(&entry.path, budget.input_bytes)
        })?;
        input_bytes = input_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| Error::new("RESOURCE_LIMIT", "ABC input byte count overflow"))?;
        let abc = perf.measure("parse", || Abc::parse(&bytes, budget))?;
        let directory = PathBuf::from("decompiled").join(&entry.relative);
        let directory_text = directory.to_string_lossy().replace('\\', "/");
        let mut offsets: Vec<u32> = abc.record_offsets().collect();
        offsets.sort_unstable();
        let exported = match &pool {
            Some(pool) if offsets.len() > 1 => perf.measure("decompile", || {
                parallel_records(pool, &bytes, budget, &offsets, &directory_text, output)
            })?,
            _ => perf.measure("decompile", || {
                offsets
                    .iter()
                    .map(|offset| export_record(&abc, *offset, &directory_text, output))
                    .collect::<Result<Vec<_>>>()
            })?,
        };
        let (mut abc_records, mut abc_methods) = (0usize, 0usize);
        let mut entry_manifest = Vec::new();
        for record in exported.into_iter().flatten() {
            push_json(
                &mut entry_manifest,
                &json!({
                    "kind": "record",
                    "entry": entry.relative.as_str(),
                    "at": record.at,
                    "source_id": abc.source_id.as_str(),
                    "name": record.name,
                    "name_utf16": record.name_utf16,
                    "methods": record.methods,
                    "warnings": record.warnings,
                    "output": record.output,
                }),
            )?;
            abc_records += 1;
            abc_methods += record.methods;
            files += 1;
            source_bytes = source_bytes
                .checked_add(record.bytes)
                .ok_or_else(|| Error::new("RESOURCE_LIMIT", "record source byte count overflow"))?;
        }
        let mut prefix = Vec::new();
        push_json(
            &mut prefix,
            &json!({
                "kind": "abc",
                "entry": entry.relative.as_str(),
                "source_id": abc.source_id.as_str(),
                "version": abc.version.map(|part| part.to_string()).join("."),
                "records": abc_records,
                "methods": abc_methods,
                "output": directory_text,
            }),
        )?;
        manifest.extend_from_slice(&prefix);
        manifest.extend_from_slice(&entry_manifest);
        records += abc_records;
        methods += abc_methods;
    }
    if records == 0 {
        return Err(Error::new("NOT_FOUND", "ABC entries have no coded methods"));
    }
    let manifest_path = output.join("manifest.jsonl");
    write_file(&manifest_path, &manifest)?;
    perf.counters.input_bytes = Some(input_bytes);
    perf.counters.records = Some(records);
    let receipt = json!({
        "record": "export",
        "detail": {
            "complete": true,
            "path": output.to_string_lossy(),
            "manifest": manifest_path.to_string_lossy(),
            "abc_entries": entries.len(),
            "records": records,
            "methods": methods,
            "files": files,
            "source_bytes": source_bytes,
            "extracted_files": extracted_files,
            "extracted_bytes": extracted_bytes,
        },
    });
    let mut bytes = Vec::new();
    push_json(&mut bytes, &receipt)?;
    Ok(bytes)
}
