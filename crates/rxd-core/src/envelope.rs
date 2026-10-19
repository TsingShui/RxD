//! ADR 0003 envelope constructors. Backends define each record's determinate detail schema.

use serde_json::{Map, Value, json};

/// Version of the ADR 0003 envelope, independent of the legacy output version.
pub const SCHEMA_VERSION: u32 = 2;

/// Navigation and byte offsets; absent values serialize as null, never omitted keys.
///
/// `at` values are content-bound backend ids. They are emitted as the short,
/// copy-only `at` coordinate: the schema prefix `rxd1:<backend>:<sha256>:` is
/// stripped because the stream header already carries `source_id`, and an Agent
/// passes the coordinate back with `--at` instead of assembling a hash itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct RecordMeta<'a> {
    pub at: Option<&'a str>,
    pub source_id: Option<&'a str>,
    pub parent_at: Option<&'a str>,
    pub context_at: Option<&'a str>,
    pub start: Option<usize>,
    pub end: Option<usize>,
}

/// Turn a content-bound id into the copy-only coordinate an Agent passes back.
///
/// The schema prefix `rxd1:<backend>:<sha256>:` is stripped structurally (64 hex
/// digits after the backend segment), so no backend needs to be named. A `.NET`
/// coordinate *is* its ECMA-335 metadata token, which already encodes its table in
/// the high byte, so it is printed as `0x0600000b` with no noun prefix; every other
/// backend keeps its own self-describing coordinate (`func:42`, `1234..5678`).
pub fn at_of(id: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    let Some(rest) = id.strip_prefix("rxd1:") else {
        return dotnet_token(id).map_or_else(|| Cow::Borrowed(id), Cow::Owned);
    };
    let Some((backend, rest)) = rest.split_once(':') else {
        return Cow::Borrowed(id);
    };
    let Some((hash, at)) = rest.split_once(':') else {
        return Cow::Borrowed(id);
    };
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) || at.is_empty() {
        return Cow::Borrowed(id);
    }
    match backend {
        "dotnet" => dotnet_token(at).map_or_else(|| Cow::Borrowed(id), Cow::Owned),
        _ => Cow::Borrowed(at),
    }
}

/// `method:0600000b` / `0x0600000b` / `0600000b` -> `0x0600000b`.
fn dotnet_token(value: &str) -> Option<String> {
    let tail = value.rsplit(':').next().unwrap_or(value);
    let body = tail
        .strip_prefix("0x")
        .or_else(|| tail.strip_prefix("0X"))
        .unwrap_or(tail);
    if body.len() != 8 || !body.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let token = u32::from_str_radix(body, 16).ok()?;
    Some(format!("0x{token:08x}"))
}

/// Compose the content-bound id a backend validates internally from the input's own
/// identity plus the coordinate an Agent copied out of a record.
pub fn full_id(backend: &str, source_id: &str, at: &str) -> String {
    format!("rxd1:{backend}:{source_id}:{at}")
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
        "at": meta.at.map(at_of),
        "source_id": meta.source_id,
        "parent_at": meta.parent_at.map(at_of),
        "context_at": meta.context_at.map(at_of),
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
