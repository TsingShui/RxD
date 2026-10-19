use super::*;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/pyc/3.12.14")
        .join(name)
}
fn invoke(command: &str, path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", command])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}
fn rows(output: &Output) -> Vec<Value> {
    success(output);
    output
        .stdout
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}
#[test]
fn pyc_constant_refs_route_search_occurrences_to_original_owner_instructions() {
    let path = corpus("graph.timestamp.pyc");
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("graph.oracle.json")).unwrap()).unwrap();
    let strings = rows(&invoke("strings", &path, &["--contains", "marker"]));
    let outline = rows(&invoke("outline", &path, &[]));
    assert!(strings.iter().any(|r| r["record"] == "string"));
    for literal in strings.iter().filter(|r| r["record"] == "string") {
        let id = literal["id"].as_str().unwrap();
        let owner = literal["detail"]["code_index"].as_u64().unwrap() as usize;
        let slot = &literal["detail"]["constant_slot"];
        let stream = rows(&invoke("refs", &path, &["--const-id", id]));
        let expected: Vec<_> = oracle["codes"][owner]["dis_instructions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| {
                matches!(
                    i["opname"].as_str(),
                    Some("LOAD_CONST" | "RETURN_CONST" | "KW_NAMES")
                ) && i["arg"] == *slot
            })
            .collect();
        let hits: Vec<_> = stream
            .iter()
            .filter(|r| r["record"] == "reference")
            .collect();
        assert_eq!(hits.len(), expected.len());
        assert_eq!(stream.last().unwrap()["complete"], true);
        assert_eq!(stream[0]["query"]["const_id"], id);
        assert!(stream[0]["query"]["name"].is_null());
        for (hit, expected) in hits.iter().zip(expected) {
            assert_eq!(hit["context_id"], literal["context_id"]);
            assert_eq!(hit["detail"]["constant_id"], id);
            assert_eq!(
                hit["detail"]["constant_path"],
                literal["detail"]["constant_path"]
            );
            assert_eq!(hit["detail"]["constant_slot"], *slot);
            assert_eq!(hit["detail"]["literal_kind"], literal["detail"]["kind"]);
            assert_eq!(
                hit["detail"]["relation"],
                if literal["detail"]["constant_path"].as_array().unwrap().len() == 1 {
                    "direct_constant"
                } else {
                    "containing_constant"
                }
            );
            assert_eq!(hit["detail"]["offset"], expected["offset"]);
            assert_eq!(hit["detail"]["opcode"], expected["opcode"]);
            assert_eq!(hit["detail"]["name"], expected["opname"]);
            assert_eq!(hit["detail"]["raw_operand"], expected["raw_arg_byte"]);
            assert_eq!(hit["detail"]["operand"], expected["arg"]);
            assert!(outline.iter().any(|r| r["id"] == hit["context_id"]));
            let fetched = invoke("get-code", &path, &[hit["context_id"].as_str().unwrap()]);
            success(&fetched);
            let view: Value = serde_json::from_slice(&fetched.stdout).unwrap();
            assert_eq!(
                view["detail"]["bytecode_hex"],
                oracle["codes"][owner]["code_hex"]
            );
            let unit = view["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["offset"] == hit["detail"]["offset"])
                .unwrap();
            assert_eq!(unit["targets"][0], hit["detail"]["target"]);
            assert_eq!(unit["start"], hit["start"]);
            assert_eq!(unit["end"], hit["end"]);
        }
    }
}

#[test]
fn pyc_constant_refs_preserve_real_extended_constant_operands() {
    let path = corpus("extended.timestamp.pyc");
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("extended.oracle.json")).unwrap()).unwrap();
    let literals = rows(&invoke("strings", &path, &["--contains", "literal-299"]));
    assert_eq!(literals.last().unwrap()["records"], 1);
    let literal = &literals[1];
    let owner = literal["detail"]["code_index"].as_u64().unwrap() as usize;
    let result = rows(&invoke(
        "refs",
        &path,
        &["--const-id", literal["id"].as_str().unwrap()],
    ));
    assert_eq!(result.last().unwrap()["records"], 1);
    let expected = oracle["codes"][owner]["dis_instructions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["opname"] == "LOAD_CONST" && i["arg"] == literal["detail"]["constant_slot"])
        .unwrap();
    assert_eq!(result[1]["detail"]["operand"], 300);
    assert_eq!(result[1]["detail"]["raw_operand"], 44);
    assert_eq!(result[1]["detail"]["offset"], expected["offset"]);
    let fetch = invoke(
        "get-code",
        &path,
        &[result[1]["context_id"].as_str().unwrap()],
    );
    success(&fetch);
    let view: Value = serde_json::from_slice(&fetch.stdout).unwrap();
    let unit = view["detail"]["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["offset"] == expected["offset"])
        .unwrap();
    assert_eq!(unit["prefixes"][0]["raw_operand"], 1);
    assert_eq!(unit["targets"][0], result[1]["detail"]["target"]);
}

fn bytes_field(value: &[u8]) -> Vec<u8> {
    let mut out = vec![b's'];
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value);
    out
}
fn module(body: &[u8], consts: &[u8]) -> Vec<u8> {
    let mut out = vec![b'c'];
    for value in [0_u32, 0, 0, 2, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend(bytes_field(body));
    out.extend_from_slice(consts);
    out.extend_from_slice(b")\0)\0");
    out.extend(bytes_field(b""));
    out.extend_from_slice(b"z\x0afixture.pyz\x08<module>z\x08<module>");
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend(bytes_field(b""));
    out.extend(bytes_field(b""));
    out
}
fn shared_input() -> Vec<u8> {
    // Serialized ref0 is str x, ref1 tuple (x, 雪), ref2 a shared child code.
    let mut consts = vec![b')', 9, b'z' | 0x80, 1, b'x'];
    consts.extend_from_slice(b"r\0\0\0\0");
    consts.extend_from_slice(&[b')' | 0x80, 2]);
    consts.extend_from_slice(b"r\0\0\0\0u\x03\0\0\0");
    consts.extend_from_slice("雪".as_bytes());
    consts.extend_from_slice(b"r\x01\0\0\0>\x01\0\0\0r\0\0\0\0z\x06unused");
    let mut child = module(&[121, 0], b"r\x01\0\0\0");
    child[0] |= 0x80;
    consts.extend(child);
    consts.extend_from_slice(b"r\x02\0\0\0");
    consts.extend(bytes_field(&[255, 0, 128]));
    module(
        &[
            100, 0, 1, 0, 100, 0, 1, 0, 100, 2, 1, 0, 100, 3, 1, 0, 100, 4, 1, 0, 172, 2, 100, 8,
            1, 0, 121, 1,
        ],
        &consts,
    )
}
#[test]
fn pyc_constant_refs_keep_equal_slots_owners_shared_paths_and_each_real_instruction() {
    let bytes = shared_input();
    let (_dir, path) = fixture(&bytes, "shared.marshal");
    let strings = rows(&invoke("strings", &path, &["--version", "3.12"]));
    let expected: [(usize, &[u64], &[u64]); 11] = [
        (0, &[0], &[0, 4]),
        (0, &[1], &[26]),
        (0, &[2, 0], &[8, 20]),
        (0, &[2, 1], &[8, 20]),
        (0, &[3, 0], &[12]),
        (0, &[3, 1], &[12]),
        (0, &[4, 0], &[16]),
        (0, &[5], &[]),
        (0, &[8], &[22]),
        (1, &[0], &[0]),
        (1, &[1], &[]),
    ];
    let literals: Vec<_> = strings.iter().filter(|r| r["record"] == "string").collect();
    assert_eq!(literals.len(), expected.len());
    for (literal, (owner, path_indices, offsets)) in literals.iter().zip(expected) {
        assert_eq!(literal["detail"]["code_index"], owner);
        assert_eq!(
            literal["detail"]["constant_path"],
            serde_json::json!(path_indices)
        );
        let result = rows(&invoke(
            "refs",
            &path,
            &[
                "--version",
                "3.12",
                "--const-id",
                literal["id"].as_str().unwrap(),
            ],
        ));
        let hits: Vec<_> = result
            .iter()
            .filter(|r| r["record"] == "reference")
            .collect();
        assert_eq!(
            hits.iter()
                .map(|h| h["detail"]["offset"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            offsets
        );
        assert_eq!(result.last().unwrap()["complete"], true);
        assert_eq!(result.last().unwrap()["detail"]["scanned_code_count"], 1);
        for hit in hits {
            assert_eq!(hit["context_id"], literal["context_id"]);
            assert_eq!(hit["detail"]["constant_id"], literal["id"]);
            assert_eq!(
                hit["detail"]["constant_path"],
                literal["detail"]["constant_path"]
            );
            assert_eq!(hit["detail"]["target"]["index"], path_indices[0]);
            assert_eq!(hit["detail"]["literal_kind"], literal["detail"]["kind"]);
            assert_eq!(
                hit["detail"]["relation"],
                if path_indices.len() == 1 {
                    "direct_constant"
                } else {
                    "containing_constant"
                }
            );
            let start = hit["start"].as_u64().unwrap() as usize;
            let end = hit["end"].as_u64().unwrap() as usize;
            assert_eq!(
                bytes[start],
                hit["detail"]["opcode"].as_u64().unwrap() as u8
            );
            assert_eq!(end - start, 2);
            assert_eq!(
                hit["detail"]["operation"],
                match hit["detail"]["name"].as_str().unwrap() {
                    "RETURN_CONST" => "constant_return",
                    "KW_NAMES" => "keyword_names",
                    _ => "constant_load",
                }
            );
            let fetched = invoke(
                "get-code",
                &path,
                &["--version", "3.12", hit["context_id"].as_str().unwrap()],
            );
            success(&fetched);
            let view: Value = serde_json::from_slice(&fetched.stdout).unwrap();
            let unit = view["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["offset"] == hit["detail"]["offset"])
                .unwrap();
            assert_eq!(unit["targets"][0], hit["detail"]["target"]);
            let mut value_id = view["detail"]["tables"]["consts"]["entries"]
                [path_indices[0] as usize]["value_id"]
                .as_str()
                .unwrap();
            for &index in &path_indices[1..] {
                let value = view["detail"]["values"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|v| v["id"] == value_id)
                    .unwrap();
                value_id = value["children"][index as usize]["value_id"]
                    .as_str()
                    .unwrap();
            }
            let value = view["detail"]["values"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["id"] == value_id)
                .unwrap();
            assert_eq!(value["kind"], literal["detail"]["kind"]);
            assert_eq!(value["raw_hex"], literal["detail"]["raw_bytes"]);
        }
    }
}

#[test]
fn pyc_constant_refs_validate_occurrence_paths_selectors_scope_and_fixed_schema() {
    let bytes = shared_input();
    let (dir, path) = fixture(&bytes, "shared.marshal");
    let strings = rows(&invoke("strings", &path, &["--version", "3.12"]));
    let literal = &strings[1];
    let id = literal["id"].as_str().unwrap();
    let digest = literal["source_id"].as_str().unwrap();
    let owner_id = literal["context_id"].as_str().unwrap();
    let outline = rows(&invoke("outline", &path, &["--version", "3.12"]));
    let other_id = outline.iter().find(|r| r["detail"]["index"] == 1).unwrap()["id"]
        .as_str()
        .unwrap();
    let all = rows(&invoke(
        "refs",
        &path,
        &["--version", "3.12", "--const-id", id],
    ));
    let scoped = rows(&invoke(
        "refs",
        &path,
        &["--version", "3.12", "--const-id", id, "--within", owner_id],
    ));
    assert_eq!(all[1..all.len() - 1], scoped[1..scoped.len() - 1]);
    assert_eq!(all[0]["detail"]["scope"], "constant_owner");
    assert_eq!(scoped[0]["detail"]["scope"], "selected_code");
    assert_js_keys(&all[0]["query"], &["name", "const_id", "within", "version"]);
    assert_eq!(all[0]["query"]["const_id"], id);
    assert_eq!(all[0]["query"]["version"], "3.12");
    assert!(all[0]["query"]["name"].is_null());
    assert!(all[0]["query"]["within"].is_null());
    assert_eq!(all[0]["detail"]["analysis"], "explicit_constant_references");
    assert_eq!(
        all.last().unwrap()["detail"]["analysis"],
        "explicit_constant_references"
    );
    assert_js_keys(
        &all[1]["detail"],
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
    let excluded = rows(&invoke(
        "refs",
        &path,
        &["--version", "3.12", "--const-id", id, "--within", other_id],
    ));
    assert_eq!(excluded.len(), 2);
    assert_eq!(excluded[1]["records"], 0);
    assert_eq!(excluded[1]["complete"], true);
    assert_eq!(excluded[1]["detail"]["scanned_code_count"], 0);
    assert_eq!(excluded[1]["detail"]["instructions"], 0);
    for invalid in [
        id.replace(":pyc:", ":wasm:"),
        id.replace(":const:", ":code:"),
        format!("{id}:0"),
        format!("rxd1:pyc:{}:const:0:0", "a".repeat(64)),
        format!("rxd1:pyc:{digest}:const:01:0"),
        format!("rxd1:pyc:{digest}:const:99:0"),
        format!("rxd1:pyc:{digest}:const:0:"),
        format!("rxd1:pyc:{digest}:const:0:00"),
        format!("rxd1:pyc:{digest}:const:0:2.01"),
        format!("rxd1:pyc:{digest}:const:0:2."),
        format!("rxd1:pyc:{digest}:const:0:2..0"),
        format!("rxd1:pyc:{digest}:const:0:+0"),
        format!("rxd1:pyc:{digest}:const:0:０"),
        format!("rxd1:pyc:{digest}:const:0:99999999999999999999999"),
        format!("rxd1:pyc:{digest}:const:0:99"),
        format!("rxd1:pyc:{digest}:const:0:2.9"),
        format!("rxd1:pyc:{digest}:const:0:0.0"),
        format!("rxd1:pyc:{digest}:const:0:2"),
        format!("rxd1:pyc:{digest}:const:0:6"),
    ] {
        failure(
            invoke(
                "refs",
                &path,
                &["--version", "3.12", "--const-id", &invalid],
            ),
            2,
            "INVALID_QUERY",
        );
    }
    failure(
        invoke(
            "refs",
            &path,
            &["--version", "3.12", "--const-id", id, "--within", id],
        ),
        2,
        "INVALID_QUERY",
    );
    for args in [
        vec!["--version", "3.12"],
        vec!["--version", "3.12", "--const-id", id, "--name", "x"],
    ] {
        let failed = invoke("refs", &path, &args);
        assert_eq!(failed.status.code(), Some(2));
        assert!(failed.stdout.is_empty());
    }
    let copied = dir.path().join("renamed.marshal");
    fs::write(&copied, &bytes).unwrap();
    assert_eq!(
        invoke("refs", &copied, &["--version", "3.12", "--const-id", id]).stdout,
        invoke("refs", &path, &["--version", "3.12", "--const-id", id]).stdout
    );
    let mut changed = bytes.clone();
    let pos = changed
        .windows(10)
        .position(|w| w == b"fixture.py")
        .unwrap();
    changed[pos] = b'F';
    fs::write(&copied, &changed).unwrap();
    failure(
        invoke("refs", &copied, &["--version", "3.12", "--const-id", id]),
        2,
        "INVALID_QUERY",
    );
    let mutable = module(&[121, 0], b")\x01[\x01\0\0\0z\x01x");
    fs::write(&copied, &mutable).unwrap();
    let forged = format!("rxd1:pyc:{:x}:const:0:0.0", Sha256::digest(&mutable));
    failure(
        invoke(
            "refs",
            &copied,
            &["--version", "3.12", "--const-id", &forged],
        ),
        2,
        "INVALID_QUERY",
    );
    let names = rows(&invoke(
        "refs",
        &path,
        &["--version", "3.12", "--name", "x"],
    ));
    assert!(names[0]["query"]["const_id"].is_null());
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
fn pyc_constant_refs_charge_path_decode_matches_and_complete_output_atomically() {
    let bytes = shared_input();
    let (dir, path) = fixture(&bytes, "shared.marshal");
    let id = rows(&invoke("strings", &path, &["--version", "3.12"]))[1]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let complete = invoke(
        "refs",
        &path,
        &["--version", "3.12", "--const-id", &id, "--perf"],
    );
    let stream = rows(&complete);
    let perf = perf_row(&complete);
    assert_eq!(stream.last().unwrap()["records"], 2);
    assert_eq!(perf["counters"]["instructions"], 14);
    assert_eq!(perf["counters"]["records"], 2);
    assert!(perf["counters"]["ast_depth"].is_null());
    let out = dir.path().join("keep.jsonl");
    for (flag, amount) in [
        ("--max-input-bytes", bytes.len()),
        ("--max-output-bytes", complete.stdout.len()),
        (
            "--max-work",
            perf["counters"]["work"].as_u64().unwrap() as usize,
        ),
        (
            "--max-entities",
            perf["counters"]["entities"].as_u64().unwrap() as usize,
        ),
        ("--max-records", 2),
        ("--max-instructions", 14),
    ] {
        let exact = amount.to_string();
        let less = (amount - 1).to_string();
        let accepted = invoke(
            "refs",
            &path,
            &["--version", "3.12", "--const-id", &id, flag, &exact],
        );
        success(&accepted);
        assert_eq!(accepted.stdout, complete.stdout, "{flag}");
        fs::write(&out, b"keep").unwrap();
        let failed = invoke(
            "refs",
            &path,
            &[
                "--version",
                "3.12",
                "--const-id",
                &id,
                flag,
                &less,
                "--perf",
                "-o",
                out.to_str().unwrap(),
            ],
        );
        let failed_perf = perf_row(&failed);
        failure(failed, 1, "RESOURCE_LIMIT");
        assert_eq!(fs::read(&out).unwrap(), b"keep");
        assert_eq!(fs::read(&path).unwrap(), bytes);
        if flag == "--max-work" {
            assert!(failed_perf["counters"]["work"].as_u64().unwrap() <= (amount - 1) as u64);
        }
    }
    let output = invoke(
        "refs",
        &path,
        &[
            "--version",
            "3.12",
            "--const-id",
            &id,
            "-o",
            out.to_str().unwrap(),
        ],
    );
    success(&output);
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(&out).unwrap(), complete.stdout);
    failure(
        invoke(
            "refs",
            &path,
            &[
                "--version",
                "3.12",
                "--const-id",
                &id,
                "-o",
                path.to_str().unwrap(),
            ],
        ),
        2,
        "INVALID_QUERY",
    );
    let mut deep = b"z\x01x".to_vec();
    for _ in 0..70 {
        let mut parent = b")\x01".to_vec();
        parent.extend(deep);
        deep = parent;
    }
    let mut consts = b")\x01".to_vec();
    consts.extend(deep);
    let deep_bytes = module(&[121, 0], &consts);
    let deep_path = dir.path().join("deep.marshal");
    fs::write(&deep_path, deep_bytes).unwrap();
    let literal = rows(&invoke("strings", &deep_path, &["--version", "3.12"]))[1].clone();
    assert_eq!(
        literal["detail"]["constant_path"].as_array().unwrap().len(),
        71
    );
    let selected = invoke(
        "refs",
        &deep_path,
        &[
            "--version",
            "3.12",
            "--const-id",
            literal["id"].as_str().unwrap(),
        ],
    );
    success(&selected);
    let exact = selected.stdout.len().to_string();
    let bounded = invoke(
        "refs",
        &deep_path,
        &[
            "--version",
            "3.12",
            "--const-id",
            literal["id"].as_str().unwrap(),
            "--max-output-bytes",
            &exact,
        ],
    );
    success(&bounded);
    assert_eq!(bounded.stdout, selected.stdout);
}

#[test]
fn pyc_constant_refs_expose_unknown_coverage_and_fail_structural_damage_in_owner() {
    let (dir, path) = fixture(
        module(&[17, 0, 121, 0], b")\x02z\x01xz\x06unused"),
        "unknown.marshal",
    );
    let strings = rows(&invoke("strings", &path, &["--version", "3.12"]));
    for literal in &strings[1..strings.len() - 1] {
        let result = rows(&invoke(
            "refs",
            &path,
            &[
                "--version",
                "3.12",
                "--const-id",
                literal["id"].as_str().unwrap(),
            ],
        ));
        assert_eq!(result.last().unwrap()["complete"], false);
        assert_eq!(result.last().unwrap()["detail"]["unsupported_count"], 1);
        assert_eq!(
            result.last().unwrap()["detail"]["unsupported"][0]["status"],
            "UNSUPPORTED_OPCODE"
        );
        assert_eq!(
            result.last().unwrap()["records"],
            if literal["detail"]["value"] == "x" {
                1
            } else {
                0
            }
        );
    }
    for body in [
        vec![100, 255],
        vec![100],
        vec![144, 0],
        vec![110, 255],
        vec![100, 0, 122, 0],
    ] {
        let bytes = module(&body, b")\x02z\x01xz\x06unused");
        fs::write(&path, &bytes).unwrap();
        let literal = rows(&invoke("strings", &path, &["--version", "3.12"]))[2].clone();
        let out = dir.path().join("keep.jsonl");
        fs::write(&out, b"keep").unwrap();
        failure(
            invoke(
                "refs",
                &path,
                &[
                    "--version",
                    "3.12",
                    "--const-id",
                    literal["id"].as_str().unwrap(),
                    "-o",
                    out.to_str().unwrap(),
                ],
            ),
            1,
            "INVALID_INPUT",
        );
        assert_eq!(fs::read(&out).unwrap(), b"keep");
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    let mut bytes = shared_input();
    bytes[27] = 255; // Root's first LOAD_CONST operand, before its independent child code.
    fs::write(&path, &bytes).unwrap();
    let literals = rows(&invoke("strings", &path, &["--version", "3.12"]));
    let root = &literals[1];
    let child = literals
        .iter()
        .find(|r| r["detail"]["code_index"] == 1 && r["detail"]["constant_slot"] == 0)
        .unwrap();
    success(&invoke(
        "refs",
        &path,
        &[
            "--version",
            "3.12",
            "--const-id",
            child["id"].as_str().unwrap(),
        ],
    ));
    failure(
        invoke(
            "refs",
            &path,
            &[
                "--version",
                "3.12",
                "--const-id",
                root["id"].as_str().unwrap(),
            ],
        ),
        1,
        "INVALID_INPUT",
    );
    let excluded = rows(&invoke(
        "refs",
        &path,
        &[
            "--version",
            "3.12",
            "--const-id",
            root["id"].as_str().unwrap(),
            "--within",
            child["context_id"].as_str().unwrap(),
        ],
    ));
    assert_eq!(excluded.last().unwrap()["complete"], true);
    assert_eq!(excluded.last().unwrap()["records"], 0);
}
