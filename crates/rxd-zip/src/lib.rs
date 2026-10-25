//! ZIP-family metadata and bounded, byte-exact selected-entry decoding.
mod archive;

use std::{io::Read, path::Path};

use archive::{invalid, resource, zip_error};
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

struct Archive {
    zip: zip::ZipArchive<archive::Reader>,
    entries: Vec<Entry>,
}

impl Archive {
    fn open(options: &Options<'_>) -> Result<Self> {
        let (reader, directory) = archive::open(options.input, options.budget)?;
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
        Ok(Self { zip, entries })
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
