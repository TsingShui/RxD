//! `rxd dae`: Dart AOT (Flutter) snapshot navigation and reconstruction.

use std::path::PathBuf;

use super::Output;
use super::table::{self, Cell};
use clap::{Args, Subcommand};
use rxd_core::envelope::{self, RecordMeta};
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
        long_about = "Columns: at, label\nList recovered functions. One # column header precedes the data rows.",
        after_long_help = "Examples:\nrxd dae functions libapp.so\nrxd dae functions libapp.so -f fl_clash\nOutput: one row per function; at is the code entry offset, label is library::class::method.\nNames come from the snapshot's library and class tables, not from source.\nNext: rxd dae decompile libapp.so --lib <library prefix>."
    )]
    Functions(FunctionsOptions),
    /// List classes (columns: at, full_name, methods).
    #[command(
        long_about = "Columns: at, full_name, methods\nList classes. One # column header precedes the data rows.",
        after_long_help = "Examples:\nrxd dae classes libapp.so -f ClashConfig\nOutput: at is class:0x<class id>, full_name is library::Class.\nmethods counts the class's own function-table entries; inherited members are not included.\nThe superclass chain is not exposed yet. Next: functions --class or decompile --class."
    )]
    Classes(ClassesOptions),
    /// List pool strings (columns: at, value).
    #[command(
        long_about = "Columns: at, value\nList pool strings. One # column header precedes the data rows.",
        after_long_help = "Examples:\nrxd dae strings libapp.so -f token\nOutput: one row per decoded pool string; at is str:0x<object ref>.\nUndecodable entries are skipped; use xref to see which code loads one."
    )]
    Strings(StringsOptions),
    /// Search instructions that load a pool string.
    #[command(
        long_about = "Columns: context, match\nList explicit references as TAB-separated rows after one # column header. Successful zero-hit queries retain the header.",
        after_long_help = "Examples:\nrxd dae xref libapp.so --at 'str:0x7f0123'\nrxd dae xref libapp.so -m Authorization\nRequired: exactly one of --at/--match; --match must resolve to one string.\nOutput: TAB-separated context, match. context is the containing function entry; match is the exact loaded string.\nOnly explicit pool loads are matched."
    )]
    Xref(XrefOptions),
    /// Reconstruct Dart-like pseudocode for whole libraries.
    #[command(
        after_long_help = "Examples:\nrxd dae decompile libapp.so --at 0x532ad0\nrxd dae decompile libapp.so --class 'class:0x8f5'\nrxd dae decompile libapp.so --lib fl_clash\nDefault stdout is Dart pseudocode, one `// ===== <file> =====` block per rendered library.\n--at copies functions.at (one function), --class copies classes.at, --lib widens to a package.\nThis is machine-level pseudocode (registers and memory access remain), not source Dart."
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
    /// Copy classes.at: list only this class's own members.
    #[arg(long)]
    class: Option<String>,
    #[command(flatten)]
    case: super::filter::CaseArgs,
}

#[derive(Args)]
pub struct ClassesOptions {
    #[command(flatten)]
    options: Options,
    /// Substring over the class name, folded unless `--case sensitive`.
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
    #[arg(long, conflicts_with = "at")]
    lib: Option<String>,
    /// Copy functions.at: render this one function only.
    #[arg(long, value_parser = parse_address, conflicts_with_all = ["lib", "class"])]
    at: Option<u64>,
    /// Copy classes.at: render every method of this class.
    #[arg(long, value_parser = parse_class, conflicts_with_all = ["lib", "at"])]
    class: Option<i64>,
}

impl Command {
    fn options(&self) -> &Options {
        match self {
            Self::Info(options) => options,
            Self::Functions(options) => &options.options,
            Self::Classes(options) => &options.options,
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
            Self::Classes(_) => "classes",
            Self::Strings(_) => "strings",
            Self::Xref(_) => "xref",
            Self::Decompile(_) => "decompile",
        }
    }
}

/// Function entry address as printed by `functions`.
fn parse_address(text: &str) -> std::result::Result<u64, String> {
    let digits = text.trim_start_matches("0x");
    u64::from_str_radix(digits, 16).map_err(|_| format!("expected 0x<address>, got {text}"))
}

/// Accept `str:0x…` (as printed by `strings`) or a bare object ref.
fn parse_ref(at: &str) -> Result<u64> {
    let digits = at.strip_prefix("str:").unwrap_or(at);
    let digits = digits.strip_prefix("0x").unwrap_or(digits);
    u64::from_str_radix(digits, 16)
        .map_err(|_| Error::new("INVALID_QUERY", format!("expected str:0x<ref>, got {at}")))
}

/// Class coordinate as printed by `classes`: class:0x<id>.
fn parse_class(text: &str) -> std::result::Result<i64, String> {
    let digits = text.strip_prefix("class:").unwrap_or(text);
    let digits = digits.strip_prefix("0x").unwrap_or(digits);
    i64::from_str_radix(digits, 16).map_err(|_| format!("expected class:0x<id>, got {text}"))
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
                let mut value = envelope::record(
                    "dart_snapshot",
                    RecordMeta {
                        source_id: Some(snapshot.source_id()),
                        ..RecordMeta::default()
                    },
                    json!({
                        "sdk": info.sdk,
                        "profile_status": info.status,
                        "compressed_pointers": info.compressed_pointers,
                        "libraries": info.libraries,
                        "functions": info.functions,
                        "platform_warning": info.platform_warning,
                    })
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
                );
                envelope::attach_protocol(&mut value, "flutter", "info");
                let mut record = serde_json::to_vec(&value)
                    .map_err(|error| Error::new("INVALID_QUERY", error.to_string()))?;
                record.push(b'\n');
                record
            }
            Command::Functions(functions) => {
                let filter = functions.filter.as_deref().map(|needle| {
                    rxd_core::filter::Contains::new(needle, functions.case.sensitive())
                });
                let class = match &functions.class {
                    Some(text) => Some(snapshot.class_name(
                        parse_class(text).map_err(|error| Error::new("INVALID_QUERY", error))?,
                    )?),
                    None => None,
                };
                let mut out = table::header(&["at", "label"]);
                perf.measure("analyze", || {
                    snapshot.functions_into(&mut out, filter.as_ref(), class.as_deref())
                })?;
                out.into_bytes()
            }
            Command::Classes(classes) => {
                let filter = classes.filter.as_deref().map(|needle| {
                    rxd_core::filter::Contains::new(needle, classes.case.sensitive())
                });
                let mut out = table::header(&["at", "full_name", "methods"]);
                perf.measure("analyze", || {
                    snapshot.classes_into(&mut out, filter.as_ref())
                })?;
                out.into_bytes()
            }
            Command::Strings(strings) => {
                let filter = strings.filter.as_deref().map(|needle| {
                    rxd_core::filter::Contains::new(needle, strings.case.sensitive())
                });
                let mut out = table::header(&["at", "value"]);
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
                let mut out = table::header(&["context", "match"]);
                for hit in hits {
                    out.push_str(
                        &table::line(&[Cell::Atom(&hit.context_at), Cell::Text(&hit.value)])
                            .map_err(|error| Error::new("INVALID_QUERY", error.to_string()))?,
                    );
                }
                out.into_bytes()
            }
            Command::Decompile(decompile) => {
                let rendered = perf.measure("analyze", || {
                    snapshot.decompile(decompile.lib.as_deref(), decompile.at, decompile.class)
                })?;
                // Plain pseudocode, one labelled block per rendered library: source
                // is read directly, never unwrapped from a record.
                let mut out = String::with_capacity(1 << 16);
                for file in rendered {
                    out.push_str("// ===== ");
                    out.push_str(&file.name);
                    out.push_str(" =====\n");
                    out.push_str(&file.source);
                    if !file.source.ends_with('\n') {
                        out.push('\n');
                    }
                }
                out.into_bytes()
            }
        };
        perf.counters.output_bytes = Some(bytes.len());
        perf.measure("write", || {
            super::write_output(&bytes, output_path.as_deref(), sink)
        })
    })();
    super::finish(perf, perf_enabled, result)
}
