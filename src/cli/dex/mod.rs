//! CLI entry point: argument parsing, dispatch, output.
//!
//! stdout carries a command's payload and nothing else: every diagnostic,
//! including `--debug` timing, goes to stderr, so redirecting a result can never
//! capture debug lines. An `-o` file receives exactly the bytes stdout received.

mod args;
mod output;
use output::{emit, emit_rows};
use rxd_core::envelope::{header, summary};
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
fn debug_timing(command: &'static str, started: Instant) {
    rxd_dex::profile::report(command, started.elapsed());
}

/// Runs the parsed command.
fn run_command(
    command: Command,
    archive_policy: apk::ArchivePolicy,
    started: Instant,
) -> Result<i32> {
    rxd_dex::profile::enable(command.debug());
    let command_name = command.name();
    // Only a member lookup has a verdict of its own (one hit, none, several); every
    // other command either produced its payload or failed.
    let status = 0;
    match command {
        Command::Xref(args) => {
            let query = args.query()?;
            let scan_started = Instant::now();
            let hits = apk::find_reference_hits_with_policy(
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
            let sorted = scan_started.elapsed();
            // Assembling the payload line by line: a pre-sized buffer measured no faster
            // than letting the String grow (macOS's allocator re-extends the large block in
            // place), so the length pass is not worth it.
            let mut payload = stream_header(
                "xref",
                std::slice::from_ref(&args.apk_path),
                args.query_json(),
            )?;
            for hit in &hits {
                push_record(
                    &mut payload,
                    "reference",
                    &[
                        ("entry", Field::Text(hit.dex_name.as_ref())),
                        ("member", Field::Text(&hit.member)),
                        ("matched", Field::Text(&hit.matched)),
                    ],
                )?;
            }
            finish_stream(&mut payload, hits.len())?;
            if args.debug {
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
            let pieces: Vec<(String, usize)> = classes
                .par_chunks(RENDER_CHUNK)
                .map(|chunk| -> Result<_> {
                    let mut payload = String::with_capacity(chunk.len() * 256);
                    let mut records = 0;
                    // One reused buffer for the dotted name: the row itself carries only
                    // the descriptor, so the dotted form is computed for `--filter` alone.
                    let mut name = String::with_capacity(96);
                    for class in chunk {
                        if let Some(pattern) = filter.as_ref() {
                            name.clear();
                            for character in class.java_name().chars() {
                                name.push(if character == '/' { '.' } else { character });
                            }
                            if !name.to_lowercase().contains(pattern) {
                                continue;
                            }
                        }
                        push_record(
                            &mut payload,
                            "class",
                            &[("descriptor", Field::Text(&class.descriptor))],
                        )?;
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
                push_record(
                    &mut payload,
                    "string",
                    &[
                        ("entry", Field::Text(entry.dex_name.as_ref())),
                        ("index", Field::Number(entry.index as u64)),
                        ("value", Field::Text(&entry.value)),
                    ],
                )?;
            }
            finish_stream(&mut payload, strings.len())?;
            emit(&payload, args.output.as_deref())?;
        }
        Command::Methods(args) => {
            let selector = apk::MethodSelector {
                class: args.class.as_deref(),
                name: args.filter.as_deref(),
                with_flags: args.with_flags,
            };
            let (chunks, records) = apk::list_methods_with_policy(
                &args.apk_path,
                archive_policy,
                &selector,
                args.threads,
                args.debug,
            )?;
            let header = stream_header(
                "methods",
                std::slice::from_ref(&args.apk_path),
                json!({
                    "class":args.class, "filter":args.filter, "with_flags":args.with_flags,
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
            let xml = rxd_dex::profile::measure(rxd_dex::profile::Stage::Manifest, || {
                manifest::decode(&data)
            })?;
            emit(&xml, args.output.as_deref())?;
        }
        Command::Getclass(args) => {
            let class_name = query::format_class_name(&args.dalvik_class)?;
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
                    rxd_dex::diag::diagnose(format_args!("{}", "-".repeat(50)));
                }
                let mut payload = String::with_capacity(source.len() + 256);
                payload.push_str(&source);
                payload.push('\n');
                emit(&payload, args.output.as_deref())?;
            }
        }
    }
    debug_timing(command_name, started);
    Ok(status)
}

/// A DEX data record. No DEX unit has a copy-only coordinate, so the envelope's six
/// nullable navigation keys would be `null` on every row; they are omitted instead of
/// repeated, and the header carries the stream identity once.
/// One detail value written without a `serde_json::Value`.
enum Field<'a> {
    Text(&'a str),
    Number(u64),
}

/// Append a JSON string literal, escaping exactly what JSON requires.
fn push_json_string(out: &mut String, value: &str) {
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
fn push_record(payload: &mut String, kind: &str, fields: &[(&str, Field<'_>)]) -> Result<()> {
    payload.push_str("{\"record\":");
    push_json_string(payload, kind);
    payload.push_str(",\"detail\":{");
    for (index, (key, value)) in fields.iter().enumerate() {
        if index > 0 {
            payload.push(',');
        }
        push_json_string(payload, key);
        payload.push(':');
        match value {
            Field::Text(text) => push_json_string(payload, text),
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

fn data_record(kind: &str, detail: Value) -> Value {
    json!({"record": kind, "detail": detail})
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
        assert_eq!(out.ends_with("}\n"), true);
    }
}
