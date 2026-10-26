use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use flate2::read::ZlibDecoder;
use rxd_core::{Error, Limits, Result, perf::Perf};

mod marshal;
mod pyz;

const MAGIC: [u8; 8] = [0x4d, 0x45, 0x49, 0x0c, 0x0b, 0x0a, 0x0b, 0x0e];
const COOKIE_BYTES: u64 = 88;
const SCAN_CHUNK: u64 = 64 << 10;
const MIN_ENTRY_BYTES: usize = 18;

#[derive(Debug)]
pub struct Summary {
    pub files: usize,
    pub bytes: u64,
    pub python: Option<String>,
    pub pyz_files: usize,
    pub pyz_bytes: u64,
    pub skipped: usize,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new("INVALID_INPUT", message)
}

fn unsupported(message: impl Into<String>) -> Error {
    Error::new("UNSUPPORTED_INPUT", message)
}

fn resource(message: impl Into<String>) -> Error {
    Error::new("RESOURCE_LIMIT", message)
}

pub fn detect(path: &Path) -> Result<bool> {
    let mut file = open_input(path)?;
    let length = file.metadata()?.len();
    Ok(scan_magic(&mut file, length, length, |_, _| Ok(true))?.is_some())
}

/// Find the launcher of a PyInstaller one-directory application.
///
/// The launcher is the regular file directly under `directory` whose CArchive
/// contains a valid PyInstaller cookie. Loose dependencies remain where they are.
pub fn find_launcher(directory: &Path) -> Result<PathBuf> {
    if !directory.is_dir() {
        return Err(invalid("PyInstaller one-dir input must be a directory"));
    }
    let mut candidates = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if detect(&path)? {
            candidates.push(path);
        }
    }
    candidates.sort();
    match candidates.len() {
        0 => Err(unsupported(
            "no PyInstaller CArchive launcher was found in the directory",
        )),
        1 => Ok(candidates.remove(0)),
        _ => Err(invalid(
            "multiple PyInstaller CArchive launchers were found",
        )),
    }
}

pub fn unpack(
    input: &Path,
    destination: &Path,
    limits: Limits,
    max_entries: usize,
    perf: &mut Perf,
) -> Result<Summary> {
    let max_entry = limits.output_bytes.unwrap_or(rxd_core::MAX_SPAN_BYTES);
    let mut file = open_input(input)?;
    let length = file.metadata()?.len();
    if length > limits.input_bytes as u64 {
        return Err(resource("PyInstaller input exceeds the input byte budget"));
    }
    perf.counters.input_bytes = usize::try_from(length).ok();
    let position = perf.measure("read", || {
        scan_magic(&mut file, length, length, |file, position| {
            Ok(parse_cookie(file, position, length)?.is_some())
        })
    })?;
    let position = position.ok_or_else(|| invalid("no valid PyInstaller CArchive cookie"))?;
    let cookie = perf
        .measure("parse", || parse_cookie(&mut file, position, length))?
        .ok_or_else(|| invalid("no valid PyInstaller CArchive cookie"))?;
    let header_len = header_length(cookie.python);
    let toc_length = usize::try_from(cookie.toc_length)
        .map_err(|_| resource("CArchive TOC exceeds the addressable byte range"))?;
    let toc = perf.measure("read", || {
        read_at(&mut file, cookie.overlay + cookie.toc_offset, toc_length)
    })?;
    let entries = perf.measure("parse", || {
        parse_toc(&toc, cookie.overlay, length, max_entries, max_entry)
    })?;

    let mut summary = Summary {
        files: 0,
        bytes: 0,
        python: Some(format!("{}.{}", cookie.python.0, cookie.python.1)),
        pyz_files: 0,
        pyz_bytes: 0,
        skipped: 0,
    };
    let mut magic: Option<[u8; 4]> = None;
    let mut bare: Vec<PathBuf> = Vec::new();
    for entry in &entries {
        if matches!(entry.kind, b'd' | b'o') {
            summary.skipped += 1;
            continue;
        }
        let data = perf.measure("read", || read_entry(&mut file, entry, max_entry))?;
        if matches!(entry.kind, b'z' | b'Z') {
            let relative = pyz_path(&entry.name)?;
            perf.measure("write", || write_new(destination, &relative, &data))?;
            summary.files += 1;
            summary.bytes += data.len() as u64;
            let directory = destination.join(extracted_directory(&relative)?);
            let modules = pyz::unpack(&data, &directory, header_len, max_entry, max_entries, perf)?;
            summary.files += modules.files;
            summary.bytes += modules.bytes;
            summary.pyz_files += modules.files;
            summary.pyz_bytes += modules.bytes;
            magic.get_or_insert(modules.magic);
        } else if matches!(entry.kind, b's' | b'm' | b'M') {
            let relative = pyc_path(&entry.name)?;
            if data.len() >= 4 && data[2..4] == [0x0d, 0x0a] {
                magic.get_or_insert(data[..4].try_into().expect("four bytes"));
                perf.measure("write", || write_new(destination, &relative, &data))?;
                summary.files += 1;
                summary.bytes += data.len() as u64;
            } else {
                let mut output = vec![0; header_len];
                output.extend_from_slice(&data);
                perf.measure("write", || write_new(destination, &relative, &output))?;
                summary.files += 1;
                summary.bytes += output.len() as u64;
                bare.push(relative);
            }
        } else {
            let relative = safe_relative(&entry.name)?;
            perf.measure("write", || write_new(destination, &relative, &data))?;
            summary.files += 1;
            summary.bytes += data.len() as u64;
        }
    }
    if !bare.is_empty() {
        let magic = magic.ok_or_else(|| {
            unsupported("no Python bytecode magic is available for bare .pyc files")
        })?;
        for relative in &bare {
            perf.measure("write", || patch_magic(&destination.join(relative), &magic))?;
        }
    }
    perf.counters.output_bytes = usize::try_from(summary.bytes).ok();
    Ok(summary)
}

fn open_input(path: &Path) -> Result<File> {
    if !fs::metadata(path)?.is_file() {
        return Err(invalid("PyInstaller input must be a regular file"));
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("PyInstaller input must be a regular file"));
    }
    Ok(file)
}

fn read_at(file: &mut File, offset: u64, length: usize) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| resource("archive read allocation failed"))?;
    bytes.resize(length, 0);
    file.read_exact(&mut bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            invalid("truncated PyInstaller archive")
        } else {
            error.into()
        }
    })?;
    Ok(bytes)
}

/// Report magic positions newest first while a bounded window remains unscanned.
fn scan_magic(
    file: &mut File,
    length: u64,
    window: u64,
    mut accept: impl FnMut(&mut File, u64) -> Result<bool>,
) -> Result<Option<u64>> {
    if length < MAGIC.len() as u64 {
        return Ok(None);
    }
    let floor = length.saturating_sub(window);
    let mut end = length;
    while end > floor {
        let start = end.saturating_sub(SCAN_CHUNK).max(floor);
        let tail = end.saturating_add(MAGIC.len() as u64 - 1).min(length);
        let span = usize::try_from(tail - start)
            .map_err(|_| resource("cookie scan window exceeds the addressable byte range"))?;
        let bytes = read_at(file, start, span)?;
        if let Some(last) = bytes.len().checked_sub(MAGIC.len()) {
            for offset in (0..=last).rev() {
                if bytes[offset..offset + MAGIC.len()] != MAGIC {
                    continue;
                }
                let position = start + offset as u64;
                if position < end && accept(file, position)? {
                    return Ok(Some(position));
                }
            }
        }
        end = start;
    }
    Ok(None)
}

#[derive(Clone, Copy)]
struct Cookie {
    overlay: u64,
    toc_offset: u64,
    toc_length: u64,
    python: (u32, u32),
}

fn parse_cookie(file: &mut File, position: u64, length: u64) -> Result<Option<Cookie>> {
    let package_end = position
        .checked_add(COOKIE_BYTES)
        .ok_or_else(|| invalid("PyInstaller cookie offset overflow"))?;
    if package_end > length {
        return Ok(None);
    }
    let bytes = read_at(file, position, COOKIE_BYTES as usize)?;
    let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
    let package_length = u64::from(word(8));
    let toc_offset = u64::from(word(12));
    let toc_length = u64::from(word(16));
    if package_length > package_end {
        return Ok(None);
    }
    let overlay = package_end - package_length;
    if overlay > length {
        return Ok(None);
    }
    let toc_end = overlay
        .checked_add(toc_offset)
        .and_then(|start| start.checked_add(toc_length));
    if toc_end.is_none_or(|end| end > length) {
        return Ok(None);
    }
    Ok(Some(Cookie {
        overlay,
        toc_offset,
        toc_length,
        python: python_version(word(20)),
    }))
}

fn python_version(version: u32) -> (u32, u32) {
    if version >= 100 {
        (version / 100, version % 100)
    } else {
        (version / 10, version % 10)
    }
}

fn header_length(python: (u32, u32)) -> usize {
    let (major, minor) = python;
    if (major, minor) >= (3, 7) {
        16
    } else if (major, minor) >= (3, 3) {
        12
    } else {
        8
    }
}

struct Entry {
    name: String,
    kind: u8,
    offset: u64,
    stored: u64,
    uncompressed: u64,
    compressed: bool,
}

fn parse_toc(
    toc: &[u8],
    overlay: u64,
    length: u64,
    max_entries: usize,
    max_entry: usize,
) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut names = HashSet::new();
    let mut position = 0usize;
    while position < toc.len() {
        let rest = &toc[position..];
        if rest.len() < 4 {
            return Err(invalid("truncated CArchive TOC entry header"));
        }
        let entry_size = u32::from_be_bytes(rest[..4].try_into().expect("four bytes")) as usize;
        if entry_size < MIN_ENTRY_BYTES || entry_size > rest.len() {
            return Err(invalid("invalid CArchive TOC entry size"));
        }
        let body = &rest[4..entry_size];
        let word = |at: usize| u32::from_be_bytes(body[at..at + 4].try_into().expect("four bytes"));
        let offset = u64::from(word(0));
        let stored = u64::from(word(4));
        let uncompressed = u64::from(word(8));
        let flag = body[12];
        let kind = body[13];
        let field = &body[14..];
        let name_end = field
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(field.len());
        let name = std::str::from_utf8(&field[..name_end])
            .map_err(|_| invalid("CArchive entry name is not UTF-8"))?;
        if name.is_empty() {
            return Err(invalid("CArchive entry has an empty name"));
        }
        if !names.insert(name.to_owned()) {
            return Err(invalid(format!("duplicate CArchive entry name: {name}")));
        }
        if flag > 1 {
            return Err(unsupported(format!(
                "unsupported CArchive compression flag {flag}"
            )));
        }
        if stored > max_entry as u64 || uncompressed > max_entry as u64 {
            return Err(resource(format!(
                "CArchive entry exceeds the output byte cap: {name}"
            )));
        }
        let start = overlay
            .checked_add(offset)
            .ok_or_else(|| invalid(format!("CArchive entry offset overflow: {name}")))?;
        let end = start
            .checked_add(stored)
            .ok_or_else(|| invalid(format!("CArchive entry size overflow: {name}")))?;
        if end > length {
            return Err(invalid(format!(
                "CArchive entry data is outside the input: {name}"
            )));
        }
        if flag == 0 && stored != uncompressed {
            return Err(invalid(format!(
                "CArchive stored and uncompressed sizes disagree: {name}"
            )));
        }
        entries.push(Entry {
            name: name.to_owned(),
            kind,
            offset: start,
            stored,
            uncompressed,
            compressed: flag == 1,
        });
        if entries.len() > max_entries {
            return Err(resource("CArchive entry count exceeds the entry limit"));
        }
        position += entry_size;
    }
    Ok(entries)
}

fn read_entry(file: &mut File, entry: &Entry, max_entry: usize) -> Result<Vec<u8>> {
    let stored = usize::try_from(entry.stored)
        .map_err(|_| resource("CArchive entry exceeds the addressable byte range"))?;
    let bytes = read_at(file, entry.offset, stored)?;
    if !entry.compressed {
        return Ok(bytes);
    }
    let expected = usize::try_from(entry.uncompressed)
        .map_err(|_| resource("CArchive entry exceeds the addressable byte range"))?;
    let data = inflate(&bytes, max_entry, Some(expected))?;
    if data.len() as u64 != entry.uncompressed {
        return Err(invalid(format!(
            "CArchive decompressed size disagrees with the TOC: {}",
            entry.name
        )));
    }
    Ok(data)
}

fn inflate(bytes: &[u8], cap: usize, reserve: Option<usize>) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    if let Some(reserve) = reserve {
        data.try_reserve_exact(reserve.min(cap))
            .map_err(|_| resource("decompression allocation failed"))?;
    }
    ZlibDecoder::new(bytes)
        .take(cap as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|_| unsupported("zlib decompression failed"))?;
    if data.len() > cap {
        return Err(resource("decompressed entry exceeds the output byte cap"));
    }
    Ok(data)
}

fn safe_relative(name: &str) -> Result<PathBuf> {
    if name.is_empty() || name.starts_with('/') || name.contains(['\0', '\\', ':']) {
        return Err(invalid(format!("unsafe archive entry name: {name}")));
    }
    let mut relative = PathBuf::new();
    for part in name.split('/') {
        if part.is_empty() || matches!(part, "." | "..") {
            return Err(invalid(format!("unsafe archive entry name: {name}")));
        }
        relative.push(part);
    }
    Ok(relative)
}

fn pyz_path(name: &str) -> Result<PathBuf> {
    let mut relative = safe_relative(name)?;
    if relative.extension().is_none() {
        let file = relative
            .file_name()
            .and_then(|file| file.to_str())
            .ok_or_else(|| invalid(format!("unsafe archive entry name: {name}")))?;
        relative.set_file_name(format!("{file}.pyz"));
    }
    Ok(relative)
}

fn pyc_path(name: &str) -> Result<PathBuf> {
    let mut relative = safe_relative(name)?;
    let file = relative
        .file_name()
        .and_then(|file| file.to_str())
        .ok_or_else(|| invalid(format!("unsafe archive entry name: {name}")))?;
    if !file.ends_with(".pyc") {
        relative.set_file_name(format!("{file}.pyc"));
    }
    Ok(relative)
}

fn extracted_directory(relative: &Path) -> Result<PathBuf> {
    let file = relative
        .file_name()
        .and_then(|file| file.to_str())
        .ok_or_else(|| invalid("PYZ entry name is not UTF-8"))?;
    Ok(relative.with_file_name(format!("{file}_extracted")))
}

fn write_new(root: &Path, relative: &Path, bytes: &[u8]) -> Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn patch_magic(path: &Path, magic: &[u8; 4]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(magic)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_version_uses_direct_and_encoded_forms() {
        assert_eq!(python_version(309), (3, 9));
        assert_eq!(python_version(313), (3, 13));
        assert_eq!(python_version(27), (2, 7));
        assert_eq!(python_version(38), (3, 8));
    }

    #[test]
    fn header_length_tracks_the_python_version() {
        assert_eq!(header_length((3, 9)), 16);
        assert_eq!(header_length((3, 7)), 16);
        assert_eq!(header_length((3, 6)), 12);
        assert_eq!(header_length((3, 3)), 12);
        assert_eq!(header_length((3, 2)), 8);
        assert_eq!(header_length((2, 7)), 8);
    }

    #[test]
    fn entry_names_reject_traversal() {
        for name in [
            "",
            "/etc/passwd",
            "../evil",
            "a/../b",
            "a//b",
            "a\\b",
            "c:name",
            "a\0b",
        ] {
            assert!(safe_relative(name).is_err(), "{name}");
        }
        assert_eq!(safe_relative("a/b.txt").unwrap(), Path::new("a/b.txt"));
    }

    #[test]
    fn python_archive_names_get_one_pyc_suffix() {
        assert_eq!(pyc_path("main").unwrap(), Path::new("main.pyc"));
        assert_eq!(pyc_path("main.pyc").unwrap(), Path::new("main.pyc"));
        assert_eq!(pyc_path("pkg/mod").unwrap(), Path::new("pkg/mod.pyc"));
        assert_eq!(pyz_path("PYZ").unwrap(), Path::new("PYZ.pyz"));
        assert_eq!(pyz_path("PYZ.pyz").unwrap(), Path::new("PYZ.pyz"));
    }
}
