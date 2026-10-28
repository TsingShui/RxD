use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::{
    inflate_optional, invalid,
    marshal::{self, Value},
    resource, write_new,
};
use rxd_core::{Budget, Result, perf::Perf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Module,
    Package,
    Data,
    Namespace,
}

impl Kind {
    pub fn has_code(self) -> bool {
        matches!(self, Self::Module | Self::Package)
    }
}

pub struct Entry {
    pub name: String,
    pub kind: Kind,
    offset: usize,
    length: usize,
}

struct Index {
    magic: [u8; 4],
    entries: Vec<Entry>,
    names: HashMap<String, usize>,
    work: usize,
    metadata: usize,
}

impl Index {
    fn parse(payload: &[u8], limits: Budget) -> Result<Self> {
        if payload.len() < 12 || !payload.starts_with(b"PYZ\0") {
            return Err(invalid("invalid PyInstaller PYZ header"));
        }
        let magic: [u8; 4] = payload[4..8].try_into().expect("four bytes");
        if magic[2..] != *b"\r\n" {
            return Err(invalid("invalid PYZ Python magic separator"));
        }
        let toc = u32::from_be_bytes(payload[8..12].try_into().expect("four bytes")) as usize;
        if !(12..payload.len()).contains(&toc) {
            return Err(invalid("PYZ TOC position is outside the archive"));
        }
        let decoded = marshal::decode(&payload[toc..], limits)?;
        let length = match &decoded.value {
            Value::Seq(items) => items.len(),
            Value::Map(items) => items.len(),
            _ => return Err(invalid("PYZ TOC must be a list or dictionary")),
        };
        if length > limits.entries
            || length.saturating_mul(std::mem::size_of::<Entry>() + 128)
                > limits.metadata_bytes.saturating_sub(decoded.metadata)
        {
            return Err(resource("PYZ entry admission exceeded"));
        }
        let mut index = Self {
            magic,
            entries: Vec::new(),
            names: HashMap::new(),
            work: decoded.work,
            metadata: 0,
        };
        index
            .entries
            .try_reserve_exact(length)
            .map_err(|_| resource("PYZ index allocation failed"))?;
        index
            .names
            .try_reserve(length)
            .map_err(|_| resource("PYZ name index allocation failed"))?;
        let mut metadata = decoded.metadata;
        let mut add = |name: &Value, info: &Value| -> Result<()> {
            let name_length = match name {
                Value::Text(name) => name.len(),
                Value::Bytes(name) => name.len(),
                _ => return Err(invalid("PYZ entry name must be a string")),
            };
            let bytes = name_length
                .saturating_mul(2)
                .saturating_add(std::mem::size_of::<Entry>() + 128);
            if bytes > limits.metadata_bytes.saturating_sub(metadata) {
                return Err(resource("PYZ index metadata admission exceeded"));
            }
            metadata += bytes;
            let work = name_length.saturating_add(1);
            if limits
                .work
                .is_some_and(|limit| work > limit.saturating_sub(index.work))
            {
                return Err(resource("PYZ index work admission exceeded"));
            }
            index.work = index
                .work
                .checked_add(work)
                .ok_or_else(|| resource("PYZ index work overflow"))?;
            let entry = entry(name, info, toc)?;
            if index
                .names
                .insert(entry.name.clone(), index.entries.len())
                .is_some()
            {
                return Err(invalid(format!(
                    "duplicate PYZ module name: {}",
                    entry.name
                )));
            }
            index.entries.push(entry);
            Ok(())
        };
        match &decoded.value {
            Value::Seq(items) => {
                for item in items {
                    let Value::Seq(pair) = item else {
                        return Err(invalid("PYZ TOC entry must be a two-element tuple"));
                    };
                    if pair.len() != 2 {
                        return Err(invalid("PYZ TOC entry must be a two-element tuple"));
                    }
                    add(&pair[0], &pair[1])?;
                }
            }
            Value::Map(items) => {
                for (name, info) in items {
                    add(name, info)?;
                }
            }
            _ => unreachable!(),
        }
        index.metadata = metadata - decoded.metadata;
        Ok(index)
    }

    fn read(&self, payload: &[u8], entry: usize, cap: Option<usize>) -> Result<Vec<u8>> {
        let entry = self
            .entries
            .get(entry)
            .ok_or_else(|| invalid("invalid PYZ entry index"))?;
        if entry.length == 0 {
            return Ok(Vec::new());
        }
        inflate_optional(
            &payload[entry.offset..entry.offset + entry.length],
            cap,
            None,
        )
    }
}

/// An immutable PYZ snapshot; member reads always use the indexed source buffer.
pub struct Archive {
    payload: Vec<u8>,
    index: Index,
}

impl Archive {
    pub fn parse(payload: Vec<u8>, limits: Budget) -> Result<Self> {
        let index = Index::parse(&payload, limits)?;
        Ok(Self { payload, index })
    }
    pub fn magic(&self) -> [u8; 4] {
        self.index.magic
    }
    pub fn entries(&self) -> &[Entry] {
        &self.index.entries
    }
    pub fn find(&self, name: &str) -> Option<usize> {
        self.index.names.get(name).copied()
    }
    pub fn work(&self) -> usize {
        self.index.work
    }
    pub fn metadata_bytes(&self) -> usize {
        self.index.metadata
    }
    pub fn read(&self, entry: usize, cap: Option<usize>) -> Result<Vec<u8>> {
        self.index.read(&self.payload, entry, cap)
    }
}

fn entry(name: &Value, info: &Value, toc: usize) -> Result<Entry> {
    let name = match name {
        Value::Text(text) => text.clone(),
        Value::Bytes(bytes) => {
            String::from_utf8(bytes.clone()).map_err(|_| invalid("PYZ entry name is not UTF-8"))?
        }
        _ => return Err(invalid("PYZ entry name must be a string")),
    };
    let Value::Seq(fields) = info else {
        return Err(invalid("PYZ entry info must be a tuple"));
    };
    if fields.len() != 3 {
        return Err(invalid("PYZ entry info must hold three integers"));
    }
    let field = |index: usize| match fields[index] {
        Value::Int(value) => {
            usize::try_from(value).map_err(|_| invalid("negative or unaddressable PYZ entry field"))
        }
        _ => Err(invalid("PYZ entry info must hold integers")),
    };
    let kind = match field(0)? {
        0 => Kind::Module,
        1 => Kind::Package,
        2 => Kind::Data,
        3 => Kind::Namespace,
        kind => {
            return Err(crate::unsupported(format!(
                "unsupported PYZ entry kind {kind}"
            )));
        }
    };
    if kind == Kind::Data {
        crate::safe_relative(&name)?;
    } else {
        module_path(&name, kind != Kind::Module)?;
        if name.chars().any(char::is_control) {
            return Err(invalid("PYZ module name contains control characters"));
        }
    }
    let offset = field(1)?;
    let length = field(2)?;
    offset
        .checked_add(length)
        .filter(|end| *end <= toc)
        .ok_or_else(|| {
            invalid(format!(
                "PYZ member range overlaps metadata or exceeds the archive: {name}"
            ))
        })?;
    if length > 0 && offset < 12 || kind.has_code() && length == 0 {
        return Err(invalid(format!("invalid PYZ member range: {name}")));
    }
    Ok(Entry {
        name,
        kind,
        offset,
        length,
    })
}

pub(crate) struct Unpacked {
    pub(crate) files: usize,
    pub(crate) bytes: u64,
    pub(crate) magic: [u8; 4],
}

pub(crate) fn unpack(
    payload: &[u8],
    destination: &Path,
    header_len: usize,
    max_entry: usize,
    max_entries: usize,
    perf: &mut Perf,
) -> Result<Unpacked> {
    let mut limits = Budget::for_input_bytes(payload.len())?;
    limits.entries = limits.entries.min(max_entries);
    let index = perf.measure("parse", || Index::parse(payload, limits))?;
    let mut header_prefix = vec![0; header_len];
    header_prefix[..4].copy_from_slice(&index.magic);
    let mut paths = HashSet::new();
    let mut files = 0;
    let mut bytes = 0u64;
    for (ordinal, module) in index.entries.iter().enumerate() {
        if module.kind == Kind::Namespace {
            continue;
        }
        let relative = if module.kind == Kind::Data {
            crate::safe_relative(&module.name)?
        } else {
            module_path(&module.name, module.kind == Kind::Package)?
        };
        if !paths.insert(relative.clone()) {
            return Err(invalid(format!(
                "duplicate PYZ output path: {}",
                relative.display()
            )));
        }
        let data = perf.measure("read", || index.read(payload, ordinal, Some(max_entry)))?;
        let mut output = rxd_core::output::Output::new(max_entry);
        use std::io::Write as _;
        if module.kind.has_code() {
            output.write_all(&header_prefix)?;
        }
        output.write_all(&data)?;
        perf.measure("write", || write_new(destination, &relative, &output))?;
        files += 1;
        bytes += output.len() as u64;
    }
    Ok(Unpacked {
        files,
        bytes,
        magic: index.magic,
    })
}

fn module_path(name: &str, package: bool) -> Result<PathBuf> {
    if name.is_empty() || name.starts_with('.') || name.contains(['\0', '/', '\\', ':']) {
        return Err(invalid(format!("unsafe PYZ module name: {name}")));
    }
    let parts: Vec<&str> = name.split('.').collect();
    if parts.iter().any(|part| part.is_empty()) {
        return Err(invalid(format!("invalid PYZ module name: {name}")));
    }
    let mut relative = PathBuf::new();
    if package {
        for part in &parts {
            relative.push(part);
        }
        relative.push("__init__.pyc");
    } else {
        for part in &parts[..parts.len() - 1] {
            relative.push(part);
        }
        relative.push(format!("{}.pyc", parts[parts.len() - 1]));
    }
    Ok(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_modules_use_init_paths() {
        assert_eq!(
            module_path("json", true).unwrap(),
            Path::new("json/__init__.pyc")
        );
        assert_eq!(
            module_path("json.decoder", false).unwrap(),
            Path::new("json/decoder.pyc")
        );
        assert_eq!(
            module_path("encodings.utf_8", false).unwrap(),
            Path::new("encodings/utf_8.pyc")
        );
    }

    #[test]
    fn module_names_reject_unsafe_components() {
        for name in [
            "", ".hidden", "a..b", "a.b.", "a/b", "a\\b", "c:mod", "a\0b",
        ] {
            assert!(module_path(name, false).is_err(), "{name}");
        }
    }
}
