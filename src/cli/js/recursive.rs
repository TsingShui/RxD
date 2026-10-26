//! Bounded directory discovery and atomic multi-file queries.

use std::{
    fs,
    path::{Path, PathBuf},
};

use rayon::prelude::*;
use rxd_core::{Error, Limits, Result, perf::Perf};
use rxd_js::{Query, SourceMode, Syntax};
use serde_json::json;

fn at_path(path: &Path, error: Error) -> Error {
    Error::new(error.code, format!("{}: {}", path.display(), error.message))
}

/// A directory scan has no stream header, so each file opens its rows with a `file`
/// data record: the only thing that ties a file-scoped `at` to its input.
fn file_record(path: &Path, input_bytes: Option<usize>) -> Result<Vec<u8>> {
    let input_bytes =
        input_bytes.ok_or_else(|| Error::new("INTERNAL_ERROR", "missing analyzed input size"))?;
    let mut row = serde_json::to_vec(&json!({
        "record": "file",
        "detail": {
            "path": path.to_string_lossy(),
            "input_bytes": input_bytes,
        }
    }))
    .map_err(|error| Error::new("INTERNAL_ERROR", error.to_string()))?;
    row.push(b'\n');
    Ok(row)
}

fn discover(input: &Path, max_files: Option<usize>) -> Result<Vec<PathBuf>> {
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
            if max_files.is_some_and(|limit| files.len() >= limit) {
                return Err(Error::new(
                    "RESOURCE_LIMIT",
                    "recursive file budget exceeded",
                ));
            }
            files
                .try_reserve(1)
                .map_err(|_| Error::new("RESOURCE_LIMIT", "directory index allocation"))?;
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
    pub project_file: Option<&'a Path>,
    pub policy: rxd_core::BudgetPolicy,
    pub max_files: Option<usize>,
    pub threads: usize,
}

/// Inputs shared by every file of one scan.
struct Shared<'a> {
    mode: SourceMode,
    syntax: Syntax,
    query: &'a Query,
    policy: rxd_core::BudgetPolicy,
}

pub(super) fn run(request: Request<'_>, perf: &mut Perf) -> Result<Vec<u8>> {
    let Request {
        input,
        mode,
        syntax,
        query,
        project_file,
        policy,
        max_files,
        threads,
    } = request;
    let shared = Shared {
        mode,
        syntax,
        query: &query,
        policy,
    };
    // Validate even when discovery yields no files.
    if matches!(&query, Query::Find { filter, .. } | Query::Strings{filter:Some(filter),..} if filter.is_empty())
    {
        return Err(Error::new("INVALID_QUERY", "filter must not be empty"));
    }
    if let Query::Resources(resources) = &query {
        resources.validate()?;
    }
    match &query {
        Query::Calls { names, .. }
        | Query::Members { names, .. }
        | Query::Identifiers { names, .. }
        | Query::Symbols { names, .. } => names.validate()?,
        Query::Modules(q) => q.names.validate()?,
        _ => {}
    }
    if matches!(
        &query,
        Query::Outline {
            within: Some(_),
            ..
        } | Query::Calls {
            within: Some(_),
            ..
        } | Query::Members {
            within: Some(_),
            ..
        } | Query::Identifiers {
            within: Some(_),
            ..
        } | Query::Symbols {
            within: Some(_),
            ..
        }
    ) || matches!(&query, Query::Resources(resources) if resources.within.is_some())
        || matches!(&query, Query::Modules(q) if q.within.is_some())
    {
        return Err(Error::new(
            "INVALID_QUERY",
            "--within requires a single-file input",
        ));
    }
    let directory = fs::symlink_metadata(input)
        .map_err(|e| at_path(input, e.into()))?
        .is_dir();
    let files = perf
        .measure("discover", || discover(input, max_files))
        .map_err(|e| at_path(input, e))?;
    if let Some(file) = project_file {
        if file.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }) {
            return Err(Error::new(
                "INVALID_QUERY",
                "--file must be a relative path inside the snapshot",
            ));
        }
        let anchor = input.join(file);
        if !files.contains(&anchor) {
            return Err(Error::new(
                "NOT_FOUND",
                "--file is not a discovered JS file",
            ));
        }
        let Query::Refs { target } = &query else {
            return Err(Error::new("INVALID_QUERY", "--file requires xref"));
        };
        return rxd_js::project_xref(&files, &anchor, target, mode, syntax, policy, perf);
    }
    if matches!(
        &query,
        Query::Refs {
            target: rxd_js::RefsTarget::Coordinate(_) | rxd_js::RefsTarget::Byte(_)
        }
    ) {
        return Err(Error::new(
            "INVALID_QUERY",
            "directory coordinates require --file RELATIVE_PATH",
        ));
    }
    let mut output = Vec::new();
    let mut records = 0;
    if files.len() == 1 && !directory {
        run_serial(&shared, &files[0], false, perf, &mut records, &mut output)?;
    } else if files.len() == 1 {
        run_serial(&shared, &files[0], true, perf, &mut records, &mut output)?;
    } else {
        run_parallel(&shared, &files, threads, perf, &mut records, &mut output)?;
    }
    perf.counters.records = Some(records);
    perf.counters.output_bytes = Some(output.len());
    Ok(output)
}

/// A single discovered input keeps the direct path: no pool, no extra worker, and the
/// caller's exact remaining budgets.
fn run_serial(
    shared: &Shared<'_>,
    path: &Path,
    file_row: bool,
    perf: &mut Perf,
    records: &mut usize,
    output: &mut Vec<u8>,
) -> Result<()> {
    let remaining = Limits::from(shared.policy.for_file(path)?);
    perf.limits = Some(remaining);
    let previous = std::mem::take(&mut perf.counters);
    let result = rxd_js::run(
        path,
        shared.mode,
        shared.syntax,
        shared.query.clone(),
        remaining,
        perf,
    );
    let input_bytes = perf.counters.input_bytes;
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
    accumulate!(
        input_bytes,
        entities,
        matches,
        records,
        output_bytes,
        semantic_nodes,
        semantic_symbols,
        semantic_references
    );
    // A scan has multiple content identities, never the last file's identity.
    perf.source_id = None;
    let bytes = result.map_err(|e| at_path(path, e))?;
    if file_row {
        output.extend_from_slice(&file_record(path, input_bytes)?);
    }
    output.extend_from_slice(&bytes);
    Ok(())
}

/// Each task retains one input; indexed collection restores discovered path order.
fn run_parallel(
    shared: &Shared<'_>,
    files: &[PathBuf],
    threads: usize,
    perf: &mut Perf,
    records: &mut usize,
    output: &mut Vec<u8>,
) -> Result<()> {
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
                let result = (|| {
                    let limits = Limits::from(shared.policy.for_file(path)?);
                    shard.limits = Some(limits);
                    rxd_js::run(
                        path,
                        shared.mode,
                        shared.syntax,
                        shared.query.clone(),
                        limits,
                        &mut shard,
                    )
                })();
                (result, shard)
            })
            .collect()
    });
    for (path, (result, shard)) in files.iter().zip(results) {
        let bytes = result.map_err(|e| at_path(path, e))?;
        let file_records = shard.counters.records.unwrap_or(0);
        let input_bytes = shard.counters.input_bytes;
        *records += file_records;
        perf.merge(&shard);
        output.extend_from_slice(&file_record(path, input_bytes)?);
        output.extend_from_slice(&bytes);
    }
    // A scan has multiple content identities, never the last file's identity.
    perf.source_id = None;
    Ok(())
}
