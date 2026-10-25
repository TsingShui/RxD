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

/// Emit ordered chunks, releasing each after writing.
pub(super) fn emit_rows(
    chunks: impl Iterator<Item = String>,
    output: Option<&Path>,
    sink: &mut Output<'_>,
) -> Result<usize> {
    let mut file = match output {
        Some(path) => {
            Some(fs::File::create(path).with_context(|| format!("write {}", path.display()))?)
        }
        None => None,
    };
    let mut writer = io::BufWriter::new(sink);
    let mut written = 0usize;
    for chunk in chunks {
        written += chunk.len();
        if let Some(file) = file.as_mut() {
            file.write_all(chunk.as_bytes())?;
        }
        writer.write_all(chunk.as_bytes())?;
    }
    if let Some(file) = file.as_mut() {
        file.flush()?;
    }
    writer.flush()?;
    Ok(written)
}
