//! Enumerate the methods a DEX file defines.
//!
//! A method is only *defined* through `class_data`: `method_ids` also lists methods the
//! code merely references. So this walks `class_defs -> class_data -> encoded_method`
//! and resolves each `method_idx` through the cached id tables.
//!
//! Two costs dominate a full listing and both are avoided here: nothing is materialized
//! (rows are appended to the caller's buffer as they are decoded) and names/prototypes
//! are decoded once per id index instead of once per method - a real APK defines 4.9M
//! methods with only ~10^5 distinct names and prototypes.

use anyhow::{Context, Result};

use super::{Dex, read_uleb};

/// Which methods to list.
#[derive(Clone, Copy, Debug, Default)]
pub struct Selector<'a> {
    /// A Dalvik descriptor (`Lx/y;`) or dotted name (`x.y`).
    pub class: Option<&'a str>,
    /// Case-insensitive substring over the method name.
    pub name: Option<&'a str>,
    /// Append `access_flags` to every record.
    pub with_flags: bool,
}

/// Append one `{"record":"method","detail":{"descriptor":…}}` line per defined method.
///
/// Returns the number of rows appended.
pub(crate) fn render(data: &[u8], selector: &Selector<'_>, out: &mut String) -> Result<usize> {
    let dex = Dex::parse(data)?;
    let mut names: Vec<Option<Box<str>>> = vec![None; dex.header.strings_size];
    let mut protos: Vec<Option<Box<str>>> = vec![None; dex.header.protos_size];
    let mut rows = 0usize;
    for class_index in 0..dex.header.classes_size {
        let class_def = dex.header.classes_off + class_index * 32;
        let descriptor = dex.type_name(dex.u32(class_def)? as usize)?;
        if let Some(wanted) = selector.class
            && !matches_class(&descriptor, wanted)
        {
            continue;
        }
        let class_data_off = dex.u32(class_def + 24)? as usize;
        if class_data_off == 0 {
            continue;
        }
        rows += render_class(
            &dex,
            class_data_off,
            &descriptor,
            selector,
            &mut names,
            &mut protos,
            out,
        )?;
    }
    crate::profile::count(crate::profile::Counter::Classes, 0);
    Ok(rows)
}

fn matches_class(descriptor: &str, wanted: &str) -> bool {
    if wanted.starts_with('L') && wanted.ends_with(';') {
        return descriptor == wanted;
    }
    descriptor
        .strip_prefix('L')
        .and_then(|name| name.strip_suffix(';'))
        .is_some_and(|name| name.replace('/', ".") == wanted)
}

fn render_class(
    dex: &Dex<'_>,
    offset: usize,
    descriptor: &str,
    selector: &Selector<'_>,
    names: &mut [Option<Box<str>>],
    protos: &mut [Option<Box<str>>],
    out: &mut String,
) -> Result<usize> {
    let mut cursor = offset;
    let static_fields = read_uleb(dex.data, &mut cursor)? as usize;
    let instance_fields = read_uleb(dex.data, &mut cursor)? as usize;
    let direct_methods = read_uleb(dex.data, &mut cursor)? as usize;
    let virtual_methods = read_uleb(dex.data, &mut cursor)? as usize;
    // Fields are skipped by length only: their values are not needed here.
    for _ in 0..static_fields + instance_fields {
        skip_uleb(dex.data, &mut cursor)?;
        skip_uleb(dex.data, &mut cursor)?;
    }
    let mut rows = 0usize;
    for count in [direct_methods, virtual_methods] {
        let mut method_idx = 0u32;
        for _ in 0..count {
            method_idx = method_idx
                .checked_add(read_uleb(dex.data, &mut cursor)?)
                .context("method index overflow")?;
            let access_flags = read_uleb(dex.data, &mut cursor)? as u16;
            skip_uleb(dex.data, &mut cursor)?; // code_off: not needed to name a method
            let index = method_idx as usize;
            if index >= dex.header.methods_size {
                anyhow::bail!("method index out of range");
            }
            let row = dex.header.methods_off + index * 8;
            let proto_idx = u16::from_le_bytes([dex.data[row + 2], dex.data[row + 3]]) as usize;
            let name_idx = dex.u32(row + 4)? as usize;
            if names[name_idx].is_none() {
                names[name_idx] = Some(dex.string(name_idx)?.into_boxed_str());
            }
            let name = names[name_idx].as_deref().unwrap_or_default();
            if let Some(pattern) = selector.name
                && !name.to_lowercase().contains(pattern)
            {
                continue;
            }
            if protos[proto_idx].is_none() {
                protos[proto_idx] = Some(dex.proto(proto_idx)?.into_boxed_str());
            }
            let mut descriptor_row = String::with_capacity(descriptor.len() + name.len() + 32);
            descriptor_row.push_str(descriptor);
            descriptor_row.push_str("->");
            descriptor_row.push_str(name);
            descriptor_row.push_str(protos[proto_idx].as_deref().unwrap_or_default());
            out.push_str("{\"record\":\"method\",\"detail\":{\"descriptor\":");
            push_json_string(out, &descriptor_row);
            if selector.with_flags {
                out.push_str(",\"access_flags\":");
                out.push_str(&access_flags.to_string());
            }
            out.push_str("}}\n");
            crate::profile::count(crate::profile::Counter::CodeItems, 1);
            rows += 1;
        }
    }
    Ok(rows)
}

/// Append a JSON string literal; the row keys are fixed, the descriptor is not.
fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            control if (control as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// Advance past one uleb128 without materializing its value.
fn skip_uleb(data: &[u8], cursor: &mut usize) -> Result<()> {
    loop {
        let byte = *data.get(*cursor).context("uleb128 outside DEX")?;
        *cursor += 1;
        if byte & 0x80 == 0 {
            return Ok(());
        }
    }
}
