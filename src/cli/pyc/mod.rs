mod fetch_commands;
mod refs;
mod static_commands;
mod strings;

use std::path::{Path, PathBuf};

use super::filter::CaseArgs;
use super::xref::Selector;
use clap::{Args, Subcommand};
use rxd_core::envelope::{self, RecordMeta};
use rxd_core::output::Output;
use rxd_core::perf::Perf;
use rxd_core::{Budget, Error, Result};
use rxd_pyc::Limits;
use serde_json::json;

#[derive(Subcommand)]
pub enum Command {
    /// Find explicit name or constant-occurrence references, with enclosing code IDs.
    Xref {
        input: PathBuf,
        #[command(flatten)]
        target: refs::Target,
        #[arg(long)]
        within: Option<String>,
        #[arg(long, value_parser = parse_version)]
        version: Option<(u8, u8)>,
        #[command(flatten)]
        options: refs::RefsOptions,
    },
    /// Enumerate lossless str/bytes constants without decoding instructions.
    Strings {
        input: PathBuf,
        #[arg(long, value_parser = parse_version)]
        version: Option<(u8, u8)>,
        /// Substring over literal text or raw bytes, folded unless `--case sensitive`.
        #[arg(short = 'f', long, conflicts_with = "filter_hex")]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        ///  substring over raw bytes, given as even hexadecimal digits. Folded unless `--case sensitive`.
        #[arg(long, value_parser = strings::valid_hex)]
        filter_hex: Option<String>,
        #[command(flatten)]
        options: static_commands::Options,
    },
    /// Fetch original instructions, tables and location evidence for one code coordinate.
    GetCode {
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        #[arg(long,value_parser=parse_version)]
        version: Option<(u8, u8)>,
        #[command(flatten)]
        options: fetch_commands::FetchOptions,
    },
    /// Enumerate bounded code-object metadata without decoding instructions.
    Codes {
        input: PathBuf,
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
    /// Reconstruct Python source from a .pyc or versioned raw marshal code object.
    Decompile {
        input: PathBuf,
        /// Interpret input as raw marshal for the exact Python version X.Y.
        #[arg(long, value_parser = parse_version)]
        version: Option<(u8, u8)>,
        /// Include source identity and an explicit completeness flag.
        #[arg(long)]
        json: bool,
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

/// Resolve filesystem aliases before any PYC command can replace its input.
/// Missing paths keep their ordinary input/write error handling. For a new
/// destination, resolve the parent so symlinked directories and `..` agree.
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

pub fn execute(mut command: Command, policy: rxd_core::BudgetPolicy) -> Result<()> {
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
    let result = execute_inner(command, &mut perf);
    super::finish(perf, emit_perf, result)
}

fn execute_inner(command: Command, perf: &mut Perf) -> Result<()> {
    let command = match command {
        Command::Xref {
            input,
            target,
            within,
            version,
            options,
        } => return refs::run(input, target, within, version, options, perf),
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
                filter,
                case.sensitive(),
                filter_hex,
                options,
                perf,
            );
        }
        Command::GetCode {
            input,
            selector,
            version,
            options,
        } => return fetch_commands::fetch(input, selector, version, options, perf),
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
            return static_commands::codes(input, version, query, options, perf);
        }
        command => command,
    };
    let Command::Decompile {
        input,
        version,
        json,
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
    if !json && !result.complete {
        return Err(Error::new(
            "INCOMPLETE_DECOMPILATION",
            "source reconstruction is incomplete; rerun with --json to inspect the partial result",
        ));
    }
    let bytes = if json {
        perf.measure("serialize", || -> Result<Vec<u8>> {
            let mut row = envelope::record(
                "source",
                RecordMeta {
                    source_id: Some(&result.source_id),
                    ..RecordMeta::default()
                },
                json!({
                    "input_bytes": result.input_bytes,
                    "python_version": result.version,
                    "representation": "decompiled_python",
                    "complete": result.complete,
                    "source": result.source,
                })
                .as_object()
                .unwrap()
                .clone(),
            );
            let object = row.as_object_mut().unwrap();
            object.insert("schema_version".into(), json!(envelope::SCHEMA_VERSION));
            object.insert("backend".into(), json!("pyc"));
            object.insert("command".into(), json!("decompile"));
            let mut writer = Output::new(options.budget.output_bytes);
            writer.json_line(&row)?;
            Ok(writer.into_bytes())
        })?
    } else {
        result.source.into_bytes()
    };
    if bytes.len() > options.budget.output_bytes {
        return Err(Error::failure(
            rxd_core::Failure::Resource,
            "serialized PYC output bytes",
        ));
    }
    perf.counters.output_bytes = Some(bytes.len());
    perf.counters.records = Some(1);
    perf.measure("write", || {
        super::write_output(&bytes, options.output.as_deref())
    })
}
