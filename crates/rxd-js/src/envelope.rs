//! Keep typed detail serialization: JS strings may contain escaped lone UTF-16 surrogates
//! that serde_json::Value cannot represent without losing the original code units.

use rxd_core::envelope::{RecordMeta, record};
use serde::{Serialize, Serializer, ser::SerializeMap};
use serde_json::{Map, Value};

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
