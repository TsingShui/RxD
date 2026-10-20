//! Lossless literal occurrences from parser evidence, never compatibility Strings or bytecode.
use crate::static_analysis::Analysis;
use pycdc::{evidence::Span, object::PyObject};
use rxd_core::{Error, Result};
use std::fmt::Write;

#[derive(Clone)]
pub struct Alias {
    pub span: Span,
    /// Path to the referenced container; empty means the owner's co_consts field.
    pub path: Vec<usize>,
}
pub struct Literal {
    pub owner: usize,
    pub path: Vec<usize>,
    pub occurrence: Option<usize>,
    pub definition: usize,
    pub aliases: Vec<Alias>,
    pub kind: &'static str,
    pub reference: bool,
}
pub enum Query<'a> {
    All,
    /// A text substring, folded unless `case_sensitive`.
    Text {
        text: &'a str,
        case_sensitive: bool,
    },
    Hex(&'a [u8]),
}
pub fn constant_id(analysis: &Analysis, owner: usize, path: &[usize]) -> String {
    let mut id = format!("rxd1:pyc:{}:const:{owner}:", analysis.source_id);
    for (position, index) in path.iter().enumerate() {
        if position != 0 {
            id.push('.');
        }
        write!(&mut id, "{index}").unwrap();
    }
    id
}
/// Resolve one strings occurrence through parser definitions in O(path depth).
/// Shared payloads retain the selected owner/path; no value search or alias expansion.
pub struct SelectedConstant {
    pub owner: usize,
    pub path: Vec<usize>,
    pub kind: &'static str,
}
pub fn resolve_id(analysis: &mut Analysis, id: &str) -> Result<SelectedConstant> {
    fn invalid() -> Error {
        Error::new("INVALID_QUERY", "invalid or stale PYC constant ID")
    }
    fn index(value: &str) -> Result<usize> {
        if value.is_empty()
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(invalid());
        }
        value.parse().map_err(|_| invalid())
    }
    analysis.charge_work(id.len())?;
    // A row prints the copy-only `const:<owner>:<path>`; identity lives in the header, so
    // the short form is expanded here instead of making a caller reassemble the hash.
    let (digest, owner, encoded_path) = match id.strip_prefix("const:") {
        Some(rest) => {
            let (owner, encoded_path) = rest.split_once(':').ok_or_else(invalid)?;
            if encoded_path.contains(':') {
                return Err(invalid());
            }
            (None, owner, encoded_path)
        }
        None => {
            let mut parts = id.split(':');
            if parts.next() != Some("rxd1") || parts.next() != Some("pyc") {
                return Err(invalid());
            }
            let digest = parts.next().unwrap_or("");
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                || parts.next() != Some("const")
            {
                return Err(invalid());
            }
            let owner = parts.next().unwrap_or("");
            let encoded_path = parts.next().ok_or_else(invalid)?;
            if parts.next().is_some() {
                return Err(invalid());
            }
            (Some(digest), owner, encoded_path)
        }
    };
    if digest.is_some_and(|digest| digest != analysis.source_id.as_str()) {
        return Err(Error::new(
            "SOURCE_MISMATCH",
            "constant ID belongs to different source content",
        ));
    }
    let owner = index(owner)?;
    if owner >= analysis.codes.len() {
        return Err(invalid());
    }
    let mut definition = analysis
        .field_definition(owner, "consts")
        .ok_or_else(invalid)?;
    let mut path = Vec::new();
    for value in encoded_path.split('.') {
        analysis.charge_work(1)?;
        if !matches!(
            analysis.evidence.occurrences[definition].object.as_deref(),
            Some(PyObject::Tuple(_) | PyObject::FrozenSet(_))
        ) {
            return Err(invalid());
        }
        let slot = index(value)?;
        let child = *analysis.evidence.occurrences[definition]
            .children
            .get(slot)
            .ok_or_else(invalid)?;
        definition = analysis.evidence.occurrences[child].definition;
        path.push(slot);
    }
    let kind = match analysis.evidence.occurrences[definition].object.as_deref() {
        Some(PyObject::Str(_)) => "str",
        Some(PyObject::Bytes(_)) => "bytes",
        _ => return Err(invalid()),
    };
    Ok(SelectedConstant { owner, path, kind })
}
/// Visit every owner/slot/immutable path, including every shared-container occurrence.
/// Marshal's reader already rejects cycles and enforces a fixed nesting cap.
pub fn enumerate(analysis: &mut Analysis, query: &Query<'_>) -> Result<Vec<Literal>> {
    let mut literals = Vec::new();
    for owner in 0..analysis.codes.len() {
        let occurrence = analysis.field_occurrence(owner, "consts").unwrap();
        let row = &analysis.evidence.occurrences[occurrence];
        let definition = row.definition;
        let mut aliases = Vec::new();
        if row.is_reference {
            let span = row.span;
            analysis.charge_work(1)?;
            aliases.push(Alias {
                span,
                path: Vec::new(),
            });
        }
        let length = analysis.evidence.occurrences[definition].children.len();
        for slot in 0..length {
            analysis.charge_work(1)?;
            let occurrence = analysis.evidence.occurrences[definition].children[slot];
            visit(
                analysis,
                owner,
                occurrence,
                &mut vec![slot],
                &mut aliases,
                &mut literals,
                query,
            )?;
        }
    }
    Ok(literals)
}
fn visit(
    analysis: &mut Analysis,
    owner: usize,
    occurrence: usize,
    path: &mut Vec<usize>,
    aliases: &mut Vec<Alias>,
    literals: &mut Vec<Literal>,
    query: &Query<'_>,
) -> Result<()> {
    analysis.charge_work(1)?;
    let row = &analysis.evidence.occurrences[occurrence];
    let definition = row.definition;
    let span = row.span;
    let is_reference = row.is_reference;
    let object = analysis.evidence.occurrences[definition]
        .object
        .as_deref()
        .unwrap();
    let kind = match object {
        PyObject::Str(_) => Some("str"),
        PyObject::Bytes(_) => Some("bytes"),
        _ => None,
    };
    if let Some(kind) = kind {
        let payload = analysis.evidence.occurrences[definition].payload.unwrap();
        // Strict UTF-8 validation and raw matching read the fixed literal payload.
        analysis.charge_work(payload.end - payload.start)?;
        let matched = match query {
            Query::All => true,
            Query::Text {
                text,
                case_sensitive,
            } => {
                if kind == "str" {
                    let Ok(value) = std::str::from_utf8(analysis.payload(definition).unwrap())
                        .map(str::to_owned)
                    else {
                        return Ok(());
                    };
                    // Charge the comparison, folded or not.
                    analysis.charge_work(value.len().saturating_add(text.len()))?;
                    rxd_core::filter::Contains::new(text, *case_sensitive).matches(&value)
                } else {
                    contains(analysis, payload, text.as_bytes())?
                }
            }
            Query::Hex(bytes) => contains(analysis, payload, bytes)?,
        };
        if !matched {
            return Ok(());
        }
        // Bound decoding, paths and retained alias evidence before allocating.
        let alias_work = aliases
            .iter()
            .try_fold(0usize, |sum, alias| sum.checked_add(alias.path.len() + 1))
            .ok_or_else(|| Error::new("RESOURCE_LIMIT", "alias work overflow"))?;
        analysis.charge_work(path.len().saturating_add(alias_work))?;
        literals.push(Literal {
            owner,
            path: path.clone(),
            occurrence: aliases.is_empty().then_some(occurrence),
            definition,
            aliases: aliases.clone(),
            kind,
            reference: is_reference,
        });
        return Ok(());
    }
    if !matches!(object, PyObject::Tuple(_) | PyObject::FrozenSet(_)) {
        return Ok(());
    }
    if is_reference {
        analysis.charge_work(path.len() + 1)?;
        aliases.push(Alias {
            span,
            path: path.clone(),
        });
    }
    let length = analysis.evidence.occurrences[definition].children.len();
    for slot in 0..length {
        // The dynamic path is bounded before each extension, including aliases.
        analysis.charge_work(1)?;
        path.push(slot);
        let child = analysis.evidence.occurrences[definition].children[slot];
        visit(analysis, owner, child, path, aliases, literals, query)?;
        path.pop();
    }
    if is_reference {
        aliases.pop();
    }
    Ok(())
}

/// Each examined byte comparison is charged before accessing bytes; no unbounded search.
fn contains(analysis: &mut Analysis, span: Span, needle: &[u8]) -> Result<bool> {
    if needle.is_empty() {
        return Ok(true);
    }
    let length = span.end - span.start;
    if needle.len() > length {
        return Ok(false);
    }
    for offset in 0..=length - needle.len() {
        let mut matched = true;
        for (index, &byte) in needle.iter().enumerate() {
            analysis.charge_work(1)?;
            if analysis.bytes[span.start + offset + index] != byte {
                matched = false;
                break;
            }
        }
        if matched {
            return Ok(true);
        }
    }
    Ok(false)
}
