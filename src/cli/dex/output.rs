use super::super::Output;
use anyhow::{Context, Result};
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

/// Write identical payload bytes to the sink and an optional file.
pub(super) fn emit(payload: &str, output: Option<&Path>, sink: &mut Output<'_>) -> Result<()> {
    if let Some(path) = output {
        fs::write(path, payload).with_context(|| format!("write {}", path.display()))?;
    }
    let mut writer = io::BufWriter::new(sink);
    writer.write_all(payload.as_bytes())?;
    writer.flush()?;
    Ok(())
}

/// Emit one listing document: the columns declare the schema once, rows follow as
/// comma-separated values. The separator belongs to the caller's chunk boundary so
/// parallel rendering keeps working.
pub(super) fn emit_table(
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
