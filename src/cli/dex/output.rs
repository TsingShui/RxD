use anyhow::{Context, Result};
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

/// Where a command's payload goes: buffered stdout.
fn payload_writer() -> impl Write {
    io::BufWriter::new(io::stdout().lock())
}

/// Write identical payload bytes to stdout and an optional file; diagnostics stay separate.
pub(super) fn emit(payload: &str, output: Option<&Path>) -> Result<()> {
    if let Some(path) = output {
        fs::write(path, payload).with_context(|| format!("write {}", path.display()))?;
    }
    #[cfg(feature = "mcp")]
    if let Some(result) = crate::query::capture_write(payload.as_bytes()) {
        return result.map_err(anyhow::Error::from);
    }
    let mut stdout = payload_writer();
    stdout.write_all(payload.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

/// Emit chunks with the same bytes as emit, releasing each after writing.
pub(super) fn emit_rows(
    chunks: impl Iterator<Item = String>,
    output: Option<&Path>,
) -> Result<usize> {
    let mut file = match output {
        Some(path) => {
            Some(fs::File::create(path).with_context(|| format!("write {}", path.display()))?)
        }
        None => None,
    };
    let mut stdout = payload_writer();
    let mut written = 0usize;
    for chunk in chunks {
        written += chunk.len();
        if let Some(file) = file.as_mut() {
            file.write_all(chunk.as_bytes())?;
        }
        #[cfg(feature = "mcp")]
        if let Some(result) = crate::query::capture_write(chunk.as_bytes()) {
            result?;
            continue;
        }
        stdout.write_all(chunk.as_bytes())?;
    }
    if let Some(file) = file.as_mut() {
        file.flush()?;
    }
    stdout.flush()?;
    Ok(written)
}
