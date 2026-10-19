//! Lossless bridge for JSON strings containing isolated UTF-16 surrogate units.
//! serde_json::Value stores Unicode scalar strings, so such literals use a typed
//! `{utf16_units: [...]}` representation while ordinary strings stay unchanged.
use serde_json::{Value, value::RawValue};
use std::collections::BTreeMap;

pub(super) fn parse(bytes: &[u8]) -> Result<Value, String> {
    if let Ok(value) = serde_json::from_slice(bytes) {
        return Ok(value);
    }
    let raw: Box<RawValue> = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    decode(raw.get(), 0)
}
fn decode(raw: &str, depth: usize) -> Result<Value, String> {
    if depth > 128 {
        return Err("backend JSON nesting exceeded".into());
    }
    if let Ok(value) = serde_json::from_str(raw) {
        return Ok(value);
    }
    match raw.as_bytes().first() {
        Some(b'{') => {
            let object: BTreeMap<String, Box<RawValue>> =
                serde_json::from_str(raw).map_err(|error| error.to_string())?;
            object
                .into_iter()
                .map(|(key, value)| Ok((key, decode(value.get(), depth + 1)?)))
                .collect::<Result<serde_json::Map<_, _>, String>>()
                .map(Value::Object)
        }
        Some(b'[') => {
            let array: Vec<Box<RawValue>> =
                serde_json::from_str(raw).map_err(|error| error.to_string())?;
            array
                .into_iter()
                .map(|value| decode(value.get(), depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Some(b'"') => {
            let mut units = Vec::new();
            let mut chars = raw[1..raw.len() - 1].chars();
            while let Some(character) = chars.next() {
                let character = if character == '\\' {
                    match chars.next().ok_or("truncated JSON escape")? {
                        'u' => {
                            let digits: String = chars.by_ref().take(4).collect();
                            if digits.len() != 4 {
                                return Err("truncated Unicode escape".into());
                            }
                            units.push(
                                u16::from_str_radix(&digits, 16)
                                    .map_err(|error| error.to_string())?,
                            );
                            continue;
                        }
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        character @ ('"' | '\\' | '/') => character,
                        _ => return Err("invalid JSON escape".into()),
                    }
                } else {
                    character
                };
                let mut buffer = [0; 2];
                units.extend_from_slice(character.encode_utf16(&mut buffer));
            }
            Ok(serde_json::json!({"utf16_units":units}))
        }
        _ => Err("invalid backend JSON".into()),
    }
}
