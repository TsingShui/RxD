//! JSONL becomes records; JSON stays an object; source output becomes text.
use serde_json::{Value, json};

pub(super) fn output() -> Value {
    // Navigation fields permit both omitted values and null.
    let nullable_string = json!({"anyOf":[{"type":"string"},{"type":"null"}]});
    json!({
        "type": "object",
        "description": "The backend's own result records. JSONL streams are returned as `records`; single results are returned unchanged; source text is returned as `text`.",
        "properties": {
            "records": {"type": "array", "items": {"type": "object"}},
            "at": nullable_string,
            "parent_at": nullable_string,
            "context_at": nullable_string,
            "source_id": nullable_string,
            "representation": nullable_string,
            "text": {"type": "string"},
        },
        "additionalProperties": true,
    })
}
