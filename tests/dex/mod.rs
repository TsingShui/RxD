//! End-to-end contracts for the migrated rasc CLI, without MCP.
pub(super) mod support;
use super::{fixture, refused, success};
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use support::{const_string_dex, deflated_zip, stored_zip};

fn invoke(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .arg("rasc")
        .args(args)
        .env_remove("RASC_MAX_INFLATED_ENTRY")
        .output()
        .unwrap()
}
fn query(command: &str, input: &Path, args: &[&str]) -> Output {
    let mut params = vec![command, input.to_str().unwrap()];
    params.extend_from_slice(args);
    invoke(&params)
}
fn text(out: &Output) -> &str {
    std::str::from_utf8(&out.stdout).unwrap()
}

fn dex_records(output: &Output) -> Vec<Value> {
    let rows: Vec<Value> = text(output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    rows
}

#[test]
fn dex_inflation_has_no_default_quota_but_preserves_explicit_override() {
    let payload = vec![7u8; 1024 * 1024];
    let bytes = deflated_zip(&[("payload.bin", &payload)]);
    let (dir, input) = fixture(&bytes, "payload.apk");
    let dest = dir.path().join("payload.bin");
    fs::write(&dest, b"preserved").unwrap();
    let failed = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["rasc", "extract"])
        .arg(&input)
        .args(["--name", "payload.bin", "-o"])
        .arg(&dest)
        .env("RASC_MAX_INFLATED_ENTRY", "1024")
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert_eq!(fs::read(&dest).unwrap(), b"preserved");
    let complete = query(
        "extract",
        &input,
        &["--name", "payload.bin", "-o", dest.to_str().unwrap()],
    );
    success(&complete);
    assert_eq!(fs::read(&dest).unwrap(), payload);
    assert!(
        rxd_core::Budget::for_input_bytes(bytes.len())
            .unwrap()
            .entry_bytes
            .is_none()
    );
}

#[test]
fn dex_extract_writes_original_entry_bytes_and_a_receipt() {
    let (dir, path) = fixture(
        deflated_zip(&[
            ("lib/arm64-v8a/liba.so", b"\x7fELF-A"),
            ("lib/arm64-v8a/libb.so", b"\x7fELF-B"),
            ("assets/note.txt", b"hello"),
        ]),
        "app.apk",
    );
    let file = dir.path().join("liba.so");
    let output = query(
        "extract",
        &path,
        &[
            "--name",
            "lib/arm64-v8a/liba.so",
            "-o",
            file.to_str().unwrap(),
        ],
    );
    success(&output);
    let rows = dex_records(&output);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["detail"]["entry"], "lib/arm64-v8a/liba.so");
    assert_eq!(rows[0]["detail"]["file_bytes"], 6);
    assert_eq!(rows[0]["detail"]["path"], file.to_str().unwrap());
    assert_eq!(
        rows[0]["detail"]["sha256"].as_str().unwrap().len(),
        64,
        "receipt carries a SHA-256"
    );
    assert_eq!(fs::read(&file).unwrap(), b"\x7fELF-A");

    // A missing entry is NOT_FOUND and must not replace an existing artifact.
    let kept = dir.path().join("kept.so");
    fs::write(&kept, b"previous").unwrap();
    let output = query(
        "extract",
        &path,
        &["--name", "missing.so", "-o", kept.to_str().unwrap()],
    );
    refused(output);
    assert_eq!(fs::read(&kept).unwrap(), b"previous");
}

#[test]
fn dex_extract_all_preserves_entry_paths_and_rejects_traversal() {
    let (dir, path) = fixture(
        deflated_zip(&[
            ("lib/arm64-v8a/liba.so", b"A"),
            ("lib/arm64-v8a/libb.so", b"B"),
            ("assets/note.txt", b"hello"),
        ]),
        "app.apk",
    );
    let destination = dir.path().join("out");
    let output = query(
        "extract",
        &path,
        &["--all", "-f", "lib/", "-o", destination.to_str().unwrap()],
    );
    success(&output);
    let rows = dex_records(&output);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        fs::read(destination.join("lib/arm64-v8a/liba.so")).unwrap(),
        b"A"
    );
    assert_eq!(
        fs::read(destination.join("lib/arm64-v8a/libb.so")).unwrap(),
        b"B"
    );

    let (bad_dir, bad) = fixture(stored_zip(&[("../escape.so", b"x")]), "bad.apk");
    let output = query(
        "extract",
        &bad,
        &["--all", "-o", bad_dir.path().join("out").to_str().unwrap()],
    );
    refused(output);
    assert!(!bad_dir.path().join("escape.so").exists());
}

#[test]
fn dex_extract_never_replaces_the_analysed_archive() {
    let (dir, path) = fixture(deflated_zip(&[("lib/a.so", b"payload")]), "app.apk");
    let before = fs::read(&path).unwrap();
    let output = query(
        "extract",
        &path,
        &["--name", "lib/a.so", "-o", path.to_str().unwrap()],
    );
    refused(output);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(dir.path().join("app.apk").exists());
}
#[test]
fn dex_getclass_preserves_source_debug_and_missing_class_status() {
    let dex = const_string_dex(2);
    let (dir, bare) = fixture(&dex, "input.dex");
    let wrapped = dir.path().join("app.apk");
    fs::write(&wrapped, deflated_zip(&[("classes.dex", &dex)])).unwrap();
    let original = query("get-class", &bare, &["--at", "Fixture1"]);
    success(&original);
    assert!(text(&original).contains("class Fixture1"));
    assert!(text(&original).contains("m1("));
    assert!(original.stderr.is_empty());
    let output_file = dir.path().join("Fixture1.java");
    let wrapped_out = query(
        "get-class",
        &wrapped,
        &[
            "--at",
            "LFixture1;",
            "--perf",
            "-o",
            output_file.to_str().unwrap(),
        ],
    );
    success(&wrapped_out);
    assert_eq!(wrapped_out.stdout, original.stdout);
    assert_eq!(fs::read(output_file).unwrap(), original.stdout);
    // One perf record on stderr; the per-entry trace is a developer switch (`RXD_TRACE`).
    assert!(
        String::from_utf8_lossy(&wrapped_out.stderr).contains("\"backend\":\"dex\""),
        "{}",
        String::from_utf8_lossy(&wrapped_out.stderr)
    );
    let missing = query("get-class", &bare, &["--at", "Absent"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no class LAbsent; in this input"));
}

#[test]
fn dex_manifest_decodes_binary_xml_and_duplicate_entries_use_last_copy() {
    let binary = support::binary_manifest_with_cdata("a ]]> b");
    let (_dir, path) = fixture(
        deflated_zip(&[("AndroidManifest.xml", &binary)]),
        "binary.apk",
    );
    let output = query("manifest", &path, &[]);
    success(&output);
    assert!(text(&output).contains("&amp;"));
    assert!(text(&output).contains("android:versionCode=\"42\""));
    assert!(text(&output).contains("<![CDATA["));
    assert!(output.stderr.is_empty());
    let (_dir, path) = fixture(
        stored_zip(&[
            ("AndroidManifest.xml", b"<first/>"),
            ("AndroidManifest.xml", b"<last/>"),
        ]),
        "duplicate.apk",
    );
    let output = query("manifest", &path, &[]);
    success(&output);
    assert_eq!(output.stdout, b"<last/>");
}

#[test]
fn dex_inflate_budget_preserves_environment_defaults_and_error_bytes() {
    let mut xml = b"<manifest>".to_vec();
    xml.resize(128 * 1024, b' ');
    let (_dir, path) = fixture(deflated_zip(&[("AndroidManifest.xml", &xml)]), "budget.apk");
    for limit in ["1024", "0", "invalid", "", " 262144 "] {
        let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["rasc", "manifest"])
            .arg(&path)
            .env("RASC_MAX_INFLATED_ENTRY", limit)
            .output()
            .unwrap();
        if limit == "1024" {
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            assert_eq!(
                output.stderr,
                b"INVALID_INPUT: AndroidManifest.xml inflates past the 1024 bytes entry limit\n"
            );
        } else {
            success(&output);
            assert_eq!(output.stdout, xml);
        }
    }
}

#[test]
fn dex_malformed_inputs_return_diagnostics_without_success_output() {
    for bytes in [b"PK\x03\x04".as_slice(), b"dex\n039\0".as_slice()] {
        let (_dir, path) = fixture(bytes, "malformed.bin");
        for command in ["classes", "strings", "get-class"] {
            let args: &[&str] = if command == "get-class" {
                &["--at", "Fixture0"]
            } else {
                &[]
            };
            let output = query(command, &path, args);
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_INPUT:"));
        }
    }
}
