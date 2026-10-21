use std::fmt::Write;

use serde::{Serialize, Serializer};
use serde_json::value::RawValue;

/// Preserve JS UTF-16 code units, including Oxc's escaped lone surrogates.
#[derive(Clone, Debug, PartialEq, Eq)]
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

    pub(crate) fn from_units(units: Vec<u16>) -> Self {
        Self(units)
    }

    pub(crate) fn units(&self) -> &[u16] {
        &self.0
    }

    pub(crate) fn to_utf8(&self) -> Option<String> {
        String::from_utf16(&self.0).ok()
    }

    pub(crate) fn len_utf16(&self) -> usize {
        self.0.len()
    }

    /// Append without a lossy UTF-8 conversion or an allocation beyond the caller's bound.
    pub(crate) fn append_bounded(&mut self, other: &Self, max_units: usize) -> Option<()> {
        let length = self.0.len().checked_add(other.0.len())?;
        if length > max_units {
            return None;
        }
        self.0.extend_from_slice(&other.0);
        Some(())
    }

    pub fn equals_str(&self, other: &str) -> bool {
        self.0.iter().copied().eq(other.encode_utf16())
    }
    /// Substring test over the exact UTF-16 units, so a filter never needs a lossy UTF-8
    /// round trip through the decoded value. Folded unless `case_sensitive`.
    pub fn contains_filter(&self, needle: &str, case_sensitive: bool) -> bool {
        if let Some(text) = self.to_utf8() {
            return rxd_core::filter::Contains::new(needle, case_sensitive).matches(&text);
        }
        let needle: Vec<u16> = if case_sensitive {
            needle.encode_utf16().collect()
        } else {
            needle.to_lowercase().encode_utf16().collect()
        };
        self.contains_prepared(needle.as_slice(), case_sensitive)
    }

    /// The same filter over a needle that is already UTF-16, so a per-literal check never
    /// re-encodes the pattern.
    pub fn contains_prepared(&self, needle: &[u16], case_sensitive: bool) -> bool {
        if needle.is_empty() {
            return true;
        }
        if case_sensitive {
            return self.0.windows(needle.len()).any(|window| window == needle);
        }
        // Use the same full-string lowercase rule as rxd_core::filter. UTF-16
        // surrogates are separators, never U+FFFD, and valid pairs are one scalar.
        let mut haystack = Vec::with_capacity(self.0.len());
        let mut segment = String::new();
        for scalar in char::decode_utf16(self.0.iter().copied()) {
            match scalar {
                Ok(c) => segment.push(c),
                Err(lone) => {
                    haystack.extend(segment.to_lowercase().encode_utf16());
                    segment.clear();
                    haystack.push(lone.unpaired_surrogate());
                }
            }
        }
        haystack.extend(segment.to_lowercase().encode_utf16());
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
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
