//! CLI entry point: argument parsing, dispatch, output.
//!
//! stdout carries a command's payload and nothing else: every diagnostic,
//! including `--debug` timing, goes to stderr, so redirecting a result can never
//! capture debug lines. An `-o` file receives exactly the bytes stdout received.

mod args;
mod output;
use output::{emit, emit_rows};
use rxd_core::envelope::{RecordMeta, SCHEMA_VERSION, header, record, summary};
use serde_json::{Map, Value, json};

use anyhow::{Result, bail};
pub(crate) use args::Command;
use rayon::prelude::*;
use rxd_dex::{apk, manifest, query};
use std::time::Instant;

/// Reads the optional per-entry inflation ceiling from the environment.
///
/// `0` and unparseable values preserve the built-in default, matching the existing CLI
/// contract. The returned policy is immutable and passed explicitly to archive operations.
fn archive_policy_from_env() -> apk::ArchivePolicy {
    archive_policy(std::env::var("RASC_MAX_INFLATED_ENTRY").ok().as_deref())
}

fn archive_policy(value: Option<&str>) -> apk::ArchivePolicy {
    let mut policy = apk::ArchivePolicy::default();
    if let Some(value) = value
        && let Ok(bytes) = value.trim().parse::<usize>()
        && bytes != 0
    {
        policy.max_inflated_entry = bytes;
    }
    policy
}
pub(super) fn execute(command: Command) -> i32 {
    restore_default_sigpipe();
    match run_command(command, archive_policy_from_env(), Instant::now()) {
        Ok(code) => code,
        Err(error) if is_broken_pipe(&error) => 0,
        Err(error) => {
            eprintln!("Error: {error:#}");
            1
        }
    }
}

/// Execute in an isolated MCP worker, preserving domain errors for the typed adapter.
#[cfg(feature = "mcp")]
pub(crate) fn execute_query(command: Command, max_entry_bytes: usize) -> rxd_core::Result<i32> {
    let mut policy = apk::ArchivePolicy::default();
    policy.max_inflated_entry = policy.max_inflated_entry.min(max_entry_bytes);
    run_command(command, policy, Instant::now()).map_err(|error| {
        let message = format!("{error:#}");
        let code = if message.contains("RESOURCE_LIMIT") || message.contains("inflated entry") {
            "RESOURCE_LIMIT"
        } else if message.contains("INVALID_QUERY") {
            "INVALID_QUERY"
        } else if message.contains("not found") || message.contains("not defined") {
            "NOT_FOUND"
        } else {
            "INVALID_INPUT"
        };
        rxd_core::Error::new(code, message)
    })
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

/// This is the only place the CLI reports timing, so `--debug` reads the same
/// for every subcommand.
fn debug_timing(started: Instant) {
    rxd_dex::diag::diagnose(format_args!(
        "[DEBUG] Total Execution Time: {:.2} us",
        started.elapsed().as_secs_f64() * 1_000_000.0
    ));
}

/// Runs the parsed command.
fn run_command(
    command: Command,
    archive_policy: apk::ArchivePolicy,
    started: Instant,
) -> Result<i32> {
    // Only a member lookup has a verdict of its own (one hit, none, several); every
    // other command either produced its payload or failed.
    let mut status = 0;
    match command {
        Command::Findrefs(args) => {
            let query = args.query()?;
            let scan_started = Instant::now();
            let mut hits = apk::find_reference_hits_with_policy(
                &args.apk_path,
                archive_policy,
                &query,
                args.threads,
                args.debug,
            )?;
            let scanned = scan_started.elapsed();
            // The comparator is the string order, so an unstable sort produces exactly the
            // bytes a stable one would: equal lines are identical lines, and there is
            // nothing else to tie-break. Sorting a wide query's ~90k lines in parallel is
            // what keeps it off the main thread.
            #[cfg(feature = "mcp")]
            crate::query::check_records(hits.len())?;
            hits.par_sort_unstable();
            let sorted = scan_started.elapsed();
            // Assembling the payload line by line: a pre-sized buffer measured no faster
            // than letting the String grow (macOS's allocator re-extends the large block in
            // place), so the length pass is not worth it.
            let mut payload = stream_header(
                "findrefs",
                std::slice::from_ref(&args.apk_path),
                args.query_json(),
            )?;
            for hit in &hits {
                append_json(
                    &mut payload,
                    &data_record(
                        "reference",
                        json!({
                            "entry":hit.dex_name,
                            "class":hit.member.split_once("->").map(|(class, _)| class),
                            "member":hit.member,
                            "matched":hit.matched,
                        }),
                    ),
                )?;
            }
            finish_stream(&mut payload, hits.len())?;
            if args.debug {
                rxd_dex::diag::diagnose(format_args!(
                    "[findrefs] rows={} scan={:.2} ms sort={:.2} ms assemble={:.2} ms payload={} MiB",
                    payload.lines().count(),
                    scanned.as_secs_f64() * 1e3,
                    (sorted - scanned).as_secs_f64() * 1e3,
                    (scan_started.elapsed() - sorted).as_secs_f64() * 1e3,
                    payload.len() / (1 << 20)
                ));
            }
            emit(&payload, args.output_path())?;
            if args.debug {
                debug_timing(started);
            }
        }
        Command::FieldsPlan(args) => {
            // Accept what a user types; the DEX carries descriptors.
            let descriptor = query::format_class_name(&args.descriptor)?;
            let lookup =
                apk::field_plan_with_policy(&args.apk, archive_policy, &descriptor, args.threads)?;
            match &lookup.plan {
                Some(plan) => {
                    let mut detail: Value = serde_json::from_str(plan)?;
                    detail.as_object_mut().unwrap().remove("schema");
                    let mut result = data_record("field_plan", detail);
                    result["backend"] = json!("dex");
                    result["command"] = json!("fields-plan");
                    result["schema_version"] = json!(SCHEMA_VERSION);
                    emit(
                        &format!("{}\n", serde_json::to_string(&result)?),
                        args.output.as_deref(),
                    )?;
                }
                None if lookup.definitions.is_empty() => {
                    eprintln!("Error: {descriptor} is not defined in the supplied code inputs");
                }
                None => eprintln!(
                    "Error: {descriptor} is defined {} times ({}); a plan must name one definition",
                    lookup.definitions.len(),
                    lookup.definitions.join(", ")
                ),
            }
            #[cfg(feature = "mcp")]
            if lookup.plan.is_none() && crate::query::is_capturing() {
                emit(
                    &format!("{}\n", json!({"definitions":lookup.definitions})),
                    None,
                )?;
            }
            status = apk::lookup_status(lookup.definitions.len());
            if args.debug {
                debug_timing(started);
            }
        }
        Command::MemberByIndex(args) => {
            let descriptor = query::format_class_name(&args.descriptor)?;
            // clap's group already rejects both or neither; this keeps the arm total.
            let (kind, index, name) = match (args.method_index, args.field_index) {
                (Some(index), None) => (apk::MemberKind::Method, index, "method"),
                (None, Some(index)) => (apk::MemberKind::Field, index, "field"),
                _ => bail!("pass exactly one of --method-index or --field-index"),
            };
            let lookup = apk::member_by_index_with_policy(
                &args.apk,
                archive_policy,
                &descriptor,
                index,
                kind,
                args.threads,
            )?;
            #[cfg(feature = "mcp")]
            crate::query::check_records(lookup.candidates.len())?;
            let mut payload = stream_header(
                "member-by-index",
                &args.apk,
                json!({
                    "descriptor":descriptor, "method_index":args.method_index, "field_index":args.field_index,
                }),
            )?;
            for candidate in &lookup.candidates {
                append_json(&mut payload, &member_record(candidate))?;
            }
            finish_stream(&mut payload, lookup.candidates.len())?;
            emit(&payload, args.output.as_deref())?;
            report_member_verdict(&lookup, name, index, &descriptor);
            status = apk::lookup_status(lookup.candidates.len());
            if args.debug {
                debug_timing(started);
            }
        }
        Command::Classes(args) => {
            let filter = args.filter.as_deref().map(str::to_lowercase);
            let started = Instant::now();
            let classes = apk::list_classes_with_policy(
                &args.apk_path,
                archive_policy,
                args.threads,
                args.debug,
            )?;
            #[cfg(feature = "mcp")]
            if crate::query::is_capturing() {
                crate::query::check_records(
                    classes
                        .iter()
                        .filter(|class| {
                            filter.as_ref().is_none_or(|pattern| {
                                class
                                    .java_name()
                                    .replace('/', ".")
                                    .to_lowercase()
                                    .contains(pattern)
                            })
                        })
                        .count(),
                )?;
            }
            let listed = started.elapsed();
            let header = stream_header(
                "classes",
                std::slice::from_ref(&args.apk_path),
                json!({"filter":args.filter}),
            )?;
            // Keep the existing parallel, ordered chunk rendering for large class indexes.
            const RENDER_CHUNK: usize = 8192;
            let pieces: Vec<(String, usize)> = classes.par_chunks(RENDER_CHUNK).map(|chunk| -> Result<_> {
                let mut payload = String::with_capacity(chunk.len() * 256);
                let mut records = 0;
                for class in chunk {
                    let name = class.java_name().replace('/', ".");
                    if filter.as_ref().is_some_and(|pattern| !name.to_lowercase().contains(pattern)) {
                        continue;
                    }
                    append_json(&mut payload, &data_record("class", json!({
                        "entry":class.dex_name.as_ref(), "descriptor":class.descriptor,
                        "name":name, "package":class.package(), "simple_name":class.simple_name(),
                    })))?;
                    records += 1;
                }
                Ok((payload, records))
            }).collect::<Result<_>>()?;
            let records = pieces.iter().map(|(_, count)| count).sum();
            let mut summary = String::new();
            finish_stream(&mut summary, records)?;
            let payload_len = emit_rows(
                std::iter::once(header)
                    .chain(pieces.into_iter().map(|(payload, _)| payload))
                    .chain(std::iter::once(summary)),
                args.output.as_deref(),
            )?;
            if args.debug {
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
            let strings = apk::list_strings_with_policy(
                &args.apk_path,
                archive_policy,
                args.threads,
                args.debug,
                args.filter.as_deref(),
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
            for entry in &strings {
                append_json(
                    &mut payload,
                    &data_record(
                        "string",
                        json!({
                            "entry":entry.dex_name.as_ref(), "index":entry.index, "value":entry.value, "class":null,
                        }),
                    ),
                )?;
            }
            finish_stream(&mut payload, strings.len())?;
            emit(&payload, args.output.as_deref())?;
        }
        Command::Entries(args) => {
            let entries = apk::list_entries(&args.apk_path)?;
            #[cfg(feature = "mcp")]
            crate::query::check_records(entries.len())?;
            let mut payload =
                stream_header("entries", std::slice::from_ref(&args.apk_path), json!({}))?;
            for entry in &entries {
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
            finish_stream(&mut payload, entries.len())?;
            emit(&payload, args.output.as_deref())?;
        }
        Command::Manifest(args) => {
            let data =
                apk::read_entry_with_policy(&args.apk_path, archive_policy, "AndroidManifest.xml")?;
            let xml = manifest::decode(&data)?;
            emit(&xml, args.output.as_deref())?;
        }
        Command::Getclass(args) => {
            let class_name = query::format_class_name(&args.dalvik_class)?;
            // `--members` prefixes the index table. It has to describe the same definition
            // the source came from, so a class defined more than once is refused rather
            // than resolved by taking the first - the table would otherwise annotate one
            // definition while the decompiler rendered another.
            let mut table: Vec<String> = Vec::new();
            if args.members {
                let inputs = [args.apk_path.clone()];
                let lookup = apk::member_lines_with_policy(
                    &inputs,
                    archive_policy,
                    &class_name,
                    args.threads,
                )?;
                match lookup.lines {
                    Some(lines) => table = lines,
                    None if lookup.definitions.is_empty() => {
                        bail!("Class {class_name} not found in APK.");
                    }
                    None => {
                        eprintln!(
                            "Error: {class_name} is defined {} times ({}); --members must name one definition",
                            lookup.definitions.len(),
                            lookup.definitions.join(", ")
                        );
                        #[cfg(feature = "mcp")]
                        if crate::query::is_capturing() {
                            emit(
                                &format!("{}\n", json!({"definitions":lookup.definitions})),
                                None,
                            )?;
                        }
                        status = apk::lookup_status(lookup.definitions.len());
                    }
                }
            }
            if status == 0 {
                let hit = apk::decompile_class_with_policy(
                    &args.apk_path,
                    archive_policy,
                    &class_name,
                    args.threads,
                    args.debug,
                )?;
                let Some((dex_name, source)) = hit else {
                    bail!("Class {class_name} not found in APK.");
                };
                if args.debug {
                    rxd_dex::diag::diagnose(format_args!("[DEBUG] Hit DEX: {dex_name}"));
                    debug_timing(started);
                    rxd_dex::diag::diagnose(format_args!("{}", "-".repeat(50)));
                }
                let mut payload = String::with_capacity(source.len() + 256);
                for line in &table {
                    payload.push_str(line);
                    payload.push('\n');
                }
                payload.push_str(&source);
                payload.push('\n');
                emit(&payload, args.output.as_deref())?;
            }
        }
    }
    Ok(status)
}

fn data_record(kind: &str, detail: Value) -> Value {
    record(
        kind,
        RecordMeta::default(),
        detail.as_object().unwrap().clone(),
    )
}

fn append_json(payload: &mut String, value: &Value) -> Result<()> {
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

fn member_record(candidate: &apk::MemberCandidate) -> Value {
    match candidate {
        apk::MemberCandidate::Field {
            entry,
            class,
            name,
            type_descriptor,
            access_flags,
            is_static,
            field_index,
            slot,
        } => data_record(
            "field",
            json!({
                "entry":entry, "class":class, "name":name, "type":type_descriptor,
                "access_flags":access_flags, "static":is_static, "field_index":field_index, "slot":slot,
            }),
        ),
        apk::MemberCandidate::Method {
            entry,
            class,
            name,
            access_flags,
            method_index,
        } => data_record(
            "method",
            json!({
                "entry":entry, "class":class, "name":name, "access_flags":access_flags, "method_index":method_index,
            }),
        ),
    }
}

/// Says why a lookup that did not end in exactly one hit did not.
///
/// The rows are already on stdout; this is the explanation, and it goes to stderr so a
/// caller reading the payload never has to filter it out.
fn report_member_verdict(lookup: &apk::MemberLookup, kind: &str, index: u32, descriptor: &str) {
    match lookup.candidates.len() {
        0 if lookup.definitions == 0 => {
            eprintln!("Error: {descriptor} is not defined in the supplied code inputs");
        }
        0 => eprintln!("Error: no {kind} index {index} is declared by {descriptor}"),
        1 => {}
        count => eprintln!(
            "Error: {kind} index {index} of {descriptor} is declared {count} times; the rows above are the candidates"
        ),
    }
}
