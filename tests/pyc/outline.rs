use super::*;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/pyc/3.12.14")
        .join(name)
}

fn outline(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "outline"])
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
fn pyc_outline_enumerates_the_independent_code_graph_without_reconstruction() {
    let path = corpus("graph.timestamp.pyc");
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("graph.oracle.json")).unwrap()).unwrap();
    let output = outline(&path, &[]);
    let rows = rows(&output);
    let codes = oracle["codes"].as_array().unwrap();
    assert_eq!(rows.len(), codes.len() + 2);
    assert_eq!(rows[0]["record"], "header");
    assert_eq!(rows[0]["schema_version"], 2);
    assert_eq!(rows[0]["detail"]["analysis"], "metadata");
    let hash = format!("{:x}", Sha256::digest(fs::read(&path).unwrap()));
    for (index, (row, code)) in rows[1..rows.len() - 1].iter().zip(codes).enumerate() {
        assert_eq!(row["record"], "code");
        assert_eq!(row["id"], format!("rxd1:pyc:{hash}:code:{index}"));
        assert_eq!(row["detail"]["index"], index);
        assert_eq!(row["detail"]["name"], code["name"]);
        assert_eq!(row["detail"]["qualname"], code["qualname"]);
        assert_eq!(row["detail"]["arg_count"], code["argcount"]);
        assert_eq!(
            row["detail"]["bytecode_bytes"],
            code["code_hex"].as_str().unwrap().len() / 2
        );
    }
    assert_eq!(rows.last().unwrap()["records"], codes.len());
    assert_eq!(rows.last().unwrap()["complete"], true);
}

#[test]
fn pyc_outline_preserves_spans_parents_filters_and_path_independent_ids() {
    let path = corpus("graph.timestamp.pyc");
    let bytes = fs::read(&path).unwrap();
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("graph.oracle.json")).unwrap()).unwrap();
    let all = rows(&outline(&path, &[]));
    let (dir, renamed) = fixture(&bytes, "renamed.bin");
    assert_eq!(outline(&renamed, &[]).stdout, outline(&path, &[]).stdout);
    assert_eq!(outline(&path, &[]).stdout, outline(&path, &[]).stdout);
    for (row, expected) in all[1..all.len() - 1]
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
            .map(|parent| all[parent as usize + 1]["id"].clone())
            .unwrap_or(Value::Null);
        assert_eq!(row["parent_id"], parent);
        assert_eq!(row["detail"]["kind"], "unknown");
        assert_eq!(row["detail"]["body_status"], "present");
    }
    // Two original code names are "same"; filtering retains their global indexes/IDs.
    let same = rows(&outline(&path, &["--name", "same"]));
    assert_eq!(same.len(), 4);
    assert_eq!(
        same[0]["query"],
        serde_json::json!({"name":"same","index":null,"version":null})
    );
    for row in &same[1..3] {
        assert_eq!(
            *row,
            all[row["detail"]["index"].as_u64().unwrap() as usize + 1]
        );
    }
    let empty = rows(&outline(&path, &["--name", "absent"]));
    assert_eq!(empty.len(), 2);
    assert_eq!(empty[1]["records"], 0);
    assert_eq!(empty[1]["complete"], true);
    let selected = rows(&outline(&path, &["--index", "4"]));
    assert_eq!(selected[1], all[5]);
    failure(outline(&path, &["--index", "99999"]), 2, "INVALID_QUERY");
    let conflict = outline(&path, &["--index", "0", "--name", "same"]);
    assert_eq!(conflict.status.code(), Some(2));
    assert!(conflict.stdout.is_empty());
    let raw = rows(&outline(&corpus("graph.marshal"), &["--version", "3.12"]));
    assert_eq!(raw[0]["query"]["version"], "3.12");
    for (raw, pyc) in raw[1..raw.len() - 1].iter().zip(&all[1..all.len() - 1]) {
        assert_eq!(raw["detail"]["name"], pyc["detail"]["name"]);
        assert_eq!(raw["start"].as_u64().unwrap() + 16, pyc["start"]);
    }
    let dest = dir.path().join("outline.jsonl");
    let written = outline(&path, &["-o", dest.to_str().unwrap()]);
    success(&written);
    assert!(written.stdout.is_empty());
    assert_eq!(fs::read(&dest).unwrap(), outline(&path, &[]).stdout);
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
fn pyc_outline_rejects_non_ascii_marshal_ascii_metadata() {
    let name = b"z\x03\xe9\x9b\xaa"; // Valid UTF-8, invalid TYPE_SHORT_ASCII payload.
    let (_dir, path) = fixture(
        metadata_module(&bytes_field(&[121, 0]), b")\0", name),
        "ascii.marshal",
    );
    failure(outline(&path, &["--version", "3.12"]), 1, "INVALID_INPUT");
}

#[test]
fn pyc_outline_retains_container_aliases_unicode_and_unavailable_bodies() {
    let name = b"u\x03\0\0\0\xe9\x9b\xaa";
    let mut child = metadata_module(&bytes_field(&[]), b")\0", name);
    child[0] |= 0x80;
    let mut consts = b")\x04".to_vec();
    consts.extend(&child);
    consts.extend_from_slice(b"r\0\0\0\0)\x01r\0\0\0\0{z\x01kr\0\0\0\x000");
    let bytes = metadata_module(&bytes_field(&[100]), &consts, b"z\x08<module>"); // Odd co_code deliberately remains unscanned.
    let (_dir, path) = fixture(&bytes, "metadata.marshal");
    let all = rows(&outline(&path, &["--version", "3.12"]));
    assert_eq!(all.len(), 4);
    assert_eq!(all[0]["detail"]["instruction_validation"], "not_scanned");
    assert_eq!(all[2]["detail"]["name"], "雪");
    assert_eq!(all[2]["detail"]["qualname"], "雪");
    assert_eq!(all[2]["detail"]["body_status"], "empty");
    assert_eq!(all[2]["detail"]["incoming"].as_array().unwrap().len(), 4);
    for incoming in &all[2]["detail"]["incoming"].as_array().unwrap()[1..] {
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
    let all = rows(&outline(&path, &["--version", "3.12"]));
    assert_eq!(all[1]["detail"]["body_status"], "unavailable");
    assert!(all[1]["detail"]["bytecode_bytes"].is_null());
    assert!(all[1]["detail"]["name"].is_null());
    assert_eq!(all[1]["detail"]["name_bytes"], "eda080");
    let mut cycle = metadata_module(&bytes_field(&[]), b")\x01r\0\0\0\0", b"z\x01x");
    cycle[0] |= 0x80;
    let (_dir, path) = fixture(cycle, "cycle.marshal");
    failure(outline(&path, &["--version", "3.12"]), 1, "INVALID_INPUT");
}

#[test]
fn pyc_outline_bounds_exact_name_match_work_cumulatively() {
    let text = "雪".repeat(100);
    let mut name = vec![b'u'];
    name.extend_from_slice(&(text.len() as u32).to_le_bytes());
    name.extend_from_slice(text.as_bytes());
    let (_dir, path) = fixture(
        metadata_module(&bytes_field(&[]), b")\0", &name),
        "match.marshal",
    );
    success(&outline(&path, &["--version", "3.12", "--max-work", "20"]));
    failure(
        outline(
            &path,
            &["--version", "3.12", "--name", &text, "--max-work", "20"],
        ),
        1,
        "RESOURCE_LIMIT",
    );
}

#[test]
fn pyc_outline_marks_code_containment_through_aliased_const_tables_without_false_spans() {
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
    let all = rows(&outline(&path, &["--version", "3.12"]));
    assert_eq!(all.len(), 5);
    assert_eq!(all[2]["detail"]["name"], "shared");
    assert_eq!(all[3]["detail"]["name"], "owner");
    let edges = all[2]["detail"]["incoming"].as_array().unwrap();
    assert_eq!(edges.len(), 2);
    let alias = &edges[1];
    assert_eq!(alias["owner_id"], all[3]["id"]);
    assert!(alias["start"].is_null());
    assert!(alias["end"].is_null());
    let start = alias["alias_start"].as_u64().unwrap() as usize;
    let end = alias["alias_end"].as_u64().unwrap() as usize;
    assert_eq!(&bytes[start..end], b"r\0\0\0\0");
    assert_eq!(all[3]["detail"]["fields"]["consts"]["reference"], true);
}

#[test]
fn pyc_outline_schema_and_effective_budgets_are_exact_and_atomic() {
    let path = corpus("graph.timestamp.pyc");
    let output = outline(&path, &["--perf"]);
    let data = rows(&output);
    assert_js_keys(
        &data[0],
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
    assert_js_keys(
        &data[0]["detail"],
        &[
            "analysis",
            "python_version",
            "code_count",
            "instruction_validation",
        ],
    );
    assert_js_keys(
        &data[1],
        &[
            "record",
            "id",
            "source_id",
            "parent_id",
            "context_id",
            "start",
            "end",
            "detail",
        ],
    );
    assert_js_keys(
        &data[1]["detail"],
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
        &data[1]["detail"]["fields"],
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
        &data[1]["detail"]["fields"]["code"],
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
    assert_eq!(
        analysis["resource_limits"],
        serde_json::json!({"input_bytes":8u64<<30,"entities":100_000,"work":16_777_216,"records":100_000,"output_bytes":32*1024*1024})
    );
    assert!(analysis["counters"]["ast_depth"].is_null());
    assert_eq!(analysis["counters"]["records"], 15);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("keep.jsonl");
    for (flag, amount) in [
        (
            "--max-input-bytes",
            fs::metadata(&path).unwrap().len() as usize,
        ),
        ("--max-output-bytes", output.stdout.len()),
        ("--max-records", 15),
        (
            "--max-entities",
            analysis["counters"]["entities"].as_u64().unwrap() as usize,
        ),
        (
            "--max-work",
            analysis["counters"]["work"].as_u64().unwrap() as usize,
        ),
    ] {
        let exact = amount.to_string();
        let less = (amount - 1).to_string();
        assert_eq!(rows(&outline(&path, &[flag, &exact])), data);
        fs::write(&dest, b"keep").unwrap();
        failure(
            outline(&path, &[flag, &less, "-o", dest.to_str().unwrap()]),
            1,
            "RESOURCE_LIMIT",
        );
        assert_eq!(fs::read(&dest).unwrap(), b"keep");
    }
    for flag in [
        "--max-input-bytes",
        "--max-output-bytes",
        "--max-records",
        "--max-entities",
        "--max-work",
    ] {
        let invalid = outline(&path, &[flag, "0"]);
        assert_eq!(invalid.status.code(), Some(2));
        assert!(invalid.stdout.is_empty());
    }
}

#[test]
fn pyc_outline_rejects_unverified_magic_and_incomplete_marshal() {
    let path = corpus("graph.timestamp.pyc");
    let original = fs::read(&path).unwrap();
    for flags in [0_u32, 1, 3] {
        let mut bytes = original.clone();
        bytes[4..8].copy_from_slice(&flags.to_le_bytes());
        let (_dir, path) = fixture(bytes, "header.pyc");
        assert_eq!(rows(&outline(&path, &[]))[1]["detail"]["name"], "<module>");
    }
    for flags in [2_u32, 4, u32::MAX] {
        let mut bytes = original.clone();
        bytes[4..8].copy_from_slice(&flags.to_le_bytes());
        let (_dir, path) = fixture(bytes, "flags.pyc");
        failure(outline(&path, &[]), 1, "INVALID_INPUT");
    }
    for magic in [3530_u16, 3532, 3494, 3570] {
        let mut bytes = original.clone();
        bytes[0..2].copy_from_slice(&magic.to_le_bytes());
        let (_dir, path) = fixture(bytes, "version.pyc");
        failure(outline(&path, &[]), 1, "INVALID_INPUT");
    }
    for bytes in [
        &original[..3],
        &original[..15],
        &original[..original.len() - 1],
    ] {
        let (_dir, path) = fixture(bytes, "short.pyc");
        failure(outline(&path, &[]), 1, "INVALID_INPUT");
    }
    let mut trailing = original.clone();
    trailing.push(0);
    let (_dir, path) = fixture(trailing, "trailing.pyc");
    failure(outline(&path, &[]), 1, "INVALID_INPUT");
    failure(outline(&corpus("graph.marshal"), &[]), 1, "INVALID_INPUT");
    failure(outline(&path, &["--version", "3.12"]), 1, "INVALID_INPUT");
    failure(
        outline(&corpus("graph.marshal"), &["--version", "3.14"]),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn pyc_outline_preserves_shared_dictionary_definition_and_reference_sites() {
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
    let all = rows(&outline(&path, &["--version", "3.12"]));
    let incoming = &all[2]["detail"]["incoming"][1];
    assert_eq!(incoming["constant_path"], serde_json::json!([0, 1]));
    assert!(incoming["start"].is_null());
    assert!(incoming["alias_start"].is_u64());
}

#[test]
fn pyc_outline_perf_retains_parser_and_index_counters_on_budget_errors() {
    let path = corpus("graph.timestamp.pyc");
    let success = outline(&path, &["--perf"]);
    let reports: Vec<Value> = String::from_utf8(success.stderr)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let entities = reports
        .iter()
        .find(|row| row["scope"] == "analysis")
        .unwrap()["counters"]["entities"]
        .as_u64()
        .unwrap()
        .to_string();
    for (flag, limit) in [
        ("--max-entities", "1"),
        ("--max-work", "1"),
        ("--max-work", entities.as_str()),
    ] {
        let output = outline(&path, &[flag, limit, "--perf"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let reports: Vec<Value> = String::from_utf8(output.stderr)
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let analysis = reports
            .iter()
            .find(|row| row["scope"] == "analysis")
            .unwrap();
        assert_eq!(analysis["error_code"], "RESOURCE_LIMIT");
        assert!(analysis["counters"]["entities"].is_u64());
        assert!(analysis["counters"]["work"].is_u64());
        let key = if flag == "--max-entities" {
            "entities"
        } else {
            "work"
        };
        assert_eq!(
            analysis["resource_limits"][key],
            limit.parse::<usize>().unwrap()
        );
    }
}
