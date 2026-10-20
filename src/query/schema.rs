//! Output contract for typed tools.
//!
//! A tool returns the backend's own records, unchanged: JSONL streams keep their
//! header/records/summary rows, single JSON results stay single objects, and source
//! text (XML, Java) arrives as `text`. The schema is therefore one shape shared by
//! every analysis tool instead of a hand-maintained per-tool copy that can drift from
//! what the backend prints.
use serde_json::{Value, json};

pub(super) fn output() -> Value {
    // The outer navigation fields are the same on every backend: `at` names the record,
    // `parent_at` its structural owner and `context_at` the caller that reported it, so a
    // caller never needs a backend-specific identity key. A row that has no value for one
    // prints `null` or omits the key (ADR 0003), and none of them is required here, so both
    // spellings validate.
    let nullable_string = json!({"anyOf":[{"type":"string"},{"type":"null"}]});
    json!({
        "type": "object",
        "description": "The backend's own result records. Streams carry header, records and summary; single results are returned unchanged; source text is returned as `text`.",
        "properties": {
            "header": {"type": "object"},
            "records": {"type": "array", "items": {"type": "object"}},
            "summary": {"type": "object"},
            "files": {"type": "array", "items": {"type": "object"}},
            "scan_summary": {"type": "object"},
            "at": nullable_string,
            "parent_at": nullable_string,
            "context_at": nullable_string,
            "source_id": nullable_string,
            "representation": nullable_string,
            "complete": {"type": "boolean"},
            "text": {"type": "string"},
        },
        "additionalProperties": true,
    })
}
