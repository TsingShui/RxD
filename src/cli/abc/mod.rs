//! Dynamic Ark ABC/HAP navigation; source records are not source-language classes.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{
    Output,
    filter::CaseArgs,
    table::{self, Cell},
};
use clap::{Args, Subcommand};
use rxd_ark::{Abc, Coordinate, EntityKind, Input, Method, Name, Operand, ReconstructionWarning};
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
        after_long_help = "Examples:\n  rxd ark export app.hap -o recovered\n  rxd ark export modules.abc -o recovered\n  rxd ark export app.hap -o recovered --entry ets/widgets.abc\n  rxd ark export app.hap -o sources --extract none\n  rg -n -C 3 'Authorization' recovered/decompiled\nOutput: a new directory with extracted/ and/or decompiled/<abc path>/<record>.js plus manifest.jsonl.\n--extract all (default) writes every original HAP file, abc writes only .abc entries, and none writes no originals.\nEach record file keeps its original module form and every coded method, including methods the initializer never references.\nmanifest.jsonl maps each record coordinate to its file, name, coded-method count and warning codes; the ABC line carries the full ABC SHA-256.\nOmit --entry to export every .abc entry; --entry restricts the export to one exact path.\nABC reconstruction overlaps the remaining extraction; records decompile in parallel with forked parsers.\nThe export is atomic: an existing output directory is refused and any failure removes the new directory.\nRecord sources remain best-effort reconstruction; follow warnings at affected operations and never execute them."
    )]
    Export(ExportOptions),
    #[command(
        about = "List strings referenced by decoded instruction operands.",
        long_about = "Columns: at, value\nList referenced strings after one # column header.",
        after_long_help = "Examples:\n  rxd ark strings app.hap --entry ets/modules.abc\n  rxd ark strings modules.abc -f Authorization\n  rxd ark xref modules.abc --at '<strings.at>'\nOutput: one row per distinct string referenced by a decoded instruction operand; at is str:0x<offset>.\nOnly explicit string operands are listed; metadata-only names and unreferenced literal-array entries are excluded.\nNext: xref --at '<strings.at>' or -m TEXT to find referencing methods."
    )]
    Strings(StringsOptions),
    #[command(
        about = "Search methods that reference a string operand.",
        long_about = "Columns: context, match\nList explicit string references after one # column header.",
        after_long_help = "Examples:\n  rxd ark xref modules.abc --at 'str:0x1234'\n  rxd ark xref modules.abc -m Authorization\nRequired: exactly one of --at/--match. --at copies strings.at.\nOutput: context, match. context is the referencing method:0x...; match is the actual string coordinate str:0x....\nOne row per method/string pair, ordered by method coordinate; read values with strings -f TEXT.\nOnly decoded string operands count, so this is not a runtime call graph or a metadata-name search."
    )]
    Xref(XrefOptions),
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
        long_about = "Columns: at, name, record, code_size, args, vregs\nList methods after one # column header.",
        after_long_help = "Examples:\n  rxd ark list method app.hap --entry ets/modules.abc\n  rxd ark list method modules.abc --at 'record:0x1234'\n  rxd ark list method modules.abc -f token\nWithout --at, searches every record in the selected ABC and reports the owning record column.\n--at restricts the search to one record copied from list record. code_size is bytecode bytes; args includes VM parameters.\nMissing code metadata is \\N; filtered zero hits are a complete empty result."
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
    /// Optional record coordinate; omit to search every record in the selected ABC.
    #[arg(long)]
    at: Option<String>,
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
pub struct StringsOptions {
    #[command(flatten)]
    options: Options,
    /// Substring over string values, folded unless --case sensitive.
    #[arg(short = 'f', long)]
    filter: Option<String>,
    #[command(flatten)]
    case: CaseArgs,
    /// Parallel scan workers.
    #[arg(long, default_value_t = super::default_threads(), value_parser = super::positive)]
    threads: usize,
}

#[derive(Args)]
#[command(group(clap::ArgGroup::new("anchor").required(true).args(["at", "filter"])))]
pub struct XrefOptions {
    #[command(flatten)]
    options: Options,
    /// Copy strings.at: str:0x<offset>.
    #[arg(long, conflicts_with = "filter")]
    at: Option<String>,
    /// Substring over string values; matches every referenced string containing it.
    #[arg(short = 'm', long = "match", conflicts_with = "at")]
    filter: Option<String>,
    #[command(flatten)]
    case: CaseArgs,
    /// Parallel scan workers.
    #[arg(long, default_value_t = super::default_threads(), value_parser = super::positive)]
    threads: usize,
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
    /// What to write under extracted/: the whole tree, only .abc entries, or nothing.
    #[arg(long, value_enum, default_value_t = ExtractMode::All)]
    extract: ExtractMode,
    /// Parallel record workers; extraction adds at most four I/O workers.
    #[arg(long, default_value_t = super::default_threads(), value_parser = super::positive)]
    threads: usize,
    /// Emit phase timings as JSONL on stderr.
    #[arg(long)]
    perf: bool,
}

/// Original-file extraction during Ark export.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ExtractMode {
    /// Extract the complete container tree.
    #[default]
    All,
    /// Extract only .abc entries.
    Abc,
    /// Write no original files.
    None,
}

impl ExtractMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Abc => "abc",
            Self::None => "none",
        }
    }
}

impl Command {
    pub fn perf(&self) -> bool {
        match self {
            Self::Decompile(options) => options.options.perf,
            Self::Export(options) => options.perf,
            Self::Strings(options) => options.options.perf,
            Self::Xref(options) => options.options.perf,
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
            Self::Strings(_) => "strings",
            Self::Xref(_) => "xref",
        }
    }
    fn input(&self) -> &PathBuf {
        match self {
            Self::Decompile(options) => &options.options.input,
            Self::Export(options) => &options.input,
            Self::Strings(options) => &options.options.input,
            Self::Xref(options) => &options.options.input,
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
            Self::Strings(options) => options.options.entry.as_deref(),
            Self::Xref(options) => options.options.entry.as_deref(),
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
            Self::Strings(options) => options.options.output.as_ref(),
            Self::Xref(options) => options.options.output.as_ref(),
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
            } => {
                table::header(&["at", "name", "record", "code_size", "args", "vregs"]).into_bytes()
            }
            Command::Decompile(_) => Vec::new(),
            Command::Strings(_) => table::header(&["at", "value"]).into_bytes(),
            Command::Xref(_) => table::header(&["context", "match"]).into_bytes(),
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
                    let filter = methods.list.filter.as_deref().map(|needle| {
                        rxd_core::filter::Contains::new(needle, methods.list.case.sensitive())
                    });
                    let mut records = Vec::new();
                    match methods.at.as_deref() {
                        Some(at) => match Coordinate::parse(at)? {
                            Coordinate::Record(offset) => records.push(offset),
                            Coordinate::Method(_) => {
                                return Err(Error::new(
                                    "INVALID_QUERY",
                                    "list method --at requires a record coordinate",
                                ));
                            }
                        },
                        None => {
                            records.extend(abc.record_offsets());
                            records.sort_unstable();
                        }
                    }
                    for offset in records {
                        let record = abc.record(offset)?;
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
                                    Cell::Atom(&format!("record:0x{offset:x}")),
                                    size.as_deref().map_or(Cell::Null, Cell::Atom),
                                    args.as_deref().map_or(Cell::Null, Cell::Atom),
                                    vregs.as_deref().map_or(Cell::Null, Cell::Atom),
                                ],
                                input.budget,
                            )?;
                        }
                    }
                }
                Command::Strings(strings) => {
                    let filter = strings.filter.as_deref().map(|needle| {
                        rxd_core::filter::Contains::new(needle, strings.case.sensitive())
                    });
                    let refs = scan_string_refs(&abc, strings.threads)?;
                    let mut names: BTreeMap<u32, std::sync::Arc<Name>> = BTreeMap::new();
                    for reference in refs {
                        names.entry(reference.string).or_insert(reference.name);
                    }
                    for (offset, name) in names {
                        if filter
                            .as_ref()
                            .is_some_and(|filter| !filter.matches(&name.text))
                        {
                            continue;
                        }
                        let quoted;
                        let value = match name.scalar_text() {
                            Some(text) => Cell::Text(text),
                            None => {
                                quoted = name.quoted();
                                Cell::Atom(&quoted)
                            }
                        };
                        append(
                            &mut bytes,
                            &[Cell::Atom(&format!("str:0x{offset:x}")), value],
                            input.budget,
                        )?;
                    }
                }
                Command::Xref(xref) => {
                    let target = xref.at.as_deref().map(parse_string_at).transpose()?;
                    let filter = xref.filter.as_deref().map(|needle| {
                        rxd_core::filter::Contains::new(needle, xref.case.sensitive())
                    });
                    let refs = scan_string_refs(&abc, xref.threads)?;
                    if let Some(offset) = target
                        && !refs.iter().any(|reference| reference.string == offset)
                    {
                        return Err(Error::new(
                            "NOT_FOUND",
                            format!(
                                "str:0x{offset:x} is not referenced by any decoded instruction"
                            ),
                        ));
                    }
                    for reference in refs {
                        let keep = match target {
                            Some(offset) => reference.string == offset,
                            None => filter
                                .as_ref()
                                .is_some_and(|filter| filter.matches(&reference.name.text)),
                        };
                        if !keep {
                            continue;
                        }
                        append(
                            &mut bytes,
                            &[
                                Cell::Atom(&format!("method:0x{:x}", reference.method)),
                                Cell::Atom(&format!("str:0x{:x}", reference.string)),
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

struct AbcSource {
    relative: String,
    bytes: Vec<u8>,
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

fn worker_pool(threads: usize) -> Result<Option<rayon::ThreadPool>> {
    if threads <= 1 {
        return Ok(None);
    }
    Ok(Some(
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .map_err(|error| {
                Error::new(
                    "RESOURCE_LIMIT",
                    format!("cannot start {threads} workers: {error}"),
                )
            })?,
    ))
}

fn parse_string_at(at: &str) -> Result<u32> {
    let value = at
        .strip_prefix("str:0x")
        .ok_or_else(|| Error::new("INVALID_QUERY", "expected a str:0x<offset> coordinate"))?;
    let offset = u32::from_str_radix(value, 16)
        .map_err(|_| Error::new("INVALID_QUERY", "invalid string coordinate"))?;
    if format!("str:0x{offset:x}") != at {
        return Err(Error::new(
            "INVALID_QUERY",
            "string coordinates must use the printed canonical spelling",
        ));
    }
    Ok(offset)
}

struct ScanRef {
    method: u32,
    string: u32,
    name: std::sync::Arc<Name>,
}

fn scan_method(abc: &Abc<'_>, method: &Method, out: &mut Vec<ScanRef>) -> Result<()> {
    let decoded = abc.decode(method).map_err(|error| {
        Error::new(
            error.code,
            format!("method:0x{:x}: {}", method.offset, error.message),
        )
    })?;
    let mut seen = BTreeSet::new();
    for instruction in &decoded.instructions {
        for operand in &instruction.operands {
            if let Operand::Id {
                kind: EntityKind::String,
                offset,
                ..
            } = operand
                && seen.insert(*offset)
            {
                out.push(ScanRef {
                    method: method.offset,
                    string: *offset,
                    name: abc.name(*offset)?,
                });
            }
        }
    }
    Ok(())
}

fn scan_string_refs(abc: &Abc<'_>, threads: usize) -> Result<Vec<ScanRef>> {
    let mut records: Vec<u32> = abc.record_offsets().collect();
    records.sort_unstable();
    let mut methods = Vec::new();
    for offset in records {
        let record = abc.record(offset)?;
        for method in &abc.members(&record)?.methods {
            if method.code_offset.is_some() {
                methods.push(method.clone());
            }
        }
    }
    if let Some(pool) = worker_pool(threads)?
        && methods.len() > 1
    {
        parallel_scan(&pool, abc, &methods)
    } else {
        let mut out = Vec::new();
        for method in &methods {
            scan_method(abc, method, &mut out)?;
        }
        Ok(out)
    }
}

fn parallel_scan<'a>(
    pool: &rayon::ThreadPool,
    abc: &Abc<'a>,
    methods: &[Method],
) -> Result<Vec<ScanRef>> {
    use rayon::prelude::*;
    let workers = methods.len().min(pool.current_num_threads()).max(1);
    let mut forks: Vec<Abc<'a>> = Vec::with_capacity(workers);
    for _ in 0..workers {
        forks.push(abc.fork());
    }
    let chunk = methods.len().div_ceil(workers);
    let parts: Result<Vec<Vec<ScanRef>>> = pool.install(|| {
        forks
            .par_iter_mut()
            .enumerate()
            .map(|(index, worker)| {
                let start = index.saturating_mul(chunk).min(methods.len());
                let end = (start + chunk).min(methods.len());
                let mut out = Vec::new();
                for method in &methods[start..end] {
                    scan_method(worker, method, &mut out)?;
                }
                Ok(out)
            })
            .collect()
    });
    Ok(parts?.into_iter().flatten().collect())
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

fn record_header(record: &rxd_ark::Record, members: &[rxd_ark::Method]) -> Result<Vec<u8>> {
    use std::io::Write as _;
    let mut header = Vec::new();
    write!(&mut header, "// record:0x{:x} ", record.offset).expect("header write");
    record.name.write_quoted(&mut header)?;
    header.push(b'\n');
    for method in members {
        if method.code_offset.is_none() {
            continue;
        }
        write!(&mut header, "// method:0x{:x} ", method.offset).expect("header write");
        method.name.write_quoted(&mut header)?;
        header.push(b'\n');
    }
    Ok(header)
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
    let mut source = record_header(&record, &members.methods)?;
    source.extend_from_slice(&reconstruction.source);
    let output_path = format!("{directory}/0x{offset:08x}.js");
    write_file(&output.join(&output_path), &source)?;
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
        bytes: source.len(),
    }))
}

fn parallel_records<'a>(
    pool: &rayon::ThreadPool,
    abc: &Abc<'a>,
    offsets: &[u32],
    directory: &str,
    output: &Path,
) -> Result<Vec<Option<ExportedRecord>>> {
    use rayon::prelude::*;
    let workers = offsets.len().min(pool.current_num_threads()).max(1);
    let mut forks: Vec<Abc<'a>> = Vec::with_capacity(workers);
    for _ in 0..workers {
        forks.push(abc.fork());
    }
    let chunk = offsets.len().div_ceil(workers);
    let parts: Result<Vec<Vec<Option<ExportedRecord>>>> = pool.install(|| {
        forks
            .par_iter_mut()
            .enumerate()
            .map(|(index, worker)| {
                let start = index.saturating_mul(chunk).min(offsets.len());
                let end = (start + chunk).min(offsets.len());
                offsets[start..end]
                    .iter()
                    .map(|offset| export_record(worker, *offset, directory, output))
                    .collect::<Result<Vec<_>>>()
            })
            .collect()
    });
    Ok(parts?.into_iter().flatten().collect())
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

/// Extraction is I/O-bound; four workers measured fastest on local disks.
const EXTRACT_WORKERS: usize = 4;

struct Decompiled {
    manifest: Vec<u8>,
    records: usize,
    methods: usize,
    files: usize,
    source_bytes: usize,
    input_bytes: usize,
}

fn decompile_sources(
    sources: &[AbcSource],
    output: &Path,
    policy: BudgetPolicy,
    threads: usize,
) -> Result<Decompiled> {
    let pool = worker_pool(threads)?;
    let mut manifest = Vec::new();
    let mut totals = Decompiled {
        manifest: Vec::new(),
        records: 0,
        methods: 0,
        files: 0,
        source_bytes: 0,
        input_bytes: 0,
    };
    for source in sources {
        let budget = budget_for(policy, source.bytes.len())?;
        totals.input_bytes = totals
            .input_bytes
            .checked_add(source.bytes.len())
            .ok_or_else(|| Error::new("RESOURCE_LIMIT", "ABC input byte count overflow"))?;
        let abc = Abc::parse(&source.bytes, budget)?;
        let directory = PathBuf::from("decompiled").join(&source.relative);
        let directory_text = directory.to_string_lossy().replace('\\', "/");
        let mut offsets: Vec<u32> = abc.record_offsets().collect();
        offsets.sort_unstable();
        let exported = match &pool {
            Some(pool) if offsets.len() > 1 => {
                parallel_records(pool, &abc, &offsets, &directory_text, output)?
            }
            _ => offsets
                .iter()
                .map(|offset| export_record(&abc, *offset, &directory_text, output))
                .collect::<Result<Vec<_>>>()?,
        };
        let (mut abc_records, mut abc_methods) = (0usize, 0usize);
        let mut entry_manifest = Vec::new();
        for record in exported.into_iter().flatten() {
            push_json(
                &mut entry_manifest,
                &json!({
                    "kind": "record",
                    "entry": source.relative.as_str(),
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
            totals.files += 1;
            totals.source_bytes = totals
                .source_bytes
                .checked_add(record.bytes)
                .ok_or_else(|| Error::new("RESOURCE_LIMIT", "record source byte count overflow"))?;
        }
        let mut prefix = Vec::new();
        push_json(
            &mut prefix,
            &json!({
                "kind": "abc",
                "entry": source.relative.as_str(),
                "source_id": abc.source_id.as_str(),
                "version": abc.version.map(|part| part.to_string()).join("."),
                "records": abc_records,
                "methods": abc_methods,
                "output": directory_text,
            }),
        )?;
        manifest.extend_from_slice(&prefix);
        manifest.extend_from_slice(&entry_manifest);
        totals.records += abc_records;
        totals.methods += abc_methods;
    }
    totals.manifest = manifest;
    Ok(totals)
}

fn export_into(options: &ExportOptions, policy: BudgetPolicy, perf: &mut Perf) -> Result<Vec<u8>> {
    let metadata = std::fs::metadata(&options.input)?;
    if !metadata.is_file() {
        return Err(Error::new("INVALID_QUERY", "expected a regular local file"));
    }
    let container = read_head(&options.input)?.starts_with(b"PK");
    if !container && options.entry.is_some() {
        return Err(Error::new(
            "INVALID_QUERY",
            "--entry requires a HAP/ZIP container",
        ));
    }
    let output = &options.output;
    let container_budget = if container {
        let budget = policy.for_file(&options.input)?;
        perf.limits = Some(Limits::from(budget));
        Some(budget)
    } else {
        None
    };
    let mut sources = Vec::new();
    if let Some(budget) = container_budget {
        let mut entries = rxd_zip::read_matching(&options.input, ".abc", budget, perf)?;
        if let Some(entry) = options.entry.as_deref() {
            if !entry.ends_with(".abc") {
                return Err(Error::new(
                    "INVALID_QUERY",
                    "--entry must select a .abc path",
                ));
            }
            if !entries.iter().any(|selected| selected.path == entry) {
                return Err(Error::new(
                    "NOT_FOUND",
                    format!("container has no .abc entry {entry:?}"),
                ));
            }
            entries.retain(|selected| selected.path == entry);
        }
        if entries.is_empty() {
            return Err(Error::new("NOT_FOUND", "container has no .abc entry"));
        }
        for entry in entries {
            sources.push(AbcSource {
                relative: entry.path,
                bytes: entry.bytes,
            });
        }
        sources.sort_by(|left, right| left.relative.cmp(&right.relative));
    } else {
        let name = options
            .input
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| Error::new("UNSUPPORTED_INPUT", "input file name is not UTF-8"))?;
        let length = usize::try_from(metadata.len())
            .map_err(|_| Error::new("RESOURCE_LIMIT", "ABC length is not addressable"))?;
        let budget = budget_for(policy, length)?;
        let bytes = perf.measure("read", || {
            rxd_core::read_bytes(&options.input, budget.input_bytes)
        })?;
        sources.push(AbcSource {
            relative: name.to_owned(),
            bytes,
        });
    }
    let (decompiled, extracted) = if let Some(budget) = container_budget {
        let root = output.join("extracted");
        let extract = options.extract;
        let wanted = options.entry.as_deref();
        let extract_threads = options.threads.clamp(1, EXTRACT_WORKERS);
        std::thread::scope(|scope| -> Result<(Decompiled, rxd_zip::Unpack)> {
            let handle = scope.spawn(|| {
                let start = std::time::Instant::now();
                let result = decompile_sources(&sources, output, policy, options.threads);
                (result, start.elapsed())
            });
            let extracted = match extract {
                ExtractMode::None => Ok(rxd_zip::Unpack { files: 0, bytes: 0 }),
                ExtractMode::Abc => rxd_zip::unpack_filtered(
                    &options.input,
                    &root,
                    budget,
                    perf,
                    extract_threads,
                    &|path| path.ends_with(".abc") && wanted.is_none_or(|wanted| wanted == path),
                ),
                ExtractMode::All => rxd_zip::unpack_filtered(
                    &options.input,
                    &root,
                    budget,
                    perf,
                    extract_threads,
                    &|_| true,
                ),
            };
            let (decompiled, elapsed) = handle
                .join()
                .map_err(|_| Error::new("INTERNAL_ERROR", "decompile worker panicked"))?;
            perf.stage_ns("decompile", rxd_core::perf::ns(elapsed));
            Ok((decompiled?, extracted?))
        })?
    } else {
        let decompiled = perf.measure("decompile", || {
            decompile_sources(&sources, output, policy, options.threads)
        })?;
        (decompiled, rxd_zip::Unpack { files: 0, bytes: 0 })
    };
    if decompiled.records == 0 {
        return Err(Error::new("NOT_FOUND", "ABC entries have no coded methods"));
    }
    let manifest_path = output.join("manifest.jsonl");
    write_file(&manifest_path, &decompiled.manifest)?;
    perf.counters.input_bytes = Some(decompiled.input_bytes);
    perf.counters.records = Some(decompiled.records);
    let receipt = json!({
        "record": "export",
        "detail": {
            "complete": true,
            "path": output.to_string_lossy(),
            "manifest": manifest_path.to_string_lossy(),
            "extract": options.extract.as_str(),
            "abc_entries": sources.len(),
            "records": decompiled.records,
            "methods": decompiled.methods,
            "files": decompiled.files,
            "source_bytes": decompiled.source_bytes,
            "extracted_files": extracted.files,
            "extracted_bytes": extracted.bytes,
        },
    });
    let mut bytes = Vec::new();
    push_json(&mut bytes, &receipt)?;
    Ok(bytes)
}
