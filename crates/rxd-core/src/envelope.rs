//! Shared record envelopes; backends own detail schemas.

use serde_json::{Map, Value, json};

pub const SCHEMA_VERSION: u32 = 2;

/// File-scoped navigation coordinates; absent values serialize as null.
#[derive(Clone, Copy, Debug, Default)]
pub struct RecordMeta<'a> {
    pub at: Option<&'a str>,
    pub source_id: Option<&'a str>,
    pub parent_at: Option<&'a str>,
    pub context_at: Option<&'a str>,
    pub start: Option<usize>,
    pub end: Option<usize>,
}

pub const REFERENCE: &str = "reference";

pub fn reference(meta: RecordMeta<'_>, detail: Map<String, Value>) -> Value {
    record(REFERENCE, meta, detail)
}

pub fn record(record: &str, meta: RecordMeta<'_>, detail: Map<String, Value>) -> Value {
    let mut value = json!({
        "record": record,
        "at": meta.at,
        "source_id": meta.source_id,
        "parent_at": meta.parent_at,
        "context_at": meta.context_at,
        "start": meta.start,
        "end": meta.end,
    });
    value
        .as_object_mut()
        .unwrap()
        .insert("detail".into(), Value::Object(detail));
    value
}

/// Attach backend, command and schema version to a headerless FETCH receipt.
pub fn attach_protocol(value: &mut Value, backend: &str, command: &str) {
    value["backend"] = Value::from(backend);
    value["command"] = Value::from(command);
    value["schema_version"] = Value::from(SCHEMA_VERSION);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_shape_preserves_detail_and_nullable_outer_keys() {
        let detail = json!({"name": "λ", "id": "detail-id", "nested": {"items": [1, null]}});
        let detail_object = detail.as_object().unwrap().clone();
        assert_eq!(SCHEMA_VERSION, 2);
        assert_eq!(
            record("method", RecordMeta::default(), detail_object.clone()),
            json!({
                "record": "method", "at": null, "source_id": null,
                "parent_at": null, "context_at": null, "start": null, "end": null,
                "detail": detail
            })
        );
        assert_eq!(
            record(
                "match",
                RecordMeta {
                    at: Some("id"),
                    source_id: Some("sha"),
                    parent_at: Some("parent"),
                    context_at: Some("context"),
                    start: Some(0),
                    end: Some(42),
                },
                detail_object.clone(),
            ),
            json!({
                "record": "match", "at": "id", "source_id": "sha",
                "parent_at": "parent", "context_at": "context", "start": 0, "end": 42,
                "detail": detail
            })
        );
        let mut receipt = record("file", RecordMeta::default(), serde_json::Map::new());
        attach_protocol(&mut receipt, "html", "extract");
        assert_eq!(
            receipt,
            json!({
                "record": "file", "at": null, "source_id": null, "parent_at": null,
                "context_at": null, "start": null, "end": null, "detail": {},
                "backend": "html", "command": "extract", "schema_version": 2
            })
        );
    }
}
