mod fetch_commands;
mod refs;
mod static_commands;
mod strings;

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use rxd_core::envelope::{self, RecordMeta};
use rxd_core::perf::Perf;
use rxd_core::{Error, Result};
use rxd_pyc::Limits;
use serde_json::json;

#[derive(Subcommand)]
pub enum Command {
    /// Find explicit name or constant-occurrence references, with enclosing code IDs.
    Xref {
        input: PathBuf,
        #[arg(
            long,
            conflicts_with = "const_id",
            required_unless_present = "const_id"
        )]
        name: Option<String>,
        #[arg(long)]
        const_id: Option<String>,
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
        #[arg(long, conflicts_with = "contains_hex")]
        contains: Option<String>,
        #[arg(long, value_parser = strings::valid_hex)]
        contains_hex: Option<String>,
        #[command(flatten)]
        options: static_commands::Options,
    },
    /// Fetch original instructions, tables and location evidence for one content-bound code ID.
    GetCode {
        input: PathBuf,
        code_id: String,
        #[arg(long,value_parser=parse_version)]
        version: Option<(u8, u8)>,
        #[command(flatten)]
        options: fetch_commands::FetchOptions,
    },
    /// Enumerate bounded code-object metadata without decoding instructions.
    Outline {
        input: PathBuf,
        #[arg(long, value_parser = parse_version)]
        version: Option<(u8, u8)>,
        #[arg(long, conflicts_with = "index")]
        name: Option<String>,
        #[arg(long)]
        index: Option<usize>,
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
    #[arg(long)]
    debug: bool,
    /// Input byte cap in bytes (1 through 8589934592); 8 GiB when omitted.
    #[arg(long, value_parser = super::input_bytes)]
    max_input_bytes: Option<usize>,
    #[arg(long, default_value_t = 32 * 1024 * 1024, value_parser = super::positive)]
    max_output_bytes: usize,
    /// Cumulative decoding, recovery, AST checks and generation work units.
    #[arg(long, default_value_t = 16_777_216, value_parser = super::positive)]
    max_work: usize,
    /// Maximum instruction-derived AST nesting (1..=128).
    #[arg(long, default_value_t = 128, value_parser = parse_ast_depth)]
    max_ast_depth: usize,
}

struct BoundedOutput {
    bytes: Vec<u8>,
    limit: usize,
}

impl std::io::Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("output byte budget exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
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

fn parse_ast_depth(value: &str) -> std::result::Result<usize, String> {
    let value = super::positive(value)?;
    if value > 128 {
        return Err("AST depth must be in 1..=128".into());
    }
    Ok(value)
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
            Self::Outline { .. } => "outline",
            Self::Strings { .. } => "strings",
            Self::GetCode { .. } => "get-code",
            Self::Xref { .. } => "xref",
        }
    }

    pub fn perf(&self) -> bool {
        match self {
            Self::Decompile { options, .. } => options.perf,
            Self::Outline { options, .. } => options.perf,
            Self::Strings { options, .. } => options.perf,
            Self::GetCode { options, .. } => options.common.perf,
            Self::Xref { options, .. } => options.common.perf,
        }
    }

    pub fn debug(&self) -> bool {
        match self {
            Self::Decompile { options, .. } => options.debug,
            Self::Outline { options, .. } => options.debug,
            Self::Strings { options, .. } => options.debug,
            Self::GetCode { options, .. } => options.common.debug,
            Self::Xref { options, .. } => options.common.debug,
        }
    }
}

pub fn execute(command: Command) -> Result<()> {
    let emit_perf = command.perf();
    let debug = command.debug();
    let mut perf = Perf::new(emit_perf || debug, "analysis", "pyc", command.name());
    let result = execute_inner(command, &mut perf);
    super::finish(perf, emit_perf, debug, result)
}

fn execute_inner(command: Command, perf: &mut Perf) -> Result<()> {
    let command = match command {
        Command::Xref {
            input,
            name,
            const_id,
            within,
            version,
            options,
        } => return refs::run(input, name, const_id, within, version, options, perf),
        Command::Strings {
            input,
            version,
            contains,
            contains_hex,
            options,
        } => return strings::run(input, version, contains, contains_hex, options, perf),
        Command::GetCode {
            input,
            code_id,
            version,
            options,
        } => return fetch_commands::fetch(input, code_id, version, options, perf),
        Command::Outline {
            input,
            version,
            name,
            index,
            options,
        } => return static_commands::outline(input, version, name, index, options, perf),
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
    let limits = Limits {
        input_bytes: super::input_limit(options.max_input_bytes),
        output_bytes: options.max_output_bytes,
        work: options.max_work,
        ast_depth: options.max_ast_depth,
    };
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
            let mut writer = BoundedOutput {
                bytes: Vec::new(),
                limit: options.max_output_bytes,
            };
            serde_json::to_writer(&mut writer, &row)
                .map_err(|error| Error::new("RESOURCE_LIMIT", error.to_string()))?;
            std::io::Write::write_all(&mut writer, b"\n")
                .map_err(|error| Error::new("RESOURCE_LIMIT", error.to_string()))?;
            Ok(writer.bytes)
        })?
    } else {
        result.source.into_bytes()
    };
    if bytes.len() > options.max_output_bytes {
        return Err(Error::new("RESOURCE_LIMIT", "output byte budget exceeded"));
    }
    perf.counters.output_bytes = Some(bytes.len());
    perf.counters.records = Some(1);
    perf.measure("write", || {
        super::write_output(&bytes, options.output.as_deref())
    })
}
