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
    // Every ceiling comes from rxd_core::Budget; no CLI flag exposes one.
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Enumerate every indexed file and its range status (metadata JSONL).
    #[command(name = "entries")]
    Entries {
        #[command(flatten)]
        common: Common,
        #[arg(long)]
        jsonl: bool,
        /// Case-insensitive substring over the archive-relative path.
        #[arg(short, long)]
        filter: Option<String>,
    },
    /// Write one original file to --output and return a JSON extraction receipt.
    Extract {
        #[command(flatten)]
        common: Common,
        /// The `at` an entries row prints: an archive-relative path.
        #[arg(long, requires = "output")]
        at: String,
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
    pub(crate) fn debug(&self) -> bool {
        self.common().debug
    }
    pub(crate) fn perf(&self) -> bool {
        self.common().perf
    }
}
