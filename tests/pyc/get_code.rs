use super::*;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/pyc/3.12.14")
        .join(name)
}
fn fetch(path: &Path, at: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "get-code"])
        .arg(path)
        .arg("--at")
        .arg(at)
        .args(args)
        .output()
        .unwrap()
}
fn code_ids(path: &Path, args: &[&str]) -> Vec<Value> {
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "codes"])
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    success(&output);
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .filter(|r: &Value| r["record"] == "code")
        .collect()
}
#[test]
fn pyc_get_code_fetches_one_independent_instruction_view_from_codes() {
    let path = corpus("graph.timestamp.pyc");
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("graph.oracle.json")).unwrap()).unwrap();
    let rows = code_ids(&path, &[]);
    for (index, code) in oracle["codes"].as_array().unwrap().iter().enumerate() {
        let output = fetch(&path, rows[index]["at"].as_str().unwrap(), &[]);
        success(&output);
        let view: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(view["record"], "code_view");
        assert_eq!(view["schema_version"], 2);
        assert_eq!(view["command"], "get-code");
        assert_eq!(view["at"], rows[index]["at"]);
        assert_eq!(view["start"], rows[index]["start"]);
        assert_eq!(view["end"], rows[index]["end"]);
        assert_eq!(view["detail"]["bytecode_hex"], code["code_hex"]);
        assert_eq!(view["detail"]["metadata"], rows[index]["detail"]);
        let units = view["detail"]["instructions"].as_array().unwrap();
        assert_eq!(units.len(), code["code_hex"].as_str().unwrap().len() / 4);
        for expected in code["dis_instructions"].as_array().unwrap() {
            let offset = expected["offset"].as_u64().unwrap();
            let actual = units.iter().find(|r| r["offset"] == offset).unwrap();
            assert_eq!(actual["opcode"], expected["opcode"]);
            assert_eq!(actual["name"], expected["opname"]);
        }
    }
}

#[test]
fn pyc_get_code_preserves_arbitrary_cache_payload_without_interpreting_it() {
    let path = corpus("graph.timestamp.pyc");
    let mut bytes = fs::read(&path).unwrap();
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("graph.oracle.json")).unwrap()).unwrap();
    let start = oracle["marshal_ranges"]["code_ranges"][0]["pyc_bytecode_start"]
        .as_u64()
        .unwrap() as usize;
    bytes[start + 60] = 100;
    bytes[start + 61] = 255;
    let (_dir, path) = fixture(bytes, "cache.pyc");
    let ids = code_ids(&path, &[]);
    let output = fetch(&path, ids[0]["at"].as_str().unwrap(), &[]);
    success(&output);
    let view: Value = serde_json::from_slice(&output.stdout).unwrap();
    let cache = view["detail"]["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["offset"] == 60)
        .unwrap();
    assert_eq!(cache["role"], "cache");
    assert_eq!(cache["opcode"], 100);
    assert_eq!(cache["name"], "LOAD_CONST");
    assert_eq!(cache["raw_hex"], "64ff");
    assert_eq!(cache["status"], "RAW_CACHE");
    assert_eq!(cache["owner_offset"], 58);
    assert!(cache["operand"].is_null());
    assert_eq!(cache["targets"], serde_json::json!([]));
}

#[test]
fn pyc_get_code_checks_ids_selected_body_and_malformed_evidence_atomically() {
    let original = corpus("graph.timestamp.pyc");
    let bytes = fs::read(&original).unwrap();
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("graph.oracle.json")).unwrap()).unwrap();
    let rows = code_ids(&original, &[]);
    let id = rows[0]["at"].as_str().unwrap();
    for bad in [
        id.replace("code:", "const:"),
        id.replace("code:0", "code:01"),
        format!("{id}x"),
        "code:".into(),
    ] {
        failure(fetch(&original, &bad, &[]), 2, "INVALID_QUERY");
    }
    // A canonical coordinate that names no code is not malformed (ADR 0015).
    failure(fetch(&original, "code:99999", &[]), 2, "NOT_FOUND");
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("keep.json");
    let nested_start = oracle["marshal_ranges"]["code_ranges"][11]["pyc_bytecode_start"]
        .as_u64()
        .unwrap() as usize;
    let mut broken = bytes.clone();
    broken[nested_start + 1] = 255; // RESUME stays structurally valid; mutate a table operand below.
    broken[nested_start + 8] = 100;
    broken[nested_start + 9] = 255;
    let damaged = dir.path().join("operand.pyc");
    fs::write(&damaged, &broken).unwrap();
    let fresh = code_ids(&damaged, &[]);
    success(&fetch(&damaged, fresh[0]["at"].as_str().unwrap(), &[])); // unrelated body stays unscanned.
    fs::write(&dest, b"keep").unwrap();
    failure(
        fetch(
            &damaged,
            fresh[11]["at"].as_str().unwrap(),
            &["-o", dest.to_str().unwrap()],
        ),
        1,
        "INVALID_INPUT",
    );
    assert_eq!(fs::read(&dest).unwrap(), b"keep");
    assert_eq!(fs::read(&damaged).unwrap(), broken);
    // A coordinate names a code object, not a file: it resolves in whatever input holds
    // that index.
    success(&fetch(&damaged, id, &[]));
    // Every corruption is inside evidence from one independently produced code object.
    for (code, field, offset, value) in [
        (10, "code", 19, 255),
        (10, "code", 19, 3),
        (11, "exception_table", 0, 0),
        (11, "exception_table", 1, 127),
        (11, "exception_table", 2, 127),
        (11, "line_table", 0, 0),
        (11, "line_table", 1, 128),
    ] {
        let mut changed = bytes.clone();
        let start = rows[code]["detail"]["fields"][field]["payload_start"]
            .as_u64()
            .unwrap() as usize;
        changed[start + offset] = value;
        let path = dir.path().join("corrupt.pyc");
        fs::write(&path, &changed).unwrap();
        let codes = code_ids(&path, &[]);
        let output = fetch(
            &path,
            codes[code]["at"].as_str().unwrap(),
            &["-o", dest.to_str().unwrap()],
        );
        failure(output, 1, "INVALID_INPUT");
        assert_eq!(fs::read(&dest).unwrap(), b"keep");
        assert_eq!(fs::read(path).unwrap(), changed);
    }
}

fn perf_row(output: &Output) -> Value {
    String::from_utf8(output.stderr.clone())
        .unwrap()
        .lines()
        .filter_map(|s| serde_json::from_str::<Value>(s).ok())
        .find(|r| r["scope"] == "analysis")
        .unwrap()
}
#[test]
fn pyc_get_code_budgets_are_exact_cumulative_and_fail_atomically_with_perf() {
    let path = corpus("graph.timestamp.pyc");
    let ids = code_ids(&path, &[]);
    let id = ids[11]["at"].as_str().unwrap();
    let complete = fetch(&path, id, &["--perf"]);
    success(&complete);
    let perf = perf_row(&complete);
    let budget =
        rxd_core::Budget::for_input_bytes(fs::metadata(&path).unwrap().len() as usize).unwrap();
    assert_eq!(
        perf["resource_limits"]["instructions"],
        Value::from(budget.instructions)
    );
    assert_eq!(perf["counters"]["instructions"], 99);
    assert!(perf["counters"]["ast_depth"].is_null());
    assert_json_output_boundary(&complete);
    assert_no_budget_flags(&path, "get-code");
    // Exact accounting lives in the library, which owns the shared ceilings.
    let mut base = shared_static_limits();
    base.input_bytes = fs::metadata(&path).unwrap().len() as usize;
    let (exact_perf, evidence) = get_code_library(&path, None, id, base, 99);
    let evidence = evidence.unwrap();
    assert_eq!(evidence.instructions.len(), 99);
    let exact_perf = perf_value(&exact_perf);
    let work = exact_perf["counters"]["work"].as_u64().unwrap() as usize;
    let entities = exact_perf["counters"]["entities"].as_u64().unwrap() as usize;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("keep.json");
    for (field, amount) in [
        ("input_bytes", base.input_bytes),
        ("entities", entities),
        ("work", work),
        ("instructions", 99),
    ] {
        let mut exact = base;
        match field {
            "input_bytes" => exact.input_bytes = amount,
            "entities" => exact.entities = amount,
            "work" => exact.work = amount,
            _ => {}
        }
        assert!(
            get_code_library(&path, None, id, exact, 99).1.is_ok(),
            "{field}"
        );
        let mut less = exact;
        match field {
            "input_bytes" => less.input_bytes = amount - 1,
            "entities" => less.entities = amount - 1,
            "work" => less.work = amount - 1,
            _ => {}
        }
        let instructions = if field == "instructions" { 98 } else { 99 };
        let error = get_code_library(&path, None, id, less, instructions)
            .1
            .err()
            .unwrap();
        assert_eq!(error.code, "RESOURCE_LIMIT", "{field}");
    }
    // A failed selection never replaces an existing output file.
    fs::write(&out, b"keep").unwrap();
    failure(
        fetch(&path, "code:99999", &["-o", out.to_str().unwrap()]),
        2,
        "NOT_FOUND",
    );
    assert_eq!(fs::read(&out).unwrap(), b"keep");
}

#[test]
fn pyc_get_code_matches_extended_operands_tables_locations_and_file_spans() {
    for label in ["graph", "extended"] {
        let path = corpus(&format!("{label}.timestamp.pyc"));
        let input = fs::read(&path).unwrap();
        let oracle: Value =
            serde_json::from_slice(&fs::read(corpus(&format!("{label}.oracle.json"))).unwrap())
                .unwrap();
        let _hash = format!("{:x}", Sha256::digest(&input));
        let ids = code_ids(&path, &[]);
        for (index, code) in oracle["codes"].as_array().unwrap().iter().enumerate() {
            let output = fetch(&path, ids[index]["at"].as_str().unwrap(), &[]);
            success(&output);
            let view: Value = serde_json::from_slice(&output.stdout).unwrap();
            let d = &view["detail"];
            assert_eq!(d["line_table_hex"], code["linetable_hex"]);
            assert_eq!(d["exception_table_hex"], code["exceptiontable_hex"]);
            let mut previous = 0;
            for unit in d["instructions"].as_array().unwrap() {
                assert_eq!(unit["offset"], previous);
                previous += unit["size"].as_u64().unwrap();
                let start = unit["start"].as_u64().unwrap() as usize;
                let end = unit["end"].as_u64().unwrap() as usize;
                assert_eq!(
                    unit["raw_hex"],
                    input[start..end]
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                );
                let off = unit["offset"].as_u64().unwrap();
                let expected = code["dis_instructions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| r["offset"] == off)
                    .unwrap();
                assert_eq!(unit["raw_operand"], expected["raw_arg_byte"]);
                if unit["role"] == "instruction" {
                    assert_eq!(unit["operand"], expected["arg"]);
                }
                let position = &code["positions"][(off / 2) as usize];
                let location = d["line_mapping"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| {
                        r["start"].as_u64().unwrap() <= off && r["end"].as_u64().unwrap() > off
                    })
                    .unwrap();
                assert_eq!(
                    serde_json::json!([
                        location["line"],
                        location["end_line"],
                        location["column"],
                        location["end_column"]
                    ]),
                    *position
                );
                if expected["argrepr"].as_str().unwrap().starts_with("to ") {
                    assert_eq!(unit["jump_target"], expected["argval"]["value"]);
                }
                for target in unit["targets"].as_array().unwrap() {
                    let value = d["values"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|v| v["id"] == target["value_id"])
                        .unwrap();
                    if target["table"] == "names" || target["table"] == "localsplus" {
                        assert_eq!(value["value"], expected["argval"]["value"]);
                    }
                    assert_eq!(
                        d["tables"][target["table"].as_str().unwrap()]["entries"]
                            [target["index"].as_u64().unwrap() as usize]["id"],
                        target["id"]
                    );
                }
            }
            assert_eq!(
                previous,
                code["code_hex"].as_str().unwrap().len() as u64 / 2
            );
            let exceptions:Vec<Value>=d["exceptions"].as_array().unwrap().iter().map(|e|serde_json::json!({"start":e["start"],"end":e["end"],"target":e["target"],"depth":e["depth"],"lasti":e["lasti"]})).collect();
            assert_eq!(exceptions, *code["exceptions"].as_array().unwrap());
            for (entry, expected) in d["tables"]["consts"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .zip(code["consts"].as_array().unwrap())
            {
                let value = d["values"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|v| v["id"] == entry["value_id"])
                    .unwrap();
                if expected["type"] == "str" {
                    assert_eq!(value["value"], expected["value"]);
                    assert_eq!(value["raw_hex"], expected["utf8_surrogatepass_hex"]);
                }
                if expected["type"] == "bytes" {
                    assert!(value["value"].is_null());
                    assert_eq!(value["raw_hex"], expected["hex"]);
                }
                if expected["type"] == "code" {
                    let index = expected["index"].as_u64().unwrap() as usize;
                    assert_eq!(value["code_id"], ids[index]["at"]);
                }
                for range in [
                    ("definition_start", "definition_end"),
                    ("payload_start", "payload_end"),
                ] {
                    if let (Some(start), Some(end)) =
                        (value[range.0].as_u64(), value[range.1].as_u64())
                    {
                        assert!(end >= start && end <= input.len() as u64);
                    }
                }
            }
        }
    }
}

#[test]
fn pyc_get_code_keeps_unknown_raw_opcodes_and_raw_marshal_but_rejects_damaged_boundaries() {
    let original = corpus("graph.timestamp.pyc");
    let bytes = fs::read(&original).unwrap();
    let ids = code_ids(&original, &[]);
    let start = ids[0]["detail"]["fields"]["code"]["payload_start"]
        .as_u64()
        .unwrap() as usize;
    for opcode in [17, 255] {
        let mut changed = bytes.clone();
        changed[start] = opcode;
        changed[start + 1] = 255;
        let (_dir, path) = fixture(&changed, "unknown.pyc");
        let ids = code_ids(&path, &[]);
        let out = fetch(&path, ids[0]["at"].as_str().unwrap(), &[]);
        success(&out);
        let view: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(view["detail"]["instructions"][0]["opcode"], opcode);
        assert_eq!(view["detail"]["instructions"][0]["raw_operand"], 255);
        assert_eq!(
            view["detail"]["instructions"][0]["status"],
            "UNSUPPORTED_OPCODE"
        );
    }
    let raw = corpus("graph.marshal");
    let raw_ids = code_ids(&raw, &["--version", "3.12"]);
    let out = fetch(
        &raw,
        raw_ids[0]["at"].as_str().unwrap(),
        &["--version", "3.12"],
    );
    success(&out);
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(view["detail"]["query"]["version"], "3.12");
    assert_eq!(view["start"].as_u64().unwrap() + 16, ids[0]["start"]);
    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("view.json");
    success(&fetch(
        &raw,
        raw_ids[0]["at"].as_str().unwrap(),
        &["--version", "3.12", "-o", saved.to_str().unwrap()],
    ));
    assert_eq!(fs::read(&saved).unwrap(), out.stdout);
    let end = ids[0]["detail"]["fields"]["code"]["payload_end"]
        .as_u64()
        .unwrap() as usize;
    let mut damaged = bytes.clone();
    damaged[end - 2] = 144;
    let (_dir, path) = fixture(damaged, "prefix.pyc");
    let fresh = code_ids(&path, &[]);
    failure(
        fetch(&path, fresh[0]["at"].as_str().unwrap(), &[]),
        1,
        "INVALID_INPUT",
    );
    failure(
        fetch(&raw, raw_ids[0]["at"].as_str().unwrap(), &[]),
        1,
        "INVALID_INPUT",
    );
    failure(
        fetch(
            &raw,
            raw_ids[0]["at"].as_str().unwrap(),
            &["--version", "3.14"],
        ),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn pyc_get_code_has_fixed_schema_with_aliases_and_a_distinct_missing_body_error() {
    let path = corpus("graph.timestamp.pyc");
    let ids = code_ids(&path, &[]);
    let out = fetch(&path, ids[0]["at"].as_str().unwrap(), &[]);
    success(&out);
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_js_keys(
        &view,
        &[
            "schema_version",
            "backend",
            "command",
            "record",
            "at",
            "source_id",
            "parent_at",
            "context_at",
            "start",
            "end",
            "detail",
        ],
    );
    let d = &view["detail"];
    assert_js_keys(
        d,
        &[
            "input_bytes",
            "python_version",
            "representation",
            "complete",
            "query",
            "metadata",
            "bytecode_hex",
            "instructions",
            "tables",
            "values",
            "line_table_hex",
            "line_mapping",
            "exception_table_hex",
            "exceptions",
        ],
    );
    assert_js_keys(&d["query"], &["code_id", "version"]);
    assert_js_keys(
        &d["tables"],
        &[
            "consts",
            "names",
            "localsplus",
            "varnames",
            "freevars",
            "cellvars",
        ],
    );
    assert_js_keys(
        &d["instructions"][0],
        &[
            "offset",
            "size",
            "start",
            "end",
            "opcode",
            "name",
            "raw_operand",
            "operand",
            "raw_hex",
            "role",
            "owner_offset",
            "logical_start",
            "logical_size",
            "prefixes",
            "jump_target",
            "targets",
            "status",
            "line",
        ],
    );
    assert_js_keys(
        &d["tables"]["consts"],
        &[
            "present",
            "start",
            "end",
            "definition_start",
            "definition_end",
            "entries",
        ],
    );
    assert_js_keys(
        &d["tables"]["consts"]["entries"][0],
        &[
            "index",
            "id",
            "value_id",
            "start",
            "end",
            "alias_start",
            "alias_end",
            "reference",
            "kind",
        ],
    );
    assert_js_keys(
        &d["values"][0],
        &[
            "id",
            "tag",
            "kind",
            "value",
            "raw_hex",
            "definition_start",
            "definition_end",
            "payload_start",
            "payload_end",
            "children",
            "code_id",
        ],
    );
    assert_js_keys(
        &d["line_mapping"][0],
        &[
            "start",
            "end",
            "line",
            "end_line",
            "column",
            "end_column",
            "table_start",
            "table_end",
        ],
    );
    let entries = d["tables"]["consts"]["entries"].as_array().unwrap();
    let a = &entries[22];
    let b = &entries[23]; // independently pinned shared string slots.
    assert_ne!(a["id"], b["id"]);
    assert_eq!(a["value_id"], b["value_id"]);
    assert_ne!(a["start"], b["start"]);
    // A static outline may retain a body that cannot be read; FETCH must fail explicitly.
    let mut bytes = vec![b'c'];
    for v in [0_u32, 0, 0, 2, 0] {
        bytes.extend(v.to_le_bytes());
    }
    bytes.extend(b"N)\0)\0)\0s\0\0\0\0z\x01xz\x01xz\x01x");
    bytes.extend(1_u32.to_le_bytes());
    bytes.extend(b"s\0\0\0\0s\0\0\0\0");
    let (_dir, path) = fixture(bytes, "missing.marshal");
    let ids = code_ids(&path, &["--version", "3.12"]);
    assert_eq!(ids[0]["detail"]["body_status"], "unavailable");
    failure(
        fetch(
            &path,
            ids[0]["at"].as_str().unwrap(),
            &["--version", "3.12"],
        ),
        1,
        "INVALID_INPUT",
    );
}

fn selected_fixture(body: &[u8], line: &[u8], exceptions: &[u8]) -> Vec<u8> {
    fn payload(out: &mut Vec<u8>, b: &[u8]) {
        out.push(b's');
        out.extend((b.len() as u32).to_le_bytes());
        out.extend(b);
    }
    let mut out = vec![b'c'];
    for value in [0_u32, 0, 0, 2, 0] {
        out.extend(value.to_le_bytes());
    }
    payload(&mut out, body);
    out.extend(b")\0)\0)\0");
    payload(&mut out, b"");
    out.extend(b"z\x01xz\x01xz\x01x");
    out.extend(1_u32.to_le_bytes());
    payload(&mut out, line);
    payload(&mut out, exceptions);
    out
}
#[test]
fn pyc_get_code_rejects_truncated_and_overflowing_selected_instructions_and_tables() {
    for (body, line, exception) in [
        (&[100][..], &[][..], &[][..]),
        (&[144, 1][..], &[][..], &[][..]),
        (&[171, 0][..], &[][..], &[][..]),
        (&[144, 1, 1, 0][..], &[][..], &[][..]),
        (
            &[144, 1, 144, 1, 144, 1, 144, 1, 100, 0][..],
            &[][..],
            &[][..],
        ),
        (&[140, 255][..], &[][..], &[][..]),
        (&[110, 255][..], &[][..], &[][..]),
        (&[110, 1, 122, 0, 0, 0][..], &[][..], &[][..]),
        (&[9, 0][..], &[0x80][..], &[][..]),
        (&[9, 0][..], &[0xe8, 0x40][..], &[][..]),
        (&[9, 0][..], &[0x87, 0][..], &[][..]),
        (
            &[9, 0][..],
            &[0xe8, 0x7f, 0x7f, 0x7f, 0x7f, 0x7f, 0x7f][..],
            &[][..],
        ),
        (&[9, 0][..], &[][..], &[0x80][..]),
        (&[9, 0][..], &[][..], &[0x80, 1, 1, 0][..]),
        (
            &[9, 0][..],
            &[][..],
            &[0xff, 0x7f, 0x7f, 0x7f, 0x7f, 0x7f][..],
        ),
    ] {
        let (_dir, path) = fixture(selected_fixture(body, line, exception), "selected.marshal");
        let ids = code_ids(&path, &["--version", "3.12"]);
        failure(
            fetch(
                &path,
                ids[0]["at"].as_str().unwrap(),
                &["--version", "3.12"],
            ),
            1,
            "INVALID_INPUT",
        );
    }
    let (_dir, path) = fixture(
        selected_fixture(&[9, 0], &[], &[0x80, 1, 0, 0]),
        "boundary.marshal",
    );
    let ids = code_ids(&path, &["--version", "3.12"]);
    let output = fetch(
        &path,
        ids[0]["at"].as_str().unwrap(),
        &["--version", "3.12"],
    );
    success(&output);
    let view: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(view["detail"]["exceptions"][0]["end"], 2);
    assert_eq!(view["detail"]["exceptions"][0]["target"], 0);
}
