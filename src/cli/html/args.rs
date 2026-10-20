use super::super::filter::CaseArgs;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args)]
pub(crate) struct Common {
    /// One regular local HTML file.
    pub(super) input: PathBuf,
    /// Write complete JSONL atomically instead of writing to stdout.
    #[arg(short, long)]
    pub(super) output: Option<PathBuf>,
    #[arg(long)]
    pub(super) debug: bool,
    #[arg(long)]
    pub(super) perf: bool,
    // The shared budget table (rxd_core::Budget) supplies input, record, output and
    // depth ceilings; no per-backend knob exposes them.
}
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Enumerate lexical_naive_v1 records (not a DOM or browser-complete parse).
    #[command(name = "entries")]
    Entries {
        #[command(flatten)]
        common: Common,
        /// Exact record kind.
        #[arg(long, value_parser = ["element", "text", "comment", "doctype"])]
        kind: Option<String>,
        /// Exact ASCII-case-folded element name.
        #[arg(long)]
        tag: Option<String>,
        /// Attribute presence or exact decoded value (NAME[=VALUE]).
        #[arg(long)]
        attr: Option<String>,
        /// Canonical element ID whose lexical subtree is selected.
        #[arg(long)]
        within: Option<String>,
        /// Maximum depth relative to --within (or top level), including zero.
        #[arg(long)]
        depth: Option<usize>,
        /// Substring over the element name or the text it holds, folded unless
        /// `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
    },
    /// Fetch original bytes from an exact catalog record.
    Extract {
        #[command(flatten)]
        common: Common,
        /// The `at` an entries row prints.
        #[arg(long, requires = "output")]
        at: String,
        /// Extract only element content rather than its outer range.
        #[arg(long)]
        content: bool,
    },
}
impl Command {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Entries { .. } => "entries",
            Self::Extract { .. } => "extract",
        }
    }
    pub(crate) fn debug(&self) -> bool {
        match self {
            Self::Entries { common, .. } | Self::Extract { common, .. } => common.debug,
        }
    }
    pub(crate) fn perf(&self) -> bool {
        match self {
            Self::Entries { common, .. } | Self::Extract { common, .. } => common.perf,
        }
    }
}
