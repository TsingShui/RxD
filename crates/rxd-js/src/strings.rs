use std::fmt::Write;

use serde::{Serialize, Serializer};
use serde_json::value::RawValue;

/// Preserve JS UTF-16 code units, including Oxc's escaped lone surrogates.
#[derive(Clone, Debug)]
pub struct JsString(Vec<u16>);

impl JsString {
    /// JSX uses XML entities, not JavaScript backslash escapes or renderer whitespace rules.
    pub fn jsx(value: &str) -> Self {
        if !value.contains('&') {
            return Self::from(value);
        }
        let allocator = oxc_allocator::Allocator::default();
        let mut decoded = None;
        oxc_syntax::xml_entities::decode_entities(value, &mut decoded, value.len(), &allocator);
        Self::from(decoded.as_ref().map_or(value, |buffer| buffer.as_str()))
    }
    pub fn decoded(value: &str, lone_surrogates: bool) -> Self {
        if !lone_surrogates {
            return Self(value.encode_utf16().collect());
        }
        let mut units = Vec::new();
        let mut chars = value.chars();
        while let Some(c) = chars.next() {
            if c == '\u{fffd}' {
                let hex: String = chars.by_ref().take(4).collect();
                // This representation is guaranteed by the pinned Oxc AST contract.
                units.push(u16::from_str_radix(&hex, 16).expect("Oxc surrogate escape"));
            } else {
                let mut buffer = [0; 2];
                units.extend_from_slice(c.encode_utf16(&mut buffer));
            }
        }
        Self(units)
    }

    pub fn contains(&self, query: &[u16]) -> bool {
        self.0.windows(query.len()).any(|window| window == query)
    }

    pub fn equals_str(&self, other: &str) -> bool {
        self.0.iter().copied().eq(other.encode_utf16())
    }
    /// Case-insensitive substring test over the exact UTF-16 units, so a filter never
    /// needs a lossy UTF-8 round trip through the decoded value.
    pub fn contains_ci(&self, needle: &str) -> bool {
        let needle: Vec<u16> = needle.to_lowercase().encode_utf16().collect();
        if needle.is_empty() {
            return true;
        }
        let haystack: Vec<u16> = self
            .0
            .iter()
            .map(|unit| {
                char::from_u32(u32::from(*unit))
                    .map(|c| c.to_lowercase().next().unwrap_or(c))
                    .map_or(*unit, |c| c as u16)
            })
            .collect();
        haystack
            .windows(needle.len())
            .any(|window| window == needle.as_slice())
    }
}

impl From<&str> for JsString {
    fn from(value: &str) -> Self {
        Self::decoded(value, false)
    }
}

impl Serialize for JsString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Ok(text) = String::from_utf16(&self.0) {
            return text.serialize(serializer);
        }
        let mut json = String::from("\"");
        for unit in &self.0 {
            write!(json, "\\u{unit:04x}").expect("writing a String cannot fail");
        }
        json.push('"');
        RawValue::from_string(json)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}
