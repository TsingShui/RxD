use super::*;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/pyc/3.12.14")
        .join(name)
}
fn refs(path: &Path, name: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "xref"])
        .arg(path)
        .args(["--name", name])
        .args(args)
        .output()
        .unwrap()
}
fn rows(output: &Output) -> Vec<Value> {
    success(output);
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
fn code_rows(path: &Path, args: &[&str]) -> Vec<Value> {
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "codes"])
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    rows(&output)
        .into_iter()
        .filter(|r| r["record"] == "code")
        .collect()
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
fn pyc_refs_route_exact_explicit_names_to_original_code() {
    let path = corpus("graph.timestamp.pyc");
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("graph.oracle.json")).unwrap()).unwrap();
    let stream = rows(&refs(&path, "marker", &[]));
    let hits: Vec<_> = stream
        .iter()
        .filter(|r| r["record"] == "reference")
        .collect();
    let expected: Vec<_> = oracle["codes"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .flat_map(|(index, code)| {
            code["dis_instructions"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|ins| ins["argval"]["value"] == "marker")
                .map(move |ins| (index, ins))
        })
        .collect();
    assert_eq!(hits.len(), expected.len());
    assert_eq!(stream.last().unwrap()["records"], hits.len());
    assert_eq!(stream.last().unwrap()["complete"], true);
    for (hit, (index, ins)) in hits.iter().zip(expected) {
        assert!(hit["at"].is_null());
        assert_eq!(hit["detail"]["code_index"], index);
        assert_eq!(hit["detail"]["offset"], ins["offset"]);
        assert_eq!(hit["detail"]["opcode"], ins["opcode"]);
        assert_eq!(hit["detail"]["name"], ins["opname"]);
        assert_eq!(hit["detail"]["raw_operand"], ins["raw_arg_byte"]);
        assert_eq!(hit["detail"]["operand"], ins["arg"]);
        let start = oracle["marshal_ranges"]["code_ranges"][index]["pyc_bytecode_start"]
            .as_u64()
            .unwrap();
        assert_eq!(
            hit["start"].as_u64().unwrap(),
            start + ins["offset"].as_u64().unwrap()
        );
        assert_eq!(
            hit["end"].as_u64().unwrap(),
            hit["start"].as_u64().unwrap() + 2
        );
        let fetch = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["pyc", "get-code"])
            .arg(&path)
            .arg("--at")
            .arg(hit["context_at"].as_str().unwrap())
            .output()
            .unwrap();
        success(&fetch);
        let view: Value = serde_json::from_slice(&fetch.stdout).unwrap();
        assert_eq!(
            view["detail"]["bytecode_hex"],
            oracle["codes"][index]["code_hex"]
        );
        let instruction = view["detail"]["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|unit| unit["offset"] == hit["detail"]["offset"])
            .unwrap();
        assert_eq!(instruction["targets"][0], hit["detail"]["target"]);
    }
    let empty = rows(&refs(&path, "mark", &[]));
    assert_eq!(empty.len(), 2);
    assert_eq!(empty[1]["records"], 0);
    assert_eq!(empty[1]["complete"], true);
}

#[test]
fn pyc_refs_distinguish_all_explicit_name_operations_with_independent_operands() {
    let path = corpus("refs.timestamp.pyc");
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("refs.oracle.json")).unwrap()).unwrap();
    let operations = [
        ("LOAD_NAME", "name_read"),
        ("STORE_NAME", "name_write"),
        ("DELETE_NAME", "name_delete"),
        ("LOAD_GLOBAL", "global_read"),
        ("STORE_GLOBAL", "global_write"),
        ("DELETE_GLOBAL", "global_delete"),
        ("LOAD_FAST", "local_read"),
        ("LOAD_FAST_CHECK", "local_read"),
        ("STORE_FAST", "local_write"),
        ("DELETE_FAST", "local_delete"),
        ("LOAD_FAST_AND_CLEAR", "local_read_clear"),
        ("MAKE_CELL", "cell_create"),
        ("LOAD_CLOSURE", "closure_capture"),
        ("LOAD_DEREF", "closure_read"),
        ("STORE_DEREF", "closure_write"),
        ("DELETE_DEREF", "closure_delete"),
        ("LOAD_ATTR", "attribute_read"),
        ("LOAD_SUPER_ATTR", "attribute_read"),
        ("STORE_ATTR", "attribute_write"),
        ("DELETE_ATTR", "attribute_delete"),
        ("IMPORT_NAME", "import_name"),
        ("IMPORT_FROM", "import_from"),
        ("LOAD_FROM_DICT_OR_GLOBALS", "dict_or_global_read"),
        ("LOAD_FROM_DICT_OR_DEREF", "dict_or_closure_read"),
    ];
    let codes = oracle["codes"].as_array().unwrap();
    let mut names = std::collections::BTreeSet::new();
    let mut covered = std::collections::BTreeSet::new();
    for code in codes {
        for ins in code["dis_instructions"].as_array().unwrap() {
            if operations.iter().any(|(op, _)| ins["opname"] == *op) {
                names.insert(ins["argval"]["value"].as_str().unwrap());
                covered.insert(ins["opname"].as_str().unwrap());
            }
        }
    }
    assert_eq!(covered.len(), operations.len()); // Fixture/compiler really exercises every family.
    for name in names {
        let stream = rows(&refs(&path, name, &[]));
        let hits: Vec<_> = stream
            .iter()
            .filter(|r| r["record"] == "reference")
            .collect();
        let expected: Vec<_> = codes
            .iter()
            .enumerate()
            .flat_map(|(index, code)| {
                code["dis_instructions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|ins| {
                        operations.iter().any(|(op, _)| ins["opname"] == *op)
                            && ins["argval"]["value"] == name
                    })
                    .map(move |ins| (index, ins))
            })
            .collect();
        assert_eq!(hits.len(), expected.len(), "name={name}");
        for (hit, (index, ins)) in hits.into_iter().zip(expected) {
            assert_eq!(hit["detail"]["code_index"], index);
            assert_eq!(hit["detail"]["offset"], ins["offset"]);
            assert_eq!(hit["detail"]["name"], ins["opname"]);
            let operation = operations
                .iter()
                .find(|(op, _)| ins["opname"] == *op)
                .unwrap()
                .1;
            assert_eq!(hit["detail"]["operation"], operation);
            assert_eq!(hit["detail"]["operand"], ins["arg"]);
            let operand = ins["arg"].as_u64().unwrap() as usize;
            let opname = ins["opname"].as_str().unwrap();
            let local = opname.contains("FAST")
                || opname.contains("DEREF")
                || matches!(opname, "MAKE_CELL" | "LOAD_CLOSURE");
            let table = if local { "localsplus" } else { "names" };
            let slot = match opname {
                "LOAD_GLOBAL" | "LOAD_ATTR" => operand >> 1,
                "LOAD_SUPER_ATTR" => operand >> 2,
                _ => operand,
            };
            assert_eq!(hit["detail"]["target"]["table"], table);
            assert_eq!(hit["detail"]["target"]["index"], slot);
            let table_names = if local {
                &codes[index]["localsplus_names_by_operand"]
            } else {
                &codes[index]["names"]
            };
            assert_eq!(table_names[slot], name);
        }
    }
    // Genuine compiler EXTENDED_ARG resolves a local above the one-byte range.
    let stream = rows(&refs(&corpus("extended.timestamp.pyc"), "local_299", &[]));
    let hits: Vec<_> = stream
        .iter()
        .filter(|r| r["record"] == "reference")
        .collect();
    assert_eq!(hits.len(), 2);
    for hit in hits {
        assert_eq!(hit["detail"]["raw_operand"], 43);
        assert_eq!(hit["detail"]["operand"], 299);
        assert_eq!(hit["detail"]["target"]["index"], 299);
    }
}

#[test]
fn pyc_refs_validate_single_code_scope_ids_and_fixed_stream_schema() {
    let path = corpus("graph.timestamp.pyc");
    let all = refs(&path, "value", &["--perf"]);
    let stream = rows(&all);
    let codes = code_rows(&path, &[]);
    let id = codes[4]["at"].as_str().unwrap();
    let narrow = refs(&path, "value", &["--within", id, "--perf"]);
    let selected = rows(&narrow);
    let expected: Vec<_> = stream
        .iter()
        .filter(|r| r["record"] == "reference" && r["context_at"] == id)
        .cloned()
        .collect();
    assert_eq!(selected[1..selected.len() - 1], expected);
    assert_eq!(selected.last().unwrap()["detail"]["scanned_code_count"], 1);
    assert_eq!(selected[0]["query"]["within"], id);
    assert_eq!(selected[0]["detail"]["scope"], "selected_code");
    assert_eq!(perf_row(&narrow)["counters"]["instructions"], 9);
    assert!(
        perf_row(&narrow)["counters"]["work"].as_u64().unwrap()
            < perf_row(&all)["counters"]["work"].as_u64().unwrap()
    );
    // Shared code aliases are scanned once; distinct owners and actual offsets remain separate.
    assert_eq!(
        stream.last().unwrap()["detail"]["scanned_code_count"],
        codes.len()
    );
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("graph.oracle.json")).unwrap()).unwrap();
    let units: usize = oracle["codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["code_hex"].as_str().unwrap().len() / 4)
        .sum();
    assert_eq!(perf_row(&all)["counters"]["instructions"], units);
    assert_js_keys(
        &stream[0],
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
    assert_js_keys(&stream[0]["query"], &["name", "at", "within", "version"]);
    assert_js_keys(
        &stream[0]["detail"],
        &[
            "analysis",
            "python_version",
            "code_count",
            "scope",
            "instruction_validation",
        ],
    );
    assert!(stream[0]["query"]["within"].is_null());
    assert!(stream[0]["query"]["at"].is_null());
    assert!(stream[0]["query"]["version"].is_null());
    assert_eq!(stream[0]["query"]["name"], "value");
    assert_eq!(stream[0]["command"], "xref");
    assert_eq!(stream[0]["schema_version"], 2);
    for hit in &stream[1..stream.len() - 1] {
        assert_js_keys(
            hit,
            &[
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
        assert!(hit["at"].is_null());
        assert!(hit["parent_at"].is_null());
        for field in [
            "constant_id",
            "constant_slot",
            "constant_path",
            "literal_kind",
            "relation",
        ] {
            assert!(hit["detail"][field].is_null());
        }
        assert_js_keys(
            &hit["detail"],
            &[
                "code_index",
                "offset",
                "opcode",
                "name",
                "raw_operand",
                "operand",
                "target",
                "operation",
                "constant_id",
                "constant_slot",
                "constant_path",
                "literal_kind",
                "relation",
            ],
        );
        assert_js_keys(
            &hit["detail"]["target"],
            &["table", "index", "id", "value_id"],
        );
        assert!(hit["source_id"].is_null());
    }
    let summary = stream.last().unwrap();
    assert_js_keys(summary, &["record", "records", "complete", "detail"]);
    assert_js_keys(
        &summary["detail"],
        &[
            "analysis",
            "code_count",
            "scanned_code_count",
            "instructions",
            "unsupported_count",
            "unsupported",
            "instruction_validation",
        ],
    );
    assert_eq!(summary["detail"]["unsupported"], serde_json::json!([]));
    for bad in [
        id.replace("code:", "const:"),
        id.replace("code:4", "code:04"),
        id.replace("code:4", "code:9999"),
        format!("{id}:0"),
        "rxd1:pyc:0:code:4".into(),
    ] {
        failure(
            refs(&path, "value", &["--within", &bad]),
            2,
            "INVALID_QUERY",
        );
    }
    let foreign = format!("rxd1:pyc:{}:{id}", "a".repeat(64));
    failure(
        refs(&path, "value", &["--within", &foreign]),
        1,
        "SOURCE_MISMATCH",
    );
    let (_dir, changed) = fixture(
        fs::read(&path)
            .unwrap()
            .into_iter()
            .enumerate()
            .map(|(i, b)| if i == 8 { b ^ 1 } else { b })
            .collect::<Vec<_>>(),
        "changed.pyc",
    );
    success(&refs(&changed, "value", &["--within", id]));
    let original = format!(
        "rxd1:pyc:{:x}:{id}",
        Sha256::digest(fs::read(&path).unwrap())
    );
    failure(
        refs(&changed, "value", &["--within", &original]),
        1,
        "SOURCE_MISMATCH",
    );
    let raw = corpus("graph.marshal");
    let raw_id = code_rows(&raw, &["--version", "3.12"])[4]["at"]
        .as_str()
        .unwrap()
        .to_owned();
    let raw_stream = rows(&refs(
        &raw,
        "value",
        &["--version", "3.12", "--within", &raw_id],
    ));
    assert_eq!(raw_stream[0]["query"]["version"], "3.12");
    for key in [
        "code_index",
        "offset",
        "opcode",
        "name",
        "raw_operand",
        "operand",
        "operation",
    ] {
        assert_eq!(raw_stream[1]["detail"][key], selected[1]["detail"][key]);
    }
    assert_eq!(
        raw_stream[1]["start"].as_u64().unwrap() + 16,
        selected[1]["start"].as_u64().unwrap()
    );
    let missing = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "xref"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
}

#[test]
fn pyc_refs_fail_scanned_corruption_atomically_and_expose_unsupported_coverage() {
    let original = corpus("graph.timestamp.pyc");
    let bytes = fs::read(&original).unwrap();
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("graph.oracle.json")).unwrap()).unwrap();
    let nested = oracle["marshal_ranges"]["code_ranges"][11]["pyc_bytecode_start"]
        .as_u64()
        .unwrap() as usize;
    let mut broken = bytes.clone();
    broken[nested + 6] = 100;
    broken[nested + 7] = 255; // LOAD_CONST invalid even for an unrelated name.
    let (dir, damaged) = fixture(&broken, "damaged.pyc");
    let ids = code_rows(&damaged, &[]);
    let output = dir.path().join("keep.jsonl");
    fs::write(&output, b"keep").unwrap();
    failure(
        refs(&damaged, "absent", &["-o", output.to_str().unwrap()]),
        1,
        "INVALID_INPUT",
    );
    assert_eq!(fs::read(&output).unwrap(), b"keep");
    let safe = rows(&refs(
        &damaged,
        "absent",
        &["--within", ids[0]["at"].as_str().unwrap()],
    ));
    assert_eq!(safe.last().unwrap()["complete"], true); // Only selected body semantics scanned.
    let start = oracle["marshal_ranges"]["code_ranges"][0]["pyc_bytecode_start"]
        .as_u64()
        .unwrap() as usize;
    let mut unknown = bytes.clone();
    unknown[start] = 17;
    let unknown_path = dir.path().join("unknown.pyc");
    fs::write(&unknown_path, &unknown).unwrap();
    let incomplete = rows(&refs(&unknown_path, "absent", &[]));
    assert_eq!(incomplete.len(), 2);
    assert_eq!(incomplete[1]["records"], 0);
    assert_eq!(incomplete[1]["complete"], false);
    assert_eq!(incomplete[1]["detail"]["unsupported_count"], 1);
    let diagnostic = &incomplete[1]["detail"]["unsupported"][0];
    assert_js_keys(
        diagnostic,
        &[
            "code_index",
            "context_at",
            "offset",
            "start",
            "end",
            "opcode",
            "name",
            "status",
        ],
    );
    assert_eq!(diagnostic["status"], "UNSUPPORTED_OPCODE");
    assert_eq!(diagnostic["opcode"], 17);
    let ids = code_rows(&unknown_path, &[]);
    let scoped = rows(&refs(
        &unknown_path,
        "absent",
        &["--within", ids[4]["at"].as_str().unwrap()],
    ));
    assert_eq!(scoped[1]["complete"], true);
    let mut cache = bytes.clone();
    cache[start + 60] = 116;
    cache[start + 61] = 0; // LOAD_GLOBAL-like raw cache payload is never a reference.
    let cache_path = dir.path().join("cache.pyc");
    fs::write(&cache_path, &cache).unwrap();
    let cache_refs = rows(&refs(&cache_path, "marker", &[]));
    let original_refs = rows(&refs(&original, "marker", &[]));
    assert_eq!(
        cache_refs.last().unwrap()["records"],
        original_refs.last().unwrap()["records"]
    );
    assert_eq!(cache_refs.last().unwrap()["complete"], true);
    // Wrong local/free kind, malformed prefix and cache spans, invalid jump boundaries.
    for (owner, offset, opcode, operand) in [
        (4, 6, 124, 2),
        (4, 6, 137, 0),
        (10, 18, 114, 255),
        (10, 18, 114, 3),
        (0, 126, 144, 1),
        (0, 126, 116, 0),
    ] {
        let mut changed = bytes.clone();
        let base = oracle["marshal_ranges"]["code_ranges"][owner]["pyc_bytecode_start"]
            .as_u64()
            .unwrap() as usize;
        changed[base + offset] = opcode;
        changed[base + offset + 1] = operand;
        fs::write(&damaged, &changed).unwrap();
        failure(
            refs(&damaged, "absent", &["-o", output.to_str().unwrap()]),
            1,
            "INVALID_INPUT",
        );
        assert_eq!(fs::read(&output).unwrap(), b"keep");
        assert_eq!(fs::read(&damaged).unwrap(), changed);
    }
    // A missing body must never be turned into a successful zero-hit empty code.
    let mut missing = vec![b'c'];
    for value in [0_u32, 0, 0, 2, 0] {
        missing.extend(value.to_le_bytes());
    }
    missing.extend(b"N)\0)\0)\0s\0\0\0\0z\x01xz\x01xz\x01x");
    missing.extend(1_u32.to_le_bytes());
    missing.extend(b"s\0\0\0\0s\0\0\0\0");
    let missing_path = dir.path().join("missing.marshal");
    fs::write(&missing_path, &missing).unwrap();
    failure(
        refs(&missing_path, "absent", &["--version", "3.12"]),
        1,
        "INVALID_INPUT",
    );
    let mut empty = missing.clone();
    empty.splice(21..22, b"s\0\0\0\0".iter().copied());
    fs::write(&missing_path, &empty).unwrap();
    let empty = rows(&refs(&missing_path, "absent", &["--version", "3.12"]));
    assert_eq!(empty[1]["records"], 0);
    assert_eq!(empty[1]["complete"], true);
    let mut odd = missing;
    odd.splice(21..22, [b's', 1, 0, 0, 0, 100]);
    fs::write(&missing_path, &odd).unwrap();
    failure(
        refs(&missing_path, "absent", &["--version", "3.12"]),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn pyc_refs_budgets_are_cumulative_exact_and_preserve_atomic_outputs() {
    let path = corpus("graph.timestamp.pyc");
    let complete = refs(&path, "marker", &["--perf"]);
    let stream = rows(&complete);
    let perf = perf_row(&complete);
    let budget = rxd_core::Budget::default();
    assert_eq!(
        perf["resource_limits"]["instructions"],
        Value::from(budget.instructions)
    );
    assert!(perf["counters"]["ast_depth"].is_null());
    assert_eq!(
        perf["counters"]["records"],
        stream.last().unwrap()["records"]
    );
    assert_no_budget_flags(&path, "xref");
    // Exact accounting lives in the library, which owns the shared ceilings.
    let bytes = fs::read(&path).unwrap();
    let mut base = shared_static_limits();
    base.input_bytes = bytes.len();
    let codes = code_rows(&path, &[]);
    let (perf, scan) = xref_name_library(&path, None, "marker", None, base, budget.instructions);
    let scan = scan.unwrap();
    assert_eq!(scan.hits.len(), stream.last().unwrap()["records"]);
    let perf = perf_value(&perf);
    let work = perf["counters"]["work"].as_u64().unwrap() as usize;
    let entities = perf["counters"]["entities"].as_u64().unwrap() as usize;
    for (field, amount, instructions) in [
        ("input_bytes", bytes.len(), budget.instructions),
        ("entities", entities, budget.instructions),
        ("work", work, budget.instructions),
        ("records", scan.hits.len(), budget.instructions),
        ("instructions", 0, 99),
    ] {
        let mut exact = base;
        match field {
            "input_bytes" => exact.input_bytes = amount,
            "entities" => exact.entities = amount,
            "work" => exact.work = amount,
            "records" => exact.records = amount,
            _ => {}
        }
        assert!(
            xref_name_library(&path, None, "marker", None, exact, instructions)
                .1
                .is_ok(),
            "{field}"
        );
        let mut less = exact;
        match field {
            "input_bytes" => less.input_bytes = amount - 1,
            "entities" => less.entities = amount - 1,
            "work" => less.work = amount - 1,
            "records" => less.records = amount - 1,
            _ => {}
        }
        let instructions = if field == "instructions" { 98 } else { 99 };
        let error = xref_name_library(&path, None, "marker", None, less, instructions)
            .1
            .err()
            .unwrap();
        assert_eq!(error.code, "RESOURCE_LIMIT", "{field}");
    }
    let id = codes[4]["at"].as_str().unwrap();
    assert!(
        xref_name_library(&path, None, "value", Some(id), base, 9)
            .1
            .is_ok()
    );
    let error = xref_name_library(&path, None, "value", Some(id), base, 8)
        .1
        .err()
        .unwrap();
    assert_eq!(error.code, "RESOURCE_LIMIT");
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("keep.jsonl");
    let output = refs(&path, "marker", &["-o", out.to_str().unwrap()]);
    success(&output);
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(&out).unwrap(), complete.stdout);
    failure(
        refs(&path, "marker", &["-o", path.to_str().unwrap()]),
        2,
        "INVALID_QUERY",
    );
}
