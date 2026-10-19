use anyhow::{Result, ensure};

use crate::model::StringRecord;
use crate::module::Module;

/// Minimum run of printable/UTF-8 bytes to treat as an embedded string.
const MIN_RUN: usize = 4;

/// Enumerate decodable UTF-8 runs in data segments plus name-section strings.
/// Valid UTF-8 becomes a JSON string; invalid runs keep raw bytes with value null.
pub fn strings(module: &Module, contains: Option<&str>) -> Result<Vec<StringRecord>> {
    let limits = module.limits();
    let mut out = Vec::new();
    let mut work = 0usize;

    for segment in &module.data_segments {
        let bytes = module.data_segment_bytes(segment.index as usize);
        work = work.saturating_add(bytes.len());
        ensure!(work <= limits.work, "RESOURCE_LIMIT: strings work");
        let mut i = 0usize;
        while i < bytes.len() {
            // Collect a maximal run of non-NUL bytes.
            if bytes[i] == 0 {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && bytes[i] != 0 {
                i += 1;
            }
            let run = &bytes[start..i];
            if run.len() < MIN_RUN {
                continue;
            }
            let record = classify(run, "data", Some(segment.index), start);
            push_if_match(&mut out, record, contains);
        }
    }

    // Function/global name-section labels are navigation strings too.
    for function in &module.functions {
        if let Some(name) = &function.name {
            let record = StringRecord {
                origin: "name",
                segment_index: None,
                offset: function.func_index as usize,
                value: Some(name.clone()),
                bytes: None,
            };
            push_if_match(&mut out, record, contains);
        }
    }

    ensure!(out.len() <= limits.records, "RESOURCE_LIMIT: records");
    Ok(out)
}

fn classify(run: &[u8], origin: &'static str, segment: Option<u32>, offset: usize) -> StringRecord {
    match std::str::from_utf8(run) {
        Ok(text) => StringRecord {
            origin,
            segment_index: segment,
            offset,
            value: Some(text.to_string()),
            bytes: None,
        },
        Err(_) => StringRecord {
            origin,
            segment_index: segment,
            offset,
            value: None,
            bytes: Some(run.to_vec()),
        },
    }
}

fn push_if_match(out: &mut Vec<StringRecord>, record: StringRecord, contains: Option<&str>) {
    if let Some(needle) = contains {
        let needle = needle.as_bytes();
        let haystack: Vec<u8> = match (&record.value, &record.bytes) {
            (Some(v), _) => v.as_bytes().to_vec(),
            (None, Some(b)) => b.clone(),
            _ => Vec::new(),
        };
        if needle.is_empty() || !haystack.windows(needle.len()).any(|w| w == needle) {
            return;
        }
    }
    out.push(record);
}
