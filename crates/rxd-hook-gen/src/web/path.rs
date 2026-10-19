use crate::{Diagnostic, ErrorKind, Result, Stage};

pub(super) struct PropertyPath {
    pub keys: Vec<String>,
    pub normalized: String,
}

fn invalid() -> Diagnostic {
    Diagnostic::new(
        ErrorKind::InvalidArguments,
        Stage::WebTarget,
        "expected a page-global property path: ASCII identifiers or JSON double-quoted bracket keys; expressions, __proto__, constructor and lexical/private bindings are unsupported",
    )
}

// Deliberately not a JS expression parser. Unicode and punctuation use JSON bracket keys.
pub(super) fn parse(input: &str) -> Result<PropertyPath> {
    let mut rest = input;
    let mut keys = vec![identifier(&mut rest)?];
    while !rest.is_empty() {
        if let Some(next) = rest.strip_prefix('.') {
            rest = next;
            keys.push(identifier(&mut rest)?);
        } else if let Some(next) = rest.strip_prefix('[') {
            let mut stream = serde_json::Deserializer::from_str(next).into_iter::<String>();
            let key = stream.next().ok_or_else(invalid)?.map_err(|_| invalid())?;
            let tail = &next[stream.byte_offset()..];
            rest = tail.strip_prefix(']').ok_or_else(invalid)?;
            keys.push(key);
        } else {
            return Err(invalid());
        }
    }
    if matches!(keys[0].as_str(), "globalThis" | "window" | "self") {
        keys.remove(0);
    }
    if keys.is_empty()
        || keys
            .iter()
            .any(|key| matches!(key.as_str(), "__proto__" | "constructor"))
    {
        return Err(invalid());
    }
    let mut normalized = String::from("globalThis");
    for key in &keys {
        normalized.push('[');
        normalized.push_str(&crate::source::js_string(key));
        normalized.push(']');
    }
    Ok(PropertyPath { keys, normalized })
}

fn identifier(rest: &mut &str) -> Result<String> {
    let end = rest
        .bytes()
        .enumerate()
        .take_while(|(i, c)| {
            c.is_ascii_alphabetic() || matches!(c, b'_' | b'$') || (*i > 0 && c.is_ascii_digit())
        })
        .count();
    if end == 0 {
        return Err(invalid());
    }
    let key = rest[..end].to_owned();
    *rest = &rest[end..];
    Ok(key)
}
