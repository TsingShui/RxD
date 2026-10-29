//! ManifestResource and .resources v1/v2 indexing; serialized objects stay opaque.
use std::{collections::BTreeSet, mem::size_of};

use anyhow::{Context, Result, ensure};
use cildec::{Metadata, PeImage, TableId, tables::ManifestResourceRow};

use crate::{Limits, binary::Reader, input::Work};

const MAGIC: &[u8] = &[0xce, 0xca, 0xef, 0xbe];

pub struct Resource<'a> {
    pub at: String,
    pub name: String,
    pub resource_type: String,
    pub parent_at: Option<String>,
    data: Option<&'a [u8]>,
    implementation: u32,
}

impl<'a> Resource<'a> {
    pub fn size(&self) -> Option<usize> {
        self.data.map(<[u8]>::len)
    }

    pub fn bytes(&self) -> Result<&'a [u8]> {
        self.data.with_context(|| format!("UNSUPPORTED_INPUT: linked resource belongs to 0x{:08x}; external files are not opened", self.implementation))
    }
}

pub struct Resources<'a> {
    pub roots: Vec<Resource<'a>>,
    retained: usize,
    work_used: usize,
}

fn owned(text: &str, work: &mut Work) -> Result<String> {
    work.retain(text.len())?;
    let mut value = String::new();
    value
        .try_reserve_exact(text.len())
        .context("RESOURCE_LIMIT: resource text allocation")?;
    value.push_str(text);
    Ok(value)
}

impl<'a> Resources<'a> {
    pub fn parse(bytes: &'a [u8], limits: Limits) -> Result<Self> {
        ensure!(
            bytes.len() <= limits.input_bytes,
            "RESOURCE_LIMIT: resource input bytes"
        );
        let image = PeImage::parse(bytes).context("INVALID_INPUT: resource PE")?;
        let metadata: Metadata<'a> = image
            .metadata()
            .context("INVALID_INPUT: resource metadata")?;
        let count = metadata.tables().row_count(TableId::ManifestResource) as usize;
        ensure!(
            count <= 0xffffff,
            "INVALID_INPUT: resource token table overflow"
        );
        let mut work = Work::new(limits.work, limits.metadata_bytes);
        work.retain(
            count
                .checked_mul(size_of::<Resource<'_>>() + 96)
                .context("RESOURCE_LIMIT: resource index size")?,
        )?;
        let mut roots = Vec::new();
        roots
            .try_reserve_exact(count)
            .context("RESOURCE_LIMIT: resource index allocation")?;
        let directory = image
            .cli_header()
            .context("INVALID_INPUT: CLI resource directory")?
            .resources;
        let payload = if directory.size == 0 {
            &[][..]
        } else {
            ensure!(
                directory.rva != 0,
                "INVALID_INPUT: null resource directory RVA"
            );
            image
                .rva_slice(directory.rva, directory.size as usize)
                .context("INVALID_INPUT: resource directory range")?
        };
        let mut names = BTreeSet::new();
        for (rid, row) in metadata.tables().iter::<ManifestResourceRow>() {
            let row = row.context("INVALID_INPUT: manifest resource row")?;
            let name = metadata
                .strings()
                .str(row.name)
                .context("INVALID_INPUT: resource name")?;
            work.charge(name.len().saturating_add(1))?;
            ensure!(
                names.insert(name),
                "INVALID_INPUT: duplicate manifest resource name"
            );
            let implementation = if row.implementation.is_null() {
                0
            } else {
                row.implementation.value()
            };
            if implementation != 0 {
                let table = match implementation >> 24 {
                    0x26 => TableId::File,
                    0x23 => TableId::AssemblyRef,
                    0x27 => TableId::ExportedType,
                    _ => anyhow::bail!("INVALID_INPUT: resource implementation table"),
                };
                ensure!(
                    implementation & 0xffffff != 0
                        && implementation & 0xffffff <= metadata.tables().row_count(table),
                    "INVALID_INPUT: resource implementation token"
                );
            }
            let data = if implementation == 0 {
                let mut reader = Reader::new(
                    payload
                        .get(row.offset as usize..)
                        .context("INVALID_INPUT: resource offset")?,
                );
                let length = reader.u32()? as usize;
                Some(reader.take(length)?)
            } else {
                None
            };
            roots.push(Resource {
                at: format!("0x{:08x}", 0x28000000 | rid),
                name: owned(name, &mut work)?,
                resource_type: if implementation != 0 {
                    "linked"
                } else if data.is_some_and(|data| data.starts_with(MAGIC)) {
                    "resources"
                } else {
                    "bytes"
                }
                .into(),
                parent_at: None,
                data,
                implementation,
            });
        }
        Ok(Self {
            roots,
            retained: work.retained(),
            work_used: work.used(),
        })
    }

    fn root(&self, at: &str) -> Result<&Resource<'a>> {
        let body = at
            .strip_prefix("0x")
            .or_else(|| at.strip_prefix("0X"))
            .unwrap_or(at);
        ensure!(
            body.len() == 8 && body.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "INVALID_QUERY: expected a ManifestResource token from resources.at"
        );
        let token = u32::from_str_radix(body, 16)?;
        ensure!(
            token >> 24 == 0x28,
            "INVALID_QUERY: expected a ManifestResource token"
        );
        self.roots
            .iter()
            .find(|resource| resource.at == format!("0x{token:08x}"))
            .with_context(|| format!("NOT_FOUND: resource 0x{token:08x}"))
    }

    pub fn members(&self, at: &str, mut limits: Limits) -> Result<Vec<Resource<'a>>> {
        let root = self.root(at)?;
        limits.metadata_bytes = limits
            .metadata_bytes
            .checked_sub(self.retained)
            .context("RESOURCE_LIMIT: cumulative resource metadata")?;
        if let Some(limit) = limits.work.as_mut() {
            *limit = limit
                .checked_sub(self.work_used)
                .context("RESOURCE_LIMIT: cumulative resource work")?;
        }
        resource_set(root.bytes()?, &root.at, limits)
    }

    /// Root selection returns the original blob; member selection removes only format framing.
    pub fn extract(&self, at: &str, limits: Limits) -> Result<&'a [u8]> {
        if let Some((parent, item)) = at.split_once("/item:") {
            ensure!(
                !item.is_empty() && item.bytes().all(|byte| byte.is_ascii_digit()),
                "INVALID_QUERY: expected resources.at item coordinate"
            );
            let index: usize = item.parse().context("INVALID_QUERY: resource item index")?;
            let members = self.members(parent, limits)?;
            return members
                .get(index)
                .context("NOT_FOUND: resource item")?
                .bytes();
        }
        self.root(at)?.bytes()
    }
}

fn resource_set<'a>(bytes: &'a [u8], parent: &str, limits: Limits) -> Result<Vec<Resource<'a>>> {
    let mut reader = Reader::new(bytes);
    ensure!(
        reader.take(4)? == MAGIC,
        "UNSUPPORTED_INPUT: resource is not a .resources container"
    );
    let header_version = reader.count()?;
    let header_length = reader.count()?;
    let header = reader.take(header_length)?;
    if header_version <= 1 {
        let mut header = Reader::new(header);
        let reader_name = header.text()?.split(',').next().unwrap_or_default();
        ensure!(
            matches!(
                reader_name,
                "System.Resources.ResourceReader"
                    | "System.Resources.Extensions.DeserializingResourceReader"
            ),
            "UNSUPPORTED_INPUT: resource reader {reader_name}"
        );
        header.text()?;
        ensure!(
            header.pos == header.bytes.len(),
            "INVALID_INPUT: resource header length"
        );
    }
    let version = reader.u32()?;
    ensure!(
        matches!(version, 1 | 2),
        "UNSUPPORTED_INPUT: .resources version {version}"
    );
    let count = reader.count()?;
    let type_count = reader.count()?;
    ensure!(
        count <= bytes.len() / 9 && type_count <= bytes.len(),
        "INVALID_INPUT: .resources counts exceed input"
    );
    let mut work = Work::new(limits.work, limits.metadata_bytes);
    work.retain(
        count
            .checked_mul(size_of::<Resource<'_>>() + 160)
            .and_then(|n| n.checked_add(type_count.checked_mul(size_of::<&str>())?))
            .context("RESOURCE_LIMIT: .resources index size")?,
    )?;
    work.charge(count.saturating_add(type_count))?;
    let mut types = Vec::new();
    types
        .try_reserve_exact(type_count)
        .context("RESOURCE_LIMIT: resource types allocation")?;
    for _ in 0..type_count {
        types.push(reader.text()?);
    }
    reader.take((8 - reader.pos % 8) % 8)?;
    let hashes = reader.take(
        count
            .checked_mul(4)
            .context("INVALID_INPUT: resource hash table")?,
    )?;
    ensure!(
        hashes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|value| i32::from_le_bytes(*value))
            .try_fold(i32::MIN, |previous, next| (next >= previous)
                .then_some(next))
            .is_some(),
        "INVALID_INPUT: unsorted resource name hashes"
    );
    let positions = reader.take(count * 4)?;
    let data_start = reader.count()?;
    let names_start = reader.pos;
    ensure!(
        data_start >= names_start && data_start <= bytes.len(),
        "INVALID_INPUT: resource name/data sections"
    );
    let names = &bytes[names_start..data_start];
    let data = &bytes[data_start..];
    let mut records = Vec::new();
    records
        .try_reserve_exact(count)
        .context("RESOURCE_LIMIT: resource records allocation")?;
    let mut offsets = Vec::new();
    offsets
        .try_reserve_exact(count)
        .context("RESOURCE_LIMIT: resource offsets allocation")?;
    let mut distinct = BTreeSet::new();
    for (index, position) in positions.as_chunks::<4>().0.iter().enumerate() {
        let position = u32::from_le_bytes(*position) as usize;
        let mut name_reader = Reader::new(
            names
                .get(position..)
                .context("INVALID_INPUT: resource name position")?,
        );
        let utf16 = name_reader.string_bytes()?;
        ensure!(
            utf16.len() % 2 == 0,
            "INVALID_INPUT: odd UTF-16 resource name length"
        );
        work.charge(utf16.len())?;
        work.retain(utf16.len().saturating_mul(4))?;
        let mut name = String::new();
        name.try_reserve_exact(utf16.len().saturating_mul(2))
            .context("RESOURCE_LIMIT: resource name allocation")?;
        for unit in char::decode_utf16(
            utf16
                .as_chunks::<2>()
                .0
                .iter()
                .map(|unit| u16::from_le_bytes(*unit)),
        ) {
            name.push(unit.context("INVALID_INPUT: invalid UTF-16 resource name")?);
        }
        let hash = utf16.as_chunks::<2>().0.iter().fold(5381u32, |hash, unit| {
            hash.wrapping_mul(33) ^ u32::from(u16::from_le_bytes(*unit))
        });
        ensure!(
            hashes[index * 4..index * 4 + 4] == hash.to_le_bytes(),
            "INVALID_INPUT: resource name hash mismatch"
        );
        ensure!(
            distinct.insert(name.clone()),
            "INVALID_INPUT: duplicate .resources name"
        );
        let offset = name_reader.count()?;
        ensure!(offset < data.len(), "INVALID_INPUT: resource value offset");
        offsets.push(offset);
        records.push((name, offset));
    }
    offsets.sort_unstable();
    offsets.dedup();
    let mut resources = Vec::new();
    resources
        .try_reserve_exact(count)
        .context("RESOURCE_LIMIT: resource members allocation")?;
    for (index, (name, offset)) in records.into_iter().enumerate() {
        let end = offsets
            .get(offsets.binary_search(&offset).unwrap() + 1)
            .copied()
            .unwrap_or(data.len());
        let mut value = Reader::new(&data[offset..end]);
        let code = value.seven()?;
        let (resource_type, framing) = if version == 2 {
            type_v2(code, &types)?
        } else if code == u32::MAX {
            ("null", Framing::Fixed(0))
        } else {
            let name = *types
                .get(code as usize)
                .context("INVALID_INPUT: resource type index")?;
            (name, type_v1(name))
        };
        let payload = match framing {
            Framing::String => {
                let payload = value.string_bytes()?;
                std::str::from_utf8(payload).context("INVALID_INPUT: resource string UTF-8")?;
                payload
            }
            Framing::Bytes => {
                let length = value.count()?;
                value.take(length)?
            }
            Framing::Fixed(length) => value.take(length)?,
            Framing::Opaque => value.take(value.bytes.len() - value.pos)?,
        };
        ensure!(
            value.pos == value.bytes.len(),
            "INVALID_INPUT: trailing resource value bytes"
        );
        resources.push(Resource {
            at: format!("{parent}/item:{index}"),
            name,
            resource_type: owned(resource_type, &mut work)?,
            parent_at: Some(owned(parent, &mut work)?),
            data: Some(payload),
            implementation: 0,
        });
    }
    Ok(resources)
}

#[derive(Clone, Copy)]
enum Framing {
    Fixed(usize),
    String,
    Bytes,
    Opaque,
}

fn type_v2<'a>(code: u32, types: &[&'a str]) -> Result<(&'a str, Framing)> {
    let result = match code {
        0 => ("null", Framing::Fixed(0)),
        1 => ("System.String", Framing::String),
        2 => ("System.Boolean", Framing::Fixed(1)),
        3 => ("System.Char", Framing::Fixed(2)),
        4 => ("System.Byte", Framing::Fixed(1)),
        5 => ("System.SByte", Framing::Fixed(1)),
        6 => ("System.Int16", Framing::Fixed(2)),
        7 => ("System.UInt16", Framing::Fixed(2)),
        8 => ("System.Int32", Framing::Fixed(4)),
        9 => ("System.UInt32", Framing::Fixed(4)),
        10 => ("System.Int64", Framing::Fixed(8)),
        11 => ("System.UInt64", Framing::Fixed(8)),
        12 => ("System.Single", Framing::Fixed(4)),
        13 => ("System.Double", Framing::Fixed(8)),
        14 => ("System.Decimal", Framing::Fixed(16)),
        15 => ("System.DateTime", Framing::Fixed(8)),
        16 => ("System.TimeSpan", Framing::Fixed(8)),
        32 => ("System.Byte[]", Framing::Bytes),
        33 => ("System.IO.Stream", Framing::Bytes),
        64.. => (
            *types
                .get((code - 64) as usize)
                .context("INVALID_INPUT: resource user type index")?,
            Framing::Opaque,
        ),
        _ => anyhow::bail!("INVALID_INPUT: unknown resource type code {code}"),
    };
    Ok(result)
}

fn type_v1(name: &str) -> Framing {
    let mut parts = name.split(',');
    let primitive = parts.next().unwrap_or_default();
    if parts.next().is_some_and(|assembly| {
        ![
            "mscorlib",
            "System.Private.CoreLib",
            "System.Runtime",
            "netstandard",
        ]
        .iter()
        .any(|core| assembly.trim().eq_ignore_ascii_case(core))
    }) {
        return Framing::Opaque;
    }
    match primitive {
        "System.String" => Framing::String,
        "System.Byte" | "System.SByte" => Framing::Fixed(1),
        "System.Int16" | "System.UInt16" => Framing::Fixed(2),
        "System.Int32" | "System.UInt32" | "System.Single" => Framing::Fixed(4),
        "System.Int64" | "System.UInt64" | "System.Double" | "System.DateTime"
        | "System.TimeSpan" => Framing::Fixed(8),
        "System.Decimal" => Framing::Fixed(16),
        _ => Framing::Opaque,
    }
}
