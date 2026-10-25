//! Enumerate definitions through class_defs/class_data, not referenced method_ids.
//! Resolve cached names/prototypes while appending rows.

use anyhow::{Context, Result};
use rxd_core::filter::Contains;
use std::borrow::Cow;
use std::collections::HashMap;

use super::{Dex, mutf8, read_uleb};

const METHOD_ID_SIZE: usize = 8;
const SPARSE_NAME_LIMIT: usize = 8;

struct NameCache<'a> {
    borrowed: Vec<Option<&'a str>>,
    decoded: HashMap<usize, Box<str>>,
}

impl<'a> NameCache<'a> {
    fn new(count: usize) -> Self {
        Self {
            borrowed: vec![None; count],
            decoded: HashMap::new(),
        }
    }

    fn get(&mut self, dex: &Dex<'a>, index: usize) -> Result<&str> {
        let slot = self
            .borrowed
            .get_mut(index)
            .context("string index out of range")?;
        if let Some(name) = *slot {
            return Ok(name);
        }
        let bytes = dex.string_bytes(index)?;
        if let Ok(name) = std::str::from_utf8(bytes) {
            *slot = Some(name);
            return Ok(name);
        }
        let decoded = self
            .decoded
            .entry(index)
            .or_insert_with(|| mutf8::decode_owned(bytes).into_boxed_str());
        Ok(decoded)
    }
}

fn decode_name(bytes: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(name) => Cow::Borrowed(name),
        Err(_) => Cow::Owned(mutf8::decode_owned(bytes)),
    }
}

fn matches_name(name: &str, filter: &Contains) -> bool {
    let Some(pattern) = filter.ascii_needle().filter(|_| name.is_ascii()) else {
        return filter.matches(name);
    };
    let Some(&first) = pattern.as_bytes().first() else {
        return true;
    };
    memchr::memchr2_iter(first, first.to_ascii_uppercase(), name.as_bytes()).any(|start| {
        name.as_bytes()
            .get(start..start + pattern.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(pattern.as_bytes()))
    })
}

fn matching_methods<'a>(
    dex: &Dex<'a>,
    names: &mut NameCache<'a>,
    filter: &Contains,
) -> Result<Vec<bool>> {
    let table = &dex.data
        [dex.header.methods_off..dex.header.methods_off + dex.header.methods_size * METHOD_ID_SIZE];
    let mut name_matches = vec![None; dex.header.strings_size];
    let mut matching_names = Vec::new();
    for row in table.as_chunks::<METHOD_ID_SIZE>().0 {
        let index = u32::from_le_bytes(row[4..8].try_into().unwrap()) as usize;
        let state = name_matches
            .get_mut(index)
            .context("string index out of range")?;
        if state.is_some() {
            continue;
        }
        let matched = matches_name(names.get(dex, index)?, filter);
        *state = Some(matched);
        if matched {
            matching_names.push(index as u32);
        }
    }
    let mut methods = vec![false; dex.header.methods_size];
    if matching_names.len() <= SPARSE_NAME_LIMIT {
        for index in matching_names {
            let needle = index.to_le_bytes();
            let finder = memchr::memmem::Finder::new(&needle);
            let mut offset = 0;
            while let Some(relative) = finder.find(&table[offset..]) {
                let hit = offset + relative;
                if hit % METHOD_ID_SIZE == 4 {
                    methods[hit / METHOD_ID_SIZE] = true;
                    offset = hit + 4;
                } else {
                    // An unaligned hit can overlap the next valid name_idx.
                    offset = hit + 1;
                }
            }
        }
    } else {
        for (index, row) in table.as_chunks::<METHOD_ID_SIZE>().0.iter().enumerate() {
            let name = u32::from_le_bytes(row[4..8].try_into().unwrap()) as usize;
            methods[index] = name_matches[name] == Some(true);
        }
    }
    Ok(methods)
}

struct MethodRenderer<'a> {
    dex: Dex<'a>,
    names: NameCache<'a>,
    prototypes: Vec<Option<Box<str>>>,
    filter: Option<Contains>,
    exact_name: Option<String>,
    selected: Option<Vec<bool>>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Selector<'a> {
    /// A Dalvik descriptor (`Lx/y;`) or dotted name (`x.y`).
    pub class: Option<&'a str>,
    /// Substring over the member name, folded unless `case_sensitive`.
    pub name: Option<&'a str>,
    /// Exact member name, distinct from substring matching.
    pub exact_name: Option<&'a str>,
    /// False folds case; true matches exact case.
    pub case_sensitive: bool,
}

/// Append complete method records in declaration order; return their count.
pub(crate) fn render_methods(
    data: &[u8],
    selector: &Selector<'_>,
    out: &mut String,
) -> Result<usize> {
    let mut renderer = MethodRenderer::new(data, selector)?;
    let mut rows = 0usize;
    for class_index in 0..renderer.dex.header.classes_size {
        let class_def = renderer.dex.header.classes_off + class_index * 32;
        let type_index = renderer.dex.u32(class_def)? as usize;
        let descriptor = decode_name(
            renderer
                .dex
                .string_bytes(renderer.dex.type_string_idx(type_index)?)?,
        );
        if let Some(wanted) = selector.class
            && !matches_class(&descriptor, wanted)
        {
            continue;
        }
        let class_data_off = renderer.dex.u32(class_def + 24)? as usize;
        if class_data_off != 0 {
            rows += renderer.render_class(class_data_off, &descriptor, out)?;
        }
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

impl<'a> MethodRenderer<'a> {
    fn new(data: &'a [u8], selector: &Selector<'_>) -> Result<Self> {
        let dex = Dex::parse(data)?;
        let mut names = NameCache::new(dex.header.strings_size);
        let filter = selector
            .name
            .filter(|name| !name.is_empty())
            .map(|name| Contains::new(name, selector.case_sensitive));
        let selected = if selector.class.is_none() {
            filter
                .as_ref()
                .map(|filter| matching_methods(&dex, &mut names, filter))
                .transpose()?
        } else {
            None
        };
        let prototypes = vec![None; dex.header.protos_size];
        Ok(Self {
            dex,
            names,
            prototypes,
            filter,
            exact_name: selector.exact_name.map(str::to_owned),
            selected,
        })
    }

    fn render_class(&mut self, offset: usize, descriptor: &str, out: &mut String) -> Result<usize> {
        let mut cursor = offset;
        let static_fields = read_uleb(self.dex.data, &mut cursor)? as usize;
        let instance_fields = read_uleb(self.dex.data, &mut cursor)? as usize;
        let direct_methods = read_uleb(self.dex.data, &mut cursor)? as usize;
        let virtual_methods = read_uleb(self.dex.data, &mut cursor)? as usize;
        for count in [static_fields, instance_fields] {
            for _ in 0..count {
                skip_uleb(self.dex.data, &mut cursor)?;
                skip_uleb(self.dex.data, &mut cursor)?;
            }
        }
        let mut rows = 0usize;
        for count in [direct_methods, virtual_methods] {
            let mut method_idx = 0u32;
            for _ in 0..count {
                method_idx = method_idx
                    .checked_add(read_uleb(self.dex.data, &mut cursor)?)
                    .context("method index overflow")?;
                skip_uleb(self.dex.data, &mut cursor)?;
                skip_uleb(self.dex.data, &mut cursor)?;
                let index = method_idx as usize;
                if index >= self.dex.header.methods_size {
                    anyhow::bail!("method index out of range");
                }
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|selected| !selected[index])
                {
                    continue;
                }
                let row = self.dex.header.methods_off + index * METHOD_ID_SIZE;
                let proto_idx =
                    u16::from_le_bytes([self.dex.data[row + 2], self.dex.data[row + 3]]) as usize;
                let name_idx = self.dex.u32(row + 4)? as usize;
                let name = self.names.get(&self.dex, name_idx)?;
                if self
                    .exact_name
                    .as_deref()
                    .is_some_and(|wanted| name != wanted)
                {
                    continue;
                }
                if self.selected.is_none()
                    && self
                        .filter
                        .as_ref()
                        .is_some_and(|filter| !matches_name(name, filter))
                {
                    continue;
                }
                let prototype = self
                    .prototypes
                    .get_mut(proto_idx)
                    .context("proto index out of range")?;
                if prototype.is_none() {
                    *prototype = Some(self.dex.proto(proto_idx)?.into_boxed_str());
                }
                let prototype = prototype.as_deref().unwrap();
                out.reserve(descriptor.len() + name.len() + prototype.len() + 40);
                out.push_str("{\"record\":\"method\",\"at\":");
                push_json_string(out, [descriptor, "->", name, prototype]);
                out.push_str(",\"detail\":{}}\n");
                crate::profile::count(crate::profile::Counter::CodeItems, 1);
                rows += 1;
            }
        }
        Ok(rows)
    }
}

fn push_json_string<'a>(out: &mut String, parts: impl IntoIterator<Item = &'a str>) {
    out.push('"');
    for value in parts {
        if value
            .bytes()
            .all(|byte| byte >= 0x20 && byte != b'"' && byte != b'\\')
        {
            out.push_str(value);
            continue;
        }
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
    }
    out.push('"');
}

fn skip_uleb(data: &[u8], cursor: &mut usize) -> Result<()> {
    loop {
        let byte = *data.get(*cursor).context("uleb128 outside DEX")?;
        *cursor += 1;
        if byte & 0x80 == 0 {
            return Ok(());
        }
    }
}

/// Append declared static/instance field records; return the row count.
pub(crate) fn render_fields(
    data: &[u8],
    selector: &Selector<'_>,
    out: &mut String,
) -> Result<usize> {
    let dex = Dex::parse(data)?;
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
        rows += render_fields_of(&dex, class_data_off, &descriptor, selector, out)?;
    }
    Ok(rows)
}

fn render_fields_of(
    dex: &Dex<'_>,
    offset: usize,
    descriptor: &str,
    selector: &Selector<'_>,
    out: &mut String,
) -> Result<usize> {
    let mut cursor = offset;
    let static_fields = read_uleb(dex.data, &mut cursor)? as usize;
    let instance_fields = read_uleb(dex.data, &mut cursor)? as usize;
    // The two method counts follow; the field sections come before the method ones.
    read_uleb(dex.data, &mut cursor)?;
    read_uleb(dex.data, &mut cursor)?;
    let mut rows = 0usize;
    for count in [static_fields, instance_fields] {
        let mut field_idx = 0u32;
        for _ in 0..count {
            field_idx = field_idx
                .checked_add(read_uleb(dex.data, &mut cursor)?)
                .context("field index overflow")?;
            skip_uleb(dex.data, &mut cursor)?; // access_flags: rendered by get-class
            let index = field_idx as usize;
            if index >= dex.header.fields_size {
                anyhow::bail!("field index out of range");
            }
            let row = dex.header.fields_off + index * 8;
            let type_idx = u16::from_le_bytes([dex.data[row + 2], dex.data[row + 3]]) as usize;
            let name_idx = dex.u32(row + 4)? as usize;
            let name = dex.string(name_idx)?;
            if selector.exact_name.is_some_and(|wanted| name != wanted) {
                continue;
            }
            if let Some(pattern) = selector.name
                && !rxd_core::filter::Contains::new(pattern, selector.case_sensitive).matches(&name)
            {
                continue;
            }
            let ty = dex.type_name(type_idx)?;
            let mut rendered = String::with_capacity(descriptor.len() + name.len() + ty.len() + 4);
            rendered.push_str(descriptor);
            rendered.push_str("->");
            rendered.push_str(&name);
            rendered.push(':');
            rendered.push_str(&ty);
            out.push_str("{\"record\":\"field\",\"at\":");
            push_json_string(out, [rendered.as_str()]);
            // Flags stay out of data rows: `get-class`/`get-method` render the Java
            // modifiers, so a per-row integer would be a second spelling of the same fact.
            out.push_str(",\"detail\":{}}\n");
            rows += 1;
        }
    }
    Ok(rows)
}
