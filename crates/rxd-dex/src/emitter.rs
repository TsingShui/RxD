//! `getclass`'s Java emitter: RadGuard behind a DEX-entry seam.
//!
//! The seam is the DEX entry bytes, not rasc's own parse tree: the emitter parses
//! the entry itself, so no second parse tree has to agree with the scanner's.
//! `radguard::Decompiler::parse` borrows those bytes, which keeps the walk's
//! "consume the entry inside the walk" property - a real entry is 7-12 MiB and
//! copying it was pure added cost on every `getclass`.
//!
//! Two contracts the callers rely on:
//!
//! * a class the entry does not define is an `Err`, not an empty string, so
//!   `map_class_hit` keeps walking the remaining DEX entries;
//! * rendering is stateless per class, so one parsed entry renders many classes
//!   with nothing to reset between requests - there is no emitter state to reuse.

use anyhow::{Context, Result, bail};

/// Render one class from the bytes of the DEX entry that defines it.
pub fn render(data: &[u8], descriptor: &str) -> Result<String> {
    use crate::profile::{Stage, measure};
    let decompiler = measure(Stage::Engine, || {
        radguard::Decompiler::parse(data).context("parse DEX for the Java emitter")
    })?;
    let rendered = measure(Stage::Emit, || {
        decompiler
            .decompile_class(descriptor)
            .with_context(|| format!("could not decompile {descriptor}"))
    })?;
    match rendered {
        Some(output) => Ok(output.source),
        None => bail!("class {descriptor} is not defined in this DEX"),
    }
}
