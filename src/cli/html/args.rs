use super::super::{input_bytes, positive};
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
    /// Input byte cap in bytes (1 through 8589934592); 8 GiB when omitted.
    #[arg(long, value_parser = input_bytes)]
    pub(super) max_input_bytes: Option<usize>,
    #[arg(long, default_value_t = 100_000, value_parser = positive)]
    pub(super) max_records: usize,
    #[arg(long, default_value_t = 32 * 1024 * 1024, value_parser = positive)]
    pub(super) max_output_bytes: usize,
    #[arg(long, default_value_t = 512, value_parser = positive)]
    pub(super) max_depth: usize,
}
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Enumerate lexical_naive_v1 records (not a DOM or browser-complete parse).
    List {
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
    },
    /// Fetch original bytes from an exact catalog ID.
    Extract {
        #[command(flatten)]
        common: Common,
        /// Catalog record ID.
        #[arg(long, requires = "output")]
        id: String,
        /// Extract only element content rather than its outer range.
        #[arg(long)]
        content: bool,
    },
}
impl Command {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::List { .. } => "list",
            Self::Extract { .. } => "extract",
        }
    }
    pub(crate) fn debug(&self) -> bool {
        match self {
            Self::List { common, .. } | Self::Extract { common, .. } => common.debug,
        }
    }
    pub(crate) fn perf(&self) -> bool {
        match self {
            Self::List { common, .. } | Self::Extract { common, .. } => common.perf,
        }
    }
}
