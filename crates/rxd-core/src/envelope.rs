//! Shared output envelope constructors. Backends define each record's determinate detail schema.

use serde_json::{Map, Value, json};

/// Version of the shared envelope, independent of the legacy output version.
pub const SCHEMA_VERSION: u32 = 2;

/// Navigation and byte offsets; absent values serialize as null, never omitted keys.
///
/// `at` values are the backend's own coordinates, emitted verbatim: the stream header
/// carries `source_id`, and an Agent passes a printed coordinate back with `--at`.
#[derive(Clone, Copy, Debug, Default)]
pub struct RecordMeta<'a> {
    pub at: Option<&'a str>,
    pub source_id: Option<&'a str>,
    pub parent_at: Option<&'a str>,
    pub context_at: Option<&'a str>,
    pub start: Option<usize>,
    pub end: Option<usize>,
}

/// Build a stream header. Use null for query parameters that do not apply.
pub fn header(
    backend: &str,
    command: &str,
    source_id: Option<&str>,
    input_bytes: Option<usize>,
    query: Value,
    detail: Map<String, Value>,
) -> Value {
    let mut value = json!({
        "record": "header",
        "backend": backend,
        "command": command,
        "source_id": source_id,
        "input_bytes": input_bytes,
        "schema_version": SCHEMA_VERSION,
        "query": query,
    });
    value
        .as_object_mut()
        .unwrap()
        .insert("detail".into(), Value::Object(detail));
    value
}

/// Build a stream data record or a single-object result, preserving detail verbatim.
/// The row kind every backend's `xref` emits.
///
/// One name for one shape: the cross-reference skeleton (`context_at` + `detail.target` +
/// the backend's evidence) is shared, so its spelling is not repeated per backend. Backend
/// specific kinds (`class`, `table`, `entity`, …) stay with their backend.
pub const REFERENCE: &str = "reference";

/// A cross-reference row.
pub fn reference(meta: RecordMeta<'_>, detail: Map<String, Value>) -> Value {
    record(REFERENCE, meta, detail)
}

/// True for the stream wrapper rows a JSONL stream must not carry.
pub fn is_wrapper(value: &Value) -> bool {
    matches!(
        value.get("record").and_then(Value::as_str),
        Some("header" | "summary" | "scan_summary")
    )
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

/// Attach protocol identity to a single-object result that carries no stream header.
///
/// A single-record FETCH receipt has no header line, so `backend`, `command` and
/// `schema_version` live at the top level next to the record fields.
pub fn attach_protocol(value: &mut Value, backend: &str, command: &str) {
    value["backend"] = Value::from(backend);
    value["command"] = Value::from(command);
    value["schema_version"] = Value::from(SCHEMA_VERSION);
}

/// Build a stream summary; records counts data records, excluding header and summary.
pub fn summary(records: usize, complete: bool, detail: Map<String, Value>) -> Value {
    let mut value = json!({
        "record": "summary",
        "records": records,
        "complete": complete,
    });
    value
        .as_object_mut()
        .unwrap()
        .insert("detail".into(), Value::Object(detail));
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_shape_preserves_detail_and_nullable_outer_keys() {
        let detail = json!({"name": "λ", "id": "detail-id", "nested": {"items": [1, null]}});
        let detail_object = detail.as_object().unwrap().clone();
        let query = json!({"limit": 10});

        assert_eq!(SCHEMA_VERSION, 2);
        assert_eq!(
            header(
                "jvm",
                "outline",
                Some("sha"),
                Some(42),
                query.clone(),
                detail_object.clone()
            ),
            json!({
                "record": "header", "backend": "jvm", "command": "outline",
                "source_id": "sha", "input_bytes": 42, "schema_version": 2,
                "query": query, "detail": detail
            })
        );
        assert_eq!(
            header(
                "js",
                "outline",
                None,
                None,
                json!(null),
                detail_object.clone()
            ),
            json!({
                "record": "header", "backend": "js", "command": "outline",
                "source_id": null, "input_bytes": null, "schema_version": 2,
                "query": null, "detail": detail
            })
        );
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
        assert_eq!(
            summary(2, true, detail_object),
            json!({"record": "summary", "records": 2, "complete": true, "detail": detail})
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
        assert_eq!(
            summary(0, false, serde_json::Map::new()),
            json!({"record": "summary", "records": 0, "complete": false, "detail": {}})
        );
    }
}
