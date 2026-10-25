//! CLI entry point: argument parsing, dispatch, output.
//!
//! stdout carries a command's payload and nothing else: every diagnostic,
//! including the `--perf` record, goes to stderr, so redirecting a result can never
//! capture debug lines. An `-o` file receives exactly the bytes stdout received.

mod args;
mod output;
use output::{emit, emit_rows};
use rxd_core::envelope::{header, is_wrapper, summary};
use serde_json::{Map, Value, json};

use anyhow::{Result, bail};
pub(crate) use args::Command;
use rayon::prelude::*;
use rxd_dex::{apk, manifest, query};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Instant;

/// Reads the optional per-entry inflation ceiling from the environment.
///
/// `0` and unparseable values preserve the built-in default, matching the existing CLI
/// contract. The returned policy is immutable and passed explicitly to archive operations.
fn archive_policy_from_env(default: usize) -> apk::ArchivePolicy {
    archive_policy(
        std::env::var("RASC_MAX_INFLATED_ENTRY").ok().as_deref(),
        default,
    )
}

fn archive_policy(value: Option<&str>, default: usize) -> apk::ArchivePolicy {
    let mut policy = apk::ArchivePolicy {
        max_inflated_entry: default,
    };
    if let Some(value) = value
        && let Ok(bytes) = value.trim().parse::<usize>()
        && bytes != 0
    {
        policy.max_inflated_entry = bytes;
    }
    policy
}
pub(super) fn execute(command: Command, policy: rxd_core::BudgetPolicy) -> rxd_core::Result<i32> {
    let budget = policy.for_file(command.input())?;
    restore_default_sigpipe();
    let (perf_on, name) = (command.perf(), command.name());
    let result = match run_command(
        command,
        archive_policy_from_env(budget.entry_bytes),
        Instant::now(),
    ) {
        Ok(code) => Ok(code),
        // A consumer that closed the pipe is a normal Unix idiom, not a failure.
        Err(error) if is_broken_pipe(&error) => Ok(0),
        Err(error) => Err(super::classify(error)),
    };
    emit_perf(perf_on, name);
    result
}

/// Execute in an isolated MCP worker, preserving domain errors for the typed adapter.
#[cfg(feature = "mcp")]
pub(crate) fn execute_query(command: Command, max_entry_bytes: usize) -> rxd_core::Result<i32> {
    let mut policy = apk::ArchivePolicy::default();
    policy.max_inflated_entry = policy.max_inflated_entry.min(max_entry_bytes);
    run_command(command, policy, Instant::now()).map_err(super::classify)
}

/// Whether an error is only a consumer that closed the pipe.
fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    })
}

/// Restore the default `SIGPIPE` disposition so that piping into `head`, `grep`,
/// or a closed consumer terminates quietly, the way standard Unix filters do.
///
/// Rust ignores `SIGPIPE` at startup, which turns an early-closed pipe into a
/// `BrokenPipe` write error and then a panic; with `panic = "abort"` that aborts
/// the process and prints a panic message for a completely normal shell idiom.
#[cfg(unix)]
fn restore_default_sigpipe() {
    // SAFETY: setting the disposition of SIGPIPE to SIG_DFL is
    // async-signal-safe and only affects this process.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_default_sigpipe() {}

/// Emit the shared perf record under `--perf`.
///
/// A DEX archive has no content hash to report (hashing 523 MiB would cost more than the
/// query), so `source_id` stays null and the archive counts ride in `extra`.
fn emit_perf(perf_on: bool, command: &'static str) {
    if !perf_on {
        return;
    }
    let mut perf = rxd_core::perf::Perf::new(true, "analysis", "dex", command);
    let (stages, counters) = rxd_dex::profile::values();
    for (stage, nanos) in stages {
        perf.stage_ns(stage, nanos);
    }
    let counter = |name: &str| {
        counters
            .iter()
            .find(|(label, _)| *label == name)
            .map(|(_, value)| *value as usize)
    };
    perf.counters.input_bytes = counter("bytes");
    perf.counters.entities = counter("classes");
    perf.counters.work = counter("code_items");
    perf.counters.matches = counter("hits");
    for name in ["entries", "dex", "classes", "code_items", "hits", "strings"] {
        if let Some(value) = counter(name) {
            perf.extra(name, value);
        }
    }
    perf.finish(None);
    perf.emit();
}

/// Runs the parsed command.
fn run_command(
    command: Command,
    archive_policy: apk::ArchivePolicy,
    _started: Instant,
) -> Result<i32> {
    rxd_dex::profile::enable(command.perf());
    // Only a member lookup has a verdict of its own (one hit, none, several); every
    // other command either produced its payload or failed.
    let status = 0;
    match command {
        Command::Xref(args) => {
            let target = args.target()?;
            let query = args.query(&target)?;
            let scan_started = Instant::now();
            let hits = apk::find_reference_hits_with_policy(
                &args.apk_path,
                archive_policy,
                &query,
                args.threads,
                args.perf,
            )?;
            let scanned = scan_started.elapsed();
            // The comparator is the string order, so an unstable sort produces exactly the
            // bytes a stable one would: equal lines are identical lines, and there is
            // nothing else to tie-break. Sorting a wide query's ~90k lines in parallel is
            // what keeps it off the main thread.
            #[cfg(feature = "mcp")]
            crate::query::check_records(hits.len())?;
            let sorted = scan_started.elapsed();
            // Assembling the payload line by line: a pre-sized buffer measured no faster
            // than letting the String grow (macOS's allocator re-extends the large block in
            // place), so the length pass is not worth it.
            let mut payload =
                stream_header("xref", std::slice::from_ref(&args.apk_path), target.json())?;
            for hit in &hits {
                push_reference(&mut payload, &hit.dex_name, &hit.member, &hit.matched)?;
            }
            finish_stream(&mut payload, hits.len())?;
            if args.perf {
                rxd_dex::diag::diagnose(format_args!(
                    "[xref] rows={} scan={:.2} ms sort={:.2} ms assemble={:.2} ms payload={} MiB",
                    payload.lines().count(),
                    scanned.as_secs_f64() * 1e3,
                    (sorted - scanned).as_secs_f64() * 1e3,
                    (scan_started.elapsed() - sorted).as_secs_f64() * 1e3,
                    payload.len() / (1 << 20)
                ));
            }
            emit(&payload, args.output_path())?;
        }
        Command::Classes(args) => {
            let filter = args
                .filter
                .as_deref()
                .map(|needle| rxd_core::filter::Contains::new(needle, args.case.sensitive()));
            // An exact class is one identity, so it skips the dotted-name buffer below.
            let class = args
                .class
                .as_deref()
                .map(query::format_class_name)
                .transpose()?;
            let started = Instant::now();
            let classes = apk::list_classes_with_policy(
                &args.apk_path,
                archive_policy,
                args.threads,
                args.perf,
            )?;
            #[cfg(feature = "mcp")]
            if crate::query::is_capturing() {
                crate::query::check_records(
                    classes
                        .iter()
                        .filter(|entry| {
                            class
                                .as_ref()
                                .is_none_or(|wanted| entry.descriptor == *wanted)
                                && filter.as_ref().is_none_or(|pattern| {
                                    pattern.matches(&entry.java_name().replace('/', "."))
                                })
                        })
                        .count(),
                )?;
            }
            let listed = started.elapsed();
            let header = stream_header(
                "classes",
                std::slice::from_ref(&args.apk_path),
                json!({"class":args.class,"filter":args.filter}),
            )?;
            // Keep the existing parallel, ordered chunk rendering for large class indexes.
            const RENDER_CHUNK: usize = 8192;
            let pieces: Vec<(String, usize)> = classes
                .par_chunks(RENDER_CHUNK)
                .map(|chunk| -> Result<_> {
                    let mut payload = String::with_capacity(chunk.len() * 256);
                    let mut records = 0;
                    // One reused buffer for the dotted name: the row itself carries only
                    // the descriptor, so the dotted form is computed for `--filter` alone.
                    let mut name = String::with_capacity(96);
                    for entry in chunk {
                        if class
                            .as_ref()
                            .is_some_and(|wanted| entry.descriptor != *wanted)
                        {
                            continue;
                        }
                        if let Some(pattern) = filter.as_ref() {
                            name.clear();
                            for character in entry.java_name().chars() {
                                name.push(if character == '/' { '.' } else { character });
                            }
                            if !pattern.matches(&name) {
                                continue;
                            }
                        }
                        // Only a class that departs from the root carries a `super`, so
                        // the default costs nothing on an index of hundreds of thousands.
                        match entry.super_descriptor.as_deref() {
                            Some(name) => push_record_at(
                                &mut payload,
                                "class",
                                Some(&entry.descriptor),
                                &[("super", Field::Text(name))],
                            )?,
                            None => {
                                push_record_at(&mut payload, "class", Some(&entry.descriptor), &[])?
                            }
                        }
                        records += 1;
                    }
                    Ok((payload, records))
                })
                .collect::<Result<_>>()?;
            let records = pieces.iter().map(|(_, count)| count).sum();
            let mut summary = String::new();
            finish_stream(&mut summary, records)?;
            let payload_len = emit_rows(
                std::iter::once(header)
                    .chain(pieces.into_iter().map(|(payload, _)| payload))
                    .chain(std::iter::once(summary)),
                args.output.as_deref(),
            )?;
            if args.perf {
                // The window now covers rendering and writing, not rendering alone.
                rxd_dex::diag::diagnose(format_args!(
                    "[classes] render={:.2} ms payload={} MiB (list {:.2} ms)",
                    started.elapsed().as_secs_f64() * 1e3 - listed.as_secs_f64() * 1e3,
                    payload_len / (1 << 20),
                    listed.as_secs_f64() * 1e3
                ));
            }
        }
        Command::Strings(args) => {
            let filter = args
                .filter
                .as_deref()
                .map(|needle| rxd_core::filter::Contains::new(needle, args.case.sensitive()));
            let strings = apk::list_strings_with_policy(
                &args.apk_path,
                archive_policy,
                args.threads,
                args.perf,
                filter.as_ref(),
                args.limit,
                args.offset,
            )?;
            #[cfg(feature = "mcp")]
            crate::query::check_records(strings.len())?;
            let mut payload = stream_header(
                "strings",
                std::slice::from_ref(&args.apk_path),
                json!({
                    "filter":args.filter, "offset":args.offset, "limit":args.limit,
                }),
            )?;
            {
                use rayon::prelude::*;
                // Render a bounded batch at a time: a whole-output piece vector would
                // hold a second copy of the 89 MiB payload at peak.
                const BATCH: usize = 96 * 1024;
                for start in (0..strings.len()).step_by(BATCH) {
                    let batch = &strings[start..(start + BATCH).min(strings.len())];
                    let pieces: Vec<String> = batch
                        .par_chunks(8192)
                        .map(|chunk| {
                            let mut out = String::with_capacity(chunk.len() * 104);
                            for entry in chunk {
                                push_string_row(
                                    &mut out,
                                    entry.dex_name.as_ref(),
                                    entry.index as u64,
                                    &entry.value,
                                )?;
                            }
                            Ok::<String, anyhow::Error>(out)
                        })
                        .collect::<Result<Vec<_>>>()?;
                    for piece in pieces {
                        payload.push_str(&piece);
                    }
                }
            }
            finish_stream(&mut payload, strings.len())?;
            emit(&payload, args.output.as_deref())?;
        }
        Command::GetMethod(args) => {
            let at = args.selector.resolve()?;
            let (class, name, prototype) = apk::parse_method_coordinate(&at)?;
            let source = apk::decompile_method_with_policy(
                &args.apk_path,
                archive_policy,
                &class,
                &name,
                &prototype,
                args.threads,
                args.perf,
            )?;
            let payload = fetch_record(
                "get-method",
                "method_body",
                &at,
                serde_json::json!({
                    "representation": "java",
                    "decompiled": true,
                    "complete": true,
                    "code": source,
                }),
            )?;
            emit(&payload, args.output.as_deref())?;
            return Ok(status);
        }
        Command::Fields(args) => {
            let selector = apk::MethodSelector {
                class: args.class.as_deref(),
                name: args.filter.as_deref(),
                exact_name: args.name.as_deref(),
                case_sensitive: args.case.sensitive(),
            };
            let (chunks, records) = apk::list_fields_with_policy(
                &args.apk_path,
                archive_policy,
                &selector,
                args.threads,
                args.perf,
            )?;
            let header = stream_header(
                "fields",
                std::slice::from_ref(&args.apk_path),
                json!({"class":args.class, "name":args.name, "filter":args.filter}),
            )?;
            let mut summary = String::new();
            finish_stream(&mut summary, records)?;
            emit_rows(
                std::iter::once(header)
                    .chain(chunks)
                    .chain(std::iter::once(summary)),
                args.output.as_deref(),
            )?;
        }
        Command::Methods(args) => {
            let selector = apk::MethodSelector {
                class: args.class.as_deref(),
                name: args.filter.as_deref(),
                exact_name: args.name.as_deref(),
                case_sensitive: args.case.sensitive(),
            };
            let (chunks, records) = apk::list_methods_with_policy(
                &args.apk_path,
                archive_policy,
                &selector,
                args.threads,
                args.perf,
            )?;
            let header = stream_header(
                "methods",
                std::slice::from_ref(&args.apk_path),
                json!({
                    "class":args.class, "filter":args.filter,
                }),
            )?;
            let mut summary = String::new();
            finish_stream(&mut summary, records)?;
            emit_rows(
                std::iter::once(header)
                    .chain(chunks)
                    .chain(std::iter::once(summary)),
                args.output.as_deref(),
            )?;
        }
        Command::Entries(args) => {
            let entries = rxd_dex::profile::measure(rxd_dex::profile::Stage::ZipIndex, || {
                apk::list_entries(&args.apk_path)
            })?;
            rxd_dex::profile::count(rxd_dex::profile::Counter::Entries, entries.len() as u64);
            #[cfg(feature = "mcp")]
            crate::query::check_records(entries.len())?;
            let mut payload = stream_header(
                "entries",
                std::slice::from_ref(&args.apk_path),
                json!({"filter":args.filter}),
            )?;
            let pattern = args
                .filter
                .as_deref()
                .map(|needle| rxd_core::filter::Contains::new(needle, args.case.sensitive()));
            let mut listed = 0usize;
            for entry in &entries {
                if pattern
                    .as_ref()
                    .is_some_and(|pattern| !pattern.matches(&entry.name))
                {
                    continue;
                }
                listed += 1;
                append_json(
                    &mut payload,
                    &data_record(
                        "entry",
                        json!({
                            "name":entry.name,
                            "compression":match entry.compression { 0 => "stored", 8 => "deflated", _ => "other" },
                            "compressed_size":entry.compressed_size,
                            "uncompressed_size":entry.uncompressed_size,
                        }),
                    ),
                )?;
            }
            finish_stream(&mut payload, listed)?;
            emit(&payload, args.output.as_deref())?;
        }
        Command::Extract(args) => {
            let destination = args
                .output
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("INVALID_QUERY: extract requires --output"))?;
            let entries = apk::list_entries(&args.apk_path)?;
            let pattern = args
                .filter
                .as_deref()
                .map(|needle| rxd_core::filter::Contains::new(needle, args.case.sensitive()));
            let names: Vec<String> = match &args.name {
                Some(name) => vec![name.clone()],
                None => entries
                    .iter()
                    .filter(|entry| {
                        pattern
                            .as_ref()
                            .is_none_or(|pattern| pattern.matches(&entry.name))
                    })
                    .map(|entry| entry.name.clone())
                    .collect(),
            };
            if names.is_empty() {
                bail!("NOT_FOUND: no entry matches the selection");
            }
            if args.all {
                if destination.exists() && !destination.is_dir() {
                    bail!("INVALID_QUERY: --all requires a directory for --output");
                }
                std::fs::create_dir_all(destination)?;
                let mut seen = std::collections::BTreeSet::new();
                for name in &names {
                    let relative = extract_relative(name)?;
                    if !seen.insert(relative) {
                        bail!("INVALID_QUERY: two entries extract to the same path");
                    }
                }
            } else if destination.is_dir() {
                bail!("INVALID_QUERY: --name requires a file for --output");
            }
            let mut payload = stream_header(
                "extract",
                std::slice::from_ref(&args.apk_path),
                json!({"name":args.name, "all":args.all, "filter":args.filter}),
            )?;
            let mut records = 0usize;
            apk::extract_entries_with_policy(
                &args.apk_path,
                archive_policy,
                &names,
                |entry, bytes| -> Result<()> {
                    let target = if args.all {
                        let target = destination.join(extract_relative(entry)?);
                        if let Some(parent) = target.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        target
                    } else {
                        destination.to_path_buf()
                    };
                    if same_file(&target, &args.apk_path) {
                        bail!("INVALID_QUERY: output must not replace the input");
                    }
                    super::write_output(bytes, Some(&target))?;
                    let path = target.to_str().ok_or_else(|| {
                        anyhow::anyhow!("INVALID_QUERY: output path must be UTF-8")
                    })?;
                    // The receipt names the exact bytes written, so it reports their full
                    // SHA-256 rather than the sampled stream identity in `source_id`.
                    let sha256 = format!("{:x}", Sha256::digest(bytes));
                    append_json(
                        &mut payload,
                        &data_record(
                            "file",
                            json!({
                                "entry":entry,
                                "path":path,
                                "file_bytes":bytes.len(),
                                "sha256":sha256,
                            }),
                        ),
                    )?;
                    records += 1;
                    Ok(())
                },
            )?;
            finish_stream(&mut payload, records)?;
            emit(&payload, None)?;
        }
        Command::Manifest(args) => {
            let data =
                apk::read_entry_with_policy(&args.apk_path, archive_policy, "AndroidManifest.xml")?;
            let xml = rxd_dex::profile::measure(rxd_dex::profile::Stage::Manifest, || {
                manifest::decode(&data)
            })?;
            emit(&xml, args.output.as_deref())?;
        }
        Command::Getclass(args) => {
            let at = args.selector.resolve()?;
            let class_name = query::format_class_name(&at)?;
            if status == 0 {
                let hit = apk::decompile_class_with_policy(
                    &args.apk_path,
                    archive_policy,
                    &class_name,
                    args.threads,
                    args.perf,
                )?;
                let Some((dex_name, source)) = hit else {
                    bail!(rxd_core::Error::failure(
                        rxd_core::Failure::NotFound,
                        format!("no class {class_name} in this input"),
                    ));
                };
                if args.perf {
                    rxd_dex::diag::diagnose(format_args!("[DEBUG] Hit DEX: {dex_name}"));
                    rxd_dex::diag::diagnose(format_args!("{}", "-".repeat(50)));
                }
                let payload = fetch_record(
                    "get-class",
                    "class_body",
                    &class_name,
                    serde_json::json!({
                        "representation": "java",
                        "decompiled": true,
                        "complete": true,
                        "code": source,
                    }),
                )?;
                emit(&payload, args.output.as_deref())?;
            }
        }
    }
    Ok(status)
}

/// One FETCH result: the source is a field of a record, never the whole answer, so a
/// consumer parses the same envelope as every other command.
fn fetch_record(
    command: &str,
    kind: &str,
    at: &str,
    detail: serde_json::Value,
) -> anyhow::Result<String> {
    let mut record = serde_json::Map::new();
    record.insert("record".into(), serde_json::json!(kind));
    record.insert("at".into(), serde_json::json!(at));
    record.insert("backend".into(), serde_json::json!("dex"));
    record.insert("command".into(), serde_json::json!(command));
    record.insert("schema_version".into(), serde_json::json!(2));
    // A DEX archive carries no content hash, so the record names no input identity.
    // Putting the carrier's size here would make the same DEX print two different records
    // depending on whether it sits in a bare file or an APK.
    record.insert("detail".into(), detail);
    let mut line = serde_json::to_string(&serde_json::Value::Object(record))?;
    line.push('\n');
    Ok(line)
}

/// A DEX data record. No DEX unit has a copy-only coordinate, so the envelope's six
/// nullable navigation keys would be `null` on every row; they are omitted instead of
/// repeated, and the header carries the stream-wide facts once.
/// One detail value written without a `serde_json::Value`.
enum Field<'a> {
    Text(&'a str),
    #[cfg(test)]
    Number(u64),
}

/// Append a JSON string literal, escaping exactly what JSON requires.
/// Append a reference row: the caller's coordinate plus the matched target.
///
/// An offset and a mnemonic were considered and dropped: nothing that consumes a hit
/// acts on the instruction itself, only on which method references what.
fn push_reference(payload: &mut String, entry: &str, caller: &str, matched: &str) -> Result<()> {
    payload.push_str("{\"record\":\"");
    payload.push_str(rxd_core::envelope::REFERENCE);
    payload.push_str("\",\"context_at\":");
    push_json_string(payload, caller);
    payload.push_str(",\"detail\":{\"target\":");
    push_json_string(payload, matched);
    // Which DEX the hit lives in is the one fact the row coordinate cannot express; an
    // APK can hold the same class name twice across DEX files.
    payload.push_str(",\"entry\":");
    push_json_string(payload, entry);
    payload.push_str("}}\n");
    #[cfg(feature = "mcp")]
    if crate::query::output_limit().is_some_and(|limit| payload.len() > limit) {
        bail!("RESOURCE_LIMIT: query output byte budget exceeded");
    }
    Ok(())
}
fn push_json_string(out: &mut String, value: &str) {
    // Almost every DEX string is plain UTF-8 without an escape-worthy byte; one
    // scan lets those skip the per-character match below.
    if !value
        .bytes()
        .any(|byte| byte < 0x20 || byte == b'"' || byte == b'\\')
    {
        out.push('"');
        out.push_str(value);
        out.push('"');
        return;
    }
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            control if (control as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// Append one `{record, detail}` row straight into the payload. A large archive renders
/// hundreds of thousands of rows, so building a `Value` per row only to serialize it is
/// pure allocation; this writes the same bytes in place.
/// One string row, written with its constant keys inline. A string listing is
/// hundreds of thousands of rows, so re-escaping the same five keys per row is pure
/// overhead; this keeps the bytes identical to [`push_record`].
fn push_string_row(payload: &mut String, entry: &str, index: u64, value: &str) -> Result<()> {
    payload.push_str("{\"record\":\"string\",\"detail\":{\"entry\":");
    push_json_string(payload, entry);
    payload.push_str(",\"index\":");
    use std::fmt::Write as _;
    let _ = write!(payload, "{index}");
    payload.push_str(",\"value\":");
    push_json_string(payload, value);
    payload.push_str("}}\n");
    #[cfg(feature = "mcp")]
    if crate::query::output_limit().is_some_and(|limit| payload.len() > limit) {
        bail!("RESOURCE_LIMIT: query output byte budget exceeded");
    }
    Ok(())
}

#[cfg(test)]
fn push_record(payload: &mut String, kind: &str, fields: &[(&str, Field<'_>)]) -> Result<()> {
    push_record_at(payload, kind, None, fields)
}

/// Append one row whose identity is its `at` coordinate.
///
/// `at` is what `--at` takes back, so a record that names a selectable object carries it
/// there instead of repeating it inside `detail`.
fn push_record_at(
    payload: &mut String,
    kind: &str,
    at: Option<&str>,
    fields: &[(&str, Field<'_>)],
) -> Result<()> {
    payload.push_str("{\"record\":");
    push_json_string(payload, kind);
    if let Some(at) = at {
        payload.push_str(",\"at\":");
        push_json_string(payload, at);
    }
    payload.push_str(",\"detail\":{");
    for (index, (key, value)) in fields.iter().enumerate() {
        if index > 0 {
            payload.push(',');
        }
        push_json_string(payload, key);
        payload.push(':');
        match value {
            Field::Text(text) => push_json_string(payload, text),
            #[cfg(test)]
            Field::Number(number) => payload.push_str(&number.to_string()),
        }
    }
    payload.push_str("}}\n");
    #[cfg(feature = "mcp")]
    if crate::query::output_limit().is_some_and(|limit| payload.len() > limit) {
        bail!("RESOURCE_LIMIT: query output byte budget exceeded");
    }
    Ok(())
}

/// A safe destination path for an archive entry written under a directory.
///
/// Entry names are attacker-controlled, so only ordinary relative components are
/// accepted: no absolute paths, `..`, root/prefix components, or Windows separators.
fn extract_relative(name: &str) -> Result<std::path::PathBuf> {
    use std::path::Component;
    if name.is_empty() || name.contains('\\') {
        bail!("INVALID_QUERY: unsafe entry path {name}");
    }
    let mut relative = std::path::PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            _ => bail!("INVALID_QUERY: unsafe entry path {name}"),
        }
    }
    if relative.as_os_str().is_empty() {
        bail!("INVALID_QUERY: unsafe entry path {name}");
    }
    Ok(relative)
}

/// Whether writing `destination` would replace the analysed input.
fn same_file(destination: &Path, input: &Path) -> bool {
    let input = std::fs::canonicalize(input).ok();
    let resolved = std::fs::canonicalize(destination).ok().or_else(|| {
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        Some(
            std::fs::canonicalize(parent)
                .ok()?
                .join(destination.file_name()?),
        )
    });
    input
        .zip(resolved)
        .is_some_and(|(input, destination)| input == destination)
}

fn data_record(kind: &str, detail: Value) -> Value {
    json!({"record": kind, "detail": detail})
}

fn append_json(payload: &mut String, value: &Value) -> Result<()> {
    if is_wrapper(value) {
        return Ok(());
    }
    let encoded = serde_json::to_string(value)?;
    #[cfg(feature = "mcp")]
    if crate::query::output_limit()
        .is_some_and(|limit| encoded.len().saturating_add(1) > limit.saturating_sub(payload.len()))
    {
        bail!("RESOURCE_LIMIT: query output byte budget exceeded");
    }
    payload.push_str(&encoded);
    payload.push('\n');
    Ok(())
}

fn stream_header(command: &str, inputs: &[std::path::PathBuf], query: Value) -> Result<String> {
    let input_bytes = inputs
        .iter()
        .try_fold(0usize, |total, path| -> Result<usize> {
            let bytes = usize::try_from(std::fs::metadata(path)?.len())?;
            total
                .checked_add(bytes)
                .ok_or_else(|| anyhow::anyhow!("input byte count overflow"))
        })?;
    let mut payload = String::new();
    append_json(
        &mut payload,
        &header("dex", command, None, Some(input_bytes), query, Map::new()),
    )?;
    Ok(payload)
}

fn finish_stream(payload: &mut String, records: usize) -> Result<()> {
    append_json(payload, &summary(records, true, Map::new()))
}

#[cfg(test)]
mod json_writer {
    use super::*;

    /// The hand-written writer must produce exactly the bytes `serde_json` would, for
    /// every string a record can carry (entry names, descriptors, member names).
    #[test]
    fn json_string_writer_matches_serde_json() {
        let mut cases = vec![
            String::new(),
            "plain".to_owned(),
            "quote\"inside".to_owned(),
            "back\\slash".to_owned(),
            "tab\tnewline\ncr\r".to_owned(),
            "bell\u{8}form\u{c}".to_owned(),
            "low\u{1}controls\u{1f}".to_owned(),
            "del\u{7f}keep".to_owned(),
            "λ|🙂|世界".to_owned(),
            "/slash/and;semicolon".to_owned(),
            "line\u{2028}sep\u{2029}".to_owned(),
            "classes|\"\n.dex".to_owned(),
        ];
        // Deterministic fuzz over the byte classes that matter for escaping.
        let alphabet: Vec<char> = "\"\\/bfnrtu0123456789abcλ🙂\n\r\t\u{0}\u{1}\u{1f}\u{7f}\u{7e}"
            .chars()
            .collect();
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..2_000 {
            let mut next = || {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                (seed >> 33) as usize
            };
            let len = next() % 24;
            let mut value = String::new();
            for _ in 0..len {
                value.push(alphabet[next() % alphabet.len()]);
            }
            cases.push(value);
        }
        for case in cases {
            let mut out = String::new();
            push_json_string(&mut out, &case);
            assert_eq!(
                out,
                serde_json::to_string(&case).unwrap(),
                "escaped bytes differ for {case:?}"
            );
        }
    }

    /// A whole row parses back to the same JSON object the `Value` path produced.
    #[test]
    fn record_writer_matches_value_path() {
        let mut out = String::new();
        push_record(
            &mut out,
            "class",
            &[
                ("entry", Field::Text("classes|\"\n.dex")),
                ("descriptor", Field::Text("La/a$a;")),
                ("index", Field::Number(7)),
            ],
        )
        .unwrap();
        let row: Value = serde_json::from_str(out.trim_end()).unwrap();
        let expected = data_record(
            "class",
            json!({"entry":"classes|\"\n.dex", "descriptor":"La/a$a;", "index":7}),
        );
        assert_eq!(row, expected);
        assert!(out.ends_with("}\n"));
    }
}
