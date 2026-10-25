//! APK ZIP discovery and inflation; ZIP64 and encryption are unsupported.
//! Declared sizes must match inflated bytes.

use crate::bytes::{read_u16, read_u32};
use anyhow::{Context, Result, bail, ensure};
use libdeflater::{DecompressionError, Decompressor};
use std::borrow::Cow;

#[derive(Clone, Debug)]
pub struct ZipEntry {
    pub name: String,
    pub uncompressed_size: usize,
    pub compressed_size: usize,
    pub(crate) local_header_offset: usize,
    pub compression: u16,
    /// Bare DEX payload: no local ZIP header.
    pub(crate) bare: bool,
}

impl ZipEntry {
    pub(crate) fn bare(name: &str, size: usize) -> Self {
        Self {
            name: name.to_owned(),
            uncompressed_size: size,
            compressed_size: size,
            local_header_offset: 0,
            compression: 0,
            bare: true,
        }
    }
}

/// Random access to borrowed archive bytes; no whole-archive copy.
pub(crate) trait BytesSource {
    fn source_len(&self) -> usize;

    fn range(&self, offset: usize, len: usize) -> Result<Cow<'_, [u8]>>;
}

impl BytesSource for [u8] {
    fn source_len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn range(&self, offset: usize, len: usize) -> Result<Cow<'_, [u8]>> {
        let end = offset.checked_add(len).context("range overflow")?;
        self.get(offset..end)
            .map(Cow::Borrowed)
            .context("range out of bounds")
    }
}

impl BytesSource for Vec<u8> {
    fn source_len(&self) -> usize {
        self.len()
    }

    fn range(&self, offset: usize, len: usize) -> Result<Cow<'_, [u8]>> {
        self.as_slice().range(offset, len)
    }
}

pub(crate) fn parse_zip_entries<S: BytesSource + ?Sized>(
    source: &S,
    include: impl FnMut(&[u8]) -> bool,
) -> Result<Vec<ZipEntry>> {
    parse_zip_entries_bounded(source, include, usize::MAX, usize::MAX)
}

#[derive(Debug)]
pub(crate) struct DirectoryLimit(pub String);
impl std::fmt::Display for DirectoryLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for DirectoryLimit {}

/// Session reads bound directory allocation; one-shot CLI keeps its own policy.
pub(crate) fn parse_zip_entries_bounded<S: BytesSource + ?Sized>(
    source: &S,
    mut include: impl FnMut(&[u8]) -> bool,
    max_entries: usize,
    max_directory_bytes: usize,
) -> Result<Vec<ZipEntry>> {
    let total = source.source_len();
    let search_start = total.saturating_sub(65_557);
    // The EOCD lives in the tail, and its fixed fields are inside the bytes that one read
    // brings back, so locating the directory costs a single range read either way.
    let tail = source.range(search_start, total - search_start)?;
    let eocd = tail
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .context("EOCD not found")?;
    let cd_size = read_u32(&tail, eocd + 12)? as usize;
    let cd_offset = read_u32(&tail, eocd + 16)? as usize;
    if cd_size > max_directory_bytes {
        return Err(DirectoryLimit(format!(
            "ZIP directory exceeds the {max_directory_bytes} byte limit"
        ))
        .into());
    }
    let cd_end = cd_offset
        .checked_add(cd_size)
        .context("central directory overflow")?;
    if cd_end > total {
        bail!("bad central directory range");
    }
    let mut entries = Vec::new();
    let mut offset = cd_offset;
    let mut visited = 0usize;
    while offset + 46 <= cd_end {
        if visited >= max_entries {
            return Err(DirectoryLimit(format!(
                "ZIP directory exceeds the {max_entries} entry limit"
            ))
            .into());
        }
        visited += 1;
        let header = source.range(offset, 46)?;
        if header.get(..4) != Some(b"PK\x01\x02") {
            bail!("bad central directory signature at {offset}");
        }
        let name_len = read_u16(&header, 28)? as usize;
        let extra_len = read_u16(&header, 30)? as usize;
        let comment_len = read_u16(&header, 32)? as usize;
        let name_start = offset + 46;
        let name_end = name_start
            .checked_add(name_len)
            .context("ZIP name overflow")?;
        let name_bytes = source
            .range(name_start, name_len)
            .context("bad ZIP name range")?;
        if include(&name_bytes) {
            entries.push(ZipEntry {
                name: String::from_utf8_lossy(&name_bytes).into_owned(),
                uncompressed_size: read_u32(&header, 24)? as usize,
                compressed_size: read_u32(&header, 20)? as usize,
                local_header_offset: read_u32(&header, 42)? as usize,
                compression: read_u16(&header, 10)?,
                bare: false,
            });
        }
        offset = name_end
            .checked_add(extra_len)
            .and_then(|v| v.checked_add(comment_len))
            .context("central directory entry overflow")?;
    }
    Ok(entries)
}

/// Default explicit entry quota; CLI/MCP leaves it unset.
pub(crate) const DEFAULT_MAX_INFLATED_ENTRY: usize = rxd_core::MAX_ENTRY_BYTES;

fn over_limit(entry: &ZipEntry, max_inflated: usize) -> anyhow::Error {
    let limit = if max_inflated < 1 << 20 {
        format!("{max_inflated} bytes")
    } else {
        format!("{} MiB", max_inflated >> 20)
    };
    anyhow::anyhow!("{} inflates past the {limit} entry limit", entry.name)
}

fn incomplete_stream(entry: &ZipEntry) -> anyhow::Error {
    anyhow::anyhow!("{} is not a complete deflate stream", entry.name)
}

fn compressed_slice<'a, S: BytesSource + ?Sized>(
    source: &'a S,
    entry: &ZipEntry,
) -> Result<Cow<'a, [u8]>> {
    // A bare entry has no local header: the payload is the file.
    if entry.bare {
        return source
            .range(0, entry.compressed_size)
            .context("bad bare payload range");
    }
    let offset = entry.local_header_offset;
    // One read covers the fixed header and the name/extra lengths; a missing or
    // unreadable header reports the same thing a wrong signature does.
    let header = source
        .range(offset, 30)
        .ok()
        .filter(|header| header.get(..4) == Some(b"PK\x03\x04"))
        .with_context(|| format!("bad local header for {}", entry.name))?;
    let name_len = read_u16(&header, 26)? as usize;
    let extra_len = read_u16(&header, 28)? as usize;
    let start = offset + 30 + name_len + extra_len;
    let end = start
        .checked_add(entry.compressed_size)
        .context("compressed range overflow")?;
    source
        .range(start, end - start)
        .context("bad compressed range")
}

/// Read up to aim bytes; libdeflate needs a margin for partial matches.
pub(crate) fn inflate_prefix<S: BytesSource + ?Sized>(
    source: &S,
    entry: &ZipEntry,
    max_inflated: impl Into<Option<usize>>,
    aim: usize,
) -> Result<Vec<u8>> {
    let max_inflated = max_inflated.into();
    const MARGIN: usize = 64 * 1024;
    if entry.compression != 8 {
        return inflate_entry(source, entry, max_inflated);
    }
    let aim = aim.min(entry.uncompressed_size);
    if let Some(limit) = max_inflated
        && aim > limit
    {
        return Err(over_limit(entry, limit));
    }
    let compressed = compressed_slice(source, entry)?;
    if compressed.is_empty() {
        bail!("deflate made no progress for {}", entry.name);
    }
    let capacity = aim
        .checked_add(MARGIN)
        .context("RESOURCE_LIMIT: inflate prefix size overflow")?;
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(capacity)
        .context("RESOURCE_LIMIT: inflate prefix allocation")?;
    buffer.resize(capacity, 0);
    let mut decompressor = Decompressor::new();
    match decompressor.deflate_decompress(&compressed, &mut buffer) {
        Ok(written) => buffer.truncate(written),
        Err(DecompressionError::InsufficientSpace) => buffer.truncate(aim),
        Err(_error) => return Err(incomplete_stream(entry)),
    }
    Ok(buffer)
}

/// Borrow uncompressed entry bytes directly from the source mapping.
pub(crate) fn stored_entry<'a, S: BytesSource + ?Sized>(
    source: &'a S,
    entry: &ZipEntry,
) -> Result<Cow<'a, [u8]>> {
    if entry.compression != 0 {
        bail!("{} is not a stored entry", entry.name);
    }
    let bytes = compressed_slice(source, entry)?;
    if bytes.len() != entry.uncompressed_size {
        bail!(
            "size mismatch for {}: expected {}, got {}",
            entry.name,
            entry.uncompressed_size,
            bytes.len()
        );
    }
    Ok(bytes)
}

pub(crate) fn inflate_entry<S: BytesSource + ?Sized>(
    source: &S,
    entry: &ZipEntry,
    max_inflated: impl Into<Option<usize>>,
) -> Result<Vec<u8>> {
    let max_inflated = max_inflated.into();
    if entry.compression == 0 {
        return Ok(stored_entry(source, entry)?.into_owned());
    }
    let compressed = compressed_slice(source, entry)?;
    let compressed: &[u8] = &compressed;
    match entry.compression {
        8 => {
            let declared = entry.uncompressed_size;
            let initial = declared.min(entry.compressed_size.saturating_mul(4).max(64 * 1024));
            if let Some(limit) = max_inflated
                && initial > limit
            {
                return Err(over_limit(entry, limit));
            }
            let (output, written) = {
                let mut output = Vec::new();
                output
                    .try_reserve_exact(initial)
                    .context("RESOURCE_LIMIT: inflate allocation")?;
                output.resize(initial, 0);
                let mut decompressor = Decompressor::new();
                let written = loop {
                    match decompressor.deflate_decompress(compressed, &mut output) {
                        Ok(written) => break written,
                        Err(DecompressionError::InsufficientSpace) if output.len() < declared => {
                            let grown = output.len().saturating_mul(2).max(64 * 1024).min(declared);
                            if let Some(limit) = max_inflated
                                && grown > limit
                            {
                                return Err(over_limit(entry, limit));
                            }
                            ensure!(
                                grown > output.len(),
                                "INVALID_INPUT: deflate made no progress"
                            );
                            output
                                .try_reserve_exact(grown - output.len())
                                .context("RESOURCE_LIMIT: inflate allocation")?;
                            output.resize(grown, 0);
                        }
                        Err(_error) => return Err(incomplete_stream(entry)),
                    }
                };
                (output, written)
            };
            if written != declared {
                bail!(
                    "size mismatch for {}: expected {declared}, got {written}",
                    entry.name
                );
            }
            Ok(output)
        }
        method => bail!("unsupported compression method {method} for {}", entry.name),
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn inflate_entry<S: BytesSource + ?Sized>(source: &S, entry: &ZipEntry) -> Result<Vec<u8>> {
        super::inflate_entry(source, entry, usize::MAX)
    }

    #[test]
    fn an_entry_past_the_inflated_limit_is_rejected() {
        let payload = vec![7u8; 128 * 1024];
        let zip = build_zip(&[("classes.dex", &payload, true)]);
        let entry = &parse_zip_entries(&zip, |_| true).unwrap()[0];
        let error = super::inflate_entry(&zip, entry, 1024)
            .unwrap_err()
            .to_string();
        assert!(error.contains("entry limit"), "unexpected error: {error}");
        // The same entry under a ceiling above it still inflates, so the limit is a
        // policy, not a size check on the archive.
        assert_eq!(
            super::inflate_entry(&zip, entry, usize::MAX).unwrap(),
            payload
        );
    }

    pub(crate) fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn deflate(payload: &[u8]) -> Vec<u8> {
        let mut compressor = flate2::Compress::new(flate2::Compression::default(), false);
        // Stored deflate blocks cost 5 bytes per 64 KiB plus the payload, so doubling
        // is comfortably above the worst case for any input these tests build.
        let mut out = vec![0u8; payload.len() * 2 + 1024];
        compressor
            .compress(payload, &mut out, flate2::FlushCompress::Finish)
            .expect("deflate");
        out.truncate(compressor.total_out() as usize);
        out
    }

    /// Build stored/deflated test ZIPs; CRC is unused by the reader.
    pub(crate) fn build_zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut local = Vec::new();
        let mut central = Vec::new();
        for (name, payload, deflated) in entries {
            let stored = if *deflated {
                deflate(payload)
            } else {
                payload.to_vec()
            };
            let offset = local.len() as u32;
            local.extend_from_slice(b"PK\x03\x04");
            local.extend_from_slice(&20u16.to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(&if *deflated { 8u16 } else { 0u16 }.to_le_bytes());
            local.extend_from_slice(&[0; 4]);
            local.extend_from_slice(&0u32.to_le_bytes());
            local.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            local.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            local.extend_from_slice(&(name.len() as u16).to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(name.as_bytes());
            local.extend_from_slice(&stored);

            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&if *deflated { 8u16 } else { 0u16 }.to_le_bytes());
            central.extend_from_slice(&[0; 4]);
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            central.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let mut data = local;
        let central_offset = data.len() as u32;
        let central_size = central.len() as u32;
        data.extend_from_slice(&central);
        data.extend_from_slice(b"PK\x05\x06");
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        data.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        data.extend_from_slice(&central_size.to_le_bytes());
        data.extend_from_slice(&central_offset.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data
    }

    const CENTRAL_COMPRESSED_SIZE: usize = 20;
    const CENTRAL_UNCOMPRESSED_SIZE: usize = 24;
    const CENTRAL_NAME_LENGTH: usize = 28;
    const CENTRAL_LOCAL_OFFSET: usize = 42;

    fn patch_central_entry(zip: &mut [u8], field: usize, value: u32) {
        let start = zip
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .expect("archive has a central directory");
        write_u32(zip, start + field, value);
    }

    pub(crate) fn temp_apk(tag: &str, data: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("rasc-{tag}-{}.apk", std::process::id()));
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn parser_filters_entries_by_name_and_inflates_both_methods() {
        let first = vec![7u8; 4096];
        let second = b"second payload".to_vec();
        let zip = build_zip(&[
            ("classes.dex", &first, true),
            ("classes2.dex", &second, false),
            ("AndroidManifest.xml", b"<manifest/>", false),
        ]);
        let entries = parse_zip_entries(&zip, |name| name.starts_with(b"classes")).unwrap();
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["classes.dex", "classes2.dex"]);
        assert_eq!(entries[0].uncompressed_size, first.len());
        assert_eq!(entries[0].compression, 8);
        assert_eq!(inflate_entry(&zip, &entries[0]).unwrap(), first);
        assert_eq!(entries[1].compression, 0);
        assert_eq!(inflate_entry(&zip, &entries[1]).unwrap(), second);
    }

    #[test]
    fn stored_entry_size_mismatch_is_rejected() {
        // Stored entry sizes must agree.
        let mut zip = build_zip(&[("classes.dex", b"payload", false)]);
        patch_central_entry(&mut zip, CENTRAL_COMPRESSED_SIZE, 3);
        let entry = &parse_zip_entries(&zip, |_| true).unwrap()[0];
        let error = inflate_entry(&zip, entry).unwrap_err().to_string();
        assert!(error.contains("size mismatch"), "unexpected error: {error}");
    }

    #[test]
    fn deflate_size_mismatch_is_rejected() {
        let payload = vec![1u8; 2048];
        let mut zip = build_zip(&[("classes.dex", &payload, true)]);
        patch_central_entry(&mut zip, CENTRAL_UNCOMPRESSED_SIZE, 2048 + 64);
        let entry = &parse_zip_entries(&zip, |_| true).unwrap()[0];
        let error = inflate_entry(&zip, entry).unwrap_err().to_string();
        assert!(error.contains("size mismatch"), "unexpected error: {error}");
    }

    #[test]
    fn deflate_grows_the_buffer_for_a_high_ratio_entry() {
        // An all-zero payload compresses far below a quarter of its size, so the
        // first capacity is not enough and the grow path has to run.
        let payload = vec![0u8; 128 * 1024];
        let zip = build_zip(&[("classes.dex", &payload, true)]);
        let entry = &parse_zip_entries(&zip, |_| true).unwrap()[0];
        let compressed = zip.len();
        assert!(
            entry.uncompressed_size > compressed * 4,
            "fixture is not high-ratio enough"
        );
        assert_eq!(inflate_entry(&zip, entry).unwrap(), payload);
    }

    #[test]
    fn zip64_placeholder_uncompressed_size_is_rejected() {
        // Reject unsupported oversized declarations before reserving their payload.
        let payload = vec![3u8; 4096];
        let mut zip = build_zip(&[("classes.dex", &payload, true)]);
        patch_central_entry(&mut zip, CENTRAL_UNCOMPRESSED_SIZE, 0xFFFFFFF0);
        let entry = &parse_zip_entries(&zip, |_| true).unwrap()[0];
        let error = inflate_entry(&zip, entry).unwrap_err().to_string();
        assert!(error.contains("classes.dex"), "unexpected error: {error}");
    }

    #[test]
    fn deflate_into_an_undersized_buffer_is_rejected() {
        let payload = vec![2u8; 2048];
        let mut zip = build_zip(&[("classes.dex", &payload, true)]);
        patch_central_entry(&mut zip, CENTRAL_UNCOMPRESSED_SIZE, 1024);
        let entry = &parse_zip_entries(&zip, |_| true).unwrap()[0];
        assert!(inflate_entry(&zip, entry).is_err());
    }

    #[test]
    fn archive_without_end_of_central_directory_is_rejected() {
        let zip = build_zip(&[("classes.dex", b"payload", false)]);
        let truncated = &zip[..zip.len() - 30];
        let error = parse_zip_entries(truncated, |_| true)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("EOCD not found"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn bad_central_directory_signature_is_rejected() {
        let mut zip = build_zip(&[("classes.dex", b"payload", false)]);
        let start = zip
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .unwrap();
        zip[start + 3] = b'X';
        let error = parse_zip_entries(&zip, |_| true).unwrap_err().to_string();
        assert!(
            error.contains("central directory signature"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn zip64_end_of_central_directory_placeholders_are_rejected() {
        // ZIP64 EOCD placeholders must not drive a 4 GiB directory scan.
        let mut zip = build_zip(&[("classes.dex", b"payload", false)]);
        let eocd = zip
            .windows(4)
            .rposition(|window| window == b"PK\x05\x06")
            .expect("archive has an EOCD");
        zip[eocd + 8..eocd + 12].copy_from_slice(&[0xFF; 4]);
        write_u32(&mut zip, eocd + 12, u32::MAX);
        write_u32(&mut zip, eocd + 16, u32::MAX);
        let error = parse_zip_entries(&zip, |_| true).unwrap_err().to_string();
        assert!(
            error.contains("bad central directory range"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn concatenated_archives_use_the_last_end_of_central_directory() {
        // Backward EOCD search selects the trailing concatenated archive.
        let archive = build_zip(&[("classes.dex", b"payload", false)]);
        let mut both = archive.clone();
        both.extend_from_slice(&archive);
        let entries = parse_zip_entries(&both, |_| true).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(inflate_entry(&both, &entries[0]).unwrap(), b"payload");
    }

    #[test]
    fn data_descriptor_flag_uses_the_central_directory_sizes() {
        // Data descriptors defer local sizes; central-directory sizes remain authoritative.
        let payload = b"streamed payload".to_vec();
        let mut zip = build_zip(&[("classes.dex", &payload, false)]);
        let local = zip
            .windows(4)
            .position(|window| window == b"PK\x03\x04")
            .unwrap();
        zip[local + 6..local + 8].copy_from_slice(&0x0008u16.to_le_bytes());
        write_u32(&mut zip, local + 18, 0);
        write_u32(&mut zip, local + 22, 0);
        let entries = parse_zip_entries(&zip, |_| true).unwrap();
        assert_eq!(inflate_entry(&zip, &entries[0]).unwrap(), payload);
    }

    #[test]
    fn central_name_length_past_the_directory_is_rejected() {
        let mut zip = build_zip(&[("classes.dex", b"payload", false)]);
        patch_central_entry(&mut zip, CENTRAL_NAME_LENGTH, 0xFFFF);
        let error = parse_zip_entries(&zip, |_| true).unwrap_err().to_string();
        assert!(
            error.contains("bad ZIP name range"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn central_local_header_offset_past_the_end_is_rejected() {
        let mut zip = build_zip(&[("classes.dex", b"payload", false)]);
        patch_central_entry(&mut zip, CENTRAL_LOCAL_OFFSET, u32::MAX);
        let entry = &parse_zip_entries(&zip, |_| true).unwrap()[0];
        assert!(inflate_entry(&zip, entry).is_err());
    }
}
