//! End-to-end contracts for the migrated rasc CLI, without MCP.
pub(super) mod support;
use super::{fixture, success};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use support::{const_string_dex, deflated_zip, stored_zip};

fn invoke(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .arg("dex")
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

fn assert_keys(row: &Value, keys: &[&str]) {
    let mut actual: Vec<_> = row
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut expected = keys.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

fn assert_record(row: &Value) {
    // DEX units have no copy-only coordinate, so a data record is `record` + `detail`
    // only; the stream header carries identity once instead of every row repeating it.
    let keys = vec!["record", "detail"];
    assert_keys(row, &keys);
    let fields: &[&str] = match row["record"].as_str().unwrap() {
        "class" => &["descriptor"],
        "string" => &["entry", "index", "value"],
        "reference" => &["entry", "member", "matched"],
        "entry" => &[
            "name",
            "compression",
            "compressed_size",
            "uncompressed_size",
        ],
        "field" => &[
            "entry",
            "class",
            "name",
            "type",
            "access_flags",
            "static",
            "field_index",
            "slot",
        ],
        // `methods` prints one descriptor row per definition; `get-class --members` prints
        // its member table as text, so this is the only `method` record shape.
        "method" if row["detail"].get("access_flags").is_some() => &["access_flags", "descriptor"],
        "method" => &["descriptor"],

        other => panic!("unexpected dex record {other}"),
    };
    assert_keys(&row["detail"], fields);
}

fn dex_records(output: &Output) -> Vec<Value> {
    let rows: Vec<Value> = text(output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let header = &rows[0];
    assert_keys(
        header,
        &[
            "record",
            "backend",
            "command",
            "source_id",
            "input_bytes",
            "schema_version",
            "query",
            "detail",
        ],
    );
    assert_eq!(header["record"], "header");
    assert_eq!(header["backend"], "dex");
    assert_eq!(header["schema_version"], 2);
    assert!(header["source_id"].is_null());
    assert!(header["input_bytes"].as_u64().unwrap() > 0);
    assert_keys(&header["detail"], &[]);
    for row in &rows[1..rows.len() - 1] {
        assert_record(row);
    }
    let summary = rows.last().unwrap();
    assert_keys(summary, &["record", "records", "complete", "detail"]);
    assert_eq!(summary["record"], "summary");
    assert_eq!(summary["records"], rows.len() - 2);
    assert_eq!(summary["complete"], true);
    assert_keys(&summary["detail"], &[]);
    rows[1..rows.len() - 1].to_vec()
}

#[test]
fn dex_classes_and_references_match_for_bare_stored_and_deflated_inputs() {
    let dex = const_string_dex(3);
    let (dir, bare) = fixture(&dex, "input.dex");
    let expected_classes: Vec<_> = (0..3)
        .map(|i| json!({"descriptor":format!("LFixture{i};")}))
        .collect();
    let expected_refs: Vec<_> = (0..3).map(|i| json!({"entry":"classes.dex", "member":format!("LFixture{i};->m{i}"), "matched":"Authorization"})).collect();
    for (name, bytes) in [
        ("stored.apk", stored_zip(&[("classes.dex", &dex)])),
        ("compressed.apk", deflated_zip(&[("classes.dex", &dex)])),
        ("copy.dex", dex),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        for threads in ["1", "2"] {
            let output = query("classes", &path, &["--threads", threads]);
            success(&output);
            assert_eq!(
                dex_records(&output)
                    .iter()
                    .map(|r| r["detail"].clone())
                    .collect::<Vec<_>>(),
                expected_classes
            );
            assert!(output.stderr.is_empty());
            let alias = Command::new(env!("CARGO_BIN_EXE_rxd"))
                .args(["apk", "classes"])
                .arg(&path)
                .args(["--threads", threads])
                .env_remove("RASC_MAX_INFLATED_ENTRY")
                .output()
                .unwrap();
            assert_eq!(alias.status.code(), output.status.code());
            assert_eq!(alias.stdout, output.stdout);
            assert_eq!(alias.stderr, output.stderr);
            let output = query(
                "xref",
                &path,
                &["--thread", threads, "string", "Authorization"],
            );
            success(&output);
            assert_eq!(
                dex_records(&output)
                    .iter()
                    .map(|r| r["detail"].clone())
                    .collect::<Vec<_>>(),
                expected_refs
            );
        }
    }
    let output = query("classes", &bare, &["-f", "fIxTuRe1"]);
    success(&output);
    assert_eq!(dex_records(&output).len(), 1);
    assert!(text(&output).contains("LFixture1;"));
    let output = query("xref", &bare, &["string", "no-match"]);
    success(&output);
    assert!(dex_records(&output).is_empty());
    for kind in ["type", "method", "field"] {
        let output = query("xref", &bare, &[kind, "no-match"]);
        success(&output);
        assert!(dex_records(&output).is_empty());
    }
}

#[test]
fn dex_strings_filter_pagination_and_output_file_keep_identical_bytes() {
    let (dir, path) = fixture(const_string_dex(3), "input.dex");
    let output_file = dir.path().join("strings.txt");
    let output = query(
        "strings",
        &path,
        &[
            "--filter",
            "FIXTURE",
            "--offset",
            "1",
            "--limit",
            "1",
            "-o",
            output_file.to_str().unwrap(),
        ],
    );
    success(&output);
    assert_eq!(
        dex_records(&output)[0]["detail"],
        json!({"entry":"classes.dex", "index":2, "value":"LFixture1;"})
    );
    assert_eq!(fs::read(output_file).unwrap(), output.stdout);
    for args in [&["--limit", "0"][..], &["--offset", "99"][..]] {
        let output = query("strings", &path, args);
        success(&output);
        assert!(dex_records(&output).is_empty());
    }
}

#[test]
fn dex_get_method_renders_one_method_from_its_methods_coordinate() {
    let dex = const_string_dex(1);
    let (_dir, path) = fixture(stored_zip(&[("classes.dex", &dex)]), "getmethod.apk");
    let listed = query("methods", &path, &["--class", "Fixture0"]);
    success(&listed);
    let coordinate = dex_records(&listed)[0]["detail"]["descriptor"]
        .as_str()
        .expect("a methods row names its descriptor")
        .to_owned();
    // The coordinate a `methods` row prints is exactly what `--at` takes back.
    let rendered = query("get-method", &path, &["--at", &coordinate]);
    success(&rendered);
    assert!(!text(&rendered).trim().is_empty(), "{coordinate}");
    let missing = query("get-method", &path, &["--at", "LNo/Such;->f()V"]);
    assert_ne!(missing.status.code(), Some(0));
    let malformed = query("get-method", &path, &["--at", "not-a-coordinate"]);
    assert_ne!(malformed.status.code(), Some(0));
}

#[test]
fn dex_methods_lists_definitions_and_honours_class_and_name_filters() {
    let dex = const_string_dex(1);
    let (_dir, path) = fixture(stored_zip(&[("classes.dex", &dex)]), "methods.apk");
    let all = query("methods", &path, &[]);
    success(&all);
    let rows = dex_records(&all);
    assert!(!rows.is_empty(), "{}", text(&all));
    assert!(rows.iter().all(|row| row["record"] == "method"));
    assert!(
        rows.iter().any(|row| {
            row["detail"]["descriptor"]
                .as_str()
                .is_some_and(|value| value.starts_with("LFixture0;->"))
        }),
        "{}",
        text(&all)
    );
    // A class filter lists that class only, and every row names it.
    let one = query("methods", &path, &["--class", "Fixture0"]);
    success(&one);
    let rows = dex_records(&one);
    assert!(!rows.is_empty(), "{}", text(&one));
    assert!(
        rows.iter().all(|row| {
            row["detail"]["descriptor"]
                .as_str()
                .is_some_and(|value| value.starts_with("LFixture0;->"))
        }),
        "{}",
        text(&one)
    );
    // A name filter that nothing matches lists nothing.
    let none = query("methods", &path, &["--filter", "zzz_no_such_method"]);
    success(&none);
    assert_eq!(dex_records(&none).len(), 0);
    // Access flags are opt-in.
    let flagged = query("methods", &path, &["--class", "Fixture0", "--with-flags"]);
    success(&flagged);
    assert!(
        dex_records(&flagged)
            .iter()
            .all(|row| row["detail"]["access_flags"].is_u64()),
        "{}",
        text(&flagged)
    );
}

#[test]
fn dex_entries_preserve_directory_order_duplicates_and_root_selection() {
    let one = const_string_dex(1);
    let two = const_string_dex(2);
    let entries = [
        ("nested/classes.dex", two.as_slice()),
        ("classes.dex", one.as_slice()),
        ("classes.dex", two.as_slice()),
        ("fallback.dex", two.as_slice()),
    ];
    let (_dir, path) = fixture(stored_zip(&entries), "entries.apk");
    let output = query("entries", &path, &[]);
    success(&output);
    let expected: Vec<_> = entries.iter().map(|(name, bytes)| json!({"name":name, "compression":"stored", "compressed_size":bytes.len(), "uncompressed_size":bytes.len()})).collect();
    assert_eq!(
        dex_records(&output)
            .iter()
            .map(|r| r["detail"].clone())
            .collect::<Vec<_>>(),
        expected
    );
    let output = query("classes", &path, &[]);
    success(&output);
    assert_eq!(dex_records(&output).len(), 1);
    let (_dir, path) = fixture(stored_zip(&[("fallback.dex", &two)]), "fallback.apk");
    let output = query("classes", &path, &[]);
    success(&output);
    assert_eq!(dex_records(&output).len(), 2);
    // A class row carries only its descriptor; which entry supplied it is covered by
    // `entries` above (and by `also_in` when one descriptor comes from several entries).
    assert!(
        dex_records(&output)
            .iter()
            .all(|r| r["detail"]["descriptor"]
                .as_str()
                .is_some_and(|d| d.starts_with('L'))),
        "{:?}",
        dex_records(&output)
    );
}

#[test]
fn dex_getclass_preserves_source_debug_and_missing_class_status() {
    let dex = const_string_dex(2);
    let (dir, bare) = fixture(&dex, "input.dex");
    let wrapped = dir.path().join("app.apk");
    fs::write(&wrapped, deflated_zip(&[("classes.dex", &dex)])).unwrap();
    let original = query("get-class", &bare, &["Fixture1"]);
    success(&original);
    assert!(text(&original).contains("class Fixture1"));
    assert!(text(&original).contains("m1("));
    assert!(original.stderr.is_empty());
    let output_file = dir.path().join("Fixture1.java");
    let wrapped_out = query(
        "get-class",
        &wrapped,
        &["LFixture1;", "--debug", "-o", output_file.to_str().unwrap()],
    );
    success(&wrapped_out);
    assert_eq!(wrapped_out.stdout, original.stdout);
    assert_eq!(fs::read(output_file).unwrap(), original.stdout);
    assert!(String::from_utf8_lossy(&wrapped_out.stderr).contains("Hit DEX: classes.dex"));
    let missing = query("get-class", &bare, &["Absent"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("Class LAbsent; not found in APK."));
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
            .args(["dex", "manifest"])
            .arg(&path)
            .env("RASC_MAX_INFLATED_ENTRY", limit)
            .output()
            .unwrap();
        if limit == "1024" {
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            assert_eq!(
                output.stderr,
                b"Error: AndroidManifest.xml inflates past the 1024 bytes entry limit\n"
            );
        } else {
            success(&output);
            assert_eq!(output.stdout, xml);
        }
    }
}

#[test]
fn dex_retires_skill_installation_and_root_help_exposes_the_shared_entry() {
    let home = tempfile::tempdir().unwrap();
    for backend in ["dex", "apk"] {
        let rejected = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args([backend, "skill", "--print"])
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env_remove("CODEX_HOME")
            .output()
            .unwrap();
        assert_eq!(rejected.status.code(), Some(2));
        assert!(rejected.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("unrecognized subcommand 'skill'")
        );
        let help = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args([backend, "--help"])
            .output()
            .unwrap();
        success(&help);
        assert!(!text(&help).contains("skill"));
    }
    let help = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .arg("--help")
        .output()
        .unwrap();
    success(&help);
    assert!(text(&help).contains("skill"));
    assert!(text(&help).contains("RxD skill"));
    let help = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["skill", "install", "--help"])
        .output()
        .unwrap();
    success(&help);
    for parameter in ["--agent", "--dir", "--print", "DIR/rxd/SKILL.md"] {
        assert!(text(&help).contains(parameter), "{parameter}");
    }
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
}

#[test]
fn dex_malformed_inputs_return_diagnostics_without_success_output() {
    for bytes in [b"PK\x03\x04".as_slice(), b"dex\n039\0".as_slice()] {
        let (_dir, path) = fixture(bytes, "malformed.bin");
        for command in ["classes", "strings", "get-class"] {
            let args: &[&str] = if command == "get-class" {
                &["Fixture0"]
            } else {
                &[]
            };
            let output = query(command, &path, args);
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains("Error:"));
        }
    }
}
