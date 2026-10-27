//! Listing documents: the columns declare the schema once, rows carry values only.
use super::Output;
use anyhow::{Context, Result};
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

/// Emit one listing document to the sink and an optional file.
///
/// `chunks` yields already-rendered, comma-separated row values, so a backend can
/// keep rendering chunks in parallel; the separator between chunks belongs here.
pub(crate) fn emit_table(
    chunks: impl Iterator<Item = String>,
    columns: &str,
    output: Option<&Path>,
    sink: &mut Output<'_>,
) -> Result<()> {
    let mut file = match output {
        Some(path) => {
            Some(fs::File::create(path).with_context(|| format!("write {}", path.display()))?)
        }
        None => None,
    };
    let mut writer = io::BufWriter::new(sink);
    write_both(
        &mut file,
        &mut writer,
        format!("{{\"columns\":{columns},\"rows\":[").as_bytes(),
    )?;
    let mut first = true;
    for chunk in chunks {
        if chunk.is_empty() {
            continue;
        }
        if !first {
            write_both(&mut file, &mut writer, b",")?;
        }
        write_both(&mut file, &mut writer, chunk.as_bytes())?;
        first = false;
    }
    write_both(&mut file, &mut writer, b"]}\n")?;
    if let Some(file) = file.as_mut() {
        file.flush()?;
    }
    writer.flush()?;
    Ok(())
}

fn write_both(
    file: &mut Option<fs::File>,
    writer: &mut io::BufWriter<&mut Output<'_>>,
    bytes: &[u8],
) -> Result<()> {
    if let Some(file) = file.as_mut() {
        file.write_all(bytes)?;
    }
    writer.write_all(bytes)?;
    Ok(())
}
