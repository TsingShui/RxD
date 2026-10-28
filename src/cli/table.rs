//! Tables: one # column header, then TAB-separated rows.
//! Atoms are verbatim, text is JSON-quoted, and absent values use `\N`.
use anyhow::Result;

pub(crate) const NULL: &str = "\\N";

/// One comment-style column header, retained even when a table has no data rows.
pub(crate) fn header(columns: &[&str]) -> String {
    format!("# {}\n", columns.join("\t"))
}

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
