//! Shared CLI case mode; matching lives in rxd_core::filter.

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
