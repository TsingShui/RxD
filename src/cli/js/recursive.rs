//! Bounded directory discovery and atomic multi-file queries.

use std::{
    fs,
    path::{Path, PathBuf},
};

use rayon::prelude::*;
use rxd_core::envelope::{SCHEMA_VERSION, summary};
use rxd_core::{Error, Limits, Result, perf::Perf};
use rxd_js::{Query, SourceMode, Syntax};
use serde_json::json;

fn at_path(path: &Path, error: Error) -> Error {
    Error::new(error.code, format!("{}: {}", path.display(), error.message))
}

fn discover(input: &Path, max_files: usize) -> Result<Vec<PathBuf>> {
    let metadata = fs::symlink_metadata(input).map_err(Error::from)?;
    if metadata.is_file() {
        return Ok(vec![input.to_owned()]);
    }
    if !metadata.is_dir() {
        return Err(Error::new(
            "INVALID_INPUT",
            "recursive input must be a regular file or directory, not a symlink",
        ));
    }
    let mut files = Vec::new();
    // Retain only one iterator per level, not an unbounded directory worklist.
    let mut stack = vec![(input.to_owned(), fs::read_dir(input)?)];
    while let Some((directory, entries)) = stack.last_mut() {
        let Some(entry) = entries.next() else {
            stack.pop();
            continue;
        };
        let entry = entry.map_err(|e| at_path(directory, e.into()))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| at_path(&path, e.into()))?;
        if kind.is_dir() {
            if stack.len() >= 256 {
                return Err(Error::new(
                    "RESOURCE_LIMIT",
                    "directory depth budget exceeded",
                ));
            }
            let entries = fs::read_dir(&path).map_err(|e| at_path(&path, e.into()))?;
            stack.push((path, entries));
        } else if kind.is_file()
            && matches!(
                path.extension().and_then(|s| s.to_str()),
                Some("js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx")
            )
        {
            if files.len() >= max_files {
                return Err(Error::new(
                    "RESOURCE_LIMIT",
                    "recursive file budget exceeded",
                ));
            }
            files.push(path);
        }
        // Skip symlinks (including directory links) and special files.
    }
    files.sort();
    Ok(files)
}

/// One directory-or-file scan request. Grouped so every helper keeps a readable signature.
pub(super) struct Request<'a> {
    pub input: &'a Path,
    pub mode: SourceMode,
    pub syntax: Syntax,
    pub query: Query,
    pub limits: Limits,
    pub max_files: usize,
    pub threads: usize,
}

/// Inputs shared by every file of one scan.
struct Shared<'a> {
    mode: SourceMode,
    syntax: Syntax,
    query: &'a Query,
    limits: Limits,
}

pub(super) fn run(request: Request<'_>, perf: &mut Perf) -> Result<Vec<u8>> {
    let Request {
        input,
        mode,
        syntax,
        query,
        limits,
        max_files,
        threads,
    } = request;
    let shared = Shared {
        mode,
        syntax,
        query: &query,
        limits,
    };
    // Validate even when discovery yields no files.
    if matches!(&query, Query::Find { filter, .. } if filter.is_empty()) {
        return Err(Error::new("INVALID_QUERY", "filter must not be empty"));
    }
    if matches!(&query, Query::Identifiers { name, .. } if name.is_empty()) {
        return Err(Error::new("INVALID_QUERY", "name must not be empty"));
    }
    if matches!(
        &query,
        Query::Outline {
            within: Some(_),
            ..
        }
    ) {
        return Err(Error::new(
            "INVALID_QUERY",
            "--within requires a single-file input",
        ));
    }
    let files = perf
        .measure("discover", || discover(input, max_files))
        .map_err(|e| at_path(input, e))?;
    let mut output = Vec::new();
    let mut records = 0;
    if files.len() == 1 {
        run_serial(&shared, &files[0], perf, &mut records, &mut output)?;
    } else {
        run_parallel(&shared, &files, threads, perf, &mut records, &mut output)?;
    }
    let summary = perf
        .measure("serialize", || {
            let mut scan_summary = summary(
                records,
                true,
                json!({"files":files.len()}).as_object().unwrap().clone(),
            );
            scan_summary["record"] = json!("scan_summary");
            scan_summary["schema_version"] = json!(SCHEMA_VERSION);
            scan_summary["backend"] = json!("js");
            scan_summary["command"] = json!(query.name());
            serde_json::to_vec(&scan_summary)
        })
        .map_err(|e| Error::new("FORMAT_ERROR", e.to_string()))?;
    if summary.len() + 1 > limits.output_bytes - output.len() {
        return Err(Error::new("RESOURCE_LIMIT", "output byte budget exceeded"));
    }
    output.extend_from_slice(&summary);
    output.push(b'\n');
    perf.counters.records = Some(records);
    perf.counters.output_bytes = Some(output.len());
    Ok(output)
}

/// A single discovered input keeps the direct path: no pool, no extra worker, and the
/// caller's exact remaining budgets.
fn run_serial(
    shared: &Shared<'_>,
    path: &Path,
    perf: &mut Perf,
    records: &mut usize,
    output: &mut Vec<u8>,
) -> Result<()> {
    let limits = shared.limits;
    let remaining = Limits {
        records: limits.records - *records,
        output_bytes: limits.output_bytes - output.len(),
        ..limits
    };
    let previous = std::mem::take(&mut perf.counters);
    let result = rxd_js::run(
        path,
        shared.mode,
        shared.syntax,
        shared.query.clone(),
        remaining,
        perf,
    );
    *records += perf.counters.records.unwrap_or(0);
    // Timings already accumulate in Perf; counters need explicit summation.
    macro_rules! accumulate {
        ($($field:ident),*) => {
            $(perf.counters.$field = match (previous.$field, perf.counters.$field) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
            };)*
        };
    }
    accumulate!(input_bytes, entities, matches, records, output_bytes);
    // A scan has multiple content identities, never the last file's identity.
    perf.source_id = None;
    let bytes = result.map_err(|e| at_path(path, e))?;
    output.extend_from_slice(&bytes);
    Ok(())
}

/// One file per task, bounded by the worker count: each worker holds a single parsed input
/// and its AST, so the pool size is what caps in-flight memory.
///
/// `collect` on an indexed parallel iterator restores the discovered path order, so the
/// stream, its budgets and its error attribution stay byte-identical to a serial scan. Each
/// file runs with the full budget and the aggregate is checked in order afterwards, which
/// reports the same first offending path a serial scan would.
fn run_parallel(
    shared: &Shared<'_>,
    files: &[PathBuf],
    threads: usize,
    perf: &mut Perf,
    records: &mut usize,
    output: &mut Vec<u8>,
) -> Result<()> {
    let limits = shared.limits;
    let metrics = perf.enabled();
    let command = shared.query.name();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        // Parsing and the semantic front end recurse; keep the depth the single-file path gets.
        .stack_size(32 * 1024 * 1024)
        .build()
        .map_err(|error| Error::new("RESOURCE_LIMIT", error.to_string()))?;
    let results: Vec<(Result<Vec<u8>>, Perf)> = pool.install(|| {
        files
            .par_iter()
            .map(|path| {
                let mut shard = Perf::new(metrics, "analysis", "js", command);
                shard.limits = perf.limits;
                let result = rxd_js::run(
                    path,
                    shared.mode,
                    shared.syntax,
                    shared.query.clone(),
                    limits,
                    &mut shard,
                );
                (result, shard)
            })
            .collect()
    });
    for (path, (result, shard)) in files.iter().zip(results) {
        let bytes = result.map_err(|e| at_path(path, e))?;
        let file_records = shard.counters.records.unwrap_or(0);
        if *records + file_records > limits.records {
            return Err(at_path(
                path,
                Error::new("RESOURCE_LIMIT", "output record budget exceeded"),
            ));
        }
        if output.len() + bytes.len() > limits.output_bytes {
            return Err(at_path(
                path,
                Error::new("RESOURCE_LIMIT", "output byte budget exceeded"),
            ));
        }
        *records += file_records;
        perf.merge(&shard);
        output.extend_from_slice(&bytes);
    }
    // A scan has multiple content identities, never the last file's identity.
    perf.source_id = None;
    Ok(())
}
