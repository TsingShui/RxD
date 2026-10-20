//! Where the per-entry trace goes.
//!\n//! `--perf` emits the shared perf record as JSONL on stderr, so a text line would corrupt
//! that stream for whoever parses it. The trace stays available for local investigation
//! behind `RXD_TRACE`, a developer switch rather than part of the CLI contract.

use std::fmt::Arguments;

/// Writes a diagnostic line, on stderr, when `RXD_TRACE` is set.
pub fn diagnose(args: Arguments<'_>) {
    if std::env::var_os("RXD_TRACE").is_none() {
        return;
    }
    eprintln!("{args}");
}
