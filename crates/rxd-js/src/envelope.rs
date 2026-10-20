//! Keep typed detail serialization: JS strings may contain escaped lone UTF-16 surrogates
//! that serde_json::Value cannot represent without losing the original code units.

use rxd_core::envelope::{RecordMeta, record};
use serde::{Serialize, Serializer, ser::SerializeMap};
use serde_json::{Map, Value};

/// Detail coordinates are the same copy-only form as row coordinates.
///
/// A caller pastes an id whatever key it came from, so an id nested in `detail` must be
/// the short coordinate too; the header keeps the content-bound identity.
pub fn short(id: &str) -> String {
    rxd_core::envelope::at_of(id).into_owned()
}

/// Serialize an owned id field as the copy-only coordinate.
pub fn serialize_short<S: Serializer>(value: &str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&short(value))
}

/// Serialize an optional id field as the copy-only coordinate.
pub fn serialize_short_option<S: Serializer>(
    value: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(id) => serializer.serialize_some(&short(id)),
        None => serializer.serialize_none(),
    }
}

/// Rewrite every content-bound id inside a JSON detail tree to its short coordinate.
pub fn shorten_detail_ids(value: &mut Value) {
    match value {
        Value::String(id) if id.starts_with("rxd1:") => *id = short(id),
        Value::Array(items) => items.iter_mut().for_each(shorten_detail_ids),
        Value::Object(map) => map.values_mut().for_each(shorten_detail_ids),
        _ => {}
    }
}

pub struct Body<T> {
    pub outer: Value,
    pub detail: T,
}

impl<T: Serialize> Serialize for Body<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        for key in [
            "record",
            "at",
            "source_id",
            "parent_at",
            "context_at",
            "start",
            "end",
            "backend",
            "command",
            "schema_version",
            "input_bytes",
            "query",
        ] {
            if let Some(value) = self.outer.get(key) {
                map.serialize_entry(key, value)?;
            }
        }
        map.serialize_entry("detail", &self.detail)?;
        map.end()
    }
}

pub fn body<T>(kind: &str, meta: RecordMeta<'_>, detail: T) -> Body<T> {
    Body {
        outer: record(kind, meta, Map::new()),
        detail,
    }
}

impl<T> Body<T> {
    pub fn single(mut self, command: &str) -> Self {
        self.outer["backend"] = Value::from("js");
        self.outer["command"] = Value::from(command);
        self.outer["schema_version"] = Value::from(rxd_core::envelope::SCHEMA_VERSION);
        self
    }
}

#[derive(Serialize)]
pub struct ExtendedDetail<T> {
    #[serde(flatten)]
    pub base: Map<String, Value>,
    #[serde(flatten)]
    pub extra: T,
}
