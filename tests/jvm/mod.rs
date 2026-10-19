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
        "id",
        "source_id",
        "parent_id",
        "context_id",
        "start",
        "end",
        "detail",
    ];
    if row["record"] == "bytecode" {
        keys.extend(["backend", "command", "schema_version"]);
        assert_eq!(row["backend"], "jvm");
        assert_eq!(row["command"], "get-method");
        assert_eq!(row["schema_version"], 2);
    }
    assert_keys(row, &keys);
    assert!(row["source_id"].is_string());
    assert!(row["parent_id"].is_null());
    let fields: &[&str] = match row["record"].as_str().unwrap() {
        "class" => &[
            "entry",
            "name",
            "super",
            "interfaces",
            "major",
            "minor",
            "access",
            "signature",
            "source_file",
            "fields",
            "methods",
        ],
        "field" => &[
            "entry",
            "class",
            "name",
            "descriptor",
            "access",
            "signature",
            "constant_index",
        ],
        "method" => &[
            "entry",
            "class",
            "name",
            "descriptor",
            "access",
            "signature",
            "exceptions",
            "has_code",
        ],
        "string" => &["entry", "class", "index", "value", "utf16"],
        "reference" => &[
            "entry",
            "class",
            "method",
            "method_descriptor",
            "byte_offset",
            "mnemonic",
            "target",
        ],
        "bytecode" => &[
            "representation",
            "entry",
            "class",
            "name",
            "descriptor",
            "max_stack",
            "max_locals",
            "handlers",
            "instructions",
            "complete",
        ],
        other => panic!("unexpected JVM record: {other}"),
    };
    assert_keys(&row["detail"], fields);
    if row["record"] == "method" || row["record"] == "bytecode" {
        assert!(row["id"].as_str().unwrap().starts_with("rxd1:jvm:"));
    } else {
        assert!(row["id"].is_null());
    }
    if row["record"] == "reference" {
        assert!(row["context_id"].as_str().unwrap().starts_with("rxd1:jvm:"));
    } else {
        assert!(row["context_id"].is_null());
    }
    if row["record"] != "bytecode" {
        assert!(row["start"].is_null());
        assert!(row["end"].is_null());
    }
}

fn jvm_jsonl(output: &Output) -> Vec<Value> {
    let rows = jsonl(output);
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
    assert_eq!(header["backend"], "jvm");
    assert_eq!(header["schema_version"], 2);
    assert!(header["input_bytes"].as_u64().unwrap() > 0);
    assert_keys(&header["detail"], &["analysis", "release"]);
    assert_eq!(header["detail"]["analysis"], "metadata");
    for row in &rows[1..rows.len() - 1] {
        assert_record(row);
    }
    let summary = rows.last().unwrap();
    assert_keys(summary, &["record", "records", "complete", "detail"]);
    assert_eq!(summary["record"], "summary");
    assert_eq!(summary["records"], rows.len() - 2);
    assert_eq!(summary["complete"], true);
    if header["command"] == "outline" {
        assert_keys(&summary["detail"], &["classes"]);
        assert_eq!(
            summary["detail"]["classes"],
            rows.iter().filter(|r| r["record"] == "class").count()
        );
    } else {
        assert_keys(&summary["detail"], &[]);
    }
    rows
}

fn methods(path: &Path) -> Vec<Value> {
    jvm_jsonl(&run(&["outline"], path))
        .into_iter()
        .filter(|v| v["record"] == "method")
        .collect()
}

fn method(path: &Path, name: &str) -> String {
    methods(path)
        .into_iter()
        .find(|v| v["detail"]["name"] == name)
        .unwrap()["id"]
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
    let rows = jvm_jsonl(&run(&["outline", "--release", "17"], &path));
    let selected = rows
        .iter()
        .find(|v| v["record"] == "method" && v["detail"]["name"] == "add")
        .unwrap();
    assert_ne!(selected["id"], base_id);
    assert_eq!(
        selected["detail"]["entry"],
        "META-INF/versions/17/fixture/FlowFixture.class"
    );
    let output = run(
        &[
            "get-method",
            selected["id"].as_str().unwrap(),
            "--release",
            "17",
        ],
        &path,
    );
    success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("return (left - right);"));
    failure(
        run(&["get-method", &base_id, "--release", "17"], &path),
        2,
        "INVALID_QUERY",
    );
    failure(
        run(&["outline", "--max-entries", "1"], &path),
        1,
        "RESOURCE_LIMIT",
    );
    let duplicate = directory.path().join("duplicate.jar");
    write(
        &duplicate,
        &[("first.class", &original), ("second.class", &original)],
    );
    failure(
        run(&["outline"], &duplicate),
        1,
        "duplicate class definition",
    );
}

#[test]
fn jvm_native_methods_recover_control_flow_values_calls_and_exceptions() {
    let path = input("fixture/FlowFixture.class");
    let rows = methods(&path);
    for (name, expected) in [
        ("add", "return (left + right);"),
        ("absolute", "(- value)"),
        ("sum", "< count"),
        ("wide", "value + (long scale)"),
        // RadGuard escapes non-ASCII, so the literal arrives escaped.
        ("text", "\\u524d\\u7aef\\u0000"),
        ("length", "values.length"),
        ("choose", "case 100:"),
        ("dense", "case 2:"),
        ("guarded", "catch (RuntimeException"),
        ("array", "new int[count]"),
        ("element", "values[index]"),
        ("flag", "return values[index];"),
        ("narrowBranch", "(byte value)"),
        ("root", "Math.sqrt(value)"),
        ("merged", "new Object()"),
        ("create", "new fixture.FlowFixture(value)"),
        // The backend parenthesizes every compound expression, so the assertion
        // pins the assignment rather than the exact operand spelling.
        ("increment", "this.value = ("),
        ("discard", "text.length();"),
        ("locked", "static synchronized"),
        ("block", "synchronized(lock)"),
    ] {
        let id = rows.iter().find(|v| v["detail"]["name"] == name).unwrap()["id"]
            .as_str()
            .unwrap();
        let output = run(&["get-method", id], &path);
        success(&output);
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(expected),
            "{name}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn jvm_stored_and_deflated_jars_preserve_class_content_ids_and_original_bytecode() {
    let class = input("fixture/FlowFixture.class");
    let id = method(&class, "add");
    let java = run(&["get-method", &id], &class);
    success(&java);
    assert_eq!(
        java.stdout,
        b"public static int add(int left, int right)\n{\n    return (left + right);\n}\n"
    );
    let raw = run(&["get-method", &id, "--bytecode"], &class);
    success(&raw);
    let record: Value = serde_json::from_slice(&raw.stdout).unwrap();
    assert_record(&record);
    assert_eq!(record["record"], "bytecode");
    assert_eq!(record["id"], id);
    assert_eq!(record["detail"]["representation"], "bytecode");
    assert_eq!(record["detail"]["complete"], true);
    assert_eq!(record["detail"]["max_stack"], 2);
    let bytes: Vec<u8> = record["detail"]["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|i| {
            i["bytes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| b.as_u64().unwrap() as u8)
        })
        .collect();
    // Independent javap -c expectation for FlowFixture.add(int,int).
    assert_eq!(bytes, [0x1a, 0x1b, 0x60, 0xac]);
    assert_eq!(record["detail"]["instructions"][2]["byte_offset"], 2);
    let start = record["start"].as_u64().unwrap() as usize;
    let end = record["end"].as_u64().unwrap() as usize;
    assert_eq!(&fs::read(&class).unwrap()[start..end], bytes);
    for instruction in record["detail"]["instructions"].as_array().unwrap() {
        assert_keys(
            instruction,
            &[
                "byte_offset",
                "opcode",
                "mnemonic",
                "bytes",
                "operands",
                "targets",
                "constant_index",
                "reference",
            ],
        );
    }
    for name in ["flow-stored.jar", "flow-deflated.jar"] {
        let path = input(name);
        assert_eq!(method(&path, "add"), id);
        let output = run(&["get-method", &id], &path);
        success(&output);
        assert_eq!(output.stdout, run(&["get-method", &id], &class).stdout);
        let raw = run(&["get-method", &id, "--bytecode"], &path);
        success(&raw);
        let jar: Value = serde_json::from_slice(&raw.stdout).unwrap();
        assert_record(&jar);
        assert_eq!(jar["detail"]["entry"], "fixture/FlowFixture.class");
        assert_eq!(
            jar["detail"]["instructions"],
            record["detail"]["instructions"]
        );
    }
}

#[test]
fn jvm_content_identity_rejects_stale_ids_and_failures_preserve_output_files() {
    let path = input("fixture/FlowFixture.class");
    let id = method(&path, "add");
    failure(
        run(&["get-method", &id], &input("fixture/ModernFixture.class")),
        2,
        "INVALID_QUERY",
    );
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("method.java");
    fs::write(&destination, "existing").unwrap();
    let output = run(
        &[
            "get-method",
            &id,
            "-o",
            destination.to_str().unwrap(),
            "--max-work",
            "1",
        ],
        &path,
    );
    failure(output, 1, "RESOURCE_LIMIT");
    assert_eq!(fs::read(&destination).unwrap(), b"existing");
    let output = run(
        &["get-method", &id, "-o", destination.to_str().unwrap()],
        &path,
    );
    success(&output);
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(&destination).unwrap(),
        run(&["get-method", &id], &path).stdout
    );
}

#[test]
fn jvm_modern_dynamic_bytecode_remains_extractable_without_guessed_java() {
    let path = input("fixture/ModernFixture.class");
    for name in ["concat", "lambda"] {
        let id = method(&path, name);
        let raw = run(&["get-method", &id, "--bytecode"], &path);
        success(&raw);
        let record: Value = serde_json::from_slice(&raw.stdout).unwrap();
        assert!(
            record["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["mnemonic"] == "invokedynamic")
        );
        failure(run(&["get-method", &id], &path), 1, "UNSUPPORTED_BYTECODE");
    }
    let path = input("fixture/InterfaceSuperFixture.class");
    let id = method(&path, "call");
    success(&run(&["get-method", &id, "--bytecode"], &path));
    failure(
        run(&["get-method", &id], &path),
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
    let (_dir, path) = fixture(bytes, "amplified.class");
    failure(
        run(&["outline", "--max-class-bytes", "2000"], &path),
        1,
        "metadata budget",
    );
}

#[test]
fn jvm_corrupt_branches_stack_underflow_and_budgets_fail_without_partial_output() {
    let path = input("fixture/FlowFixture.class");
    failure(
        run(&["outline", "--max-input-bytes", "10"], &path),
        1,
        "RESOURCE_LIMIT",
    );
    failure(
        run(&["outline", "--max-output-bytes", "10"], &path),
        1,
        "RESOURCE_LIMIT",
    );
    failure(
        run(
            &["outline", "--max-class-bytes", "10"],
            &input("flow-deflated.jar"),
        ),
        1,
        "RESOURCE_LIMIT",
    );
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
        failure(run(&["get-method", &id], &changed), 1, diagnostic);
    }
    let (_dir, truncated) = fixture(&original[..original.len() - 1], "truncated.class");
    failure(run(&["outline"], &truncated), 1, "INVALID_INPUT");
    let mut padded = original.clone();
    let switch = padded
        .windows(4)
        .position(|w| w == [0x1a, 0xab, 0, 0])
        .unwrap();
    padded[switch + 2] = 0xff;
    padded[switch + 3] = 0x7f;
    let (_dir, changed) = fixture(padded, "padding.class");
    let id = method(&changed, "choose");
    let result = run(&["get-method", &id], &changed);
    success(&result);
    assert!(String::from_utf8_lossy(&result.stdout).contains("case 100:"));
    let mut archive = fs::read(input("flow-stored.jar")).unwrap();
    let trailer = archive
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .unwrap();
    archive[trailer + 16..trailer + 20].copy_from_slice(&u32::MAX.to_le_bytes());
    let (_dir, zip64) = fixture(archive, "zip64.jar");
    failure(run(&["outline"], &zip64), 1, "UNSUPPORTED_INPUT");
}

#[test]
fn jvm_pinned_production_gson_queries_do_not_require_java_or_d8() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/jvm/gson-2.11.0.jar");
    let rows = methods(&path);
    assert_eq!(rows.len(), 1210);
    let filtered = jvm_jsonl(&run(
        &[
            "outline",
            "--class",
            "com.google.gson.GsonBuilder",
            "--method",
            "serializeNulls",
        ],
        &path,
    ));
    assert_eq!(filtered.len(), 4);
    assert_eq!(filtered[2]["detail"]["name"], "serializeNulls");
    let descriptor_filter = jvm_jsonl(&run(
        &[
            "outline",
            "--class",
            "Lcom/google/gson/GsonBuilder;",
            "--method",
            "serializeNulls",
        ],
        &path,
    ));
    assert_eq!(filtered[2], descriptor_filter[2]);
    let id = rows
        .iter()
        .find(|v| {
            v["detail"]["class"] == "com/google/gson/GsonBuilder"
                && v["detail"]["name"] == "serializeNulls"
        })
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .env("PATH", "")
        .args(["jar", "get-method"])
        .arg(&path)
        .arg(id)
        .output()
        .unwrap();
    success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("this.serializeNulls = true;"));
}

#[test]
fn jvm_get_class_assembles_whole_class_and_degrades_undecodable_members_honestly() {
    let path = input("flow-deflated.jar");
    let whole = run(&["get-class", "fixture.FlowFixture"], &path);
    success(&whole);
    let source = String::from_utf8_lossy(&whole.stdout);
    // One source file with the class header, its field, and multiple method bodies.
    assert!(source.contains("public class FlowFixture {"));
    assert!(source.contains("private int value;"));
    assert!(source.contains("public static int add(int left, int right)\n    {"));
    assert!(source.contains("public static int sum(int count)\n    {"));
    // Descriptor form selects the same class.
    let by_descriptor = run(&["get-class", "Lfixture/FlowFixture;"], &path);
    success(&by_descriptor);
    assert_eq!(whole.stdout, by_descriptor.stdout);
    // A missing class is an INVALID_QUERY with no partial stdout.
    let missing = run(&["get-class", "fixture.Absent"], &path);
    failure(missing.clone(), 2, "INVALID_QUERY");
    assert!(missing.stdout.is_empty());
    // -o writes the source atomically and keeps stdout empty.
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("FlowFixture.java");
    let written = run(
        &[
            "get-class",
            "fixture.FlowFixture",
            "-o",
            destination.to_str().unwrap(),
        ],
        &path,
    );
    success(&written);
    assert!(written.stdout.is_empty());
    assert_eq!(fs::read(&destination).unwrap(), whole.stdout);

    // A method whose body cannot be decoded degrades to a throwing stub rather
    // than failing the whole class or emitting guessed Java.
    let degraded = run(
        &["get-class", "Lfixture/InterfaceSuperFixture;"],
        &input("fixture/InterfaceSuperFixture.class"),
    );
    success(&degraded);
    let text = String::from_utf8_lossy(&degraded.stdout);
    assert!(text.contains("class InterfaceSuperFixture"));
    assert!(text.contains("throw new UnsupportedOperationException("));
}

#[test]
fn jvm_strings_lists_class_constant_literals_with_navigation_identity() {
    let path = input("flow-deflated.jar");
    let rows = jvm_jsonl(&run(&["strings"], &path));
    let header = &rows[0];
    assert_eq!(header["record"], "header");
    assert_eq!(header["command"], "strings");
    let input_bytes = fs::read(&path).unwrap();
    assert_eq!(header["input_bytes"], input_bytes.len());
    assert_eq!(
        header["source_id"],
        format!("{:x}", Sha256::digest(&input_bytes))
    );
    assert_eq!(header["detail"]["release"], 8);
    let summary = rows.last().unwrap();
    assert_eq!(summary["record"], "summary");
    assert_eq!(summary["complete"], true);
    let strings: Vec<&Value> = rows.iter().filter(|v| v["record"] == "string").collect();
    // The Unicode + NUL + emoji literal from FlowFixture.text() is preserved exactly,
    // and carries the owning class so a reader can route to get-class.
    let unicode = strings
        .iter()
        .find(|v| v["detail"]["value"] == "前端\u{0}😀")
        .expect("unicode literal present");
    assert_eq!(unicode["detail"]["class"], "fixture/FlowFixture");
    assert_eq!(unicode["detail"]["entry"], "fixture/FlowFixture.class");
    assert!(unicode["detail"]["utf16"].is_null());
    assert_eq!(
        unicode["source_id"],
        format!(
            "{:x}",
            Sha256::digest(fs::read(input("fixture/FlowFixture.class")).unwrap())
        )
    );
    assert!(unicode["detail"]["index"].as_u64().is_some());
    // Only string literals, never method/field names, class names or descriptors.
    assert!(
        strings
            .iter()
            .all(|v| v["detail"]["value"] != "add" && v["detail"]["value"] != "value")
    );
    // Case-insensitive filter narrows the listing.
    let filtered = jvm_jsonl(&run(&["strings", "--filter", "前端"], &path));
    let filtered_strings: Vec<&Value> = filtered
        .iter()
        .filter(|v| v["record"] == "string")
        .collect();
    assert_eq!(filtered_strings.len(), 1);
    assert_eq!(filtered_strings[0]["detail"]["value"], "前端\u{0}😀");
    // --class scopes to one class and still emits a complete header/summary stream.
    let scoped = jvm_jsonl(&run(&["strings", "--class", "fixture.FlowFixture"], &path));
    assert_eq!(scoped.last().unwrap()["complete"], true);
    assert!(scoped.iter().any(|v| v["record"] == "string"));
    let absent = jvm_jsonl(&run(&["strings", "--class", "fixture.Absent"], &path));
    assert!(!absent.iter().any(|v| v["record"] == "string"));
    assert_eq!(absent.last().unwrap()["complete"], true);
}

#[test]
fn jvm_xref_locates_string_type_method_and_field_uses_with_owning_unit() {
    let path = input("flow-deflated.jar");

    // String literal reference: the ldc site in FlowFixture.text().
    let string_hits = jvm_jsonl(&run(&["xref", "string", "前端"], &path));
    assert_eq!(string_hits[0]["command"], "xref");
    let refs: Vec<&Value> = string_hits
        .iter()
        .filter(|v| v["record"] == "reference")
        .collect();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0]["detail"]["class"], "fixture/FlowFixture");
    assert_eq!(refs[0]["detail"]["method"], "text");
    assert_eq!(refs[0]["context_id"], method(&path, "text"));
    success(&run(
        &["get-method", refs[0]["context_id"].as_str().unwrap()],
        &path,
    ));
    assert_eq!(refs[0]["detail"]["mnemonic"], "ldc");
    assert!(
        refs[0]["detail"]["target"]
            .as_str()
            .unwrap()
            .contains("前端")
    );

    // gson exercises real type/method/field cross-references.
    let gson = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/jvm/gson-2.11.0.jar");

    // Method call sites, with every hit carrying its owning class and method.
    let methods = jvm_jsonl(&run(&["xref", "method", "toJson"], &gson));
    let method_refs: Vec<&Value> = methods
        .iter()
        .filter(|v| v["record"] == "reference")
        .collect();
    assert!(!method_refs.is_empty());
    assert!(method_refs.iter().all(|v| {
        v["detail"]["class"].is_string()
            && v["detail"]["method"].is_string()
            && v["detail"]["target"].as_str().unwrap().contains("toJson")
    }));

    // Owner-class substring filter narrows method hits to the matching owner.
    let filtered = jvm_jsonl(&run(
        &["xref", "method", "append", "--class", "StringBuilder"],
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
            .contains("StringBuilder.append")
    }));

    // Type reference by substring.
    let types = jvm_jsonl(&run(&["xref", "type", "JsonReader"], &gson));
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
    let fields = jvm_jsonl(&run(&["xref", "field", "factories"], &gson));
    let field_refs: Vec<&Value> = fields
        .iter()
        .filter(|v| v["record"] == "reference")
        .collect();
    assert!(!field_refs.is_empty());
    assert!(field_refs.iter().all(|v| {
        v["detail"]["target"]
            .as_str()
            .unwrap()
            .contains(".factories:")
    }));

    // No match is a clean, complete empty result.
    let empty = jvm_jsonl(&run(&["xref", "method", "zzzznotexist"], &gson));
    assert!(!empty.iter().any(|v| v["record"] == "reference"));
    assert_eq!(empty.last().unwrap()["complete"], true);
}
