//! The `--case` flag every filter shares.
//!
//! One default for the whole CLI: filters fold case. `--case sensitive` is the explicit way
//! to ask for exact matching, so a caller who needs it never has to remember which command
//! happens to be strict. The folding rule itself lives in `rxd_core::filter`, so the
//! libraries and the CLI cannot drift apart.

use clap::{Args, ValueEnum};

/// Whether matching folds case.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum CaseMode {
    #[default]
    Insensitive,
    Sensitive,
}

impl CaseMode {
    pub fn is_sensitive(self) -> bool {
        self == Self::Sensitive
    }
}

/// The shared flag. Flatten it into any command that takes `-f/--filter`.
#[derive(Debug, Args)]
pub struct CaseArgs {
    /// Match case: `insensitive` (default) folds case; `sensitive` compares exactly.
    #[arg(long, value_enum, default_value_t = CaseMode::Insensitive)]
    pub case: CaseMode,
}

impl CaseArgs {
    pub fn sensitive(&self) -> bool {
        self.case.is_sensitive()
    }
}
