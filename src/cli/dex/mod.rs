//! DEX CLI dispatch and payload output; diagnostics stay on stderr.

mod args;
mod output;
use output::emit;
use serde_json::{Value, json};

use anyhow::{Result, bail};
pub(crate) use args::Command;
use rayon::prelude::*;
use rxd_dex::{apk, manifest, query};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Instant;

/// Read optional inflation override; zero/invalid values preserve the supplied policy.
fn archive_policy_from_env(default: Option<usize>) -> apk::ArchivePolicy {
    archive_policy(
        std::env::var("RASC_MAX_INFLATED_ENTRY").ok().as_deref(),
        default,
    )
}

fn archive_policy(value: Option<&str>, default: Option<usize>) -> apk::ArchivePolicy {
    let mut policy = apk::ArchivePolicy {
        max_inflated_entry: default,
    };
    if let Some(value) = value
        && let Ok(bytes) = value.trim().parse::<usize>()
        && bytes != 0
    {
        policy.max_inflated_entry = Some(bytes);
    }
    policy
}
pub(super) fn execute(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    sink: &mut super::Output<'_>,
) -> rxd_core::Result<i32> {
    let budget = policy.for_file(command.input())?;
    super::restore_default_sigpipe();
    let (perf_on, name) = (command.perf(), command.name());
    let result = match run_command(command, archive_policy_from_env(budget.entry_bytes), sink) {
        Ok(()) => Ok(0),
        // A consumer that closed the pipe is a normal Unix idiom, not a failure.
        Err(error) if is_broken_pipe(&error) => Ok(0),
        Err(error) => Err(super::classify(error)),
    };
    emit_perf(perf_on, name);
    result
}

/// Whether an error is only a consumer that closed the pipe.
fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    })
}

/// Emit shared perf JSONL; archive-wide source identity is absent.
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
    sink: &mut super::Output<'_>,
) -> Result<()> {
    let limit = sink.remaining();
    rxd_dex::profile::enable(command.perf());
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
            // Unstable sort is deterministic because equal hits render identical rows.
            let sorted = scan_started.elapsed();

            let header = super::table::header(&["entry", "context", "match"]);
            if limit.is_some_and(|limit| header.len() > limit) {
                bail!("RESOURCE_LIMIT: query output byte budget exceeded");
            }
            let mut payload = header.to_owned();
            for hit in &hits {
                push_reference(
                    &mut payload,
                    hit.has_entry.then_some(hit.dex_name.as_str()),
                    &hit.member,
                    &hit.matched,
                    limit,
                )?;
            }
            if args.perf {
                rxd_dex::diag::diagnose(format_args!(
                    "[xref] rows={} scan={:.2} ms sort={:.2} ms assemble={:.2} ms payload={} MiB",
                    hits.len(),
                    scanned.as_secs_f64() * 1e3,
                    (sorted - scanned).as_secs_f64() * 1e3,
                    (scan_started.elapsed() - sorted).as_secs_f64() * 1e3,
                    payload.len() / (1 << 20)
                ));
            }
            emit(&payload, args.output_path(), sink)?;
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
            let listed = started.elapsed();

            const RENDER_CHUNK: usize = 8192;
            let mut pieces: Vec<String> = classes
                .par_chunks(RENDER_CHUNK)
                .map(|chunk| -> Result<_> {
                    let mut payload = String::with_capacity(chunk.len() * 256);
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
                        payload.push_str(&entry.descriptor);
                        payload.push('\n');
                    }
                    Ok(payload)
                })
                .collect::<Result<_>>()?;
            pieces.insert(0, super::table::header(&["at"]));
            emit(&pieces.concat(), args.output.as_deref(), sink)?;
            let payload_len = 0usize;
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
            let mut payload = super::table::header(&["at", "value", "entry"]);
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
                                    limit,
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
            emit(&payload, args.output.as_deref(), sink)?;
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
            emit(&source, args.output.as_deref(), sink)?;
            return Ok(());
        }
        Command::Fields(args) => {
            let selector = apk::MethodSelector {
                class: args.class.as_deref(),
                name: args.filter.as_deref(),
                exact_name: args.name.as_deref(),
                case_sensitive: args.case.sensitive(),
            };
            let (mut chunks, _) = apk::list_fields_with_policy(
                &args.apk_path,
                archive_policy,
                &selector,
                args.threads,
                args.perf,
            )?;
            chunks.insert(0, super::table::header(&["at"]));
            emit(&chunks.concat(), args.output.as_deref(), sink)?;
        }
        Command::Methods(args) => {
            let selector = apk::MethodSelector {
                class: args.class.as_deref(),
                name: args.filter.as_deref(),
                exact_name: args.name.as_deref(),
                case_sensitive: args.case.sensitive(),
            };
            let (mut chunks, _) = apk::list_methods_with_policy(
                &args.apk_path,
                archive_policy,
                &selector,
                args.threads,
                args.perf,
            )?;
            chunks.insert(0, super::table::header(&["at"]));
            emit(&chunks.concat(), args.output.as_deref(), sink)?;
        }
        Command::Entries(args) => {
            let entries = rxd_dex::profile::measure(rxd_dex::profile::Stage::ZipIndex, || {
                apk::list_entries(&args.apk_path)
            })?;
            rxd_dex::profile::count(rxd_dex::profile::Counter::Entries, entries.len() as u64);
            let mut payload = super::table::header(&[
                "name",
                "compressed_size",
                "uncompressed_size",
                "compression",
            ]);
            let pattern = args
                .filter
                .as_deref()
                .map(|needle| rxd_core::filter::Contains::new(needle, args.case.sensitive()));
            for entry in &entries {
                if pattern
                    .as_ref()
                    .is_some_and(|pattern| !pattern.matches(&entry.name))
                {
                    continue;
                }
                use super::table::{self, Cell};
                payload.push_str(&table::line(&[
                    Cell::Text(&entry.name),
                    Cell::Atom(&entry.compressed_size.to_string()),
                    Cell::Atom(&entry.uncompressed_size.to_string()),
                    Cell::Text(match entry.compression {
                        0 => "stored",
                        8 => "deflated",
                        _ => "other",
                    }),
                ])?);
            }
            emit(&payload, args.output.as_deref(), sink)?;
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
            let mut payload = String::new();
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
                    super::write_output(bytes, Some(&target), sink)?;
                    let path = target.to_str().ok_or_else(|| {
                        anyhow::anyhow!("INVALID_QUERY: output path must be UTF-8")
                    })?;
                    // Extraction receipts use full SHA-256, not sampled stream identity.
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
                        limit,
                    )?;
                    Ok(())
                },
            )?;
            emit(&payload, None, sink)?;
        }
        Command::Manifest(args) => {
            let data =
                apk::read_entry_with_policy(&args.apk_path, archive_policy, "AndroidManifest.xml")?;
            let xml = rxd_dex::profile::measure(rxd_dex::profile::Stage::Manifest, || {
                manifest::decode(&data)
            })?;
            emit(&xml, args.output.as_deref(), sink)?;
        }
        Command::Getclass(args) => {
            let at = args.selector.resolve()?;
            let class_name = query::format_class_name(&at)?;
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
            emit(&source, args.output.as_deref(), sink)?;
        }
    }
    Ok(())
}

/// Entry identity is retained even when the member coordinate is unambiguous.
fn push_reference(
    payload: &mut String,
    entry: Option<&str>,
    caller: &str,
    matched: &str,
    limit: Option<usize>,
) -> Result<()> {
    use super::table::{self, Cell};
    let row = table::line(&[
        entry.map_or(Cell::Null, Cell::Text),
        Cell::Atom(caller),
        Cell::Text(matched),
    ])?;
    if limit.is_some_and(|limit| row.len() > limit.saturating_sub(payload.len())) {
        bail!("RESOURCE_LIMIT: query output byte budget exceeded");
    }
    payload.push_str(&row);
    Ok(())
}
/// Append a string-table row without an intermediate JSON record.
fn push_string_row(
    payload: &mut String,
    entry: &str,
    index: u64,
    value: &str,
    limit: Option<usize>,
) -> Result<()> {
    use super::table::{self, Cell};
    let row = table::line(&[
        Cell::Atom(&index.to_string()),
        Cell::Text(value),
        Cell::Text(entry),
    ])?;
    payload.push_str(&row);
    if limit.is_some_and(|limit| payload.len() > limit) {
        bail!("RESOURCE_LIMIT: query output byte budget exceeded");
    }
    Ok(())
}

/// Accept only safe relative extraction paths; reject traversal and foreign separators.
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

fn append_json(payload: &mut String, value: &Value, limit: Option<usize>) -> Result<()> {
    let encoded = serde_json::to_string(value)?;
    if limit
        .is_some_and(|limit| encoded.len().saturating_add(1) > limit.saturating_sub(payload.len()))
    {
        bail!("RESOURCE_LIMIT: query output byte budget exceeded");
    }
    payload.push_str(&encoded);
    payload.push('\n');
    Ok(())
}
