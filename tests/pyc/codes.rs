use super::*;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/pyc/3.12.14")
        .join(name)
}

fn codes(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "codes"])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

fn rows(output: &Output) -> Vec<Value> {
    success(output);
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn pyc_codes_enumerates_the_independent_code_graph_without_reconstruction() {
    let path = corpus("graph.timestamp.pyc");
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("graph.oracle.json")).unwrap()).unwrap();
    let output = codes(&path, &[]);
    let rows = rows(&output);
    let codes = oracle["codes"].as_array().unwrap();
    assert_eq!(rows.len(), codes.len());
    for (index, (row, code)) in rows[..].iter().zip(codes).enumerate() {
        assert_eq!(row["record"], "code");
        assert_eq!(row["at"], format!("code:{index}"));
        assert_eq!(row["detail"]["index"], index);
        assert_eq!(row["detail"]["name"], code["name"]);
        assert_eq!(row["detail"]["qualname"], code["qualname"]);
        assert_eq!(row["detail"]["arg_count"], code["argcount"]);
        assert_eq!(
            row["detail"]["bytecode_bytes"],
            code["code_hex"].as_str().unwrap().len() / 2
        );
    }
    assert_eq!(rows.len(), codes.len());
}

#[test]
fn pyc_codes_preserves_spans_parents_filters_and_path_independent_ids() {
    let path = corpus("graph.timestamp.pyc");
    let bytes = fs::read(&path).unwrap();
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("graph.oracle.json")).unwrap()).unwrap();
    let all = rows(&codes(&path, &[]));
    let (dir, renamed) = fixture(&bytes, "renamed.bin");
    assert_eq!(codes(&renamed, &[]).stdout, codes(&path, &[]).stdout);
    assert_eq!(codes(&path, &[]).stdout, codes(&path, &[]).stdout);
    for (row, expected) in all[..]
        .iter()
        .zip(oracle["marshal_ranges"]["code_ranges"].as_array().unwrap())
    {
        let index = row["detail"]["index"].as_u64().unwrap() as usize;
        assert_eq!(row["start"], expected["pyc_marshal_start"]);
        assert_eq!(row["end"], expected["pyc_marshal_end"]);
        let field = &row["detail"]["fields"]["code"];
        assert_eq!(field["payload_start"], expected["pyc_bytecode_start"]);
        assert_eq!(field["payload_end"], expected["pyc_bytecode_end"]);
        let start = field["payload_start"].as_u64().unwrap() as usize;
        let end = field["payload_end"].as_u64().unwrap() as usize;
        assert_eq!(
            bytes[start..end]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            oracle["codes"][index]["code_hex"]
        );
        let edge = oracle["graph_edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|edge| edge["code"] == index)
            .unwrap();
        let parent = edge["parent"]
            .as_u64()
            .map(|parent| all[parent as usize]["at"].clone())
            .unwrap_or(Value::Null);
        assert_eq!(row["parent_at"], parent);
        assert_eq!(row["detail"]["kind"], "unknown");
        assert_eq!(row["detail"]["body_status"], "present");
    }
    // Two original code names are "same"; filtering retains their global indexes/IDs.
    let same = rows(&codes(&path, &["--name", "same"]));
    assert_eq!(same.len(), 2);
    for row in &same {
        assert_eq!(
            all[row["detail"]["index"].as_u64().unwrap() as usize]["at"],
            row["at"]
        );
    }
    let empty = rows(&codes(&path, &["--name", "absent"]));
    assert!(empty.is_empty());
    let selected = rows(&codes(&path, &["--at", "code:4"]));
    assert_eq!(selected[0], all[4]);
    failure(codes(&path, &["--at", "code:99999"]), 2, "NOT_FOUND");
    let intersected = rows(&codes(&path, &["--at", "code:0", "--name", "same"]));
    assert!(intersected.is_empty());
    let raw = rows(&codes(&corpus("graph.marshal"), &["--version", "3.12"]));
    for (raw, pyc) in raw[..].iter().zip(&all[..]) {
        assert_eq!(raw["detail"]["name"], pyc["detail"]["name"]);
        assert_eq!(raw["start"].as_u64().unwrap() + 16, pyc["start"]);
    }
    let dest = dir.path().join("codes.jsonl");
    let written = codes(&path, &["-o", dest.to_str().unwrap()]);
    success(&written);
    assert!(written.stdout.is_empty());
    assert_eq!(fs::read(&dest).unwrap(), codes(&path, &[]).stdout);
}

// Diagnostic marshal fixture builder; no input is executed and no private parser is called.
fn bytes_field(value: &[u8]) -> Vec<u8> {
    let mut out = vec![b's'];
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value);
    out
}
fn metadata_module(body: &[u8], consts: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = vec![b'c'];
    for value in [0_u32, 0, 0, 2, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(body);
    out.extend_from_slice(consts);
    out.extend_from_slice(b")\0)\0");
    out.extend(bytes_field(b""));
    out.extend_from_slice(b"z\x0afixture.py");
    out.extend_from_slice(name);
    out.extend_from_slice(name);
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend(bytes_field(b""));
    out.extend(bytes_field(b""));
    out
}

#[test]
fn pyc_codes_rejects_non_ascii_marshal_ascii_metadata() {
    let name = b"z\x03\xe9\x9b\xaa"; // Valid UTF-8, invalid TYPE_SHORT_ASCII payload.
    let (_dir, path) = fixture(
        metadata_module(&bytes_field(&[121, 0]), b")\0", name),
        "ascii.marshal",
    );
    failure(codes(&path, &["--version", "3.12"]), 1, "INVALID_INPUT");
}

#[test]
fn pyc_codes_retains_container_aliases_unicode_and_unavailable_bodies() {
    let name = b"u\x03\0\0\0\xe9\x9b\xaa";
    let mut child = metadata_module(&bytes_field(&[]), b")\0", name);
    child[0] |= 0x80;
    let mut consts = b")\x04".to_vec();
    consts.extend(&child);
    consts.extend_from_slice(b"r\0\0\0\0)\x01r\0\0\0\0{z\x01kr\0\0\0\x000");
    let bytes = metadata_module(&bytes_field(&[100]), &consts, b"z\x08<module>"); // Odd co_code deliberately remains unscanned.
    let (_dir, path) = fixture(&bytes, "metadata.marshal");
    let all = rows(&codes(&path, &["--version", "3.12"]));
    assert_eq!(all.len(), 2);
    assert_eq!(all[1]["detail"]["name"], "雪");
    assert_eq!(all[1]["detail"]["qualname"], "雪");
    assert_eq!(all[1]["detail"]["body_status"], "empty");
    assert_eq!(all[1]["detail"]["incoming"].as_array().unwrap().len(), 4);
    for incoming in &all[1]["detail"]["incoming"].as_array().unwrap()[1..] {
        let start = incoming["start"].as_u64().unwrap() as usize;
        let end = incoming["end"].as_u64().unwrap() as usize;
        assert_eq!(&bytes[start..end], b"r\0\0\0\0");
        assert_eq!(incoming["reference"], true);
    }
    failure(
        run(&path, &["--version", "3.12", "--json"]),
        1,
        "INVALID_INPUT",
    );
    let (_dir, path) = fixture(
        metadata_module(b"N", b")\0", b"u\x03\0\0\0\xed\xa0\x80"),
        "unavailable.marshal",
    );
    let all = rows(&codes(&path, &["--version", "3.12"]));
    assert_eq!(all[0]["detail"]["body_status"], "unavailable");
    assert!(all[0]["detail"]["bytecode_bytes"].is_null());
    assert!(all[0]["detail"]["name"].is_null());
    assert_eq!(all[0]["detail"]["name_bytes"], "eda080");
    let mut cycle = metadata_module(&bytes_field(&[]), b")\x01r\0\0\0\0", b"z\x01x");
    cycle[0] |= 0x80;
    let (_dir, path) = fixture(cycle, "cycle.marshal");
    failure(codes(&path, &["--version", "3.12"]), 1, "INVALID_INPUT");
}

#[test]
fn pyc_codes_bounds_exact_name_match_work_cumulatively() {
    let text = "雪".repeat(100);
    let mut name = vec![b'u'];
    name.extend_from_slice(&(text.len() as u32).to_le_bytes());
    name.extend_from_slice(text.as_bytes());
    let (_dir, path) = fixture(
        metadata_module(&bytes_field(&[]), b")\0", &name),
        "match.marshal",
    );
    let mut limits = shared_static_limits();
    limits.work = Some(20);
    let mut perf = pyc_perf("codes");
    let mut analysis =
        rxd_pyc::static_analysis::Analysis::load(&path, Some((3, 12)), limits, &mut perf).unwrap();
    assert!(analysis.select_codes(None, None, None).is_ok());
    let error = analysis.select_codes(None, Some(&text), None).unwrap_err();
    assert_eq!(error.code, "RESOURCE_LIMIT");
}

#[test]
fn pyc_codes_marks_code_containment_through_aliased_const_tables_without_false_spans() {
    let mut child = metadata_module(&bytes_field(&[]), b")\0", b"z\x06shared");
    child[0] |= 0x80;
    let mut shared = vec![b')' | 0x80, 1];
    shared.extend(child);
    let owner = metadata_module(&bytes_field(&[]), b"r\0\0\0\0", b"z\x05owner");
    let mut consts = b")\x02".to_vec();
    consts.extend(shared);
    consts.extend(owner);
    let bytes = metadata_module(&bytes_field(&[]), &consts, b"z\x08<module>");
    let (_dir, path) = fixture(&bytes, "aliased-table.marshal");
    let all = rows(&codes(&path, &["--version", "3.12"]));
    assert_eq!(all.len(), 3);
    assert_eq!(all[1]["detail"]["name"], "shared");
    assert_eq!(all[2]["detail"]["name"], "owner");
    let edges = all[1]["detail"]["incoming"].as_array().unwrap();
    assert_eq!(edges.len(), 2);
    let alias = &edges[1];
    assert_eq!(all[2]["at"], "code:2");
    // Ids nested in `detail` are the same copy-only coordinate as `at`.
    assert_eq!(alias["owner_id"], all[2]["at"]);
    assert!(alias["start"].is_null());
    assert!(alias["end"].is_null());
    let start = alias["alias_start"].as_u64().unwrap() as usize;
    let end = alias["alias_end"].as_u64().unwrap() as usize;
    assert_eq!(&bytes[start..end], b"r\0\0\0\0");
    assert_eq!(all[2]["detail"]["fields"]["consts"]["reference"], true);
}

#[test]
fn pyc_codes_schema_and_effective_budgets_are_exact_and_atomic() {
    let path = corpus("graph.timestamp.pyc");
    let output = codes(&path, &["--perf"]);
    let data = rows(&output);
    assert_js_keys(
        &data[0],
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
    assert_js_keys(
        &data[0]["detail"],
        &[
            "index",
            "name",
            "qualname",
            "filename",
            "name_bytes",
            "qualname_bytes",
            "filename_bytes",
            "kind",
            "arg_count",
            "posonly_arg_count",
            "kwonly_arg_count",
            "n_locals",
            "stack_size",
            "flags",
            "first_line",
            "body_status",
            "bytecode_bytes",
            "consts_count",
            "names_count",
            "varnames_count",
            "freevars_count",
            "cellvars_count",
            "localsplus_count",
            "line_table_bytes",
            "exception_table_bytes",
            "fields",
            "incoming",
            "incoming_complete",
            "ownership",
        ],
    );
    assert_js_keys(
        &data[0]["detail"]["fields"],
        &[
            "arg_count",
            "posonly_arg_count",
            "kwonly_arg_count",
            "stack_size",
            "flags",
            "code",
            "consts",
            "names",
            "localsplus_names",
            "localsplus_kinds",
            "filename",
            "name",
            "qualname",
            "first_line",
            "line_table",
            "exception_table",
        ],
    );
    assert_js_keys(
        &data[0]["detail"]["fields"]["code"],
        &[
            "start",
            "end",
            "definition_start",
            "definition_end",
            "payload_start",
            "payload_end",
            "reference",
            "tag",
        ],
    );
    let perf: Vec<Value> = String::from_utf8(output.stderr)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(perf.len(), 2);
    let analysis = perf.iter().find(|row| row["scope"] == "analysis").unwrap();
    let budget =
        rxd_core::Budget::for_input_bytes(fs::metadata(&path).unwrap().len() as usize).unwrap();
    assert_eq!(
        analysis["resource_limits"],
        serde_json::json!({
            "input_bytes": budget.input_bytes,
            "entities": budget.semantic_nodes,
            "metadata_bytes": budget.metadata_bytes,
            "marshal_objects": budget.semantic_nodes,
            "marshal_expanded_objects": budget.semantic_nodes,
            "marshal_copied_bytes": budget.metadata_bytes,
        })
    );
    assert!(analysis["counters"]["ast_depth"].is_null());
    assert_eq!(analysis["counters"]["records"], 15);
    assert_no_budget_flags(&path, "codes");
    // Exact accounting lives in the library, which owns the shared ceilings.
    let entities = analysis["counters"]["entities"].as_u64().unwrap() as usize;
    let work = analysis["counters"]["work"].as_u64().unwrap() as usize;
    for (field, amount) in [
        ("input_bytes", fs::metadata(&path).unwrap().len() as usize),
        ("entities", entities),
        ("work", work),
    ] {
        let mut exact = shared_static_limits();
        match field {
            "input_bytes" => exact.input_bytes = amount,
            "entities" => exact.entities = amount,
            _ => exact.work = Some(amount),
        }
        let mut perf = pyc_perf("codes");
        let mut loaded =
            rxd_pyc::static_analysis::Analysis::load(&path, None, exact, &mut perf).unwrap();
        assert_eq!(loaded.select_codes(None, None, None).unwrap().len(), 15);

        let mut less = exact;
        match field {
            "input_bytes" => less.input_bytes = amount - 1,
            "entities" => less.entities = amount - 1,
            _ => less.work = Some(amount - 1),
        }
        let mut perf = pyc_perf("codes");
        let error = rxd_pyc::static_analysis::Analysis::load(&path, None, less, &mut perf)
            .and_then(|mut loaded| loaded.select_codes(None, None, None));
        assert_eq!(error.err().unwrap().code, "RESOURCE_LIMIT", "{field}");
    }
    // A rejected selector leaves an existing output file untouched.
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("keep.jsonl");
    fs::write(&dest, b"keep").unwrap();
    failure(
        codes(&path, &["--at", "code:99999", "-o", dest.to_str().unwrap()]),
        2,
        "NOT_FOUND",
    );
    assert_eq!(fs::read(&dest).unwrap(), b"keep");
}

#[test]
fn pyc_codes_rejects_unverified_magic_and_incomplete_marshal() {
    let path = corpus("graph.timestamp.pyc");
    let original = fs::read(&path).unwrap();
    for flags in [0_u32, 1, 3] {
        let mut bytes = original.clone();
        bytes[4..8].copy_from_slice(&flags.to_le_bytes());
        let (_dir, path) = fixture(bytes, "header.pyc");
        assert_eq!(rows(&codes(&path, &[]))[0]["detail"]["name"], "<module>");
    }
    for flags in [2_u32, 4, u32::MAX] {
        let mut bytes = original.clone();
        bytes[4..8].copy_from_slice(&flags.to_le_bytes());
        let (_dir, path) = fixture(bytes, "flags.pyc");
        failure(codes(&path, &[]), 1, "INVALID_INPUT");
    }
    for magic in [3530_u16, 3532, 3494, 3570] {
        let mut bytes = original.clone();
        bytes[0..2].copy_from_slice(&magic.to_le_bytes());
        let (_dir, path) = fixture(bytes, "version.pyc");
        failure(codes(&path, &[]), 1, "INVALID_INPUT");
    }
    for bytes in [
        &original[..3],
        &original[..15],
        &original[..original.len() - 1],
    ] {
        let (_dir, path) = fixture(bytes, "short.pyc");
        failure(codes(&path, &[]), 1, "INVALID_INPUT");
    }
    let mut trailing = original.clone();
    trailing.push(0);
    let (_dir, path) = fixture(trailing, "trailing.pyc");
    failure(codes(&path, &[]), 1, "INVALID_INPUT");
    failure(codes(&corpus("graph.marshal"), &[]), 1, "INVALID_INPUT");
    failure(codes(&path, &["--version", "3.12"]), 1, "INVALID_INPUT");
    failure(
        codes(&corpus("graph.marshal"), &["--version", "3.14"]),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn pyc_codes_preserves_shared_dictionary_definition_and_reference_sites() {
    let mut child = metadata_module(&bytes_field(&[]), b")\0", b"z\x06shared");
    child[0] |= 0x80;
    let mut dictionary = vec![b'{' | 0x80, b'z', 1, b'k'];
    dictionary.extend(child);
    dictionary.push(b'0');
    let owner = metadata_module(&bytes_field(&[]), b")\x01r\0\0\0\0", b"z\x05owner");
    let mut consts = b")\x02".to_vec();
    consts.extend(dictionary);
    consts.extend(owner);
    let (_dir, path) = fixture(
        metadata_module(&bytes_field(&[]), &consts, b"z\x08<module>"),
        "dict-alias.marshal",
    );
    let all = rows(&codes(&path, &["--version", "3.12"]));
    let incoming = &all[1]["detail"]["incoming"][1];
    assert_eq!(incoming["constant_path"], serde_json::json!([0, 1]));
    assert!(incoming["start"].is_null());
    assert!(incoming["alias_start"].is_u64());
}

#[test]
fn pyc_codes_perf_retains_parser_and_index_counters_on_budget_errors() {
    let path = corpus("graph.timestamp.pyc");
    let success = codes(&path, &["--perf"]);
    let reports: Vec<Value> = String::from_utf8(success.stderr)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_no_budget_flags(&path, "codes");
    let entities = reports
        .iter()
        .find(|row| row["scope"] == "analysis")
        .unwrap()["counters"]["entities"]
        .as_u64()
        .unwrap();
    // A budget error still reports the parser and index counters already spent.
    for (field, limit) in [
        ("entities", 1usize),
        ("work", 1),
        ("work", entities as usize),
    ] {
        let mut limits = shared_static_limits();
        limits.entities = limit;
        limits.work = Some(limit);
        if field == "entities" {
            limits.work = shared_static_limits().work;
        }
        let mut perf = pyc_perf("codes");
        let error = rxd_pyc::static_analysis::Analysis::load(&path, None, limits, &mut perf)
            .map(|_| ())
            .unwrap_err();
        assert_eq!(error.code, "RESOURCE_LIMIT");
        let report = perf_value(&perf);
        assert!(report["counters"]["entities"].is_u64());
        assert!(report["counters"]["work"].is_u64());
        assert_eq!(report["resource_limits"][field], limit);
    }
}
