//! Bounded ASAR metadata enumeration and byte-exact file extraction.
//! Archive contents are never executed or interpreted as JavaScript.
mod archive;
mod local;

use std::{collections::BTreeMap, fs, path::Path};

use rxd_core::envelope::{self, RecordMeta};
use rxd_core::{Error, Limits, Origin, Result, output::Output, perf::Perf};
use serde_json::json;

use local::invalid;

pub enum Query {
    Entries {
        contains: Option<String>,
        /// Whether `contains` folds case; the CLI's `--case` decides it.
        case_sensitive: bool,
    },
    Extract {
        at: String,
    },
}

pub struct Options<'a> {
    pub input: &'a Path,
    pub output: Option<&'a Path>,
    pub limits: Limits,
    pub max_entries: usize,
    pub max_header_bytes: usize,
}

pub enum Response {
    List(Vec<u8>),
    Extract { bytes: Vec<u8>, receipt: Vec<u8> },
}

enum Storage {
    Packed { offset: u64 },
    Unpacked,
    Link { target: String },
}
struct Entry {
    size: u64,
    storage: Storage,
}

fn storage(entry: &Entry) -> &'static str {
    match entry.storage {
        Storage::Packed { .. } => "packed",
        Storage::Unpacked => "unpacked",
        Storage::Link { .. } => "link",
    }
}

fn open_archive(
    input: &Path,
    max_header_bytes: usize,
    max_entries: usize,
    perf: &mut Perf,
) -> Result<(archive::Archive, BTreeMap<String, Entry>)> {
    if input.to_str().is_none() {
        return Err(invalid("ASAR input path must be UTF-8"));
    }
    perf.measure("container", || {
        if !fs::symlink_metadata(input)?.is_file() {
            return Err(invalid(
                "ASAR input must be a regular file, not an application directory or symlink",
            ));
        }
        archive::Archive::open(input, max_header_bytes, max_entries)
    })
}

fn check_output(input: &Path, at: &str, entry: &Entry, output: Option<&Path>) -> Result<()> {
    let Some(destination) = output.and_then(|path| fs::canonicalize(path).ok()) else {
        return Ok(());
    };
    let mut sources = vec![input.to_owned()];
    if matches!(entry.storage, Storage::Unpacked) {
        let mut unpacked = input.as_os_str().to_owned();
        unpacked.push(".unpacked");
        sources.push(Path::new(&unpacked).join(at));
    }
    for source in sources {
        if fs::canonicalize(source).is_ok_and(|source| source == destination) {
            return Err(Error::new(
                "INVALID_QUERY",
                "output must not replace an input file",
            ));
        }
    }
    Ok(())
}

/// Read one selected entry into memory without publishing an extraction artifact.
/// Optional output is checked only to prevent a later query from replacing its source.
pub fn read_entry(at: &str, options: Options<'_>, perf: &mut Perf) -> Result<Vec<u8>> {
    local::validate_entry(at)?;
    let (mut archive, entries) = open_archive(
        options.input,
        options.max_header_bytes,
        options.max_entries,
        perf,
    )?;
    let entry = entries
        .get(at)
        .ok_or_else(|| invalid(format!("entry not found: {at}")))?;
    check_output(options.input, at, entry, options.output)?;
    perf.measure("read", || {
        archive.read(at, entry, options.limits.input_bytes)
    })
}

pub fn run(query: Query, options: Options<'_>, perf: &mut Perf) -> Result<Response> {
    let Options {
        input,
        output,
        limits,
        max_entries,
        max_header_bytes,
    } = options;
    let path = input
        .to_str()
        .ok_or_else(|| invalid("ASAR input path must be UTF-8"))?;
    if let Query::Extract { at: file } = &query {
        local::validate_entry(file)?;
    }
    let (mut archive, entries) = open_archive(input, max_header_bytes, max_entries, perf)?;
    match query {
        Query::Entries {
            contains,
            case_sensitive,
        } => {
            perf.counters.records = Some(entries.len());
            let mut result = Output::new(None);
            perf.measure("serialize", || -> Result<()> {
                for (path, entry) in &entries {
                    if contains.as_ref().is_some_and(|needle| {
                        !rxd_core::filter::Contains::new(needle, case_sensitive).matches(path)
                    }) {
                        continue;
                    }
                    let mut detail =
                        json!({"path":path, "bytes":entry.size, "storage":storage(entry)});
                    if matches!(entry.storage, Storage::Packed { .. }) {
                        let error = archive.range_error(entry);
                        detail["range_valid"] = json!(error.is_none());
                        if let Some(error) = error {
                            detail["range_error"] = json!(error.code());
                        }
                    }
                    if let Storage::Link { target } = &entry.storage {
                        detail["link"] = json!(target);
                    }
                    result.json_line(&envelope::record(
                        "file",
                        RecordMeta {
                            at: Some(path),
                            ..RecordMeta::default()
                        },
                        detail.as_object().unwrap().clone(),
                    ))?;
                }
                Ok(())
            })?;
            perf.counters.output_bytes = Some(result.len());
            Ok(Response::List(result.into_bytes()))
        }
        Query::Extract { at: file } => {
            let destination =
                output.ok_or_else(|| Error::new("INVALID_QUERY", "extract requires --output"))?;
            let destination_path = destination
                .to_str()
                .ok_or_else(|| invalid("output path must be UTF-8"))?;
            let entry = entries
                .get(&file)
                .ok_or_else(|| invalid(format!("entry not found: {file}")))?;
            check_output(input, &file, entry, Some(destination))?;
            let bytes = perf.measure("read", || archive.read(&file, entry, limits.input_bytes))?;
            perf.counters.input_bytes = Some(bytes.len());
            let source_id = perf.measure("hash", || rxd_core::identity::input_id(&bytes));
            if perf.enabled() {
                perf.source_id = Some(source_id.clone());
            }
            let mut receipt = Output::new(None);
            perf.measure("serialize", || -> Result<()> {
                let mut receipt_record = envelope::record(
                    "file",
                    RecordMeta {
                        source_id: Some(&source_id),
                        ..RecordMeta::default()
                    },
                    json!({
                        "complete":true,
                        "file_bytes":bytes.len(),
                        "path":destination_path,
                        "storage":storage(entry),
                        "representation":"original",
                        "origin":Origin { container:path.to_owned(), format:"asar", entry:file },
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                );
                envelope::attach_protocol(&mut receipt_record, "asar", "extract");
                receipt.json_line(&receipt_record)
            })?;
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

fn safe_member(path: &str) -> Result<std::path::PathBuf> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains(['\0', '\\', ':'])
        || path.starts_with('/')
    {
        return Err(invalid(format!("unsafe ASAR entry path: {path}")));
    }
    let mut relative = std::path::PathBuf::new();
    for part in path.split('/') {
        if part.is_empty() || matches!(part, "." | "..") {
            return Err(invalid(format!("unsafe ASAR entry path: {path}")));
        }
        relative.push(part);
    }
    Ok(relative)
}

fn write_member(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    std::io::Write::write_all(&mut file, bytes)?;
    Ok(())
}

/// Extract every regular file (and resolvable link) into a new directory.
pub fn unpack(
    input: &Path,
    destination: &Path,
    limits: Limits,
    max_entries: usize,
    max_header_bytes: usize,
    perf: &mut Perf,
) -> Result<Unpack> {
    let (mut archive, entries) = open_archive(input, max_header_bytes, max_entries, perf)?;
    let max_entry = limits.output_bytes.unwrap_or(rxd_core::MAX_SPAN_BYTES);
    let mut links = Vec::new();
    let mut files = 0usize;
    let mut bytes = 0u64;
    for (path, entry) in &entries {
        let relative = safe_member(path)?;
        let target = destination.join(relative);
        if let Storage::Link { target: link } = &entry.storage {
            links.push((path.clone(), link.clone()));
            continue;
        }
        let data = perf.measure("read", || archive.read(path, entry, max_entry))?;
        perf.measure("write", || write_member(&target, &data))?;
        files += 1;
        bytes = bytes.saturating_add(data.len() as u64);
    }
    for (path, link) in links {
        let source = safe_member(&link)?;
        let data = fs::read(destination.join(source))?;
        let target = destination.join(safe_member(&path)?);
        perf.measure("write", || write_member(&target, &data))?;
        files += 1;
        bytes = bytes.saturating_add(data.len() as u64);
    }
    Ok(Unpack { files, bytes })
}
