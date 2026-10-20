use anyhow::{Result, ensure};

use crate::model::StringRecord;
use crate::module::Module;

/// Minimum run of printable/UTF-8 bytes to treat as an embedded string.
const MIN_RUN: usize = 4;

/// Enumerate decodable UTF-8 runs in data segments plus name-section strings.
/// Valid UTF-8 becomes a JSON string; invalid runs keep raw bytes with value null.
pub fn strings(module: &Module) -> Result<Vec<StringRecord>> {
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
            out.push(classify(run, "data", Some(segment.index), start));
        }
    }

    // Function/global name-section labels are navigation strings too.
    for function in &module.functions {
        if let Some(name) = &function.name {
            out.push(StringRecord {
                origin: "name",
                segment_index: None,
                offset: function.func_index as usize,
                value: Some(name.clone()),
                bytes: None,
            });
        }
    }

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
