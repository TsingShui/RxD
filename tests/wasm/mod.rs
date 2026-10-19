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
        "id",
        "source_id",
        "parent_id",
        "context_id",
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
    assert!(row["source_id"].is_string());
    assert!(row["parent_id"].is_null());
    let fields: &[&str] = match row["record"].as_str().unwrap() {
        "type" => &["type_index", "params", "results"],
        "import" => &["module", "name", "kind", "func_index", "type_index"],
        "export" => &["name", "kind", "index"],
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
        "string" => &["origin", "segment_index", "offset", "value", "bytes"],
        "reference" => &[
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
    assert_keys(&row["detail"], fields);
    if row["record"] == "function_view" {
        for instruction in row["detail"]["instructions"].as_array().unwrap() {
            assert_keys(instruction, &["offset", "opcode", "operand"]);
        }
    }
    if row["record"] == "function" || row["record"] == "function_view" {
        assert!(row["id"].as_str().unwrap().starts_with("rxd1:wasm:"));
    } else {
        assert!(row["id"].is_null());
    }
    if row["record"] == "reference" {
        assert!(
            row["context_id"]
                .as_str()
                .unwrap()
                .starts_with("rxd1:wasm:")
        );
    } else {
        assert!(row["context_id"].is_null());
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
    assert_eq!(header["backend"], "wasm");
    assert_eq!(header["schema_version"], 2);
    assert!(header["input_bytes"].as_u64().unwrap() > 0);
    assert_keys(
        &header["detail"],
        &["analysis", "imported_funcs", "total_funcs"],
    );
    for row in &rows[1..rows.len() - 1] {
        assert_record(row);
        assert_eq!(row["source_id"], header["source_id"]);
    }
    let summary = rows.last().unwrap();
    assert_keys(summary, &["record", "records", "complete", "detail"]);
    assert_eq!(summary["record"], "summary");
    assert_eq!(summary["records"], rows.len() - 2);
    assert_eq!(summary["complete"], true);
    assert_keys(&summary["detail"], &[]);
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

fn func_id(path: &Path, index: u64) -> String {
    wasm_jsonl(&run("outline", path, &["--func", &index.to_string()]))
        .into_iter()
        .find(|row| row["record"] == "function")
        .unwrap_or_else(|| panic!("missing function {index}"))["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn get(path: &Path, id: &str) -> Value {
    let out = run("get-function", path, &[id]);
    success(&out);
    let row: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_record(&row);
    assert_eq!(row["detail"]["complete"], true);
    row
}

fn get_raw(path: &Path, id: &str) -> Value {
    let out = run("get-function", path, &[id, "--raw"]);
    success(&out);
    let row: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_record(&row);
    assert_eq!(row["detail"]["complete"], true);
    row
}

#[test]
fn wasm_get_returns_one_instruction_view_record_with_decoded_immediates() {
    let path = input();
    let id = func_id(&path, 1);
    let row = get_raw(&path, &id);
    assert_eq!(row["id"], id);
    assert_eq!(row["detail"]["representation"], "wasm");
    assert_eq!(row["detail"]["decompiled"], false);
    assert!(row["detail"]["reason"].is_null());
    assert!(row["detail"]["code"].is_null());
    assert_eq!(row["detail"]["func"]["func_index"], 1);
    assert_eq!(row["detail"]["func"]["name"], "sum");
    // Body byte range and raw bytes preserved.
    assert!(row["detail"]["body_file_offset"].as_u64().unwrap() > 0);
    assert!(row["detail"]["body_size"].as_u64().unwrap() > 0);
    let start = row["start"].as_u64().unwrap() as usize;
    let end = row["end"].as_u64().unwrap() as usize;
    let body_hex: String = fs::read(&path).unwrap()[start..end]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(row["detail"]["bytecode_hex"], body_hex);
    assert_keys(
        &row["detail"]["func"],
        &["func_index", "type_index", "name", "params", "results"],
    );

    let instructions = row["detail"]["instructions"].as_array().unwrap();
    assert!(!instructions.is_empty());
    // First instruction is an i32.const with a decoded immediate at a code-body offset.
    let first = &instructions[0];
    assert_eq!(first["opcode"], "i32.const");
    assert_eq!(first["operand"]["kind"], "i32");
    assert!(first["offset"].as_u64().is_some());
    // The function uses structured control flow and a conditional branch.
    let opcodes: Vec<&str> = instructions
        .iter()
        .map(|i| i["opcode"].as_str().unwrap())
        .collect();
    assert!(opcodes.contains(&"loop"));
    assert!(opcodes.contains(&"br_if"));
    assert!(opcodes.contains(&"local.set"));
}

#[test]
fn wasm_get_decompiles_structured_control_flow_by_default() {
    let path = input();
    let id = func_id(&path, 1);
    // Default get attempts decompilation.
    let row = get(&path, &id);
    assert_eq!(row["detail"]["representation"], "clike");
    assert_eq!(row["detail"]["decompiled"], true);
    assert!(row["detail"]["reason"].is_null());
    let code = row["detail"]["code"].as_str().unwrap();
    // Loop back-edge becomes a `loop`, forward branch a labeled `goto`.
    assert!(code.contains("function sum(a:int):int"));
    assert!(code.contains("loop "));
    assert!(code.contains("goto "));
    assert!(code.contains("return c;") || code.contains("return "));
    // Line -> code-body offset mapping is present.
    assert!(!row["detail"]["mapping"].as_array().unwrap().is_empty());
    // Instruction list is empty on the decompiled path.
    assert!(row["detail"]["instructions"].as_array().unwrap().is_empty());
}

#[test]
fn wasm_get_decompiles_calls_with_signature_driven_arity() {
    let path = input();
    // Function `run` calls the imported `log` (1 arg) and `sum` (1 arg).
    let id = func_id(&path, 2);
    let row = get(&path, &id);
    assert_eq!(row["detail"]["representation"], "clike");
    let code = row["detail"]["code"].as_str().unwrap();
    // Imported func takes exactly one argument; sum takes exactly one.
    assert!(code.contains("(42)"));
    assert!(code.contains("sum(10)"));
}

#[test]
fn wasm_get_raw_forces_instruction_view_over_decompilation() {
    let path = input();
    let id = func_id(&path, 1);
    let row = get_raw(&path, &id);
    assert_eq!(row["detail"]["representation"], "wasm");
    assert_eq!(row["detail"]["decompiled"], false);
    assert!(row["detail"]["code"].is_null());
    assert!(!row["detail"]["instructions"].as_array().unwrap().is_empty());
}

#[test]
fn wasm_outline_lists_all_structure_in_section_order_with_metadata_analysis() {
    let path = input();
    let rows = wasm_jsonl(&run("outline", &path, &[]));
    let header = &rows[0];
    assert_eq!(header["backend"], "wasm");
    assert_eq!(header["detail"]["analysis"], "metadata");
    assert_eq!(header["command"], "outline");
    // Merged index space: one imported function, three total.
    assert_eq!(header["detail"]["imported_funcs"], 1);
    assert_eq!(header["detail"]["total_funcs"], 3);

    // Every structural record kind is present.
    for kind in [
        "type", "import", "export", "function", "table", "memory", "global", "data", "element",
    ] {
        assert!(
            !records_of(&rows, kind).is_empty(),
            "missing outline record kind: {kind}"
        );
    }
}

#[test]
fn wasm_outline_assigns_content_bound_ids_in_merged_index_space() {
    let path = input();
    let rows = wasm_jsonl(&run("outline", &path, &[]));
    let source_id = rows[0]["source_id"].as_str().unwrap();
    let functions = records_of(&rows, "function");
    // Imported func (index 0) is not a defined function; defined start at 1.
    let indices: Vec<u64> = functions
        .iter()
        .map(|f| f["detail"]["func_index"].as_u64().unwrap())
        .collect();
    assert_eq!(indices, vec![1, 2]);
    for f in &functions {
        let func = &f["detail"];
        let index = func["func_index"].as_u64().unwrap();
        assert_eq!(
            f["id"].as_str().unwrap(),
            format!("rxd1:wasm:{source_id}:func:{index}")
        );
        // Body byte range within the module file is preserved.
        assert!(func["body_file_offset"].as_u64().unwrap() > 0);
        assert!(func["body_size"].as_u64().unwrap() > 0);
    }
}

#[test]
fn wasm_outline_imported_function_is_enumerated_but_not_a_defined_function() {
    let path = input();
    let rows = wasm_jsonl(&run("outline", &path, &[]));
    let imports = records_of(&rows, "import");
    let func_import = imports
        .iter()
        .find(|i| i["detail"]["kind"] == "func")
        .expect("imported function record");
    assert_eq!(func_import["detail"]["func_index"], 0);
    assert_eq!(func_import["detail"]["module"], "env");
    assert_eq!(func_import["detail"]["name"], "log");
    // No defined-function record carries index 0.
    assert!(
        records_of(&rows, "function")
            .iter()
            .all(|f| f["detail"]["func_index"] != 0)
    );
}

#[test]
fn wasm_outline_filters_by_exact_func_index_and_name_label() {
    let path = input();
    let by_index = wasm_jsonl(&run("outline", &path, &["--func", "1"]));
    let functions = records_of(&by_index, "function");
    assert_eq!(functions.len(), 1);
    assert_eq!(functions[0]["detail"]["func_index"], 1);
    // Filtered outline emits only the matching function, no other kinds.
    assert!(records_of(&by_index, "type").is_empty());
    assert_eq!(by_index[0]["query"]["func"], 1);

    let by_name = wasm_jsonl(&run("outline", &path, &["--func-name", "sum"]));
    let named = records_of(&by_name, "function");
    assert_eq!(named.len(), 1);
    assert_eq!(named[0]["detail"]["name"], "sum");
    assert_eq!(by_name[0]["query"]["func_name"], "sum");
}

#[test]
fn wasm_refs_finds_direct_call_and_ref_func_with_code_body_offsets() {
    let path = input();
    let rows = wasm_jsonl(&run("refs", &path, &["--func", "1"]));
    let refs = records_of(&rows, "reference");
    // `run` (index 2) references `sum` (index 1) via both ref.func and call.
    let opcodes: Vec<&str> = refs
        .iter()
        .map(|r| r["detail"]["opcode"].as_str().unwrap())
        .collect();
    assert!(opcodes.contains(&"call"));
    assert!(opcodes.contains(&"ref.func"));
    for r in &refs {
        let caller = get_raw(&path, r["context_id"].as_str().unwrap());
        assert_eq!(caller["detail"]["func"]["func_index"], 2);
        assert_eq!(r["detail"]["caller_index"], 2);
        assert_eq!(r["detail"]["target_index"], 1);
        // Offset is within the caller's code body.
        assert!(r["detail"]["offset"].as_u64().is_some());
    }
}

#[test]
fn wasm_strings_preserves_valid_utf8_and_raw_invalid_bytes() {
    let path = input();
    let rows = wasm_jsonl(&run("strings", &path, &[]));
    let strings = records_of(&rows, "string");
    let data: Vec<&Value> = strings
        .iter()
        .filter(|s| s["detail"]["origin"] == "data")
        .collect();
    // Valid UTF-8 run decodes to a JSON string with no raw bytes.
    let hello = data
        .iter()
        .find(|s| s["detail"]["value"] == "hello")
        .expect("valid utf-8 string");
    assert_eq!(hello["detail"]["segment_index"], 0);
    assert!(hello["detail"]["bytes"].is_null());
    // Invalid byte run keeps raw bytes with value null; no lossy replacement.
    let invalid = data
        .iter()
        .find(|s| s["detail"]["value"].is_null())
        .expect("invalid utf-8 run");
    assert!(invalid["detail"]["bytes"].is_array());
}

#[test]
fn wasm_strings_contains_matches_by_byte_subsequence() {
    let path = input();
    let rows = wasm_jsonl(&run("strings", &path, &["--contains", "ell"]));
    let strings = records_of(&rows, "string");
    assert!(
        strings
            .iter()
            .any(|s| s["detail"]["value"].as_str() == Some("hello"))
    );
    assert!(strings.iter().all(|s| s["detail"]["value"] != "world"));
}

#[test]
fn wasm_rejects_non_wasm_input_without_partial_stdout() {
    let (_dir, path) = fixture(b"not a wasm module".as_slice(), "bad.wasm");
    let out = run("outline", &path, &[]);
    failure(out, 1, "INVALID_INPUT");
}

#[test]
fn wasm_get_output_file_matches_stdout_bytes() {
    let path = input();
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("out.jsonl");
    let stdout = run("outline", &path, &[]);
    success(&stdout);
    let to_file = run("outline", &path, &["-o", out_path.to_str().unwrap()]);
    success(&to_file);
    assert!(to_file.stdout.is_empty(), "-o must not also write stdout");
    let file_bytes = fs::read(&out_path).unwrap();
    assert_eq!(file_bytes, stdout.stdout);
}

#[test]
fn wasm_invalid_func_index_query_is_rejected() {
    let path = input();
    // Index 0 is an imported function with no code body.
    let imported = run("refs", &path, &["--func", "0"]);
    success(&imported); // refs on an imported target is valid: it has callers potentially
    // Out-of-range function index for outline filter yields an empty function set.
    let rows = wasm_jsonl(&run("outline", &path, &["--func", "9999"]));
    assert!(records_of(&rows, "function").is_empty());
}

#[test]
fn wasm_get_rejects_imported_out_of_range_and_foreign_hash() {
    let path = input();
    let source_id = wasm_jsonl(&run("outline", &path, &[]))[0]["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // Imported function index 0 has no code body.
    failure(
        run(
            "get-function",
            &path,
            &[&format!("rxd1:wasm:{source_id}:func:0")],
        ),
        2,
        "INVALID_QUERY",
    );
    // Out-of-range index.
    failure(
        run(
            "get-function",
            &path,
            &[&format!("rxd1:wasm:{source_id}:func:9999")],
        ),
        2,
        "INVALID_QUERY",
    );
    // Foreign content hash.
    failure(
        run("get-function", &path, &["rxd1:wasm:deadbeef:func:1"]),
        2,
        "INVALID_QUERY",
    );
}

#[test]
fn wasm_get_scales_memory_indices_in_decompiled_output() {
    let path = scale_input();
    let source_id = wasm_jsonl(&run("outline", &path, &[]))[0]["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let row = get(&path, &format!("rxd1:wasm:{source_id}:func:0"));
    assert_eq!(row["detail"]["representation"], "clike");
    let code = row["detail"]["code"].as_str().unwrap();
    // Dynamic (base + (i << 2)) folds to array indexing base[i]:int.
    assert!(
        code.contains("[b]:int"),
        "expected dynamic index fold, got:\n{code}"
    );
    // Static byte offset 12 scales by element size 4 to index 3.
    assert!(
        code.contains("[3]:int"),
        "expected static offset scaling, got:\n{code}"
    );
    // The raw byte-offset form must not survive for these aligned accesses.
    assert!(
        !code.contains("[12]:int"),
        "unscaled offset leaked:\n{code}"
    );
    assert!(!code.contains("<< 2"), "unscaled shift leaked:\n{code}");
}

#[test]
fn wasm_get_degrades_to_instruction_view_on_uncovered_opcode() {
    let path = simd_input();
    let source_id = wasm_jsonl(&run("outline", &path, &[]))[0]["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let row = get(&path, &format!("rxd1:wasm:{source_id}:func:0"));
    // Degraded, but the record is still complete; no guessed code.
    assert_eq!(row["detail"]["representation"], "wasm");
    assert_eq!(row["detail"]["decompiled"], false);
    assert_eq!(row["detail"]["code"], Value::Null);
    assert_eq!(row["detail"]["reason"], "UNSUPPORTED_OPCODE: v128.load");
    // Instructions decoded before the uncovered opcode are preserved.
    let instructions = row["detail"]["instructions"].as_array().unwrap();
    assert_eq!(instructions.len(), 1);
    assert_eq!(instructions[0]["opcode"], "i32.const");
    // Raw body bytes are always preserved for the fallback.
    assert!(!row["detail"]["bytecode_hex"].as_str().unwrap().is_empty());
}
