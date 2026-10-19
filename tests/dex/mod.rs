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
    let mut keys = vec![
        "record",
        "id",
        "source_id",
        "parent_id",
        "context_id",
        "start",
        "end",
        "detail",
    ];
    if row["record"] == "field_plan" {
        keys.extend(["backend", "command", "schema_version"]);
        assert_eq!(row["backend"], "dex");
        assert_eq!(row["command"], "fields-plan");
        assert_eq!(row["schema_version"], 2);
    }
    assert_keys(row, &keys);
    for key in ["id", "source_id", "parent_id", "context_id", "start", "end"] {
        assert!(row[key].is_null(), "{key}: {row}");
    }
    let fields: &[&str] = match row["record"].as_str().unwrap() {
        "class" => &["entry", "descriptor", "name", "package", "simple_name"],
        "string" => &["entry", "index", "value", "class"],
        "reference" => &["entry", "class", "member", "matched"],
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
        "method" => &["entry", "class", "name", "access_flags", "method_index"],
        "field_plan" => &[
            "descriptor",
            "dex_class_def_idx",
            "dex_type_idx",
            "class_access_flags",
            "instance_fields",
            "static_fields",
            "counts",
            "instance_ref_mask",
        ],
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
fn dex_envelope_escapes_strings_entry_names_and_member_names_without_extra_rows() {
    let mut dex = const_string_dex(1);
    let value = "λ|\"\\\n1234567";
    assert_eq!(value.len(), b"Authorization".len());
    let at = dex.windows(13).position(|w| w == b"Authorization").unwrap();
    dex[at..at + 13].copy_from_slice(value.as_bytes());
    dex[at - 1] = value.encode_utf16().count() as u8;
    let at = dex.windows(3).position(|w| w == b"m0\0").unwrap();
    dex[at..at + 2].copy_from_slice(b"|x");
    let entry = "classes|\"\n.dex";
    let (_dir, path) = fixture(stored_zip(&[(entry, &dex)]), "escaped.apk");
    let out = query("strings", &path, &["--limit", "1"]);
    success(&out);
    assert_eq!(
        dex_records(&out)[0]["detail"],
        json!({"entry":entry, "index":0, "value":value, "class":null})
    );
    let header: Value = serde_json::from_str(text(&out).lines().next().unwrap()).unwrap();
    assert_eq!(header["command"], "strings");
    assert_eq!(
        header["query"],
        json!({"filter":null, "offset":null, "limit":1})
    );
    assert_eq!(header["input_bytes"], fs::metadata(&path).unwrap().len());
    let out = query("xref", &path, &["string", "λ"]);
    success(&out);
    assert_eq!(
        dex_records(&out)[0]["detail"],
        json!({"entry":entry, "class":"LFixture0;", "member":"LFixture0;->|x", "matched":value})
    );
    let out = invoke(&[
        "member-by-index",
        "--apk",
        path.to_str().unwrap(),
        "--descriptor",
        "Fixture0",
        "--method-index",
        "0",
    ]);
    success(&out);
    assert_eq!(dex_records(&out)[0]["detail"]["name"], "|x");
}

#[test]
fn dex_classes_and_references_match_for_bare_stored_and_deflated_inputs() {
    let dex = const_string_dex(3);
    let (dir, bare) = fixture(&dex, "input.dex");
    let expected_classes: Vec<_> = (0..3).map(|i| json!({"entry":"classes.dex", "descriptor":format!("LFixture{i};"), "name":format!("Fixture{i}"), "package":"", "simple_name":format!("Fixture{i}")})).collect();
    let expected_refs: Vec<_> = (0..3).map(|i| json!({"entry":"classes.dex", "class":format!("LFixture{i};"), "member":format!("LFixture{i};->m{i}"), "matched":"Authorization"})).collect();
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
        json!({"entry":"classes.dex", "index":2, "value":"LFixture1;", "class":null})
    );
    assert_eq!(fs::read(output_file).unwrap(), output.stdout);
    for args in [&["--limit", "0"][..], &["--offset", "99"][..]] {
        let output = query("strings", &path, args);
        success(&output);
        assert!(dex_records(&output).is_empty());
    }
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
    assert!(
        dex_records(&output)
            .iter()
            .all(|r| r["detail"]["entry"] == "fallback.dex")
    );
}

#[test]
fn dex_getclass_preserves_source_member_table_debug_and_missing_class_status() {
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
    let members = query("get-class", &wrapped, &["--members", "Fixture1"]);
    success(&members);
    assert!(text(&members).starts_with("# members methods:"));
    assert!(members.stdout.ends_with(&original.stdout));
    let missing_members = query("get-class", &bare, &["--members", "Absent"]);
    assert_eq!(missing_members.status.code(), Some(1));
    assert!(missing_members.stdout.is_empty());
    let missing = query("get-class", &bare, &["Absent"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("Class LAbsent; not found in APK."));
}

#[test]
fn dex_runtime_field_slots_differ_from_field_ids_and_method_indices_resolve() {
    let (_dir, path) = fixture(support::member_dex(), "members.dex");
    let input = path.to_str().unwrap();
    let plan = invoke(&["fields-plan", "--apk", input, "--descriptor", "Fixture0"]);
    success(&plan);
    let record: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_record(&record);
    assert_eq!(record["detail"]["instance_ref_mask"], "0x1");
    assert_keys(&record["detail"]["counts"], &["instance", "static"]);
    for field in record["detail"]["instance_fields"]
        .as_array()
        .unwrap()
        .iter()
        .chain(record["detail"]["static_fields"].as_array().unwrap())
    {
        assert_keys(field, &["field_index", "name", "type", "access_flags"]);
    }
    let instance = invoke(&[
        "member-by-index",
        "--apk",
        input,
        "--descriptor",
        "Fixture0",
        "--field-index",
        "0",
    ]);
    success(&instance);
    let row = dex_records(&instance).remove(0);
    assert_eq!(row["detail"]["field_index"], 1);
    assert_eq!(row["detail"]["slot"], 0);
    assert_eq!(row["detail"]["static"], false);
    let static_field = invoke(&[
        "member-by-index",
        "--apk",
        input,
        "--descriptor",
        "Fixture0",
        "--field-index",
        "1",
    ]);
    success(&static_field);
    let row = dex_records(&static_field).remove(0);
    assert_eq!(row["detail"]["field_index"], 0);
    assert_eq!(row["detail"]["slot"], 1);
    assert_eq!(row["detail"]["static"], true);
    let method = invoke(&[
        "member-by-index",
        "--apk",
        input,
        "--descriptor",
        "Fixture0",
        "--method-index",
        "0",
    ]);
    success(&method);
    let row = dex_records(&method).remove(0);
    assert_eq!(row["detail"]["method_index"], 0);
    assert_eq!(row["detail"]["class"], "LFixture0;");
    assert_eq!(row["detail"]["name"], "m0");
}

#[test]
fn dex_lookup_verdicts_preserve_exit_three_and_ambiguous_candidates_on_exit_four() {
    let dex = const_string_dex(1);
    let (dir, path) = fixture(&dex, "first.dex");
    let second = dir.path().join("second.dex");
    fs::write(&second, &dex).unwrap();
    let input = path.to_str().unwrap();
    let other = second.to_str().unwrap();
    for command in ["fields-plan", "member-by-index"] {
        let mut args = vec![command, "--apk", input, "--descriptor", "Absent"];
        if command == "member-by-index" {
            args.extend(["--method-index", "0"]);
        }
        let missing = invoke(&args);
        assert_eq!(missing.status.code(), Some(3));
        if command == "member-by-index" {
            assert!(dex_records(&missing).is_empty());
        } else {
            assert!(missing.stdout.is_empty());
        }
        let mut args = vec![command, "--apk", input, other, "--descriptor", "Fixture0"];
        if command == "member-by-index" {
            args.extend(["--method-index", "0"]);
        }
        let ambiguous = invoke(&args);
        assert_eq!(ambiguous.status.code(), Some(4));
        assert!(!ambiguous.stderr.is_empty());
        if command == "member-by-index" {
            assert_eq!(dex_records(&ambiguous).len(), 2);
            let header: Value =
                serde_json::from_str(text(&ambiguous).lines().next().unwrap()).unwrap();
            assert_eq!(header["input_bytes"], 2 * dex.len());
            assert_eq!(header["query"]["descriptor"], "LFixture0;");
        } else {
            assert!(ambiguous.stdout.is_empty());
        }
    }
    let out = invoke(&[
        "member-by-index",
        "--apk",
        input,
        "--descriptor",
        "Fixture0",
        "--method-index",
        "99",
    ]);
    assert_eq!(out.status.code(), Some(3));
    assert!(dex_records(&out).is_empty());
    let out = query(
        "get-class",
        &dir.path().join("duplicate.apk"),
        &["--members", "Fixture0"],
    );
    assert_eq!(out.status.code(), Some(1));
    let duplicate = dir.path().join("duplicate.apk");
    fs::write(
        &duplicate,
        stored_zip(&[("classes.dex", &dex), ("classes2.dex", &dex)]),
    )
    .unwrap();
    let out = query("get-class", &duplicate, &["--members", "Fixture0"]);
    assert_eq!(out.status.code(), Some(4));
    assert!(out.stdout.is_empty());
}

#[test]
fn dex041_queries_keep_physical_entry_and_logical_member_names() {
    let container = support::dex041_container(&[1, 2]);
    let (_dir, path) = fixture(
        deflated_zip(&[("classes.dex", &container)]),
        "container.apk",
    );
    for command in ["classes", "xref"] {
        let args: &[&str] = if command == "xref" {
            &["string", "Authorization"]
        } else {
            &[]
        };
        let output = query(command, &path, args);
        success(&output);
        assert!(text(&output).contains("classes.dex!classes1.dex"));
        assert!(text(&output).contains("classes.dex!classes2.dex"));
    }
    // The legacy strings format names the physical entry even for logical members.
    let strings = query("strings", &path, &[]);
    success(&strings);
    assert_eq!(dex_records(&strings).len(), 10);
    assert!(
        dex_records(&strings)
            .iter()
            .all(|row| row["detail"]["entry"] == "classes.dex")
    );
    let output = query("get-class", &path, &["--members", "Fixture1"]);
    success(&output);
    assert!(text(&output).contains("class Fixture1"));
    assert!(text(&output).contains("method_ids=1"));
    let output = query("get-class", &path, &["--members", "Fixture0"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let (dir, path) = fixture(&container, "input.dex");
    let output = invoke(&[
        "member-by-index",
        "--apk",
        path.to_str().unwrap(),
        "--descriptor",
        "Fixture0",
        "--method-index",
        "0",
        "-o",
        dir.path().join("candidates").to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(dex_records(&output).len(), 2);
    assert_eq!(
        fs::read(dir.path().join("candidates")).unwrap(),
        output.stdout
    );
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
fn dex_cli_rejects_mcp_and_invalid_indices_without_analyzing_input() {
    let help = invoke(&["--help"]);
    success(&help);
    let help = text(&help);
    assert!(!help.contains("mcp"));
    for command in [
        "get-class",
        "entries",
        "strings",
        "xref",
        "manifest",
        "classes",
        "fields-plan",
        "member-by-index",
    ] {
        assert!(help.contains(command), "{command}");
    }
    assert_eq!(invoke(&["mcp"]).status.code(), Some(2));
    for indices in [&[][..], &["--field-index", "0", "--method-index", "0"][..]] {
        let mut args = vec![
            "member-by-index",
            "--apk",
            "missing.dex",
            "--descriptor",
            "Fixture0",
        ];
        args.extend_from_slice(indices);
        assert_eq!(invoke(&args).status.code(), Some(2));
    }
    assert_eq!(
        invoke(&["xref", "missing.dex", "method"]).status.code(),
        Some(1)
    );
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

#[test]
fn dex_benchmark_records_complete_verdicts_and_marks_standalone_manifest_inapplicable() {
    let (dir, path) = fixture(support::member_dex(), "input.dex");
    let report_dir = dir.path().join("report");
    let output = Command::new("python3")
        .args([
            "-B",
            "benches/dex.py",
            "--binary",
            env!("CARGO_BIN_EXE_rxd"),
            "--input",
        ])
        .arg(&path)
        .args(["--class", "Fixture0", "--samples", "1", "--output"])
        .arg(&report_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    success(&output);
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(report_dir.join("results.json")).unwrap()).unwrap();
    assert!(report.get("failure").is_none());
    let workloads = report["workloads"].as_array().unwrap();
    assert_eq!(workloads.len(), 16);
    for workload in workloads {
        assert_eq!(workload["samples"].as_array().unwrap().len(), 1);
        assert_eq!(workload["stats"]["samples"], 1);
        assert!(workload["stats"]["p95_ns"].as_u64().unwrap() > 0);
        assert!(workload["migration_match"].is_null());
    }
    let ambiguous = workloads
        .iter()
        .find(|w| w["name"] == "input-1/member_method_ambiguous")
        .unwrap();
    assert_eq!(ambiguous["samples"][0]["exit_code"], 4);
    assert!(ambiguous["samples"][0]["stdout_bytes"].as_u64().unwrap() > 0);
    assert_eq!(report["not_applicable"][0]["name"], "input-1/manifest");
    assert!(report_dir.join("results.md").is_file());
}
