use super::super::Output;
use anyhow::{Context, Result};
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

/// Write identical payload bytes to the sink and an optional file.
pub(super) fn emit(payload: &str, output: Option<&Path>, sink: &mut Output<'_>) -> Result<()> {
    if sink.remaining().is_some_and(|limit| payload.len() > limit) {
        anyhow::bail!("RESOURCE_LIMIT: query output byte budget exceeded");
    }
    if let Some(path) = output {
        fs::write(path, payload).with_context(|| format!("write {}", path.display()))?;
    }
    let mut writer = io::BufWriter::new(sink);
    writer.write_all(payload.as_bytes())?;
    writer.flush()?;
    Ok(())
}
