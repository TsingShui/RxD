//! ZIP-family metadata and bounded, byte-exact selected-entry decoding.
mod archive;

use std::{fs::File, io::Read, path::Path};

use archive::{invalid, resource, zip_error};
use rayon::prelude::*;
use rxd_core::{
    Budget, Error, Origin, Result,
    envelope::{self, RecordMeta},
    output::Output,
    perf::Perf,
};
use serde_json::json;

pub enum Query {
    Entries {
        contains: Option<String>,
        case_sensitive: bool,
    },
    Extract {
        at: String,
    },
}

pub struct Options<'a> {
    pub input: &'a Path,
    pub output: Option<&'a Path>,
    pub budget: Budget,
}

pub enum Response {
    List(Vec<u8>),
    Extract { bytes: Vec<u8>, receipt: Vec<u8> },
}

pub struct SelectedEntry {
    pub bytes: Vec<u8>,
    pub path: String,
    pub at: String,
}

struct Entry {
    path: String,
    at: String,
    bytes: u64,
    compressed_bytes: u64,
    crc32: u32,
    data_start: u64,
    flags: u16,
    method: u16,
    kind: &'static str,
    range_valid: bool,
}

impl Entry {
    fn encrypted(&self) -> bool {
        self.flags & 0x41 != 0
    }
    fn compression(&self) -> &'static str {
        match self.method {
            0 => "stored",
            8 => "deflate",
            _ => "unsupported",
        }
    }
    fn readable(&self) -> bool {
        self.kind == "file"
            && safe_path(&self.path)
            && self.range_valid
            && self.bytes <= rxd_core::MAX_SPAN_BYTES as u64
            && !self.encrypted()
            && matches!(self.method, 0 | 8)
    }
}

fn safe_path(path: &str) -> bool {
    let path = path.strip_suffix('/').unwrap_or(path);
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains(['\0', '\\', ':'])
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

/// Compressed entries above this size stream through the ZIP reader instead of a
/// one-shot libdeflater buffer, bounding the temporary allocation.
const MAX_FAST_INFLATE: u64 = 64 << 20;

struct Archive {
    zip: zip::ZipArchive<archive::Reader>,
    entries: Vec<Entry>,
    /// Positioned-read handle for fast Stored/Deflate entry decoding.
    raw: Option<File>,
}

impl Archive {
    fn open(options: &Options<'_>) -> Result<Self> {
        let (reader, directory, raw) = archive::open(options.input, options.budget)?;
        Self::from_reader(reader, directory, raw)
    }

    fn from_bytes(bytes: Vec<u8>, budget: Budget) -> Result<Self> {
        let (reader, directory) = archive::open_bytes(bytes, budget)?;
        Self::from_reader(reader, directory, None)
    }

    fn from_reader(
        reader: archive::Reader,
        directory: archive::Directory,
        raw: Option<File>,
    ) -> Result<Self> {
        let mut zip = zip::ZipArchive::new(reader).map_err(zip_error)?;
        if zip.offset() != 0 {
            return Err(Error::new(
                "UNSUPPORTED_INPUT",
                "prepended/self-extracting ZIP archives are unsupported",
            ));
        }
        if zip.len() != directory.flags.len() {
            return Err(invalid("inconsistent ZIP central index"));
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(zip.len())
            .map_err(|_| resource("ZIP entry allocation failed"))?;
        for index in 0..zip.len() {
            let file = zip.by_index_raw(index).map_err(zip_error)?;
            let flags = directory.flags[index];
            if flags & (1 << 11) != 0 && std::str::from_utf8(file.name_raw()).is_err() {
                return Err(invalid("ZIP entry has invalid UTF-8 filename"));
            }
            if !(directory.start..directory.end).contains(&file.central_header_start()) {
                return Err(invalid("ZIP entry header is outside the central directory"));
            }
            let mode = file.unix_mode().unwrap_or(0) & 0o170000;
            let kind = match mode {
                0o120000 => "link",
                0o040000 => "directory",
                0 | 0o100000 if file.is_dir() => "directory",
                0 | 0o100000 => "file",
                _ => "special",
            };
            let range_valid = file.header_start() < directory.start
                && file.data_start() <= directory.start
                && file
                    .data_start()
                    .checked_add(file.compressed_size())
                    .is_some_and(|end| end <= directory.start);
            entries.push(Entry {
                path: file.name().to_owned(),
                at: format!("zip:{index}"),
                bytes: file.size(),
                compressed_bytes: file.compressed_size(),
                crc32: file.crc32(),
                data_start: file.data_start(),
                flags,
                method: directory.methods[index],
                kind,
                range_valid,
            });
        }
        Ok(Self { zip, entries, raw })
    }

    fn select(&self, at: &str) -> Result<usize> {
        if let Some(index) = at.strip_prefix("zip:") {
            let index = index
                .parse::<usize>()
                .ok()
                .filter(|index| format!("zip:{index}") == at)
                .ok_or_else(|| {
                    Error::new(
                        "INVALID_QUERY",
                        "expected zip:<central-directory-index> or an exact entry path",
                    )
                })?;
            if index >= self.entries.len() {
                return Err(Error::new(
                    "NOT_FOUND",
                    format!("ZIP coordinate not found: {at}"),
                ));
            }
            return Ok(index);
        }
        let mut hits = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.path == at);
        let (index, _) = hits
            .next()
            .ok_or_else(|| Error::new("NOT_FOUND", format!("ZIP entry not found: {at}")))?;
        if hits.next().is_some() {
            return Err(Error::new(
                "AMBIGUOUS",
                "duplicate ZIP path; select one entries.at coordinate",
            ));
        }
        Ok(index)
    }

    fn read(&mut self, index: usize, budget: Budget) -> Result<Vec<u8>> {
        let entry = &self.entries[index];
        if !safe_path(&entry.path) {
            return Err(invalid(
                "ZIP entry path must be canonical and archive-relative, without backslashes, traversal or drive prefixes",
            ));
        }
        if entry.kind != "file" || entry.encrypted() || !matches!(entry.method, 0 | 8) {
            return Err(Error::new(
                "UNSUPPORTED_INPUT",
                "only regular, unencrypted Stored/Deflate ZIP entries can be read",
            ));
        }
        if !entry.range_valid {
            return Err(invalid(
                "ZIP entry data range is outside the payload region",
            ));
        }
        let limit = budget
            .entry_bytes
            .unwrap_or(rxd_core::MAX_SPAN_BYTES)
            .min(rxd_core::MAX_SPAN_BYTES);
        let expected = usize::try_from(entry.bytes)
            .ok()
            .filter(|bytes| *bytes <= limit)
            .ok_or_else(|| resource("ZIP expanded entry exceeds byte limit"))?;
        if let Some(bytes) = self.read_fast(entry, expected)? {
            return Ok(bytes);
        }
        self.read_streaming(index, expected)
    }

    /// Decode Stored/Deflate entries from their declared spans.
    ///
    /// Returns `None` when the entry should use the streaming ZIP reader: bytes-only
    /// archives, very large compressed spans, or unsupported platforms.
    fn read_fast(&self, entry: &Entry, expected: usize) -> Result<Option<Vec<u8>>> {
        let (Some(raw), true) = (&self.raw, cfg!(any(unix, windows))) else {
            return Ok(None);
        };
        if entry.method == 8 && entry.compressed_bytes > MAX_FAST_INFLATE {
            return Ok(None);
        }
        if entry.method == 0 && entry.compressed_bytes != entry.bytes {
            // Let the streaming reader apply the same consistency checks.
            return Ok(None);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(expected)
            .map_err(|_| resource("ZIP expanded entry allocation failed"))?;
        bytes.resize(expected, 0);
        if entry.method == 0 {
            positioned_read(raw, entry.data_start, &mut bytes)?;
        } else {
            let compressed = usize::try_from(entry.compressed_bytes)
                .map_err(|_| resource("ZIP compressed entry is not addressable"))?;
            let mut packed = Vec::new();
            packed
                .try_reserve_exact(compressed)
                .map_err(|_| resource("ZIP compressed entry allocation failed"))?;
            packed.resize(compressed, 0);
            positioned_read(raw, entry.data_start, &mut packed)?;
            let written = libdeflater::Decompressor::new()
                .deflate_decompress(&packed, &mut bytes)
                .map_err(|error| invalid(format!("ZIP deflate failed: {error:?}")))?;
            if written != expected {
                return Err(invalid(
                    "ZIP expanded entry does not match its declared size",
                ));
            }
        }
        if crc32fast::hash(&bytes) != entry.crc32 {
            return Err(invalid("ZIP entry CRC mismatch"));
        }
        Ok(Some(bytes))
    }

    fn read_streaming(&mut self, index: usize, expected: usize) -> Result<Vec<u8>> {
        let entry = &self.entries[index];
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(expected)
            .map_err(|_| resource("ZIP expanded entry allocation failed"))?;
        let mut file = self
            .zip
            .by_index(index)
            .map_err(zip_error)?
            .take(entry.bytes + 1);
        let mut buffer = [0; 65536];
        loop {
            let len = file
                .read(&mut buffer)
                .map_err(|error| invalid(format!("ZIP entry decode/CRC failed: {error}")))?;
            if len == 0 {
                break;
            }
            if len > expected - bytes.len() {
                return Err(invalid("ZIP expanded entry exceeds its declared size"));
            }
            bytes.extend_from_slice(&buffer[..len]);
        }
        if bytes.len() != expected {
            return Err(invalid(
                "ZIP expanded entry does not match its declared size",
            ));
        }
        Ok(bytes)
    }
}

fn positioned_read(file: &File, offset: u64, mut buffer: &mut [u8]) -> Result<()> {
    let mut at = offset;
    while !buffer.is_empty() {
        let read = read_at(file, at, buffer)
            .map_err(|error| invalid(format!("ZIP entry read failed: {error}")))?;
        if read == 0 {
            return Err(invalid("ZIP entry data ends before its declared size"));
        }
        at += read as u64;
        buffer = &mut buffer[read..];
    }
    Ok(())
}

#[cfg(unix)]
fn read_at(file: &File, offset: u64, buffer: &mut [u8]) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buffer, offset)
}

#[cfg(windows)]
fn read_at(file: &File, offset: u64, buffer: &mut [u8]) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buffer, offset)
}

#[cfg(not(any(unix, windows)))]
fn read_at(_file: &File, _offset: u64, _buffer: &mut [u8]) -> std::io::Result<usize> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "positioned reads are unsupported on this platform",
    ))
}

fn check_output(input: &Path, output: Option<&Path>) -> Result<()> {
    if let Some(destination) = output.and_then(|path| path.canonicalize().ok())
        && input
            .canonicalize()
            .is_ok_and(|source| source == destination)
    {
        return Err(Error::new(
            "INVALID_QUERY",
            "output must not replace an input file",
        ));
    }
    Ok(())
}

pub fn read_entry(at: &str, options: Options<'_>, perf: &mut Perf) -> Result<SelectedEntry> {
    check_output(options.input, options.output)?;
    let mut archive = perf.measure("container", || Archive::open(&options))?;
    let index = archive.select(at)?;
    let bytes = perf.measure("read", || archive.read(index, options.budget))?;
    let entry = &archive.entries[index];
    Ok(SelectedEntry {
        bytes,
        path: entry.path.clone(),
        at: entry.at.clone(),
    })
}

/// Select an exact path or the sole suffix match from one immutable archive buffer.
/// Select an exact path or the sole suffix match from one archive index.
fn unique_index(archive: &Archive, exact: Option<&str>, suffix: &str) -> Result<usize> {
    if let Some(path) = exact {
        if !path.ends_with(suffix) {
            return Err(Error::new(
                "INVALID_QUERY",
                format!("--entry must select a {suffix} path"),
            ));
        }
        return archive.select(path);
    }
    let matches: Vec<_> = archive
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.kind == "file" && entry.path.ends_with(suffix))
        .collect();
    match matches.as_slice() {
        [] => Err(Error::new(
            "NOT_FOUND",
            format!("container has no {suffix} entry"),
        )),
        [(index, _)] => Ok(*index),
        entries => {
            let candidates = entries
                .iter()
                .map(|(_, entry)| format!("{:?}", entry.path))
                .collect::<Vec<_>>();
            Err(Error::new(
                "AMBIGUOUS",
                format!(
                    "multiple {suffix} entries; select --entry from: {}",
                    candidates.join(", ")
                ),
            ))
        }
    }
}

/// Select one entry from a file-backed archive without loading the whole container.
pub fn read_unique_entry(
    input: &Path,
    exact: Option<&str>,
    suffix: &str,
    budget: Budget,
    perf: &mut Perf,
) -> Result<SelectedEntry> {
    let mut archive = perf.measure("container", || {
        Archive::open(&Options {
            input,
            output: None,
            budget,
        })
    })?;
    let index = unique_index(&archive, exact, suffix)?;
    let bytes = perf.measure("inflate", || archive.read(index, budget))?;
    let entry = &archive.entries[index];
    Ok(SelectedEntry {
        bytes,
        path: entry.path.clone(),
        at: entry.at.clone(),
    })
}

/// Read every regular entry whose path ends with `suffix`, in central-directory order.
pub fn read_matching(
    input: &Path,
    suffix: &str,
    budget: Budget,
    perf: &mut Perf,
) -> Result<Vec<SelectedEntry>> {
    let mut archive = perf.measure("container", || {
        Archive::open(&Options {
            input,
            output: None,
            budget,
        })
    })?;
    let mut selected = Vec::new();
    for index in 0..archive.entries.len() {
        let (path, at) = {
            let entry = &archive.entries[index];
            if entry.kind != "file" || !entry.path.ends_with(suffix) {
                continue;
            }
            (entry.path.clone(), entry.at.clone())
        };
        let bytes = perf.measure("inflate", || archive.read(index, budget))?;
        selected.push(SelectedEntry { bytes, path, at });
    }
    Ok(selected)
}

pub fn read_unique_entry_from_bytes(
    bytes: Vec<u8>,
    exact: Option<&str>,
    suffix: &str,
    budget: Budget,
    perf: &mut Perf,
) -> Result<SelectedEntry> {
    let mut archive = perf.measure("container", || Archive::from_bytes(bytes, budget))?;
    let index = unique_index(&archive, exact, suffix)?;
    let bytes = perf.measure("inflate", || archive.read(index, budget))?;
    let entry = &archive.entries[index];
    Ok(SelectedEntry {
        bytes,
        path: entry.path.clone(),
        at: entry.at.clone(),
    })
}

pub fn run(query: Query, options: Options<'_>, perf: &mut Perf) -> Result<Response> {
    check_output(options.input, options.output)?;
    let mut archive = perf.measure("container", || Archive::open(&options))?;
    match query {
        Query::Entries {
            contains,
            case_sensitive,
        } => {
            let filter = contains
                .as_deref()
                .map(|needle| rxd_core::filter::Contains::new(needle, case_sensitive));
            let mut result = Output::new(options.budget.output_bytes);
            perf.measure("serialize", || -> Result<()> {
                for entry in &archive.entries {
                    if filter.as_ref().is_some_and(|filter| !filter.matches(&entry.path)) { continue; }
                    result.json_line(&envelope::record("file", RecordMeta { at: Some(&entry.at), ..RecordMeta::default() },
                        json!({ "path":entry.path, "bytes":entry.bytes, "compressed_bytes":entry.compressed_bytes,
                            "kind":entry.kind, "method":entry.method, "compression":entry.compression(),
                            "encrypted":entry.encrypted(), "crc32":format!("{:08x}", entry.crc32),
                            "data_start":entry.data_start, "range_valid":entry.range_valid,
                            "path_valid":safe_path(&entry.path), "readable":entry.readable() }).as_object().unwrap().clone()))?;
                }
                Ok(())
            })?;
            perf.counters.records = Some(archive.entries.len());
            perf.counters.output_bytes = Some(result.len());
            Ok(Response::List(result.into_bytes()))
        }
        Query::Extract { at } => {
            let output = options.output.and_then(Path::to_str).ok_or_else(|| {
                Error::new("INVALID_QUERY", "extract requires a UTF-8 --output path")
            })?;
            let index = archive.select(&at)?;
            let bytes = perf.measure("read", || archive.read(index, options.budget))?;
            let entry = &archive.entries[index];
            let source_id = perf.measure("hash", || rxd_core::identity::input_id(&bytes));
            let mut receipt = Output::new(options.budget.output_bytes);
            perf.measure("serialize", || -> Result<()> {
                let mut record = envelope::record("file", RecordMeta { source_id: Some(&source_id), ..RecordMeta::default() },
                    json!({"complete":true, "file_bytes":bytes.len(), "path":output, "entry_path":entry.path,
                        "representation":"original", "compression":entry.compression(),
                        "origin":Origin {container:options.input.to_string_lossy().into_owned(), format:"zip", entry:entry.at.clone()} })
                        .as_object().unwrap().clone());
                envelope::attach_protocol(&mut record, "zip", "extract");
                receipt.json_line(&record)
            })?;
            if perf.enabled() {
                perf.source_id = Some(source_id);
            }
            perf.counters.input_bytes = Some(bytes.len());
            perf.counters.records = Some(1);
            perf.counters.output_bytes = Some(bytes.len() + receipt.len());
            Ok(Response::Extract {
                bytes,
                receipt: receipt.into_bytes(),
            })
        }
    }
}

/// Whole-archive extraction result.
pub struct Unpack {
    pub files: usize,
    pub bytes: u64,
}

fn unpack_path(path: &str) -> Result<std::path::PathBuf> {
    let path = path.strip_suffix('/').unwrap_or(path);
    if !safe_path(path) {
        return Err(invalid(format!("unsafe ZIP entry path: {path}")));
    }
    let mut relative = std::path::PathBuf::new();
    for part in path.split('/') {
        relative.push(part);
    }
    Ok(relative)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    std::io::Write::write_all(&mut file, bytes)?;
    Ok(())
}

struct Task {
    index: Option<usize>,
    relative: std::path::PathBuf,
}

/// Extract every regular Stored/Deflate file into a new directory.
pub fn unpack(input: &Path, destination: &Path, budget: Budget, perf: &mut Perf) -> Result<Unpack> {
    unpack_filtered(input, destination, budget, perf, 1, &|_| true)
}

/// Extract entries selected by `include` in parallel, preserving ZIP order for errors.
pub fn unpack_filtered(
    input: &Path,
    destination: &Path,
    budget: Budget,
    perf: &mut Perf,
    threads: usize,
    include: &(dyn Fn(&str) -> bool + Sync),
) -> Result<Unpack> {
    let mut archive = perf.measure("container", || {
        Archive::open(&Options {
            input,
            output: None,
            budget,
        })
    })?;
    let mut tasks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (index, entry) in archive.entries.iter().enumerate() {
        if !include(&entry.path) {
            continue;
        }
        let relative = unpack_path(&entry.path)?;
        let key = entry
            .path
            .strip_suffix('/')
            .unwrap_or(&entry.path)
            .to_owned();
        if !seen.insert(key.clone()) {
            return Err(invalid(format!("duplicate ZIP entry path: {key}")));
        }
        if entry.kind == "directory" {
            tasks.push(Task {
                index: None,
                relative,
            });
            continue;
        }
        if entry.kind != "file" || entry.encrypted() || !matches!(entry.method, 0 | 8) {
            return Err(Error::new(
                "UNSUPPORTED_INPUT",
                format!(
                    "cannot unpack ZIP entry {}: only regular unencrypted Stored/Deflate files are supported",
                    entry.path
                ),
            ));
        }
        tasks.push(Task {
            index: Some(index),
            relative,
        });
    }
    if tasks.is_empty() {
        return Ok(Unpack { files: 0, bytes: 0 });
    }
    let workers = threads.min(tasks.len()).max(1);
    if workers == 1 {
        let mut files = 0usize;
        let mut bytes = 0u64;
        for task in tasks {
            let target = destination.join(&task.relative);
            match task.index {
                None => perf.measure("write", || std::fs::create_dir_all(&target))?,
                Some(index) => {
                    let data = perf.measure("read", || archive.read(index, budget))?;
                    perf.measure("write", || write_file(&target, &data))?;
                    files += 1;
                    bytes = bytes.saturating_add(data.len() as u64);
                }
            }
        }
        return Ok(Unpack { files, bytes });
    }
    drop(archive);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .map_err(|error| {
            resource(format!(
                "cannot start {workers} extraction workers: {error}"
            ))
        })?;
    let chunk = tasks.len().div_ceil(workers);
    let started = perf.start_stage();
    let parts: Vec<Result<(usize, u64)>> = pool.install(|| {
        tasks
            .par_chunks(chunk)
            .map(|part| {
                let mut archive = Archive::open(&Options {
                    input,
                    output: None,
                    budget,
                })?;
                let mut files = 0usize;
                let mut bytes = 0u64;
                for task in part {
                    let target = destination.join(&task.relative);
                    match task.index {
                        None => std::fs::create_dir_all(&target)?,
                        Some(index) => {
                            let data = archive.read(index, budget)?;
                            write_file(&target, &data)?;
                            files += 1;
                            bytes = bytes.saturating_add(data.len() as u64);
                        }
                    }
                }
                Ok((files, bytes))
            })
            .collect()
    });
    perf.record("read", started);
    let mut files = 0usize;
    let mut bytes = 0u64;
    for part in parts {
        let (part_files, part_bytes) = part?;
        files += part_files;
        bytes = bytes.saturating_add(part_bytes);
    }
    Ok(Unpack { files, bytes })
}
