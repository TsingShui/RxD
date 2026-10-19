//! ADR 0003 envelope constructors. Backends define each record's determinate detail schema.

use serde_json::{Map, Value, json};

/// Version of the ADR 0003 envelope, independent of the legacy output version.
pub const SCHEMA_VERSION: u32 = 2;

/// Navigation and byte offsets; absent values serialize as null, never omitted keys.
#[derive(Clone, Copy, Debug, Default)]
pub struct RecordMeta<'a> {
    pub id: Option<&'a str>,
    pub source_id: Option<&'a str>,
    pub parent_id: Option<&'a str>,
    pub context_id: Option<&'a str>,
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
pub fn record(record: &str, meta: RecordMeta<'_>, detail: Map<String, Value>) -> Value {
    let mut value = json!({
        "record": record,
        "id": meta.id,
        "source_id": meta.source_id,
        "parent_id": meta.parent_id,
        "context_id": meta.context_id,
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
                "record": "method", "id": null, "source_id": null,
                "parent_id": null, "context_id": null, "start": null, "end": null,
                "detail": detail
            })
        );
        assert_eq!(
            record(
                "match",
                RecordMeta {
                    id: Some("id"),
                    source_id: Some("sha"),
                    parent_id: Some("parent"),
                    context_id: Some("context"),
                    start: Some(0),
                    end: Some(42),
                },
                detail_object.clone(),
            ),
            json!({
                "record": "match", "id": "id", "source_id": "sha",
                "parent_id": "parent", "context_id": "context", "start": 0, "end": 42,
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
                "record": "file", "id": null, "source_id": null, "parent_id": null,
                "context_id": null, "start": null, "end": null, "detail": {},
                "backend": "html", "command": "extract", "schema_version": 2
            })
        );
        assert_eq!(
            summary(0, false, serde_json::Map::new()),
            json!({"record": "summary", "records": 0, "complete": false, "detail": {}})
        );
    }
}
