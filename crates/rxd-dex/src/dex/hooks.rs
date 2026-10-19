//! Fixed-capture declared-method selection for offline hook generation.
//! Unlike legacy archive discovery, duplicate physical entries are never discarded.
use super::{Dex, container, members};
use crate::archive::{self, ZipEntry};
use anyhow::{Context, Result, bail};
use std::{
    fmt,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub enum Selector<'a> {
    Signature {
        class: &'a str,
        name: &'a str,
        prototype: &'a str,
    },
    MethodIndex {
        class: &'a str,
        index: u32,
    },
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub expanded_bytes: usize,
    pub target_bytes: usize,
    pub analysis_steps: u64,
    pub timeout: Duration,
}
#[derive(Debug, Clone)]
pub enum Failure {
    InvalidInput(String),
    ResourceLimit(String),
    Timeout,
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) | Self::ResourceLimit(message) => f.write_str(message),
            Self::Timeout => f.write_str("DEX analysis time budget exceeded"),
        }
    }
}
impl std::error::Error for Failure {}

#[derive(Debug)]
pub struct DeclaredMethod {
    pub entry: String,
    pub class: String,
    pub name: String,
    pub prototype: String,
    pub method_index: u32,
    pub access_flags: u32,
    /// Other original declared names with this exact prototype. Runtime adapters
    /// use these facts to detect name-alias collisions without parsing display text.
    pub same_prototype_names: Vec<String>,
}
#[derive(Debug, Default)]
pub struct Lookup {
    /// One entry per class_def, including duplicate physical/logical definitions.
    pub definitions: Vec<String>,
    pub methods: Vec<DeclaredMethod>,
}
struct Budget {
    limits: Limits,
    started: Instant,
    expanded: usize,
    steps: u64,
}
impl Budget {
    fn charge(&mut self, steps: u64, bytes: usize) -> Result<()> {
        if self.started.elapsed() >= self.limits.timeout {
            return Err(Failure::Timeout.into());
        }
        self.steps = self
            .steps
            .checked_add(steps)
            .filter(|&n| n <= self.limits.analysis_steps)
            .ok_or_else(|| Failure::ResourceLimit("DEX analysis step budget exceeded".into()))?;
        self.expanded = self
            .expanded
            .checked_add(bytes)
            .filter(|&n| n <= self.limits.expanded_bytes)
            .ok_or_else(|| Failure::ResourceLimit("DEX expanded byte budget exceeded".into()))?;
        Ok(())
    }
    fn target(&self, value: &str) -> Result<()> {
        if value.len() > self.limits.target_bytes {
            return Err(Failure::ResourceLimit("DEX target byte budget exceeded".into()).into());
        }
        Ok(())
    }
}

/// Analyze only `bytes`, never a path. `entry` is an exact physical/logical name;
/// a repeated ZIP name remains ambiguous even when that name is explicitly selected.
pub fn resolve(
    bytes: &[u8],
    selector: Selector<'_>,
    entry: Option<&str>,
    limits: Limits,
) -> std::result::Result<Lookup, Failure> {
    let mut budget = Budget {
        limits,
        started: Instant::now(),
        expanded: 0,
        steps: 0,
    };
    resolve_inner(bytes, selector, entry, &mut budget).map_err(|error| {
        if let Some(failure) = error.downcast_ref::<Failure>() {
            failure.clone()
        } else if error.is::<archive::DirectoryLimit>() {
            Failure::ResourceLimit(error.to_string())
        } else {
            Failure::InvalidInput(format!("{error:#}"))
        }
    })
}
fn resolve_inner(
    bytes: &[u8],
    selector: Selector<'_>,
    selected_entry: Option<&str>,
    budget: &mut Budget,
) -> Result<Lookup> {
    let class = match selector {
        Selector::Signature { class, .. } | Selector::MethodIndex { class, .. } => class,
    };
    budget.target(class)?;
    budget.charge(1, 0)?;
    let mut entries = if bytes.starts_with(b"dex\n") {
        vec![ZipEntry::bare("classes.dex", bytes.len())]
    } else {
        // Directory metadata is bounded before allocating names/records.
        let mut visited = 0;
        let entries = archive::parse_zip_entries_bounded(
            bytes,
            |name| {
                visited += 1;
                !name.contains(&b'/') && name.ends_with(b".dex")
            },
            usize::try_from(budget.limits.analysis_steps).unwrap_or(usize::MAX),
            budget.limits.expanded_bytes,
        )?;
        budget.charge(visited, 0)?;
        entries
    };
    if entries.iter().any(|e| e.name.starts_with("classes")) {
        entries.retain(|e| e.name.starts_with("classes"));
    }
    if entries.is_empty() {
        bail!("input has no root DEX entries");
    }
    let mut lookup = Lookup::default();
    for physical in entries {
        budget.target(&physical.name)?;
        budget.charge(1, physical.uncompressed_size)?;
        let data = if physical.compression == 0 {
            archive::stored_entry(bytes, &physical)?
        } else {
            std::borrow::Cow::Owned(archive::inflate_entry(
                bytes,
                &physical,
                budget.limits.expanded_bytes - (budget.expanded - physical.uncompressed_size),
            )?)
        };
        validate_header(&data)?;
        // Check metadata/copy work before logical_dexes can allocate a container view.
        let offsets = container::logical_offsets(&data)?;
        let copies = if offsets.len() > 1 {
            offsets.len().checked_mul(data.len()).ok_or_else(|| {
                Failure::ResourceLimit("DEX logical-view byte count overflow".into())
            })?
        } else {
            0
        };
        budget.charge(offsets.len() as u64, copies)?;
        for logical in container::logical_dexes(&physical.name, &data)? {
            budget.charge(1, 0)?;
            budget.target(&logical.name)?;
            if selected_entry.is_some_and(|entry| entry != logical.name) {
                continue;
            }
            // The borrowed scanner understands container-relative offsets directly;
            // only the decompiler needs standard_header_view/checksum normalization.
            validate_header(&logical.data)?;
            let dex = Dex::parse(&logical.data)?;
            budget.charge(dex.header.classes_size as u64, 0)?;
            for class_index in 0..dex.header.classes_size {
                budget.charge(1, 0)?;
                let type_index = dex.u32(dex.header.classes_off + class_index * 32)? as usize;
                let descriptor = bounded_type(&dex, type_index, budget)?;
                budget.target(&descriptor)?;
                if descriptor != class {
                    continue;
                }
                budget.charge((dex.header.methods_size + dex.header.fields_size) as u64, 0)?;
                budget.charge(0, logical.name.len())?;
                lookup.definitions.push(logical.name.clone());
                // Preflight names before the shared member reader allocates decoded
                // rows. Repeated string offsets cannot amplify allocation unchecked.
                for (offset, count, fields) in [
                    (dex.header.methods_off, dex.header.methods_size, false),
                    (dex.header.fields_off, dex.header.fields_size, true),
                ] {
                    for index in 0..count {
                        budget.charge(1, 128)?;
                        bounded_string(&dex, dex.u32(offset + index * 8 + 4)? as usize, budget)?;
                        if fields {
                            bounded_type(
                                &dex,
                                crate::bytes::read_u16(dex.data, offset + index * 8 + 2)? as usize,
                                budget,
                            )?;
                        }
                    }
                }
                let declared = members::class_members_at(&dex, class_index, type_index, class)?;
                budget.charge(0, 0)?;
                for method in declared
                    .direct_methods
                    .iter()
                    .chain(&declared.virtual_methods)
                {
                    budget.charge(1, 0)?;
                    budget.target(&method.name)?;
                    match selector {
                        Selector::MethodIndex { index, .. } if method.method_index != index => {
                            continue;
                        }
                        Selector::Signature { name, .. } if method.name != name => continue,
                        _ => {}
                    }
                    let prototype = bounded_prototype(&dex, method.proto_idx as usize, budget)?;
                    if let Selector::Signature {
                        prototype: expected,
                        ..
                    } = selector
                        && prototype != expected
                    {
                        continue;
                    }
                    let mut same_prototype_names = Vec::new();
                    for sibling in declared
                        .direct_methods
                        .iter()
                        .chain(&declared.virtual_methods)
                    {
                        if sibling.method_index == method.method_index {
                            continue;
                        }
                        budget.charge(1, 0)?;
                        if bounded_prototype(&dex, sibling.proto_idx as usize, budget)? == prototype
                        {
                            budget.charge(0, sibling.name.len())?;
                            same_prototype_names.push(sibling.name.clone());
                        }
                    }
                    lookup.methods.push(DeclaredMethod {
                        entry: logical.name.clone(),
                        class: class.into(),
                        name: method.name.clone(),
                        prototype,
                        method_index: method.method_index,
                        access_flags: method.access_flags,
                        same_prototype_names,
                    });
                }
            }
        }
    }
    Ok(lookup)
}
fn bounded_prototype(dex: &Dex<'_>, index: usize, budget: &mut Budget) -> Result<String> {
    if index >= dex.header.protos_size {
        bail!("proto index out of range");
    }
    let row = dex.header.protos_off + index * 12;
    let parameters = dex.u32(row + 8)? as usize;
    let mut out = String::from("(");
    if parameters != 0 {
        let size = dex.u32(parameters)? as usize;
        super::check_table(dex.data, parameters + 4, size, 2, "type_list")?;
        budget.charge(size as u64, 0)?;
        for n in 0..size {
            let descriptor = bounded_type(
                dex,
                crate::bytes::read_u16(dex.data, parameters + 4 + n * 2)? as usize,
                budget,
            )?;
            budget.target(&descriptor)?;
            if out
                .len()
                .checked_add(descriptor.len())
                .context("prototype length overflow")?
                > budget.limits.target_bytes
            {
                return Err(
                    Failure::ResourceLimit("DEX prototype byte budget exceeded".into()).into(),
                );
            }
            out.push_str(&descriptor);
        }
    }
    out.push(')');
    let result = bounded_type(dex, dex.u32(row + 4)? as usize, budget)?;
    budget.target(&result)?;
    if out
        .len()
        .checked_add(result.len())
        .context("prototype length overflow")?
        > budget.limits.target_bytes
    {
        return Err(Failure::ResourceLimit("DEX prototype byte budget exceeded".into()).into());
    }
    out.push_str(&result);
    budget.charge(0, out.len())?;
    Ok(out)
}
fn bounded_string(dex: &Dex<'_>, index: usize, budget: &mut Budget) -> Result<String> {
    let raw = dex.string_bytes(index)?;
    if raw.len() > budget.limits.target_bytes {
        return Err(Failure::ResourceLimit("DEX name/type byte budget exceeded".into()).into());
    }
    budget.charge(
        1,
        raw.len()
            .checked_mul(2)
            .context("decoded name byte count overflow")?,
    )?;
    // Hook identity must never use the legacy display reader's lossy fallback.
    let decoded = super::mutf8::decode_mutf8(raw)
        .map_err(|offset| anyhow::anyhow!("invalid MUTF-8 identity at byte {offset}"))?;
    let mut offset = dex.u32(dex.header.strings_off + index * 4)? as usize;
    let declared_length = super::read_uleb(dex.data, &mut offset)? as usize;
    if decoded.encode_utf16().count() != declared_length {
        bail!("DEX identity string UTF-16 length does not match its bytes");
    }
    budget.target(&decoded)?;
    Ok(decoded)
}
fn bounded_type(dex: &Dex<'_>, index: usize, budget: &mut Budget) -> Result<String> {
    bounded_string(dex, dex.type_string_idx(index)?, budget)
}
fn validate_header(data: &[u8]) -> Result<()> {
    if data.len() < 0x70
        || !matches!(
            data.get(..8),
            Some(
                b"dex\n035\0"
                    | b"dex\n037\0"
                    | b"dex\n038\0"
                    | b"dex\n039\0"
                    | b"dex\n040\0"
                    | b"dex\n041\0"
            )
        )
    {
        bail!("invalid or unsupported DEX magic/version");
    }
    let v041 = data.get(..8) == Some(b"dex\n041\0");
    let header_size = crate::bytes::read_u32(data, 0x24)? as usize;
    if header_size != if v041 { 0x78 } else { 0x70 }
        || crate::bytes::read_u32(data, 0x28)? != 0x12345678
    {
        bail!("invalid DEX header size or endian tag");
    }
    if !v041 && crate::bytes::read_u32(data, 0x20)? as usize != data.len() {
        bail!("DEX file size does not match capture");
    }
    Ok(())
}
