use super::super::{input_bytes, positive};
use clap::{Args, Subcommand};
use std::path::PathBuf;
#[derive(Args)]
pub(crate) struct Common {
    /// ASAR regular file, identified by its contents.
    pub(super) input: PathBuf,
    /// Write output atomically; extract writes raw file bytes here.
    #[arg(short, long)]
    pub(super) output: Option<PathBuf>,
    #[arg(long)]
    pub(super) debug: bool,
    #[arg(long)]
    pub(super) perf: bool,
    /// Maximum extracted file size, independent of the total archive size.
    /// Input byte cap in bytes (1 through 8589934592); 8 GiB when omitted.
    #[arg(long, value_parser = input_bytes)]
    pub(super) max_input_bytes: Option<usize>,
    #[arg(long, default_value_t = 100_000, value_parser = positive)]
    pub(super) max_records: usize,
    /// Bound listing bytes, or extracted bytes plus the JSON receipt.
    #[arg(long, default_value_t = 32 * 1024 * 1024, value_parser = positive)]
    pub(super) max_output_bytes: usize,
    #[arg(long, default_value_t = 8 * 1024 * 1024, value_parser = positive)]
    pub(super) max_header_bytes: usize,
    #[arg(long, default_value_t = 10_000, value_parser = positive)]
    pub(super) max_entries: usize,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// List every indexed file and its range status (metadata JSONL).
    List {
        #[command(flatten)]
        common: Common,
        #[arg(long)]
        jsonl: bool,
    },
    /// Write one original file to --output and return a JSON extraction receipt.
    Extract {
        #[command(flatten)]
        common: Common,
        /// Exact relative index path, with no file type or extension restriction.
        #[arg(long, requires = "output")]
        file: String,
    },
}

impl Command {
    pub(super) fn common(&self) -> &Common {
        match self {
            Self::List { common, .. } | Self::Extract { common, .. } => common,
        }
    }
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::List { .. } => "list",
            Self::Extract { .. } => "extract",
        }
    }
}

impl Command {
    pub(crate) fn debug(&self) -> bool {
        self.common().debug
    }
    pub(crate) fn perf(&self) -> bool {
        self.common().perf
    }
}
