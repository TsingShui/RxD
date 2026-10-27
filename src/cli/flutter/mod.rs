//! `rxd dae`: Dart AOT (Flutter) snapshot navigation and reconstruction.

use std::path::PathBuf;

use super::Output;
use clap::{Args, Subcommand};
use rxd_core::perf::Perf;
use rxd_core::{Error, Result};
use rxd_flutter::Snapshot;
use serde_json::json;

#[derive(Subcommand)]
pub enum Command {
    /// Report the detected snapshot profile and entry counts.
    #[command(
        after_long_help = "Examples:\nrxd dae info libapp.so\nOutput: one info record with the detected Dart SDK profile, its verification status,\nlibrary and function counts, and whether the container needed the magic fallback.\nAn unverified profile means the layout was matched by version fingerprint only."
    )]
    Info(Options),
    /// List recovered functions (columns: at, label).
    #[command(
        after_long_help = "Examples:\nrxd dae functions libapp.so\nrxd dae functions libapp.so -f fl_clash\nOutput: one row per function; at is the code entry offset, label is library::class::method.\nNames come from the snapshot's library and class tables, not from source.\nNext: rxd dae decompile libapp.so --lib <library prefix>."
    )]
    Functions(FunctionsOptions),
    /// Reconstruct Dart-like pseudocode for whole libraries.
    #[command(
        after_long_help = "Examples:\nrxd dae decompile libapp.so --lib fl_clash\nrxd dae decompile libapp.so\nOutput: one dae_decompile record with the rendered library files.\nThis is machine-level pseudocode (registers and memory access remain), not source Dart.\nRendering runs the upstream decompiler: expect seconds, not milliseconds."
    )]
    Decompile(DecompileOptions),
}

#[derive(Args)]
pub struct Options {
    /// One ELF ARM64 libapp.so snapshot from a Flutter Android release build.
    input: PathBuf,
    /// Atomically write the result to FILE instead of stdout.
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Write timing/resource JSONL to stderr without changing the payload.
    #[arg(long)]
    perf: bool,
}

#[derive(Args)]
pub struct FunctionsOptions {
    #[command(flatten)]
    options: Options,
    /// Substring over the label, folded unless `--case sensitive`.
    #[arg(short = 'f', long)]
    filter: Option<String>,
    #[command(flatten)]
    case: super::filter::CaseArgs,
}

#[derive(Args)]
pub struct DecompileOptions {
    #[command(flatten)]
    options: Options,
    /// Restrict rendering to libraries whose name starts with this prefix.
    #[arg(long)]
    lib: Option<String>,
}

impl Command {
    fn options(&self) -> &Options {
        match self {
            Self::Info(options) => options,
            Self::Functions(options) => &options.options,
            Self::Decompile(options) => &options.options,
        }
    }

    pub fn perf(&self) -> bool {
        self.options().perf
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Info(_) => "info",
            Self::Functions(_) => "functions",
            Self::Decompile(_) => "decompile",
        }
    }
}

pub fn execute(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    sink: &mut Output<'_>,
) -> Result<()> {
    let options = command.options();
    let mut perf = Perf::new(options.perf, "analysis", "flutter", command.name());
    let perf_enabled = options.perf;
    let input = options.input.clone();
    let output_path = options.output.clone();
    let budget = policy.for_file(&input)?;
    perf.resource_limits = Some([("input_bytes", budget.input_bytes)].into_iter().collect());
    let result = (|| {
        let snapshot = perf.measure("read", || Snapshot::read(&input))?;
        if snapshot.input_bytes() > budget.input_bytes {
            return Err(Error::new(
                "INPUT_TOO_LARGE",
                "input exceeds the admitted size",
            ));
        }
        let bytes = match command {
            Command::Info(_) => {
                let info = perf.measure("analyze", || snapshot.info())?;
                let mut record = serde_json::to_vec(&json!({
                    "record": "dae_info",
                    "backend": "flutter",
                    "command": "info",
                    "sdk": info.sdk,
                    "profile_status": info.status,
                    "compressed_pointers": info.compressed_pointers,
                    "libraries": info.libraries,
                    "functions": info.functions,
                    "platform_warning": info.platform_warning,
                }))
                .map_err(|error| Error::new("INVALID_QUERY", error.to_string()))?;
                record.push(b'\n');
                record
            }
            Command::Functions(functions) => {
                let filter = functions.filter.as_deref().map(|needle| {
                    rxd_core::filter::Contains::new(needle, functions.case.sensitive())
                });
                let mut out = String::with_capacity(1 << 21);
                perf.measure("analyze", || {
                    snapshot.functions_into(&mut out, filter.as_ref())
                })?;
                out.into_bytes()
            }
            Command::Decompile(decompile) => {
                let rendered =
                    perf.measure("analyze", || snapshot.decompile(decompile.lib.as_deref()))?;
                let files: Vec<_> = rendered
                    .into_iter()
                    .map(|file| json!({"name": file.name, "source": file.source}))
                    .collect();
                let mut record = serde_json::to_vec(&json!({
                    "record": "dae_decompile",
                    "backend": "flutter",
                    "command": "decompile",
                    "complete": true,
                    "files": files,
                }))
                .map_err(|error| Error::new("INVALID_QUERY", error.to_string()))?;
                record.push(b'\n');
                record
            }
        };
        perf.counters.output_bytes = Some(bytes.len());
        perf.measure("write", || {
            super::write_output(&bytes, output_path.as_deref(), sink)
        })
    })();
    super::finish(perf, perf_enabled, result)
}
