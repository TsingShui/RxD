use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use rxd_core::{Result, perf::Perf};

use crate::{
    inflate, invalid,
    marshal::{self, Value},
    resource, write_new,
};

pub(crate) struct Unpacked {
    pub(crate) files: usize,
    pub(crate) bytes: u64,
    pub(crate) magic: [u8; 4],
}

struct Module {
    name: String,
    package: bool,
    offset: u64,
    length: u64,
}

pub(crate) fn unpack(
    payload: &[u8],
    destination: &Path,
    header_len: usize,
    max_entry: usize,
    max_entries: usize,
    perf: &mut Perf,
) -> Result<Unpacked> {
    if payload.len() < 12 || !payload.starts_with(b"PYZ\0") {
        return Err(invalid("invalid PYZ header"));
    }
    let magic: [u8; 4] = payload[4..8].try_into().expect("four bytes");
    let toc_position = u32::from_be_bytes(payload[8..12].try_into().expect("four bytes")) as usize;
    if toc_position > payload.len() {
        return Err(invalid("PYZ TOC position is outside the archive"));
    }
    let toc = perf.measure("parse", || marshal::decode(&payload[toc_position..]))?;
    let modules = toc_modules(&toc)?;
    if modules.len() > max_entries {
        return Err(resource("PYZ module count exceeds the entry limit"));
    }
    let mut header_prefix = vec![0; header_len];
    header_prefix[..4].copy_from_slice(&magic);
    let mut paths = HashSet::new();
    let mut files = 0;
    let mut bytes = 0u64;
    for module in modules {
        let relative = module_path(&module.name, module.package)?;
        if !paths.insert(relative.clone()) {
            return Err(invalid(format!(
                "duplicate PYZ module path: {}",
                relative.display()
            )));
        }
        let start = usize::try_from(module.offset)
            .map_err(|_| resource("PYZ module offset exceeds the addressable byte range"))?;
        let length = usize::try_from(module.length)
            .map_err(|_| resource("PYZ module length exceeds the addressable byte range"))?;
        let end = start
            .checked_add(length)
            .filter(|end| *end <= payload.len())
            .ok_or_else(|| {
                invalid(format!(
                    "PYZ module data is outside the archive: {}",
                    module.name
                ))
            })?;
        let data = if length == 0 {
            Vec::new()
        } else {
            perf.measure("read", || inflate(&payload[start..end], max_entry, None))?
        };
        let mut output = Vec::with_capacity(header_prefix.len() + data.len());
        output.extend_from_slice(&header_prefix);
        output.extend_from_slice(&data);
        perf.measure("write", || write_new(destination, &relative, &output))?;
        files += 1;
        bytes += output.len() as u64;
    }
    Ok(Unpacked {
        files,
        bytes,
        magic,
    })
}

fn toc_modules(toc: &Value) -> Result<Vec<Module>> {
    match toc {
        Value::Seq(items) => items
            .iter()
            .map(|item| match item {
                Value::Seq(pair) if pair.len() == 2 => module(&pair[0], &pair[1]),
                _ => Err(invalid("PYZ TOC entry must be a two-element tuple")),
            })
            .collect(),
        Value::Map(pairs) => pairs
            .iter()
            .map(|(name, info)| module(name, info))
            .collect(),
        _ => Err(invalid("PYZ TOC must be a list or dictionary")),
    }
}

fn module(name: &Value, info: &Value) -> Result<Module> {
    let name = match name {
        Value::Text(text) => text.clone(),
        Value::Bytes(bytes) => {
            String::from_utf8(bytes.clone()).map_err(|_| invalid("PYZ module name is not UTF-8"))?
        }
        _ => return Err(invalid("PYZ module name must be a string")),
    };
    let fields = match info {
        Value::Seq(fields) if fields.len() >= 3 => fields,
        _ => return Err(invalid("PYZ module info must hold at least three integers")),
    };
    let field = |value: &Value| match value {
        Value::Int(number) => {
            u64::try_from(*number).map_err(|_| invalid("PYZ module field is negative"))
        }
        _ => Err(invalid("PYZ module info must hold integers")),
    };
    Ok(Module {
        package: field(&fields[0])? != 0,
        offset: field(&fields[1])?,
        length: field(&fields[2])?,
        name,
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
