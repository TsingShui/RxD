//! `rxd dae`: Dart AOT (Flutter) snapshot navigation and reconstruction.

use std::path::PathBuf;

use super::Output;
use super::table::{self, Cell};
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
    /// List pool strings (columns: at, value).
    #[command(
        after_long_help = "Examples:\nrxd dae strings libapp.so -f token\nOutput: one row per decoded pool string; at is str:0x<object ref>.\nUndecodable entries are skipped; use xref to see which code loads one."
    )]
    Strings(StringsOptions),
    /// Search instructions that load a pool string.
    #[command(
        after_long_help = "Examples:\nrxd dae xref libapp.so --at 'str:0x7f0123'\nrxd dae xref libapp.so -m Authorization\nRequired: exactly one of --at/--match; --match must resolve to one string.\nColumns: at, context_at, instruction, value. at is an instruction address,\ncontext_at the containing function entry; value is the loaded string.\nOnly pool loads are matched, by instruction encoding; matches are then decoded."
    )]
    Xref(XrefOptions),
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
pub struct StringsOptions {
    #[command(flatten)]
    options: Options,
    /// Substring over the string value, folded unless `--case sensitive`.
    #[arg(short = 'f', long)]
    filter: Option<String>,
    #[command(flatten)]
    case: super::filter::CaseArgs,
}

#[derive(Args)]
#[command(group(clap::ArgGroup::new("anchor").required(true).args(["at", "filter"])))]
pub struct XrefOptions {
    #[command(flatten)]
    options: Options,
    /// Copy strings.at: str:0x<object ref>, or an object ref in hexadecimal.
    #[arg(long, conflicts_with = "filter")]
    at: Option<String>,
    /// Substring over the string value; must resolve to exactly one pool string.
    #[arg(short = 'm', long = "match", conflicts_with = "at")]
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
            Self::Strings(options) => &options.options,
            Self::Xref(options) => &options.options,
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
            Self::Strings(_) => "strings",
            Self::Xref(_) => "xref",
            Self::Decompile(_) => "decompile",
        }
    }
}

/// Accept `str:0x…` (as printed by `strings`) or a bare object ref.
fn parse_ref(at: &str) -> Result<u64> {
    let digits = at.strip_prefix("str:").unwrap_or(at);
    let digits = digits.strip_prefix("0x").unwrap_or(digits);
    u64::from_str_radix(digits, 16)
        .map_err(|_| Error::new("INVALID_QUERY", format!("expected str:0x<ref>, got {at}")))
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
            Command::Strings(strings) => {
                let filter = strings.filter.as_deref().map(|needle| {
                    rxd_core::filter::Contains::new(needle, strings.case.sensitive())
                });
                let mut out = String::with_capacity(1 << 20);
                perf.measure("analyze", || {
                    snapshot.strings_into(&mut out, filter.as_ref())
                })?;
                out.into_bytes()
            }
            Command::Xref(references) => {
                let target = match (&references.at, &references.filter) {
                    (Some(at), None) => rxd_flutter::Target::Ref(parse_ref(at)?),
                    (None, Some(text)) => {
                        rxd_flutter::Target::Text(text, references.case.sensitive())
                    }
                    _ => unreachable!("clap enforces exactly one xref anchor"),
                };
                let hits = perf.measure("analyze", || snapshot.xref(target))?;
                let mut out = String::with_capacity(1 << 12);
                for hit in hits {
                    out.push_str(
                        &table::line(&[
                            Cell::Atom(&hit.at),
                            Cell::Atom(&hit.context_at),
                            Cell::Text(&hit.instruction),
                            Cell::Text(&hit.value),
                        ])
                        .map_err(|error| Error::new("INVALID_QUERY", error.to_string()))?,
                    );
                }
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
