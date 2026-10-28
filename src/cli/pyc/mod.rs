mod refs;
mod static_commands;
mod strings;

use std::path::{Path, PathBuf};

use serde_json::json;

use super::filter::CaseArgs;
use super::xref::Selector;
use clap::{Args, Subcommand};
use rxd_core::output::Output;
use rxd_core::perf::Perf;
use rxd_core::{Budget, Error, Result};

#[derive(Subcommand)]
pub enum Command {
    /// List modules without decompressing PYZ members (columns: at, kind).
    #[command(
        long_about = "Columns: at, kind\nList modules, packages and namespace packages in original PYZ directory order, without decompressing members.",
        after_long_help = "Examples:\nrxd pyc modules PYZ.pyz -f networking\nrxd pyc codes PYZ.pyz --within '<modules.at>'\nrxd pyc decompile PYZ.pyz --at '<modules.at>'\nkind is module, package or namespace; namespace packages have no executable body. Non-code data entries are excluded.\nStandalone PYC/raw marshal has one module, code:0; raw marshal requires --version X.Y."
    )]
    Modules {
        /// One PYC, PyInstaller PYZ, or raw marshal with --version X.Y.
        input: PathBuf,
        /// Raw marshal format as X.Y; omit for PYC/PYZ.
        #[arg(long, value_parser = parse_version)]
        version: Option<(u8, u8)>,
        /// Substring over full module coordinates, folded unless --case sensitive.
        #[arg(short = 'f', long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: static_commands::Options,
    },
    /// Search name and constant references.
    #[command(
        long_about = "Columns: entry, context, match\nList explicit references as TAB-separated rows after one # column header. Successful zero-hit queries retain the header.",
        after_long_help = "Examples:\nrxd pyc strings PYZ.pyz -f token\nrxd pyc xref PYZ.pyz --at '<strings.at>'\nrxd pyc xref PYZ.pyz -m token --kind name\nrxd pyc xref PYZ.pyz -m token --kind name --within yt_dlp.YoutubeDL\nRequired: exactly one of --at/--match; --match requires --kind name.\nConstant coordinates copy strings.at, not codes.at; names are literal text.\nPYZ scans all code-bearing modules unless --within or a constant coordinate selects a module.\nOutput: TAB-separated entry, context, match. entry is the full PYZ module name (\\N for standalone PYC); context selects its code body with decompile --at.\nmatch is the actual referenced name or selected constant coordinate; equal-looking names are not resolved runtime calls."
    )]
    Xref {
        /// One PYC, PyInstaller PYZ, or raw marshal with --version X.Y.
        input: PathBuf,
        #[command(flatten)]
        target: refs::Target,
        /// Copy codes.at: a module scopes all its code; a code coordinate selects one owner.
        #[arg(long)]
        within: Option<String>,
        /// Raw marshal format as X.Y, e.g. 3.12; omit for a normal .pyc header.
        #[arg(long, value_parser = parse_version)]
        version: Option<(u8, u8)>,
        #[command(flatten)]
        options: refs::RefsOptions,
    },
    /// List str/bytes constants (columns: at, value).
    #[command(
        long_about = "Columns: at, value\nList str/bytes constant occurrences across PYC or PYZ code-bearing modules.",
        after_long_help = "Examples:\nrxd pyc strings PYZ.pyz -f Authorization\nrxd pyc strings module.pyc --filter-hex ff00\nCoordinates: MODULE::const:N:PATH for PYZ; const:N:PATH for standalone PYC.\nUse xref --at '<strings.at>' for loads, then decompile --at '<xref.context>'.\nText and hexadecimal filters are exclusive; absent UTF-8 text is \\N."
    )]
    Strings {
        /// One PYC, PyInstaller PYZ, or raw marshal with --version X.Y.
        input: PathBuf,
        /// Raw marshal format as X.Y, e.g. 3.12; omit for a normal .pyc header.
        #[arg(long, value_parser = parse_version)]
        version: Option<(u8, u8)>,
        /// Substring over literal text or raw bytes, folded unless `--case sensitive`.
        #[arg(short = 'f', long, conflicts_with = "filter_hex")]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        /// Byte-exact substring as even hexadecimal digits without 0x or separators; use this for non-UTF-8 constants.
        #[arg(long, value_parser = strings::valid_hex)]
        filter_hex: Option<String>,
        #[command(flatten)]
        options: static_commands::Options,
    },
    /// List code-object metadata (columns: at, qualname).
    #[command(
        long_about = "Columns: at, qualname\nList code objects across PYC or PYZ code-bearing modules.",
        after_long_help = "Examples:\nrxd pyc modules PYZ.pyz -f networking\nrxd pyc codes PYZ.pyz --within yt_dlp.networking._helper\nrxd pyc codes PYZ.pyz -f proxy\nrxd pyc codes module.pyc --at code:0\n--within copies modules.at and parses only that module; omit it for whole-input search.\n--at selects exactly one listed row; it is exclusive with --within.\nPYZ module roots use the full module name; nested code uses MODULE::code:N.\nStandalone PYC retains code:N; its module scope is code:0. Coordinates preserve per-module depth-first indices.\n-f searches module and qualified code names; --name selects an exact original code name.\nWithout a stored qualname, the name cell uses the original code name.\nNext: decompile --at '<codes.at>' or xref --within '<codes.at>'.",
        mut_arg("at", |arg| arg.value_name("CODE_AT").help("Optional module or code coordinate copied from codes.at")),
    )]
    Codes {
        /// One PYC, PyInstaller PYZ, or raw marshal with --version X.Y.
        input: PathBuf,
        /// Raw marshal format as X.Y, e.g. 3.12; omit for a normal .pyc header.
        #[arg(long, value_parser = parse_version)]
        version: Option<(u8, u8)>,
        /// Substring over the code name, folded unless `--case sensitive`.
        #[arg(short = 'f', long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        /// Exact code name; not a substring.
        #[arg(long)]
        name: Option<String>,
        /// Copy modules.at to list only that module's code objects, including its root.
        #[arg(long, conflicts_with = "at")]
        within: Option<String>,
        #[command(flatten)]
        selector: Selector,
        #[command(flatten)]
        options: static_commands::Options,
    },
    /// Reconstruct a PYZ, one module, or a code body as Python.
    #[command(
        after_long_help = "Examples:\nrxd pyc decompile module.pyc\nrxd pyc decompile PYZ.pyz -o recovered/\nrxd pyc decompile PYZ.pyz -o recovered/ --threads 8\nrxd pyc decompile PYZ.pyz --at yt_dlp.YoutubeDL\nrxd pyc decompile PYZ.pyz --at '<codes.at or xref.context>' -o recovered.py\nWithout --at, a PYZ reconstructs every code-bearing module in directory order. With -o DIR it writes one source file per module (json/__init__.py, json/decoder.py) plus manifest.jsonl; without -o it prints # ===== <module> ===== blocks to stdout.\n--threads controls parallel module reconstruction and file writes (default: logical CPUs); bounded budgets stay serial.\nA decompiler panic becomes a labelled PYC_FALLBACK file/block and is reported on stderr, while other failures abort atomically.\nWith --at, a full module name reconstructs that module and MODULE::code:N one complete code body.\nStandalone PYC defaults to its whole module; --at code:N selects a code body.\nCode-body output is a source fragment; its enclosing binding, defaults and decorators are outside the selection.\nSource labels use the same coordinates as codes. Unrecoverable bodies retain labelled original-bytecode comments.\nRaw marshal requires --version X.Y; omit it for PYC/PYZ. Invalid input and resource failures are atomic errors."
    )]
    Decompile {
        /// One PYC, PyInstaller PYZ, or raw marshal with --version X.Y.
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        /// Interpret input as raw marshal for the Python format X.Y.
        #[arg(long, value_parser = parse_version)]
        version: Option<(u8, u8)>,
        #[command(flatten)]
        options: Options,
        /// Parallel module workers for a whole-PYZ decompile.
        #[arg(long, default_value_t = super::default_threads(), value_parser = super::positive)]
        threads: usize,
    },
}

#[derive(Args)]
pub struct Options {
    /// Write to FILE; a whole-PYZ decompile writes one source file per module into a new DIR.
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Write timing/resource JSONL to stderr without changing the result payload.
    #[arg(long)]
    perf: bool,
    #[arg(skip)]
    budget: Budget,
}

/// Resolve input/output aliases before writing, including the parent of new destinations.
fn validate_output(input: &Path, output: Option<&Path>) -> Result<()> {
    let Some(output) = output else {
        return Ok(());
    };
    let input_identity = std::fs::canonicalize(input).ok();
    let output_identity = std::fs::canonicalize(output).ok().or_else(|| {
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        Some(
            std::fs::canonicalize(parent)
                .ok()?
                .join(output.file_name()?),
        )
    });
    #[cfg(unix)]
    let same_file = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(input)
            .ok()
            .zip(std::fs::metadata(output).ok())
            .is_some_and(|(input, output)| {
                input.dev() == output.dev() && input.ino() == output.ino()
            })
    };
    #[cfg(not(unix))]
    let same_file = false;
    if input == output || input_identity.is_some() && input_identity == output_identity || same_file
    {
        return Err(Error::new(
            "INVALID_QUERY",
            "output path must differ from input path",
        ));
    }
    Ok(())
}

fn parse_version(value: &str) -> std::result::Result<(u8, u8), String> {
    let (major, minor) = value
        .split_once('.')
        .ok_or_else(|| "expected Python version X.Y".to_owned())?;
    if minor.contains('.') {
        return Err("expected Python version X.Y".into());
    }
    Ok((
        major
            .parse()
            .map_err(|_| "expected Python version X.Y".to_owned())?,
        minor
            .parse()
            .map_err(|_| "expected Python version X.Y".to_owned())?,
    ))
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Modules { .. } => "modules",
            Self::Decompile { .. } => "decompile",
            Self::Codes { .. } => "codes",
            Self::Strings { .. } => "strings",
            Self::Xref { .. } => "xref",
        }
    }

    pub fn perf(&self) -> bool {
        match self {
            Self::Decompile { options, .. } => options.perf,
            Self::Codes { options, .. } | Self::Modules { options, .. } => options.perf,
            Self::Strings { options, .. } => options.perf,
            Self::Xref { options, .. } => options.common.perf,
        }
    }
}

pub fn execute(
    mut command: Command,
    policy: rxd_core::BudgetPolicy,
    sink: &mut super::Output<'_>,
) -> Result<()> {
    let (input, budget) = match &mut command {
        Command::Decompile { input, options, .. } => (input, &mut options.budget),
        Command::Codes { input, options, .. }
        | Command::Strings { input, options, .. }
        | Command::Modules { input, options, .. } => (input, &mut options.budget),
        Command::Xref { input, options, .. } => (input, &mut options.common.budget),
    };
    *budget = policy.for_file(input)?;
    let emit_perf = command.perf();
    let mut perf = Perf::new(emit_perf, "analysis", "pyc", command.name());
    let result = execute_inner(command, &mut perf, sink);
    super::finish(perf, emit_perf, result)
}

fn is_pyz(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).is_ok() && &magic == b"PYZ\0"
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn report_fallback(modules: &[rxd_pyc::ModuleDecompilation]) {
    if modules.iter().all(|module| module.complete) {
        return;
    }
    let failed: Vec<&str> = modules
        .iter()
        .filter(|module| module.failed.is_some())
        .map(|module| module.name.as_str())
        .collect();
    if failed.is_empty() {
        eprintln!(
            "PYC_FALLBACK: partial reconstruction; original bytecode is included in labelled comments"
        );
    } else {
        eprintln!(
            "PYC_FALLBACK: {} module(s) failed: {}",
            failed.len(),
            failed.join(", ")
        );
    }
}

fn write_new_parts(path: &Path, bytes: &[u8], newline: bool) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    if newline {
        file.write_all(b"\n")?;
    }
    Ok(())
}

fn write_module_files(
    output: &Path,
    items: &[(PathBuf, &rxd_pyc::ModuleDecompilation, usize)],
    threads: usize,
) -> Result<usize> {
    if items.is_empty() {
        return Ok(0);
    }
    let workers = threads.min(items.len()).max(1);
    let chunk = items.len().div_ceil(workers);
    let parts: Result<Vec<usize>> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for part in items.chunks(chunk) {
            handles.push(scope.spawn(move || -> Result<usize> {
                let mut bytes = 0usize;
                for (relative, module, body_bytes) in part {
                    let target = output.join(relative);
                    write_new_parts(
                        &target,
                        module.source.as_bytes(),
                        !module.source.ends_with('\n'),
                    )?;
                    bytes = bytes.checked_add(*body_bytes).ok_or_else(|| {
                        Error::new("RESOURCE_LIMIT", "PYZ output byte count overflow")
                    })?;
                }
                Ok(bytes)
            }));
        }
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| Error::new("INTERNAL_ERROR", "PYC write worker panicked"))?
            })
            .collect()
    });
    let mut total = 0usize;
    for part in parts? {
        total = total
            .checked_add(part)
            .ok_or_else(|| Error::new("RESOURCE_LIMIT", "PYZ output byte count overflow"))?;
    }
    Ok(total)
}

fn decompile_directory(
    input: &Path,
    version: Option<(u8, u8)>,
    options: &Options,
    perf: &mut Perf,
    threads: usize,
) -> Result<()> {
    let output = options
        .output
        .as_deref()
        .ok_or_else(|| Error::new("INVALID_QUERY", "whole-PYZ decompile requires -o DIR"))?;
    if output.symlink_metadata().is_ok() {
        return Err(Error::new(
            "INVALID_QUERY",
            "output directory must not already exist",
        ));
    }
    std::fs::create_dir(output)?;
    let result = (|| -> Result<()> {
        let modules = rxd_pyc::run_input_modules(input, version, options.budget, perf, threads)?;
        let mut paths = std::collections::HashSet::new();
        let mut items = Vec::new();
        let mut manifest = Vec::new();
        for module in &modules {
            if !paths.insert(module.path.clone()) {
                return Err(Error::new(
                    "INVALID_INPUT",
                    format!("duplicate PYZ source path: {}", module.path),
                ));
            }
            let body_bytes = module
                .source
                .len()
                .checked_add(usize::from(!module.source.ends_with('\n')))
                .ok_or_else(|| Error::new("RESOURCE_LIMIT", "PYZ output byte count overflow"))?;
            let line = json!({
                "module": module.name,
                "file": module.path,
                "bytes": body_bytes,
                "complete": module.complete,
                "error": module.failed,
            });
            serde_json::to_writer(&mut manifest, &line)
                .map_err(|error| Error::new("INTERNAL_ERROR", error.to_string()))?;
            manifest.push(b'\n');
            items.push((
                PathBuf::from(&module.path),
                module as &rxd_pyc::ModuleDecompilation,
                body_bytes,
            ));
        }
        let mut parents = std::collections::HashSet::new();
        parents.insert(output.to_path_buf());
        for (relative, _, _) in &items {
            if let Some(parent) = output.join(relative).parent() {
                parents.insert(parent.to_path_buf());
            }
        }
        for parent in parents {
            std::fs::create_dir_all(parent)?;
        }
        let body_bytes = perf.measure("write", || write_module_files(output, &items, threads))?;
        write_new(&output.join("manifest.jsonl"), &manifest)?;
        let total = body_bytes
            .checked_add(manifest.len())
            .ok_or_else(|| Error::new("RESOURCE_LIMIT", "PYZ output byte count overflow"))?;
        if options
            .budget
            .output_bytes
            .is_some_and(|limit| total > limit)
        {
            return Err(Error::failure(
                rxd_core::Failure::Resource,
                "serialized PYC output bytes",
            ));
        }
        perf.counters.records = Some(modules.len());
        perf.counters.output_bytes = Some(total);
        report_fallback(&modules);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(output);
    }
    result
}

fn execute_inner(command: Command, perf: &mut Perf, sink: &mut super::Output<'_>) -> Result<()> {
    let command = match command {
        Command::Modules {
            input,
            version,
            filter,
            case,
            options,
        } => {
            return static_commands::modules(
                input,
                version,
                filter,
                case.sensitive(),
                options,
                perf,
                sink,
            );
        }
        Command::Xref {
            input,
            target,
            within,
            version,
            options,
        } => return refs::run(input, target, within, version, options, perf, sink),
        Command::Strings {
            input,
            version,
            filter,
            case,
            filter_hex,
            options,
        } => {
            return strings::run(
                input,
                version,
                strings::Filter {
                    text: filter,
                    case_sensitive: case.sensitive(),
                    hex: filter_hex,
                },
                options,
                perf,
                sink,
            );
        }
        Command::Codes {
            input,
            version,
            filter,
            case,
            name,
            selector,
            within,
            options,
        } => {
            let query = static_commands::CodeQuery {
                filter,
                case_sensitive: case.sensitive(),
                name,
                selector,
                within,
            };
            return static_commands::codes(input, version, query, options, perf, sink);
        }
        command => command,
    };
    let Command::Decompile {
        input,
        version,
        selector,
        options,
        threads,
    } = command
    else {
        unreachable!()
    };
    validate_output(&input, options.output.as_deref())?;
    if selector.at.is_none() && options.output.is_some() && is_pyz(&input) {
        return decompile_directory(&input, version, &options, perf, threads);
    }
    let result = rxd_pyc::run_input(
        &input,
        version,
        selector.at.as_deref(),
        options.budget,
        perf,
        threads,
    )?;
    perf.source_id = Some(result.source_id.clone());
    perf.counters.input_bytes = Some(result.input_bytes);
    let bytes = perf.measure("serialize", || -> Result<Vec<u8>> {
        Ok(result.source.into_bytes())
    })?;
    if options
        .budget
        .output_bytes
        .is_some_and(|limit| bytes.len() > limit)
    {
        return Err(Error::failure(
            rxd_core::Failure::Resource,
            "serialized PYC output bytes",
        ));
    }
    perf.counters.output_bytes = Some(bytes.len());
    perf.counters.records = Some(result.modules);
    perf.measure("write", || {
        super::write_output(&bytes, options.output.as_deref(), sink)
    })?;
    if !result.complete {
        if result.failed.is_empty() {
            eprintln!(
                "PYC_FALLBACK: partial reconstruction; original bytecode is included in labelled comments"
            );
        } else {
            eprintln!(
                "PYC_FALLBACK: {} module(s) failed: {}",
                result.failed.len(),
                result.failed.join(", ")
            );
        }
    }
    Ok(())
}
