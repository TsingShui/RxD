//! Static WebAssembly CLI contracts using a deterministic hand-written fixture.
use super::*;

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
    if row["record"] == "function_view" {
        keys.extend(["backend", "command", "schema_version"]);
        assert_eq!(row["backend"], "wasm");
        assert_eq!(row["command"], "get-function");
        assert_eq!(row["schema_version"], 2);
    }
    assert_keys(row, &keys);
    // The header carries content identity; a row never repeats it.
    assert!(row["source_id"].is_null());
    assert!(row["parent_at"].is_null());
    let fields: &[&str] = match row["record"].as_str().unwrap() {
        "type" => &["type_index", "params", "results"],
        "import" => &[
            "module",
            "name",
            "kind",
            "func_index",
            "type_index",
            "target_at",
        ],
        "export" => &["name", "kind", "index", "target_at"],
        "function" => &[
            "func_index",
            "type_index",
            "name",
            "params",
            "results",
            "body_file_offset",
            "body_size",
        ],
        "table" => &["index", "element_type", "initial", "maximum"],
        "memory" => &["index", "initial", "maximum", "memory64", "shared"],
        "global" => &["index", "value_type", "mutable", "name"],
        "data" => &[
            "index",
            "kind",
            "memory_index",
            "offset_expr",
            "byte_length",
        ],
        "element" => &["index", "kind", "count"],
        "string" => &[
            "index",
            "origin",
            "segment_index",
            "offset",
            "value",
            "bytes",
        ],
        "reference" => &[
            "target",
            "caller_index",
            "caller_name",
            "offset",
            "opcode",
            "target_index",
        ],
        "function_view" => &[
            "representation",
            "decompiled",
            "reason",
            "code",
            "func",
            "body_file_offset",
            "body_size",
            "locals",
            "instructions",
            "mapping",
            "bytecode_hex",
            "complete",
        ],
        other => panic!("unexpected wasm record: {other}"),
    };
    if row["record"] == "function_view" {
        // The original bytecode view follows `--raw` (and stays in the
        // instruction fallback); a decompiled answer does not repeat it.
        let required = [
            "representation",
            "decompiled",
            "reason",
            "code",
            "func",
            "body_file_offset",
            "body_size",
            "locals",
            "mapping",
            "complete",
        ];
        let optional = ["instructions", "bytecode_hex"];
        let keys: Vec<&str> = row["detail"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for key in required {
            assert!(keys.contains(&key), "missing {key} in {:?}", row["detail"]);
        }
        for key in &keys {
            assert!(
                required.contains(key) || optional.contains(key),
                "unexpected {key} in {:?}",
                row["detail"]
            );
        }
    } else {
        assert_keys(&row["detail"], fields);
    }
    if row["record"] == "function_view" {
        for instruction in row["detail"]
            .get("instructions")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
        {
            assert_keys(instruction, &["offset", "opcode", "operand"]);
        }
    }
    // Every row that has an index-space identity prints its short coordinate.
    let coordinate = match row["record"].as_str().unwrap() {
        "type" => Some(format!(
            "type:{}",
            row["detail"]["type_index"].as_u64().unwrap()
        )),
        "function" => Some(format!("func:{}", row["detail"]["func_index"])),
        "global" | "table" | "memory" | "data" | "element" => Some(format!(
            "{}:{}",
            row["record"].as_str().unwrap(),
            row["detail"]["index"]
        )),
        "function_view" => Some(format!("func:{}", row["detail"]["func"]["func_index"])),
        "import" | "export" => Some(format!("{}:", row["record"].as_str().unwrap())),
        "string" | "reference" => None,
        other => panic!("unexpected wasm record: {other}"),
    };
    match coordinate {
        Some(prefix) => {
            let at = row["at"].as_str().expect("catalog rows print an at");
            assert!(at.starts_with(&prefix), "{at} does not start with {prefix}");
        }
        None => assert!(row["at"].is_null(), "{} has no identity", row["record"]),
    }
    if row["record"] == "reference" {
        let caller = row["context_at"].as_str().unwrap();
        assert!(caller.starts_with("func:"));
        assert!(
            row["detail"]["target"]
                .as_str()
                .unwrap()
                .starts_with("func:")
        );
    } else {
        assert!(row["context_at"].is_null());
    }
    if row["record"] == "function" || row["record"] == "function_view" {
        let start = row["start"].as_u64().unwrap();
        let end = row["end"].as_u64().unwrap();
        assert!(end > start);
        assert_eq!(row["start"], row["detail"]["body_file_offset"]);
        assert_eq!(end - start, row["detail"]["body_size"]);
    } else {
        assert!(row["start"].is_null());
        assert!(row["end"].is_null());
    }
}

fn wasm_jsonl(output: &Output) -> Vec<Value> {
    let rows = jsonl(output);
    for row in &rows {
        assert_record(row);
    }
    rows
}

fn input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm/fixture.wasm")
}

fn simd_input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm/simd.wasm")
}

fn scale_input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm/scale.wasm")
}

fn run(command: &str, path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["wasm", command])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

fn records_of(rows: &[Value], record: &str) -> Vec<Value> {
    rows.iter()
        .filter(|row| row["record"] == record)
        .cloned()
        .collect()
}

/// The `at` a `functions` row prints, which is exactly what `--at` accepts.
fn func_at(path: &Path, index: u64) -> String {
    wasm_jsonl(&run("functions", path, &["--at", &format!("func:{index}")]))
        .into_iter()
        .find(|row| row["record"] == "function")
        .unwrap_or_else(|| panic!("missing function {index}"))["at"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn get(path: &Path, at: &str) -> Value {
    let out = run("get-function", path, &["--at", at]);
    success(&out);
    let row: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_record(&row);
    assert_eq!(row["detail"]["complete"], true);
    row
}

fn get_raw(path: &Path, at: &str) -> Value {
    let out = run("get-function", path, &["--at", at, "--raw"]);
    success(&out);
    let row: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_record(&row);
    assert_eq!(row["detail"]["complete"], true);
    row
}
#[test]
fn wasm_xref_rows_carry_the_caller_coordinate_and_target() {
    let path = input();
    let rows = wasm_jsonl(&run("xref", &path, &["--at", "func:1"]));
    let refs = records_of(&rows, "reference");
    // `run` (index 2) references `sum` (index 1) via both ref.func and call.
    let opcodes: Vec<&str> = refs
        .iter()
        .map(|r| r["detail"]["opcode"].as_str().unwrap())
        .collect();
    assert!(opcodes.contains(&"call"));
    assert!(opcodes.contains(&"ref.func"));
    for r in &refs {
        assert_eq!(r["context_at"], "func:2");
        assert_eq!(r["detail"]["target"], "func:1");
        assert_eq!(r["detail"]["caller_index"], 2);
        assert_eq!(r["detail"]["target_index"], 1);
        // Offset is within the caller's code body.
        assert!(r["detail"]["offset"].as_u64().is_some());
        // The caller coordinate a row prints is directly fetchable.
        let caller = get_raw(&path, r["context_at"].as_str().unwrap());
        assert_eq!(caller["detail"]["func"]["func_index"], 2);
    }
}

#[test]
fn wasm_xref_searches_target_labels_and_preserves_instruction_order() {
    let path = input();
    let exact = records_of(
        &wasm_jsonl(&run("xref", &path, &["--at", "func:1"])),
        "reference",
    );
    let folded = wasm_jsonl(&run("xref", &path, &["--kind", "function", "-m", "SUM"]));
    assert_eq!(records_of(&folded, "reference"), exact);
    let strict = wasm_jsonl(&run(
        "xref",
        &path,
        &["--kind", "function", "-m", "SUM", "--case", "sensitive"],
    ));
    assert!(records_of(&strict, "reference").is_empty());
    // The caller is run, but references to sum must not match a caller-name search.
    let callers = wasm_jsonl(&run("xref", &path, &["--kind", "function", "-m", "run"]));
    assert!(records_of(&callers, "reference").is_empty());
    let imports = wasm_jsonl(&run(
        "xref",
        &path,
        &["--kind", "function", "-m", "ENV.LOG"],
    ));
    let imported = records_of(&imports, "reference");
    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0]["detail"]["target"], "func:0");
    let all = wasm_jsonl(&run("xref", &path, &["--kind", "function", "-m", ""]));
    let all_refs = records_of(&all, "reference");
    assert_eq!(all_refs.len(), exact.len() + imported.len());
    assert_eq!(all_refs[0], imported[0]);
    assert_eq!(&all_refs[1..], exact.as_slice());
    failure(run("xref", &path, &["-m", "sum"]), 2, "--kind");
    failure(
        run(
            "xref",
            &path,
            &["--at", "func:1", "-m", "sum", "--kind", "function"],
        ),
        2,
        "cannot be used",
    );
    let wrong = run("xref", &path, &["--at", "func:1", "--kind", "type"]);
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("type"));
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("function"));
    failure(wrong, 2, "invalid value 'type'");
    failure(run("xref", &path, &["--at", "func:9999"]), 2, "NOT_FOUND");
    for at in ["func:+1", "func:01", "func:-1", "type:0"] {
        failure(run("xref", &path, &["--at", at]), 2, "INVALID_QUERY");
    }
}
#[test]
fn wasm_rejects_non_wasm_input_without_partial_stdout() {
    let (_dir, path) = fixture(b"not a wasm module".as_slice(), "bad.wasm");
    let out = run("types", &path, &[]);
    failure(out, 1, "INVALID_INPUT");
}

#[test]
fn wasm_output_file_matches_stdout_bytes() {
    let path = input();
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("out.jsonl");
    let stdout = run("functions", &path, &[]);
    success(&stdout);
    let to_file = run("functions", &path, &["-o", out_path.to_str().unwrap()]);
    success(&to_file);
    assert!(to_file.stdout.is_empty(), "-o must not also write stdout");
    let file_bytes = fs::read(&out_path).unwrap();
    assert_eq!(file_bytes, stdout.stdout);
}

#[test]
fn wasm_listings_expose_no_budget_flags() {
    let path = input();
    for flag in [
        "--max-input-bytes",
        "--max-entities",
        "--max-work",
        "--max-instructions",
        "--max-records",
        "--max-output-bytes",
    ] {
        let out = run("functions", &path, &[flag, "1"]);
        failure(out, 2, "unexpected argument");
    }
}

#[test]
fn wasm_coordinates_validate_existence_before_name_filters() {
    let path = input();
    // Index 0 is an imported function with no code body; scanning callers is valid.
    success(&run("xref", &path, &["--at", "func:0"]));
    // A content search may be empty; a coordinate must name a real listed row.
    let rows = wasm_jsonl(&run("functions", &path, &["--name", "missing"]));
    assert!(records_of(&rows, "function").is_empty());
    for at in ["func:0", "func:9999"] {
        failure(
            run("functions", &path, &["--at", at, "-f", "missing"]),
            2,
            "NOT_FOUND",
        );
    }
}

#[test]
fn wasm_get_resolves_a_function_coordinate() {
    let path = input();
    let row = get(&path, "func:1");
    assert_eq!(row["at"], "func:1");
    // Imported function index 0 has no code body.
    failure(
        run("get-function", &path, &["--at", "func:0"]),
        2,
        "INVALID_QUERY",
    );
    // Out-of-range index.
    failure(
        run("get-function", &path, &["--at", "func:9999"]),
        2,
        "NOT_FOUND",
    );
    // Malformed coordinates are rejected, never guessed.
    failure(
        run("get-function", &path, &["--at", "bogus"]),
        2,
        "INVALID_QUERY",
    );
    // A coordinate of another kind is not a function.
    failure(
        run("get-function", &path, &["--at", "type:0"]),
        2,
        "INVALID_QUERY",
    );
}
