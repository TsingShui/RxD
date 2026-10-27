mod fetch_commands;
mod refs;
mod static_commands;
mod strings;

use std::path::{Path, PathBuf};

use super::filter::CaseArgs;
use super::xref::Selector;
use clap::{Args, Subcommand};
use rxd_core::output::Output;
use rxd_core::perf::Perf;
use rxd_core::{Budget, Error, Result};
use rxd_pyc::Limits;

#[derive(Subcommand)]
pub enum Command {
    /// Search name and constant references.
    #[command(
        after_long_help = "Examples:\nrxd pyc strings module.pyc -f token\nrxd pyc xref module.pyc --at '<string.at>'\nrxd pyc xref module.pyc --at token --kind name\nrxd pyc xref module.pyc -m token --kind name\nRequired: exactly one of --at/--match; --match requires --kind name.\nConstant coordinates copy strings.at (const:N:PATH), not codes.at; names are literal text.\n--within copies codes.at to restrict scanning. Output: reference JSONL; context_at names code.\nNext: rxd pyc get-code module.pyc --at '<reference.context_at>'."
    )]
    Xref {
        /// One regular .pyc file, or raw marshal with explicit --version X.Y.
        input: PathBuf,
        #[command(flatten)]
        target: refs::Target,
        /// Copy codes.at (code:N) to restrict the bytecode scan to this owner.
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
        after_long_help = "Examples:\nrxd pyc strings module.pyc -f Authorization\nrxd pyc strings module.pyc --filter-hex ff00\nOutput: string JSONL; at is const:N:PATH, context_at its code:N owner.\nUse xref --at '<string.at>' for loads, or get-code --at '<string.context_at>'.\nText and hexadecimal filters are exclusive. Non-UTF-8 bytes retain raw_bytes."
    )]
    Strings {
        /// One regular .pyc file, or raw marshal with explicit --version X.Y.
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
    /// Fetch one code object as instructions and tables.
    #[command(after_long_help = "Examples:\nrxd pyc codes module.pyc -f login\nrxd pyc get-code module.pyc --at 'code:0'\nRequired: --at copied from codes.at or xref.context_at; code:N, not const:N:PATH.\nOutput: one code_view JSON record with original instructions, caches, tables and ranges.\nNo source reconstruction here; use decompile for whole-module Python text.", mut_arg("at", |arg| arg.required(true).value_name("CODE_AT").help("Copy codes.at or xref.context_at: code:N, not a constant coordinate")))]
    GetCode {
        /// One regular .pyc file, or raw marshal with explicit --version X.Y.
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        /// Raw marshal format as X.Y, e.g. 3.12; omit for a normal .pyc header.
        #[arg(long,value_parser=parse_version)]
        version: Option<(u8, u8)>,
        #[command(flatten)]
        options: fetch_commands::FetchOptions,
    },
    /// List code-object metadata (columns: at, qualname).
    #[command(after_long_help = "Examples:\nrxd pyc codes module.pyc -f login\nrxd pyc codes module.pyc --at 'code:0'\nOutput: code JSONL in original depth-first order; at is code:N, parent_at first containment.\n--at is optional; --name exact and -f substring filters preserve original indices.\nNext: rxd pyc get-code module.pyc --at '<code.at>'. No instruction/source recovery during listing.", mut_arg("at", |arg| arg.value_name("CODE_AT").help("Optional codes.at coordinate, code:N; omit to enumerate all code objects")))]
    Codes {
        /// One regular .pyc file, or raw marshal with explicit --version X.Y.
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
        #[command(flatten)]
        selector: Selector,
        #[command(flatten)]
        options: static_commands::Options,
    },
    /// Reconstruct the whole module as Python.
    #[command(
        after_long_help = "Examples:\nrxd pyc decompile module.pyc\nrxd pyc decompile raw.marshal --version 3.12 -o recovered.py\nReconstruct the whole module, not a selected code:N.\nstdout is Python source; incomplete recovery fails without partial text.\nFor original selected bytecode use codes -> get-code --at instead."
    )]
    Decompile {
        /// One regular .pyc file, or raw marshal with explicit --version X.Y.
        input: PathBuf,
        /// Interpret input as raw marshal for the exact Python version X.Y.
        #[arg(long, value_parser = parse_version)]
        version: Option<(u8, u8)>,
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
    #[arg(skip)]
    budget: Budget,
}

fn decompile_limits(budget: Budget) -> Limits {
    Limits {
        input_bytes: budget.input_bytes,
        work: budget.work,
        // AST nesting is projected from the shared core limits the budget drives.
        ast_depth: budget.ast_depth,
        output_bytes: budget.output_bytes,
        marshal: rxd_pyc::marshal_limits(budget),
    }
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
            Self::Decompile { .. } => "decompile",
            Self::Codes { .. } => "codes",
            Self::Strings { .. } => "strings",
            Self::GetCode { .. } => "get-code",
            Self::Xref { .. } => "xref",
        }
    }

    pub fn perf(&self) -> bool {
        match self {
            Self::Decompile { options, .. } => options.perf,
            Self::Codes { options, .. } => options.perf,
            Self::Strings { options, .. } => options.perf,
            Self::GetCode { options, .. } => options.common.perf,
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
        Command::Codes { input, options, .. } | Command::Strings { input, options, .. } => {
            (input, &mut options.budget)
        }
        Command::GetCode { input, options, .. } => (input, &mut options.common.budget),
        Command::Xref { input, options, .. } => (input, &mut options.common.budget),
    };
    *budget = policy.for_file(input)?;
    let emit_perf = command.perf();
    let mut perf = Perf::new(emit_perf, "analysis", "pyc", command.name());
    let result = execute_inner(command, &mut perf, sink);
    super::finish(perf, emit_perf, result)
}

fn execute_inner(command: Command, perf: &mut Perf, sink: &mut super::Output<'_>) -> Result<()> {
    let command = match command {
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
        Command::GetCode {
            input,
            selector,
            version,
            options,
        } => return fetch_commands::fetch(input, selector, version, options, perf, sink),
        Command::Codes {
            input,
            version,
            filter,
            case,
            name,
            selector,
            options,
        } => {
            let query = static_commands::CodeQuery {
                filter,
                case_sensitive: case.sensitive(),
                name,
                selector,
            };
            return static_commands::codes(input, version, query, options, perf, sink);
        }
        command => command,
    };
    let Command::Decompile {
        input,
        version,
        options,
    } = command
    else {
        unreachable!()
    };
    validate_output(&input, options.output.as_deref())?;
    let limits = decompile_limits(options.budget);
    let result = rxd_pyc::run(&input, version, limits, perf)?;
    perf.source_id = Some(result.source_id.clone());
    perf.counters.input_bytes = Some(result.input_bytes);
    if !result.complete {
        return Err(Error::new(
            "INCOMPLETE_DECOMPILATION",
            "source reconstruction is incomplete; no partial source is written",
        ));
    }
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
    perf.counters.records = Some(1);
    perf.measure("write", || {
        super::write_output(&bytes, options.output.as_deref(), sink)
    })
}
