//! RXD_TRACE diagnostics stay separate from --perf JSONL.

use std::fmt::Arguments;

pub fn diagnose(args: Arguments<'_>) {
    if std::env::var_os("RXD_TRACE").is_none() {
        return;
    }
    eprintln!("{args}");
}
