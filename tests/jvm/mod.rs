use super::*;

fn input(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/jvm")
        .join(name)
}

fn run(args: &[&str], path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .arg("jar")
        .arg(args[0])
        .arg(path)
        .args(&args[1..])
        .output()
        .unwrap()
}

fn assert_keys(value: &Value, expected: &[&str]) {
    let mut actual: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut expected = expected.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

fn assert_record(row: &Value) {
    let mut keys = vec![
        "record",
        "at",
        "source_id",
        "parent_at",
        "context_at",
        "start",
        "end",
        "detail",
    ];
    // A single FETCH record keeps its protocol identity outside `detail`.
    if matches!(row["record"].as_str(), Some("method_body" | "class_body")) {
        keys.extend(["backend", "command", "schema_version"]);
        assert_eq!(row["backend"], "jvm");
        assert_eq!(row["command"], "get-method");
        assert_eq!(row["schema_version"], 2);
    }
    assert_keys(row, &keys);
    // Identity lives in the stream header; only a standalone FETCH names it.
    if matches!(row["record"].as_str(), Some("method_body" | "class_body")) {
        assert!(row["source_id"].is_string());
    } else {
        assert!(row["source_id"].is_null());
    }
    assert!(row["parent_at"].is_null());
    // `super`/`interfaces` are omitted when they are the default, so a class row is a
    // subset of the full shape; they are checked where a fixture actually has them.
    let fields: &[&str] = match row["record"].as_str().unwrap() {
        "class" => {
            let detail = row["detail"].as_object().unwrap();
            for key in detail.keys() {
                assert!(
                    matches!(
                        key.as_str(),
                        "super"
                            | "interfaces"
                            | "major"
                            | "minor"
                            | "access_flags"
                            | "signature"
                            | "source_file"
                            | "fields"
                            | "methods"
                    ),
                    "unexpected class detail key: {key}"
                );
            }
            for key in [
                "major",
                "minor",
                "access_flags",
                "signature",
                "source_file",
                "fields",
                "methods",
            ] {
                assert!(detail.contains_key(key), "missing class detail key: {key}");
            }
            &[]
        }
        "field" => &["proto", "signature", "constant_index"],
        "method" => &["signature", "exceptions"],
        "string" => &["class", "index", "value", "utf16"],
        "reference" => &["byte_offset", "mnemonic", "target"],
        "method_body" => &[
            "representation",
            "decompiled",
            "code",
            "max_stack",
            "max_locals",
            "handlers",
            "instructions",
            "complete",
        ],
        "class_body" => &["representation", "decompiled", "complete", "code"],
        other => panic!("unexpected JVM record: {other}"),
    };
    if !fields.is_empty() {
        assert_keys(&row["detail"], fields);
    }
    match row["record"].as_str().unwrap() {
        "method" | "method_body" | "class_body" => {
            assert!(row["at"].as_str().unwrap().starts_with('L'))
        }
        "class" => assert!(row["at"].as_str().unwrap().starts_with('L')),
        _ => assert!(row["at"].is_null()),
    }
    if row["record"] == "reference" {
        assert!(row["context_at"].as_str().unwrap().contains("->"));
    } else {
        assert!(row["context_at"].is_null());
    }
    if row["record"] != "method_body" {
        assert!(row["start"].is_null());
        assert!(row["end"].is_null());
    }
}

fn jvm_jsonl(output: &Output) -> Vec<Value> {
    let rows = jsonl(output);
    for row in &rows {
        assert_record(row);
    }
    rows
}

fn methods(path: &Path) -> Vec<Value> {
    jvm_jsonl(&run(&["methods"], path))
        .into_iter()
        .filter(|v| v["record"] == "method")
        .collect()
}

fn method(path: &Path, name: &str) -> String {
    methods(path)
        .into_iter()
        .find(|v| {
            v["at"]
                .as_str()
                .unwrap_or_default()
                .contains(&format!("->{name}("))
        })
        .unwrap()["at"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn jvm_multi_release_selection_is_explicit_and_duplicate_definitions_are_rejected() {
    use std::io::Write;
    let directory = tempfile::tempdir().unwrap();
    let original = fs::read(input("fixture/FlowFixture.class")).unwrap();
    let mut variant = original.clone();
    let at = variant
        .windows(4)
        .position(|w| w == [0x1a, 0x1b, 0x60, 0xac])
        .unwrap();
    variant[at + 2] = 0x64; // isub instead of iadd, valid same-size method.
    let write = |path: &Path, entries: &[(&str, &[u8])]| {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for &(name, bytes) in entries {
            zip.start_file(
                name,
                zip::write::FileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    };
    let path = directory.path().join("multi.jar");
    write(
        &path,
        &[
            (
                "META-INF/MANIFEST.MF",
                b"Manifest-Version: 1.0\r\nMulti-Release: tr\r\n ue\r\n\r\n",
            ),
            ("fixture/FlowFixture.class", &original),
            ("META-INF/versions/17/fixture/FlowFixture.class", &variant),
        ],
    );
    let base_id = method(&path, "add");
    assert_eq!(base_id, method(&input("fixture/FlowFixture.class"), "add"));
    let rows = jvm_jsonl(&run(&["methods", "--release", "17"], &path));
    let selected = rows
        .iter()
        .find(|v| {
            v["record"] == "method" && v["at"].as_str().unwrap_or_default().contains("->add(")
        })
        .unwrap();

    let output = run(
        &[
            "get-method",
            "--at",
            selected["at"].as_str().unwrap(),
            "--release",
            "17",
        ],
        &path,
    );
    success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("return (left - right);"));
    // The coordinate names the method, not the release: the same value renders the
    // selected variant's body.
    let variant = run(&["get-method", "--at", &base_id, "--release", "17"], &path);
    success(&variant);
    assert!(String::from_utf8_lossy(&variant.stdout).contains("return (left - right);"));
    let duplicate = directory.path().join("duplicate.jar");
    write(
        &duplicate,
        &[("first.class", &original), ("second.class", &original)],
    );
    failure(
        run(&["classes"], &duplicate),
        1,
        "duplicate class definition",
    );
}

#[test]
fn jvm_content_identity_rejects_stale_ids_and_failures_preserve_output_files() {
    let path = input("fixture/FlowFixture.class");
    let id = method(&path, "add");
    failure(
        run(
            &["get-method", "--at", &id],
            &input("fixture/ModernFixture.class"),
        ),
        2,
        "NOT_FOUND",
    );
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("method.java");
    fs::write(&destination, "existing").unwrap();
    // A failing query leaves an existing output file untouched.
    let output = run(
        &[
            "get-method",
            "--at",
            "Lfixture/Absent;->f()V",
            "-o",
            destination.to_str().unwrap(),
        ],
        &path,
    );
    failure(output, 2, "NOT_FOUND");
    assert_eq!(fs::read(&destination).unwrap(), b"existing");
    let output = run(
        &[
            "get-method",
            "--at",
            &id,
            "-o",
            destination.to_str().unwrap(),
        ],
        &path,
    );
    success(&output);
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(&destination).unwrap(),
        run(&["get-method", "--at", &id], &path).stdout
    );
}

#[test]
fn jvm_modern_dynamic_bytecode_remains_extractable_without_guessed_java() {
    let path = input("fixture/ModernFixture.class");
    for name in ["concat", "lambda"] {
        let id = method(&path, name);
        let raw = run(&["get-method", "--at", &id, "--raw"], &path);
        success(&raw);
        let record: Value = serde_json::from_slice(&raw.stdout).unwrap();
        assert!(
            record["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["mnemonic"] == "invokedynamic")
        );
        failure(
            run(&["get-method", "--at", &id], &path),
            1,
            "UNSUPPORTED_BYTECODE",
        );
    }
    let path = input("fixture/InterfaceSuperFixture.class");
    let id = method(&path, "call");
    success(&run(&["get-method", "--at", &id, "--raw"], &path));
    failure(
        run(&["get-method", "--at", &id], &path),
        1,
        "qualified interface-super",
    );
}

#[test]
fn jvm_decoded_metadata_is_bounded_even_when_names_share_a_constant() {
    let mut bytes = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52, 0, 7];
    for (tag, data) in [
        (1, b"Amplified".as_slice()),
        (7, &[0, 1]),
        (1, b"java/lang/Object".as_slice()),
        (7, &[0, 3]),
        (1, &[b'x'; 500]),
        (1, b"I".as_slice()),
    ] {
        bytes.push(tag);
        if tag == 1 {
            bytes.extend_from_slice(&(data.len() as u16).to_be_bytes());
        }
        bytes.extend_from_slice(data);
    }
    for value in [0x21u16, 2, 4, 0, 32] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    for _ in 0..32 {
        for value in [1u16, 5, 6, 0] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
    }
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    assert!(bytes.len() < 2000);
    // The metadata budget is a crate-level floor; the CLI exposes no knob for it, so a
    // class with many names sharing a long constant still lists normally.
    let (_dir, path) = fixture(bytes, "LAmplified.class");
    let rows = jvm_jsonl(&run(&["classes"], &path));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["at"], "LAmplified;");
}

#[test]
fn jvm_corrupt_branches_stack_underflow_and_budgets_fail_without_partial_output() {
    let path = input("fixture/FlowFixture.class");
    let original = fs::read(&path).unwrap();
    let at = original
        .windows(4)
        .position(|w| w == [0x1a, 0x1b, 0x60, 0xac])
        .unwrap();
    for (replacement, diagnostic) in [
        ([0xa7, 0x00, 0x01, 0xac], "instruction boundary"),
        ([0x57, 0x1b, 0x60, 0xac], "stack underflow"),
    ] {
        let mut bytes = original.clone();
        bytes[at..at + 4].copy_from_slice(&replacement);
        let (_dir, changed) = fixture(bytes, "changed.class");
        let id = method(&changed, "add");
        failure(run(&["get-method", "--at", &id], &changed), 1, diagnostic);
    }
    let (_dir, truncated) = fixture(&original[..original.len() - 1], "truncated.class");
    failure(run(&["classes"], &truncated), 1, "INVALID_INPUT");
    let mut padded = original.clone();
    let switch = padded
        .windows(4)
        .position(|w| w == [0x1a, 0xab, 0, 0])
        .unwrap();
    padded[switch + 2] = 0xff;
    padded[switch + 3] = 0x7f;
    let (_dir, changed) = fixture(padded, "padding.class");
    let id = method(&changed, "choose");
    let result = run(&["get-method", "--at", &id], &changed);
    success(&result);
    assert!(String::from_utf8_lossy(&result.stdout).contains("case 100:"));
    let mut archive = fs::read(input("flow-stored.jar")).unwrap();
    let trailer = archive
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .unwrap();
    archive[trailer + 16..trailer + 20].copy_from_slice(&u32::MAX.to_le_bytes());
    let (_dir, zip64) = fixture(archive, "zip64.jar");
    failure(run(&["classes"], &zip64), 1, "UNSUPPORTED_INPUT");
}

#[test]
fn jvm_pinned_production_gson_queries_do_not_require_java_or_d8() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/jvm/gson-2.11.0.jar");
    let rows = methods(&path);
    assert_eq!(rows.len(), 1210);
    let filtered = jvm_jsonl(&run(
        &[
            "methods",
            "--class",
            "com.google.gson.GsonBuilder",
            "--filter",
            "serializeNulls",
        ],
        &path,
    ));
    let listed: Vec<&Value> = filtered
        .iter()
        .filter(|v| v["record"] == "method")
        .collect();
    assert!(!listed.is_empty());
    assert!(
        listed[0]["at"]
            .as_str()
            .unwrap()
            .contains("->serializeNulls(")
    );
    let descriptor_filter = jvm_jsonl(&run(
        &[
            "methods",
            "--class",
            "Lcom/google/gson/GsonBuilder;",
            "--filter",
            "serializeNulls",
        ],
        &path,
    ));
    let descriptor_listed: Vec<&Value> = descriptor_filter
        .iter()
        .filter(|v| v["record"] == "method")
        .collect();
    assert_eq!(listed.len(), descriptor_listed.len());
    assert_eq!(listed[0]["at"], descriptor_listed[0]["at"]);
    let id = rows
        .iter()
        .find(|v| {
            v["at"]
                .as_str()
                .unwrap_or_default()
                .contains("Lcom/google/gson/GsonBuilder;->serializeNulls(")
        })
        .unwrap()["at"]
        .as_str()
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .env("PATH", "")
        .args(["jar", "get-method"])
        .arg(&path)
        .arg("--at")
        .arg(id)
        .output()
        .unwrap();
    success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("this.serializeNulls = true;"));
}

#[test]
fn jvm_xref_locates_string_type_method_and_field_uses_with_owning_unit() {
    let path = input("flow-deflated.jar");

    // String literal reference: the ldc site in FlowFixture.text().
    let string_hits = jvm_jsonl(&run(&["xref", "-m", "前端", "--kind", "string"], &path));
    let refs: Vec<&Value> = string_hits
        .iter()
        .filter(|v| v["record"] == "reference")
        .collect();
    assert_eq!(refs.len(), 1);
    // Shared skeleton (the caller coordinate plus the matched target) and the reference
    // site's own evidence; the caller is not re-spelled inside detail.
    assert_keys(&refs[0]["detail"], &["byte_offset", "mnemonic", "target"]);
    assert_eq!(refs[0]["context_at"], method(&path, "text"));
    success(&run(
        &[
            "get-method",
            "--at",
            refs[0]["context_at"].as_str().unwrap(),
        ],
        &path,
    ));
    assert!(
        refs[0]["detail"]["target"]
            .as_str()
            .unwrap()
            .contains("前端")
    );

    // gson exercises real type/method/field cross-references.
    let gson = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/jvm/gson-2.11.0.jar");

    // Method call sites, with every hit carrying its owning class and method.
    let methods = jvm_jsonl(&run(&["xref", "-m", "toJson", "--kind", "method"], &gson));
    let method_refs: Vec<&Value> = methods
        .iter()
        .filter(|v| v["record"] == "reference")
        .collect();
    assert!(!method_refs.is_empty());
    assert!(method_refs.iter().all(|v| {
        v["context_at"].as_str().unwrap().contains("->")
            && v["detail"]["target"].as_str().unwrap().contains("toJson")
    }));

    // Owner narrowing names one exact class (ADR 0014): the full name matches, a
    // fragment of it is not a pattern and selects nothing.
    let filtered = jvm_jsonl(&run(
        &[
            "xref",
            "-m",
            "append",
            "--kind",
            "method",
            "--class",
            "java/lang/StringBuilder",
        ],
        &gson,
    ));
    let filtered_refs: Vec<&Value> = filtered
        .iter()
        .filter(|v| v["record"] == "reference")
        .collect();
    assert!(!filtered_refs.is_empty());
    assert!(filtered_refs.iter().all(|v| {
        v["detail"]["target"]
            .as_str()
            .unwrap()
            .contains("Ljava/lang/StringBuilder;->append")
    }));
    let fragment = jvm_jsonl(&run(
        &[
            "xref",
            "-m",
            "append",
            "--kind",
            "method",
            "--class",
            "StringBuilder",
        ],
        &gson,
    ));
    assert!(
        !fragment.iter().any(|v| v["record"] == "reference"),
        "--class must not behave like a substring filter"
    );

    // Type reference by substring.
    let types = jvm_jsonl(&run(&["xref", "-m", "JsonReader", "--kind", "type"], &gson));
    assert!(
        types
            .iter()
            .filter(|v| v["record"] == "reference")
            .all(|v| v["detail"]["target"]
                .as_str()
                .unwrap()
                .contains("JsonReader"))
    );
    assert!(types.iter().any(|v| v["record"] == "reference"));

    // Field access sites.
    let fields = jvm_jsonl(&run(&["xref", "-m", "factories", "--kind", "field"], &gson));
    let field_refs: Vec<&Value> = fields
        .iter()
        .filter(|v| v["record"] == "reference")
        .collect();
    assert!(!field_refs.is_empty());
    // `-f` is a case-insensitive substring, so the match may sit inside a longer name
    // (`builderFactories`); the field part is what the query named.
    assert!(field_refs.iter().all(|v| {
        let target = v["detail"]["target"].as_str().unwrap().to_lowercase();
        let field = target.rsplit("->").next().unwrap_or_default();
        field.contains("factories:")
    }));

    // No match is a clean, complete empty result.
    let empty = jvm_jsonl(&run(
        &["xref", "-m", "zzzznotexist", "--kind", "method"],
        &gson,
    ));
    assert!(empty.is_empty());

    // Clap rejects both anchor faces before the query runs.
    failure(
        run(
            &[
                "xref",
                "--at",
                "Lfixture/FlowFixture;",
                "-m",
                "add",
                "--kind",
                "method",
            ],
            &path,
        ),
        2,
        "cannot be used",
    );
}
