use super::super::filter::CaseArgs;
use super::super::xref::Selector;
use clap::{Args, Subcommand};
use std::path::PathBuf;
#[derive(Args)]
pub(crate) struct Common {
    /// ASAR regular file, identified by its contents.
    pub(super) input: PathBuf,
    /// Write output atomically; extract writes raw file bytes here.
    #[arg(short, long)]
    pub(super) output: Option<PathBuf>,
    /// Enable backend diagnostic details on stderr; does not execute input.
    #[arg(long)]
    pub(super) debug: bool,
    /// Write timing/resource JSONL to stderr without changing the result payload.
    #[arg(long)]
    pub(super) perf: bool,
    // Every ceiling comes from rxd_core::Budget; no CLI flag exposes one.
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Enumerate every indexed file and its range status (metadata JSONL).
    #[command(
        name = "entries",
        after_long_help = "Examples:
  rxd asar entries app.asar -f main
Output: file JSONL; at is an archive-relative path, detail describes packed/unpacked storage.
Next: rxd asar extract app.asar --at '<file.at>' -o extracted.js.
Listing does not execute or parse entry contents; invalid packed ranges stay explicit."
    )]
    Entries {
        #[command(flatten)]
        common: Common,
        ///  substring over the archive-relative path. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
    },
    /// Write one original file to --output and return a JSON extraction receipt.
    #[command(mut_arg("at", |arg| arg.required(true).value_name("ENTRY_PATH").help("Copy entries.at: exact archive-relative path, e.g. main.js; not a host path")), mut_arg("output", |arg| arg.required(true).help("Write original entry bytes to this file; stdout is a receipt")), after_long_help = "Examples:
  rxd asar entries app.asar -f main
  rxd asar extract app.asar --at 'main.js' -o extracted-main.js
Required: --at and -o FILE. Use actual entries.at, not an absolute host path.
Output: original entry bytes in FILE and a file receipt on stdout.
Next: rxd js entities extracted-main.js. Analysis never executes extracted code.")]
    Extract {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        selector: Selector,
    },
}

impl Command {
    pub(super) fn common(&self) -> &Common {
        match self {
            Self::Entries { common, .. } | Self::Extract { common, .. } => common,
        }
    }
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Entries { .. } => "entries",
            Self::Extract { .. } => "extract",
        }
    }
}

impl Command {
    pub(crate) fn perf(&self) -> bool {
        self.common().perf
    }
}
