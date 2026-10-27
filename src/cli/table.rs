//! Unix-table listings: one record per line, TAB-separated cells.
//!
//! Atoms (coordinates, identifiers, numbers) are written verbatim, free text is
//! JSON-quoted so a value may contain TAB, newline or a quote, and an absent value
//! is `\N` so it stays distinct from an empty string.
use anyhow::Result;

pub(crate) const NULL: &str = "\\N";

pub(crate) enum Cell<'a> {
    Atom(&'a str),
    Text(&'a str),
    Null,
}

/// Render one line, TAB-separated, newline-terminated.
pub(crate) fn line(cells: &[Cell<'_>]) -> Result<String> {
    let mut out = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            out.push('\t');
        }
        match cell {
            Cell::Atom(value) => out.push_str(value),
            Cell::Text(value) => {
                let quoted = serde_json::to_string(value)
                    .map_err(|error| anyhow::anyhow!("INVALID_INPUT: {error}"))?;
                out.push_str(&quoted);
            }
            Cell::Null => out.push_str(NULL),
        }
    }
    out.push('\n');
    Ok(out)
}
