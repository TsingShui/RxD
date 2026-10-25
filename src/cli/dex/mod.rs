//! DEX CLI dispatch and payload output; diagnostics stay on stderr.

mod args;
mod output;
use output::{emit, emit_rows};
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
pub(super) fn execute(command: Command, policy: rxd_core::BudgetPolicy) -> rxd_core::Result<i32> {
    let budget = policy.for_file(command.input())?;
    super::restore_default_sigpipe();
    let (perf_on, name) = (command.perf(), command.name());
    let result = match run_command(command, archive_policy_from_env(budget.entry_bytes)) {
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
fn run_command(command: Command, archive_policy: apk::ArchivePolicy) -> Result<()> {
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

            let mut payload = String::new();
            for hit in &hits {
                push_reference(&mut payload, &hit.dex_name, &hit.member, &hit.matched)?;
            }
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
            let listed = started.elapsed();

            const RENDER_CHUNK: usize = 8192;
            let pieces: Vec<String> = classes
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
                    }
                    Ok(payload)
                })
                .collect::<Result<_>>()?;
            let payload_len = emit_rows(pieces.into_iter(), args.output.as_deref())?;
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
            let mut payload = String::new();
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
            return Ok(());
        }
        Command::Fields(args) => {
            let selector = apk::MethodSelector {
                class: args.class.as_deref(),
                name: args.filter.as_deref(),
                exact_name: args.name.as_deref(),
                case_sensitive: args.case.sensitive(),
            };
            let (chunks, _) = apk::list_fields_with_policy(
                &args.apk_path,
                archive_policy,
                &selector,
                args.threads,
                args.perf,
            )?;
            emit_rows(chunks.into_iter(), args.output.as_deref())?;
        }
        Command::Methods(args) => {
            let selector = apk::MethodSelector {
                class: args.class.as_deref(),
                name: args.filter.as_deref(),
                exact_name: args.name.as_deref(),
                case_sensitive: args.case.sensitive(),
            };
            let (chunks, _) = apk::list_methods_with_policy(
                &args.apk_path,
                archive_policy,
                &selector,
                args.threads,
                args.perf,
            )?;
            emit_rows(chunks.into_iter(), args.output.as_deref())?;
        }
        Command::Entries(args) => {
            let entries = rxd_dex::profile::measure(rxd_dex::profile::Stage::ZipIndex, || {
                apk::list_entries(&args.apk_path)
            })?;
            rxd_dex::profile::count(rxd_dex::profile::Counter::Entries, entries.len() as u64);
            let mut payload = String::new();
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
                    super::write_output(bytes, Some(&target))?;
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
                    )?;
                    Ok(())
                },
            )?;
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
    Ok(())
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
    // Carrier size would change identical DEX records across bare/APK inputs.
    record.insert("detail".into(), detail);
    let mut line = serde_json::to_string(&serde_json::Value::Object(record))?;
    line.push('\n');
    Ok(line)
}

enum Field<'a> {
    Text(&'a str),
    #[cfg(test)]
    Number(u64),
}

/// Append one caller/target reference row.
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

/// Append one string row with constant keys and no temporary Value.
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
