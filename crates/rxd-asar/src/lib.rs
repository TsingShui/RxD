//! Bounded ASAR metadata enumeration and byte-exact file extraction.
//! Archive contents are never executed or interpreted as JavaScript.
mod archive;
mod local;

use std::{
    fs,
    io::{self, Write},
    path::Path,
};

use rxd_core::envelope::{self, RecordMeta};
use rxd_core::{Error, Limits, Origin, Result, perf::Perf};
use serde::Serialize;
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

struct Output {
    bytes: Vec<u8>,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Output {
    fn json(&mut self, value: &impl Serialize) -> Result<()> {
        serde_json::to_writer(&mut *self, value).map_err(|e| {
            Error::new(
                if e.is_io() {
                    "RESOURCE_LIMIT"
                } else {
                    "INVALID_INPUT"
                },
                e.to_string(),
            )
        })?;
        self.write_all(b"\n")
            .map_err(|_| Error::new("RESOURCE_LIMIT", "output byte budget exceeded"))
    }
}

fn storage(entry: &Entry) -> &'static str {
    match entry.storage {
        Storage::Packed { .. } => "packed",
        Storage::Unpacked => "unpacked",
        Storage::Link { .. } => "link",
    }
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
    let (mut archive, entries) = perf.measure("container", || {
        let metadata = fs::symlink_metadata(input)?;
        if !metadata.is_file() {
            return Err(invalid(
                "ASAR input must be a regular file, not an application directory or symlink",
            ));
        }
        archive::Archive::open(input, max_header_bytes, max_entries)
    })?;
    match query {
        Query::Entries {
            contains,
            case_sensitive,
        } => {
            perf.counters.records = Some(entries.len());
            let mut result = Output { bytes: Vec::new() };
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
                    result.json(&envelope::record(
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
            perf.counters.output_bytes = Some(result.bytes.len());
            Ok(Response::List(result.bytes))
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
            // Never replace the archive or an unpacked source while preparing an artifact.
            let mut sources = vec![input.to_owned()];
            if matches!(entry.storage, Storage::Unpacked) {
                let mut unpacked = input.as_os_str().to_owned();
                unpacked.push(".unpacked");
                sources.push(Path::new(&unpacked).join(&file));
            }
            if let Ok(destination) = fs::canonicalize(destination) {
                for source in sources {
                    if fs::canonicalize(source).is_ok_and(|source| source == destination) {
                        return Err(Error::new(
                            "INVALID_QUERY",
                            "output must not replace an input file",
                        ));
                    }
                }
            }
            let bytes = perf.measure("read", || archive.read(&file, entry, limits.input_bytes))?;
            perf.counters.input_bytes = Some(bytes.len());
            let source_id = perf.measure("hash", || rxd_core::identity::input_id(&bytes));
            if perf.enabled() {
                perf.source_id = Some(source_id.clone());
            }
            let mut receipt = Output { bytes: Vec::new() };
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
                receipt.json(&receipt_record)
            })?;
            perf.counters.records = Some(1);
            perf.counters.output_bytes = Some(bytes.len() + receipt.bytes.len());
            Ok(Response::Extract {
                bytes,
                receipt: receipt.bytes,
            })
        }
    }
}
