//! Output contract for typed tools.
//!
//! A tool returns the backend's own records, unchanged: JSONL streams keep their
//! header/records/summary rows, single JSON results stay single objects, and source
//! text (XML, Java) arrives as `text`. The schema is therefore one shape shared by
//! every analysis tool instead of a hand-maintained per-tool copy that can drift from
//! what the backend prints.
use serde_json::{Value, json};

pub(super) fn output() -> Value {
    json!({
        "type": "object",
        "description": "The backend's own result records. Streams carry header, records and summary; single results are returned unchanged; source text is returned as `text`.",
        "properties": {
            "header": {"type": "object"},
            "records": {"type": "array", "items": {"type": "object"}},
            "summary": {"type": "object"},
            "files": {"type": "array", "items": {"type": "object"}},
            "scan_summary": {"type": "object"},
            "source_id": {"type": "string"},
            "representation": {"type": "string"},
            "complete": {"type": "boolean"},
            "text": {"type": "string"},
        },
        "additionalProperties": true,
    })
}
