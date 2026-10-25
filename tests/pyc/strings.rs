use super::*;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/pyc/3.12.14")
        .join(name)
}

fn strings(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "strings"])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

fn rows(output: &Output) -> Vec<Value> {
    success(output);
    output
        .stdout
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}

fn expected_constants(value: &Value, path: &mut Vec<usize>, result: &mut Vec<(Vec<usize>, Value)>) {
    match value["type"].as_str().unwrap() {
        "str" | "bytes" => result.push((path.clone(), value.clone())),
        "tuple" | "frozenset" => {
            for (index, child) in value["items"].as_array().unwrap().iter().enumerate() {
                path.push(index);
                expected_constants(child, path, result);
                path.pop();
            }
        }
        _ => {}
    }
}

#[test]
fn pyc_strings_match_independent_lossless_constants_and_occurrence_ids() {
    let path = corpus("graph.timestamp.pyc");
    let bytes = fs::read(&path).unwrap();
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("graph.oracle.json")).unwrap()).unwrap();
    let data = rows(&strings(&path, &[]));
    let mut observed = data[..].iter();
    let mut count = 0;
    for (owner, code) in oracle["codes"].as_array().unwrap().iter().enumerate() {
        let mut expected = Vec::new();
        for (slot, value) in code["consts"].as_array().unwrap().iter().enumerate() {
            expected_constants(value, &mut vec![slot], &mut expected);
        }
        for (indices, expected) in expected {
            let row = observed.next().expect("missing constant occurrence");
            let id_path = indices
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(".");
            assert_eq!(row["at"], format!("const:{owner}:{id_path}"));
            assert_eq!(row["parent_at"], format!("code:{owner}"));
            assert_eq!(row["context_at"], format!("code:{owner}"));
            assert_eq!(row["detail"]["code_index"], owner);
            assert_eq!(row["detail"]["constant_slot"], indices[0]);
            assert_eq!(row["detail"]["constant_path"], serde_json::json!(indices));
            assert_eq!(row["detail"]["kind"], expected["type"]);
            assert_eq!(row["detail"]["value"], expected["value"]);
            let raw = if expected["type"] == "str" {
                &expected["utf8_surrogatepass_hex"]
            } else {
                &expected["hex"]
            };
            assert_eq!(&row["detail"]["raw_bytes"], raw);
            let start = row["detail"]["payload_start"].as_u64().unwrap() as usize;
            let end = row["detail"]["payload_end"].as_u64().unwrap() as usize;
            assert_eq!(
                bytes[start..end]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>(),
                *raw
            );
            if row["detail"]["reference"] == true
                && let Some(start) = row["detail"]["occurrence_start"].as_u64()
            {
                let reference = oracle["marshal_ranges"]["reference_occurrences"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|reference| reference["start"].as_u64().unwrap() + 16 == start)
                    .unwrap();
                assert_eq!(
                    row["detail"]["occurrence_end"],
                    reference["end"].as_u64().unwrap() + 16
                );
                assert_eq!(
                    row["detail"]["definition_start"],
                    reference["target_start"].as_u64().unwrap() + 16
                );
                assert!(row["start"].is_null());
                assert!(row["end"].is_null());
            }
            if let Some(start) = row["start"].as_u64() {
                assert_eq!(
                    start as usize,
                    row["detail"]["payload_start"].as_u64().unwrap() as usize
                );
                assert_eq!(row["end"], row["detail"]["payload_end"]);
            }
            count += 1;
        }
    }
    assert!(observed.next().is_none());
    assert_eq!(data.len(), count);
}

#[test]
fn pyc_strings_filter_exact_text_and_raw_payload_without_losing_shared_occurrences() {
    let path = corpus("graph.timestamp.pyc");
    let all = rows(&strings(&path, &[]));
    for (args, expected) in [
        (vec!["--filter", "雪"], 3),
        (vec!["--filter-hex", "EDA080"], 3),
        (vec!["--filter-hex", "ff00"], 1),
        (vec!["--filter", "shared-container-marker"], 5),
        (vec!["--filter", "SHARED-CONTAINER-MARKER"], 5),
        (vec!["--filter", "\u{fffd}"], 0),
    ] {
        let matched = rows(&strings(&path, &args));
        assert_eq!(matched.len(), expected, "{args:?}");
        for row in &matched[..] {
            assert!(all.contains(row));
        }
    }
    for hex in ["f", "xx", "0x00", "ff 00", "éé"] {
        let invalid = strings(&path, &["--filter-hex", hex]);
        assert_eq!(invalid.status.code(), Some(2));
        assert!(invalid.stdout.is_empty());
    }
    let conflict = strings(&path, &["--filter", "a", "--filter-hex", "61"]);
    assert_eq!(conflict.status.code(), Some(2));
    assert!(conflict.stdout.is_empty());
    let empty_hex = rows(&strings(&path, &["--filter-hex", ""]));
    assert_eq!(&empty_hex[..], &all[..]);
    let empty_text = rows(&strings(&path, &["--filter", ""]));
    assert_eq!(empty_text.len(), all.len() - 3);
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
    out.extend_from_slice(body);
    out.extend_from_slice(consts);
    out.extend_from_slice(b")\0)\0");
    out.extend(bytes_field(b""));
    out.extend_from_slice(b"z\x0afixture.pyz\x08<module>z\x08<module>");
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend(bytes_field(b""));
    out.extend(bytes_field(b""));
    out
}

#[test]
fn pyc_strings_distinguish_literal_definitions_container_aliases_and_shared_const_tables() {
    let mut consts = vec![b')', 5, b')' | 0x80, 1, b'z' | 0x80, 1, b'x'];
    consts.extend_from_slice(&[b')' | 0x80, 1]);
    consts.extend_from_slice(b"r\0\0\0\0r\x02\0\0\0>\x01\0\0\0r\0\0\0\0");
    consts.extend(module(&bytes_field(&[100]), b"r\0\0\0\0"));
    let bytes = module(&bytes_field(&[100]), &consts); // Odd bodies deliberately remain unscanned.
    let (_dir, path) = fixture(&bytes, "shared.marshal");
    let all = rows(&strings(&path, &["--version", "3.12"]));
    assert_eq!(all.len(), 5);
    let expected = [
        (0, vec![0, 0]),
        (0, vec![1, 0, 0]),
        (0, vec![2, 0, 0]),
        (0, vec![3, 0, 0]),
        (1, vec![0]),
    ];
    for (row, (owner, path)) in all[..5].iter().zip(expected) {
        assert_eq!(row["detail"]["code_index"], owner);
        assert_eq!(row["detail"]["constant_path"], serde_json::json!(path));
        assert_eq!(row["detail"]["value"], "x");
        assert_eq!(row["detail"]["raw_bytes"], "78");
        assert_eq!(row["parent_at"], row["context_at"]);
        assert_eq!(
            row["detail"]["definition_start"],
            all[1]["detail"]["definition_start"]
        );
    }
    let x = all
        .iter()
        .find(|row| row["detail"]["value"] == "x")
        .unwrap();
    let start = x["start"].as_u64().unwrap() as usize;
    let end = x["end"].as_u64().unwrap() as usize;
    assert_eq!(&bytes[start..end], b"x");
    for row in &all[1..5] {
        for key in ["start", "end"] {
            assert!(row[key].is_null());
        }
        for key in ["occurrence_start", "occurrence_end"] {
            assert!(row["detail"][key].is_null());
        }
        for alias in row["detail"]["aliases"].as_array().unwrap() {
            let start = alias["start"].as_u64().unwrap() as usize;
            let end = alias["end"].as_u64().unwrap() as usize;
            assert_eq!(bytes[start], b'r');
            assert_eq!(end - start, 5);
        }
    }
    assert_eq!(
        all.iter()
            .find(|row| row["detail"]["aliases"]
                .as_array()
                .is_some_and(|aliases| aliases.len() == 2))
            .unwrap()["detail"]["aliases"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        all[4]["detail"]["aliases"][0]["constant_path"],
        serde_json::json!([])
    );
    failure(
        run(&path, &["--version", "3.12", "--json"]),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn pyc_strings_budget_work_includes_actual_hex_generation_before_allocation() {
    let payload = vec![b'a'; 500];
    let mut consts = b")\x01".to_vec();
    consts.extend(bytes_field(&payload));
    let (_dir, path) = fixture(module(&bytes_field(&[]), &consts), "work.marshal");
    let zero = strings(
        &path,
        &["--version", "3.12", "--filter", "missing", "--perf"],
    );
    let reports: Vec<Value> = String::from_utf8(zero.stderr)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let without_output = reports
        .iter()
        .find(|row| row["scope"] == "analysis")
        .unwrap()["counters"]["work"]
        .as_u64()
        .unwrap();
    let all = strings(&path, &["--version", "3.12", "--perf"]);
    success(&all);
    let reports: Vec<Value> = String::from_utf8(all.stderr)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let with_output = reports
        .iter()
        .find(|row| row["scope"] == "analysis")
        .unwrap()["counters"]["work"]
        .as_u64()
        .unwrap();
    // Missing-byte matching visits 494 bytes; raw hex still processes all 500.
    assert!(with_output > without_output);
}

#[test]
fn pyc_strings_preserve_empty_control_unicode_bytes_and_duplicate_definitions() {
    let mut consts = b")\x07u\0\0\0\0".to_vec();
    consts.extend(bytes_field(b""));
    consts.extend_from_slice(b"u\x05\0\0\0\0\n\xe9\x9b\xaa");
    consts.extend(bytes_field(b"\xffa\0b"));
    consts.extend_from_slice(b"z\x01xz\x01x[\x01\0\0\0z\x07ignored");
    let (_dir, path) = fixture(module(&bytes_field(&[100]), &consts), "literal.marshal");
    let all = rows(&strings(&path, &["--version", "3.12"]));
    assert_eq!(all.len(), 6); // Mutable list is outside the immutable-constant scope.
    assert_eq!(all[0]["detail"]["value"], "");
    assert_eq!(all[0]["start"], all[0]["end"]);
    assert_eq!(all[1]["detail"]["raw_bytes"], "");
    assert!(all[1]["detail"]["value"].is_null());
    assert_eq!(all[2]["detail"]["value"], "\0\n雪");
    assert_eq!(all[3]["detail"]["raw_bytes"], "ff610062");
    assert_ne!(all[4]["at"], all[5]["at"]);
    assert_ne!(all[4]["start"], all[5]["start"]);
    assert_eq!(
        rows(&strings(&path, &["--version", "3.12", "--filter", "a"]))[0],
        all[3]
    );
    assert_eq!(
        rows(&strings(
            &path,
            &["--version", "3.12", "--filter-hex", "610062"]
        ))[0],
        all[3]
    );
    assert_eq!(
        rows(&strings(
            &path,
            &["--version", "3.12", "--filter-hex", "000a"]
        ))[0],
        all[2]
    );
    assert_eq!(
        rows(&strings(&path, &["--version", "3.12", "--filter", ""])).len(),
        all.len()
    );
    assert!(
        rows(&strings(
            &path,
            &["--version", "3.12", "--filter", "ignored"]
        ))
        .is_empty()
    );
}

#[test]
fn pyc_strings_fixed_schema_budget_neighbors_identity_and_output_are_atomic() {
    let path = corpus("graph.timestamp.pyc");
    let output = strings(&path, &["--perf"]);
    let all = rows(&output);
    for row in &all[..] {
        assert_js_keys(
            row,
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
            &row["detail"],
            &[
                "code_index",
                "constant_slot",
                "constant_path",
                "kind",
                "value",
                "raw_bytes",
                "definition_start",
                "definition_end",
                "payload_start",
                "payload_end",
                "occurrence_start",
                "occurrence_end",
                "reference",
                "aliases",
                "index",
            ],
        );
        for alias in row["detail"]["aliases"].as_array().unwrap() {
            assert_js_keys(alias, &["start", "end", "constant_path"]);
        }
    }
    let reports: Vec<Value> = String::from_utf8(output.stderr)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(reports.len(), 2);
    let perf = reports
        .iter()
        .find(|row| row["scope"] == "analysis")
        .unwrap();
    assert_eq!(perf["command"], "strings");
    assert!(perf["counters"]["instructions"].is_null());
    assert!(perf["counters"]["ast_depth"].is_null());
    let budget =
        rxd_core::Budget::for_input_bytes(fs::metadata(&path).unwrap().len() as usize).unwrap();
    assert_eq!(
        perf["resource_limits"],
        serde_json::json!({
            "input_bytes": budget.input_bytes,
            "entities": budget.work,
            "work": budget.work,
            "metadata_bytes": budget.metadata_bytes,
            "marshal_objects": budget.work,
            "marshal_expanded_objects": budget.work,
            "marshal_copied_bytes": budget.metadata_bytes,
        })
    );
    let bytes = fs::read(&path).unwrap();
    let (dir, renamed) = fixture(&bytes, "renamed.bin");
    assert_eq!(strings(&renamed, &[]).stdout, output.stdout);
    assert_eq!(strings(&path, &[]).stdout, output.stdout);
    assert_no_budget_flags(&path, "strings");
    // Exact accounting lives in the library, which owns the shared ceilings.
    let mut base = shared_static_limits();
    base.input_bytes = bytes.len();
    let (perf, literals) = strings_library(&path, None, None, None, base);
    assert_eq!(literals.unwrap().len(), all.len());
    let perf = perf_value(&perf);
    let entities = perf["counters"]["entities"].as_u64().unwrap() as usize;
    let work = perf["counters"]["work"].as_u64().unwrap() as usize;
    for (field, amount) in [
        ("input_bytes", bytes.len()),
        ("entities", entities),
        ("work", work),
    ] {
        let mut exact = base;
        match field {
            "input_bytes" => exact.input_bytes = amount,
            "entities" => exact.entities = amount,
            _ => exact.work = amount,
        }
        assert!(
            strings_library(&path, None, None, None, exact).1.is_ok(),
            "{field}"
        );
        let mut less = exact;
        match field {
            "input_bytes" => less.input_bytes = amount - 1,
            "entities" => less.entities = amount - 1,
            _ => less.work = amount - 1,
        }
        let error = strings_library(&path, None, None, None, less)
            .1
            .err()
            .unwrap();
        assert_eq!(error.code, "RESOURCE_LIMIT", "{field}");
    }
    let destination = dir.path().join("strings.jsonl");
    let written = strings(&path, &["-o", destination.to_str().unwrap()]);
    success(&written);
    assert!(written.stdout.is_empty());
    assert_eq!(fs::read(destination).unwrap(), output.stdout);
    failure(
        strings(&path, &["-o", path.to_str().unwrap()]),
        2,
        "INVALID_QUERY",
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let mut changed = bytes.clone();
    changed[8] = 1; // Identity includes the input header even when literals are unchanged.
    let (_dir, changed) = fixture(changed, "changed.pyc");
    let changed = rows(&strings(&changed, &[]));
    assert_eq!(changed[0]["at"], all[0]["at"]);
    assert_eq!(changed[0]["detail"], all[0]["detail"]);
    let raw = rows(&strings(&corpus("graph.marshal"), &["--version", "3.12"]));
    for (raw, pyc) in raw[..].iter().zip(&all[..]) {
        assert_eq!(raw["detail"]["raw_bytes"], pyc["detail"]["raw_bytes"]);
        assert_eq!(
            raw["detail"]["payload_start"].as_u64().unwrap() + 16,
            pyc["detail"]["payload_start"]
        );
    }
    let (_, literals) = strings_library(
        &path,
        None,
        Some("absent-value"),
        None,
        shared_static_limits(),
    );
    assert!(literals.unwrap().is_empty());
}

#[test]
fn pyc_strings_reject_invalid_headers_raw_modes_and_trailing_marshal_atomically() {
    let source = fs::read(corpus("graph.timestamp.pyc")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("bad.pyc");
    let destination = dir.path().join("keep.jsonl");
    for bytes in [&source[..15], &source[..source.len() - 1], b"not pyc"] {
        fs::write(&input, bytes).unwrap();
        fs::write(&destination, b"keep").unwrap();
        failure(
            strings(&input, &["-o", destination.to_str().unwrap()]),
            1,
            "INVALID_INPUT",
        );
        assert_eq!(fs::read(&destination).unwrap(), b"keep");
    }
    let mut bytes = source.clone();
    bytes.push(0);
    fs::write(&input, &bytes).unwrap();
    failure(strings(&input, &[]), 1, "INVALID_INPUT");
    let mut bytes = source.clone();
    bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
    fs::write(&input, &bytes).unwrap();
    failure(strings(&input, &[]), 1, "INVALID_INPUT");
    failure(strings(&corpus("graph.marshal"), &[]), 1, "INVALID_INPUT");
    failure(
        strings(&corpus("graph.timestamp.pyc"), &["--version", "3.12"]),
        1,
        "INVALID_INPUT",
    );
    failure(
        strings(&corpus("graph.marshal"), &["--version", "3.14"]),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn pyc_strings_bound_long_match_work_and_deep_paths_without_false_output_limits() {
    let mut consts = b")\x01".to_vec();
    for _ in 0..80 {
        consts.extend_from_slice(b")\x01");
    }
    consts.extend(bytes_field(&vec![b'a'; 500]));
    let (_dir, path) = fixture(module(&bytes_field(&[]), &consts), "deep.marshal");
    let all = strings(&path, &["--version", "3.12"]);
    let data = rows(&all);
    assert_eq!(
        data[0]["detail"]["constant_path"].as_array().unwrap().len(),
        81
    );
    assert_no_budget_flags(&path, "strings");
    // Exact accounting lives in the library: a full-budget scan of the deep literal
    // succeeds, and the long match must exhaust an explicit 5,000-unit work ceiling.
    let all_limits = shared_static_limits();
    assert!(
        strings_library(&path, Some((3, 12)), None, None, all_limits)
            .1
            .is_ok()
    );
    let needle = format!("{}b", "a".repeat(200));
    let mut limits = all_limits;
    limits.work = 5_000;
    let (perf, literals) = strings_library(&path, Some((3, 12)), Some(&needle), None, limits);
    assert_eq!(literals.err().unwrap().code, "RESOURCE_LIMIT");
    let perf = perf_value(&perf);
    assert_eq!(perf["resource_limits"]["work"], 5_000);
    assert_eq!(perf["counters"]["work"], 5_000);
}
