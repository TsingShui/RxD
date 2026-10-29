//! Single-file manifest v1/v2/v6, per dotnet/runtime v10.0.12 HostModel/Bundle.
use std::{borrow::Cow, collections::BTreeSet};

use anyhow::{Context, Result, ensure};
use flate2::{Decompress, FlushDecompress, Status};

use crate::{Limits, binary::Reader, input::Work};

const SIGNATURE: &[u8] = &[
    0x8b, 0x12, 0x02, 0xb9, 0x6a, 0x61, 0x20, 0x38, 0x72, 0x7b, 0x93, 0x02, 0x14, 0xd7, 0xa0, 0x32,
    0x13, 0xf5, 0xb9, 0xe6, 0xef, 0xae, 0x33, 0x18, 0xee, 0x3b, 0x2d, 0xce, 0x24, 0xb3, 0x6a, 0xae,
];

pub struct BundleEntry<'a> {
    pub path: &'a str,
    pub kind: &'static str,
    pub size: usize,
    pub compressed_size: usize,
    offset: usize,
}

impl BundleEntry<'_> {
    pub fn is_assembly(&self) -> bool {
        self.kind == "assembly"
            || (self.kind == "unknown"
                && self.path.rsplit('.').next().is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("dll") || extension.eq_ignore_ascii_case("exe")
                }))
    }
}

pub struct Bundle<'a> {
    bytes: &'a [u8],
    pub entries: Vec<BundleEntry<'a>>,
}

impl<'a> Bundle<'a> {
    /// An unpatched apphost has a zero manifest pointer and is not a bundle.
    pub fn parse(bytes: &'a [u8], limits: Limits) -> Result<Option<Self>> {
        ensure!(
            bytes.len() <= limits.input_bytes,
            "RESOURCE_LIMIT: bundle input bytes"
        );
        if bytes.starts_with(b"MZ")
            && cildec::PeImage::parse(bytes).is_ok_and(|image| !image.data_directory(14).is_empty())
        {
            return Ok(None);
        }
        let marker = memchr::memmem::find_iter(bytes, SIGNATURE).find_map(|marker| {
            let pointer = bytes.get(marker.checked_sub(8)?..marker)?;
            let offset = usize::try_from(u64::from_le_bytes(pointer.try_into().ok()?)).ok()?;
            (offset > marker + SIGNATURE.len() && offset < bytes.len()).then_some((marker, offset))
        });
        let Some((marker, header)) = marker else {
            return Ok(None);
        };
        let mut reader = Reader::new(&bytes[header..]);
        let major = reader.u32()?;
        let minor = reader.u32()?;
        ensure!(
            matches!(major, 1 | 2 | 6) && minor == 0,
            "UNSUPPORTED_INPUT: bundle format {major}.{minor}"
        );
        let count = reader.count()?;
        ensure!(
            count > 0 && count <= reader.bytes.len() / if major == 6 { 26 } else { 18 },
            "INVALID_INPUT: bundle entry count"
        );
        let id = reader.text()?;
        ensure!(
            !id.is_empty() && !id.contains(['\0', '/', '\\']),
            "INVALID_INPUT: bundle id"
        );
        let mut locations = Vec::new();
        if major >= 2 {
            for _ in 0..2 {
                let offset = reader.offset()?;
                let size = reader.offset()?;
                ensure!(
                    (offset == 0 && size == 0)
                        || (offset > 0
                            && offset.checked_add(size).is_some_and(|end| end <= header)),
                    "INVALID_INPUT: bundle configuration range"
                );
                locations.push((offset, size));
            }
            ensure!(reader.u64()? & !1 == 0, "UNSUPPORTED_INPUT: bundle flags");
        }
        let mut work = Work::new(limits.work, limits.metadata_bytes);
        work.retain(
            count
                .checked_mul(std::mem::size_of::<BundleEntry<'_>>() + 80)
                .context("RESOURCE_LIMIT: bundle index size")?,
        )?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .context("RESOURCE_LIMIT: bundle index allocation")?;
        let mut names = BTreeSet::new();
        let mut ranges = Vec::new();
        ranges
            .try_reserve_exact(count)
            .context("RESOURCE_LIMIT: bundle ranges allocation")?;
        for _ in 0..count {
            let offset = reader.offset()?;
            let size = reader.offset()?;
            let compressed_size = if major == 6 { reader.offset()? } else { 0 };
            let kind = match reader.u8()? {
                0 => "unknown",
                1 => "assembly",
                2 => "native",
                3 => "deps_json",
                4 => "runtime_config_json",
                5 => "symbols",
                value => anyhow::bail!("UNSUPPORTED_INPUT: bundle entry kind {value}"),
            };
            let path = reader.text()?;
            work.charge(path.len().saturating_add(1))?;
            ensure!(
                !path.is_empty()
                    && !path.contains(['\0', '\\', ':'])
                    && path.split('/').all(|part| !matches!(part, "" | "." | "..")),
                "INVALID_INPUT: unsafe bundle entry path"
            );
            ensure!(
                names.insert(path),
                "INVALID_INPUT: duplicate bundle entry {path}"
            );
            let stored = if compressed_size == 0 {
                size
            } else {
                compressed_size
            };
            let end = offset
                .checked_add(stored)
                .context("INVALID_INPUT: bundle entry range overflow")?;
            ensure!(
                offset >= marker + SIGNATURE.len() && end <= header,
                "INVALID_INPUT: bundle entry outside payload"
            );
            if stored != 0 {
                ranges.push((offset, end));
            }
            entries.push(BundleEntry {
                path,
                kind,
                size,
                compressed_size,
                offset,
            });
        }
        let payload_start = entries
            .iter()
            .map(|entry| entry.offset)
            .min()
            .unwrap_or(header);
        for other in memchr::memmem::find_iter(&bytes[..payload_start], SIGNATURE) {
            if other != marker && other >= 8 {
                let offset = u64::from_le_bytes(bytes[other - 8..other].try_into()?);
                ensure!(
                    offset == 0 || offset >= bytes.len() as u64,
                    "INVALID_INPUT: ambiguous bundle marker"
                );
            }
        }
        ranges.sort_unstable();
        ensure!(
            ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0),
            "INVALID_INPUT: overlapping bundle entries"
        );
        for ((offset, size), kind) in locations
            .into_iter()
            .zip(["deps_json", "runtime_config_json"])
        {
            if offset != 0 || size != 0 {
                ensure!(
                    entries.iter().any(|entry| entry.kind == kind
                        && entry.offset == offset
                        && entry.size == size
                        && entry.compressed_size == 0),
                    "INVALID_INPUT: inconsistent bundle configuration location"
                );
            }
        }
        Ok(Some(Self { bytes, entries }))
    }

    pub fn entry(&self, path: &str) -> Result<&BundleEntry<'a>> {
        self.entries
            .iter()
            .find(|entry| entry.path == path)
            .with_context(|| format!("NOT_FOUND: bundle entry {path}"))
    }

    /// Decode only the selected immutable range, checking exact Deflate input/output sizes.
    pub fn extract(&self, path: &str, output_limit: Option<usize>) -> Result<Cow<'a, [u8]>> {
        let entry = self.entry(path)?;
        ensure!(
            output_limit.is_none_or(|limit| entry.size <= limit),
            "RESOURCE_LIMIT: extracted entry bytes"
        );
        ensure!(
            entry.size <= rxd_core::MAX_SPAN_BYTES,
            "UNSUPPORTED_INPUT: expanded entry exceeds address span"
        );
        if entry.compressed_size == 0 {
            return Ok(Cow::Borrowed(
                &self.bytes[entry.offset..entry.offset + entry.size],
            ));
        }
        let mut output = Vec::new();
        let mut inflate = Decompress::new(false);
        let data = &self.bytes[entry.offset..entry.offset + entry.compressed_size];
        let mut chunk = [0; 32 * 1024];
        loop {
            let before = (inflate.total_in(), inflate.total_out());
            let status = inflate
                .decompress(
                    &data[before.0 as usize..],
                    &mut chunk,
                    FlushDecompress::None,
                )
                .context("INVALID_INPUT: bundle Deflate stream")?;
            let produced = (inflate.total_out() - before.1) as usize;
            ensure!(
                inflate.total_out() <= entry.size as u64,
                "INVALID_INPUT: bundle expands past declared length"
            );
            output
                .try_reserve(produced)
                .context("RESOURCE_LIMIT: expanded bundle allocation")?;
            output.extend_from_slice(&chunk[..produced]);
            if status == Status::StreamEnd {
                ensure!(
                    inflate.total_in() == data.len() as u64 && output.len() == entry.size,
                    "INVALID_INPUT: bundle Deflate length or trailing data"
                );
                break;
            }
            ensure!(
                (inflate.total_in(), inflate.total_out()) != before,
                "INVALID_INPUT: incomplete bundle Deflate stream"
            );
        }
        Ok(Cow::Owned(output))
    }
}

/// Select exactly one managed module; never infer an entry or load sibling dependencies.
pub fn select_module<'a>(
    bytes: &'a [u8],
    entry: Option<&str>,
    limits: Limits,
) -> Result<Cow<'a, [u8]>> {
    match (Bundle::parse(bytes, limits)?, entry) {
        (Some(bundle), Some(entry)) => {
            ensure!(
                bundle.entry(entry)?.is_assembly(),
                "INVALID_QUERY: selected bundle entry is not an assembly"
            );
            bundle.extract(entry, None)
        }
        (Some(_), None) => anyhow::bail!(
            "INVALID_QUERY: single-file bundle requires --entry PATH; list it with `rxd ilspy entries`"
        ),
        (None, Some(_)) => {
            anyhow::bail!("INVALID_QUERY: --entry requires a .NET single-file bundle")
        }
        (None, None) => Ok(Cow::Borrowed(bytes)),
    }
}
