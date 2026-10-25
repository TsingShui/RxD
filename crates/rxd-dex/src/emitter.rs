//! RadGuard consumes borrowed DEX-entry bytes.
//! Missing classes are errors; class rendering has no reusable state.

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

/// Render one method from the bytes of the DEX entry that declares it.
pub fn render_method(
    data: &[u8],
    descriptor: &str,
    name: &str,
    prototype: &str,
) -> Result<Option<String>> {
    use crate::profile::{Stage, measure};
    let decompiler = measure(Stage::Engine, || {
        radguard::Decompiler::parse(data).context("parse DEX for the Java emitter")
    })?;
    let rendered = measure(Stage::Emit, || {
        decompiler
            .decompile_method(descriptor, name, prototype)
            .with_context(|| format!("could not decompile {descriptor}->{name}{prototype}"))
    })?;

    Ok(rendered.map(|output| output.source.trim_start_matches('\n').to_owned()))
}
