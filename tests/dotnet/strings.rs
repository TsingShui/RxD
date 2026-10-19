//! String discovery and navigation through the actual CLI.
use super::*;

fn string_input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/strings/Strings.dll")
}

fn string_expected() -> Value {
    serde_json::from_str(include_str!("../fixtures/dotnet/strings/expected.json")).unwrap()
}

#[test]
fn dotnet_string_inventory_retains_all_heap_identities_owners_and_lossless_units() {
    let path = string_input();
    let oracle = string_expected();
    let rows = jsonl(&run("strings", &path, &[]));
    assert_eq!(rows, jsonl(&run("strings", &path, &[])));
    let empty_filter = jsonl(&run("strings", &path, &["--contains", ""]));
    assert_eq!(rows[1..], empty_filter[1..]);
    assert_eq!(rows[0]["schema_version"], 2);
    assert_eq!(rows.last().unwrap()["records"], 12);
    assert_eq!(rows.last().unwrap()["complete"], true);
    let records: Vec<_> = rows
        .iter()
        .filter(|row| row["record"] == "string")
        .collect();
    for entry in oracle["entries"].as_array().unwrap() {
        let groups: Vec<_> = records
            .iter()
            .filter(|row| row["detail"]["offset"] == entry["offset"])
            .collect();
        let uses = entry["uses"].as_array().unwrap();
        if uses.is_empty() {
            assert_eq!(groups.len(), 1);
            let row = groups[0];
            assert_eq!(row["detail"]["status"], "unreferenced");
            assert!(row["context_id"].is_null());
            assert!(row["parent_id"].is_null());
            assert!(row["detail"]["declaring_type_id"].is_null());
            assert!(row["detail"]["method_token"].is_null());
            assert_eq!(row["detail"]["il_offsets"], serde_json::json!([]));
        } else {
            let mut owners = std::collections::BTreeMap::<u64, Vec<u64>>::new();
            for site in uses {
                owners
                    .entry(site["method_token"].as_u64().unwrap())
                    .or_default()
                    .push(site["il_offset"].as_u64().unwrap());
            }
            assert_eq!(groups.len(), owners.len());
            for (row, (method_token, offsets)) in groups.iter().zip(owners) {
                assert_eq!(row["detail"]["status"], "referenced");
                assert_eq!(row["detail"]["method_token"], method_token);
                assert_eq!(row["detail"]["il_offsets"], serde_json::json!(offsets));
                let expected_method = oracle["methods"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|method| method["token"] == method_token)
                    .unwrap();
                let method_id = format!(
                    "rxd1:dotnet:{}:method:{method_token:08x}",
                    oracle["sha256"].as_str().unwrap()
                );
                let owner_token = expected_method["declaring_type_token"].as_u64().unwrap();
                let owner_id = format!(
                    "rxd1:dotnet:{}:type:{owner_token:08x}",
                    oracle["sha256"].as_str().unwrap()
                );
                assert_eq!(row["context_id"], method_id);
                assert_eq!(row["detail"]["declaring_type_id"], owner_id);
                assert_eq!(row["parent_id"], owner_id);
                let fetched = run("get-method", &path, &[&method_id, "--il"]);
                success(&fetched);
                let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
                assert_eq!(fetched["detail"]["il_hex"], expected_method["il_hex"]);
            }
        }
        for row in groups {
            assert_eq!(row["detail"]["token"], entry["token"]);
            assert_eq!(row["detail"]["value"], entry["value"]);
            assert_eq!(row["detail"]["utf16_units"], entry["utf16_units"]);
            assert!(row["id"].is_null());
            assert!(row["start"].is_null());
            assert!(row["end"].is_null());
            assert_eq!(
                row["detail"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                [
                    "declaring_type_id",
                    "il_offsets",
                    "method_token",
                    "offset",
                    "status",
                    "token",
                    "utf16_units",
                    "value"
                ]
            );
        }
    }
    let nul = jsonl(&run("strings", &path, &["--contains", "b"]));
    assert_eq!(nul[1]["detail"]["value"], "a\0b");
    let beside_surrogate = jsonl(&run("strings", &path, &["--contains", "尾"]));
    assert_eq!(beside_surrogate.last().unwrap()["records"], 2);
    assert!(beside_surrogate[1]["detail"]["value"].is_null());
    assert_eq!(
        beside_surrogate[1]["detail"]["utf16_units"],
        serde_json::json!([112, 114, 101, 102, 105, 120, 55296, 23614])
    );
}

#[test]
fn dotnet_exact_string_refs_match_every_equal_heap_entry_and_original_load_site() {
    let path = string_input();
    let oracle = string_expected();
    for value in ["shared 世界\nλ", "", "🙂世界", "SHARED 世界\nλ", "absent"] {
        let rows = jsonl(&run("refs", &path, &["--string", value]));
        assert_eq!(rows, jsonl(&run("refs", &path, &["--string", value])));
        assert_eq!(rows[0]["query"]["string"], value);
        let mut expected = Vec::new();
        for entry in oracle["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["value"] == value)
        {
            for site in entry["uses"].as_array().unwrap() {
                expected.push((
                    site["method_token"].as_u64().unwrap(),
                    site["il_offset"].as_u64().unwrap(),
                    entry["token"].as_u64().unwrap(),
                ));
            }
        }
        expected.sort_unstable();
        assert_eq!(rows.last().unwrap()["records"], expected.len());
        assert_eq!(rows.last().unwrap()["complete"], true);
        for (row, (method, offset, token)) in rows[1..rows.len() - 1].iter().zip(expected) {
            assert_eq!(row["record"], "reference");
            assert_eq!(row["detail"]["method_token"], method);
            assert_eq!(row["detail"]["il_offset"], offset);
            assert_eq!(row["detail"]["target_token"], token);
            assert_eq!(row["detail"]["opcode"], "ldstr");
            let method_oracle = oracle["methods"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["token"] == method)
                .unwrap();
            let start = method_oracle["code_offset"].as_u64().unwrap() + offset;
            assert_eq!(row["start"], start);
            assert_eq!(row["end"], start + 5);
            let fetched = run(
                "get-method",
                &path,
                &[row["context_id"].as_str().unwrap(), "--il"],
            );
            success(&fetched);
            let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
            assert_eq!(fetched["detail"]["method"]["detail"]["token"], method);
            assert_eq!(fetched["parent_id"], row["detail"]["declaring_type_id"]);
        }
    }
    // Values not expressible as argv UTF-8 remain reachable by raw user-string token.
    for entry in oracle["entries"].as_array().unwrap() {
        let token = format!("0x{:08x}", entry["token"].as_u64().unwrap());
        let rows = jsonl(&run("refs", &path, &["--token", &token]));
        assert_eq!(
            rows.last().unwrap()["records"],
            entry["uses"].as_array().unwrap().len()
        );
        for row in &rows[1..rows.len() - 1] {
            assert_eq!(row["detail"]["target_token"], entry["token"]);
        }
    }
}

#[test]
fn dotnet_string_token_scan_rejects_damaged_unused_heap_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("damaged-heap.dll");
    let oracle = string_expected();
    let mut bytes = fs::read(string_input()).unwrap();
    let unused = oracle["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["label"] == "unused")
        .unwrap();
    let start = oracle["us_file_offset"].as_u64().unwrap() as usize
        + unused["offset"].as_u64().unwrap() as usize;
    let length = bytes[start] as usize;
    bytes[start + length] = 2; // Invalid terminal flag in an unused heap entry.
    fs::write(&path, bytes).unwrap();
    for (command, args) in [
        ("strings", vec!["--contains", "shared"]),
        ("refs", vec!["--string", "shared 世界\nλ"]),
        ("refs", vec!["--token", "0x70000001"]),
    ] {
        let output = run(command, &path, &args);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{command} {args:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        rejected(output);
    }
}

#[test]
fn dotnet_string_scans_reject_damage_even_when_the_selected_value_is_absent() {
    let oracle = string_expected();
    let directory = tempfile::tempdir().unwrap();
    let original = fs::read(string_input()).unwrap();
    let late_method = oracle["methods"][2]["code_offset"].as_u64().unwrap() as usize;
    let heap = oracle["us_file_offset"].as_u64().unwrap() as usize;
    for (name, offset, replacement) in [
        ("invalid-last-method", late_method + 18, 0xff),
        ("ldstr-interior-offset", late_method + 1, 2),
        ("truncated-heap-entry", heap + 111, 0x7f),
    ] {
        let path = directory.path().join(format!("{name}.dll"));
        let mut bytes = original.clone();
        bytes[offset] = replacement;
        fs::write(&path, bytes).unwrap();
        for (command, args) in [
            ("strings", vec!["--contains", "absent"]),
            ("refs", vec!["--string", "absent"]),
            ("refs", vec!["--token", "0x70000001"]),
        ] {
            let output = run(command, &path, &args);
            assert_eq!(output.status.code(), Some(1), "{name}: {command} {args:?}");
            assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_INPUT"));
            rejected(output);
        }
    }
    for args in [
        vec![],
        vec!["--token", "0x70000001", "--string", "shared"],
        vec!["--token", "0x70000002"],
        vec!["--token", "0x70000000"],
        vec!["--token", "0x70ffffff"],
        vec!["--token", "bad-token"],
    ] {
        let output = run("refs", &string_input(), &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        rejected(output);
    }
}

#[test]
fn dotnet_string_scans_account_cumulative_work_and_fail_atomically_at_cli_limits() {
    let path = string_input();
    let oracle = string_expected();
    success(&run("outline", &path, &["--max-work", "400"]));
    for method in oracle["methods"].as_array().unwrap() {
        let id = format!(
            "rxd1:dotnet:{}:method:{:08x}",
            oracle["sha256"].as_str().unwrap(),
            method["token"].as_u64().unwrap()
        );
        success(&run(
            "get-method",
            &path,
            &[&id, "--il", "--max-work", "400"],
        ));
    }
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("strings.jsonl");
    fs::write(&destination, b"previous result").unwrap();
    for (command, query) in [
        ("strings", vec!["--contains", "shared"]),
        ("refs", vec!["--string", "shared 世界\nλ"]),
        ("refs", vec!["--token", "0x70000001"]),
    ] {
        fs::write(&destination, b"previous result").unwrap();
        for (flag, limit) in [
            ("--max-work", "400"),
            ("--max-instructions", "2"),
            ("--max-records", "1"),
            ("--max-output-bytes", "32"),
        ] {
            let mut args = query.clone();
            args.extend([flag, limit, "-o", destination.to_str().unwrap()]);
            let output = run(command, &path, &args);
            assert_eq!(output.status.code(), Some(1), "{command} {args:?}");
            assert!(String::from_utf8_lossy(&output.stderr).contains("RESOURCE_LIMIT"));
            rejected(output);
            assert_eq!(fs::read(&destination).unwrap(), b"previous result");
        }
        let plain = run(command, &path, &query);
        success(&plain);
        let mut args = query;
        args.extend(["--perf", "-o", destination.to_str().unwrap()]);
        let output = run(command, &path, &args);
        success(&output);
        assert!(output.stdout.is_empty());
        assert_eq!(fs::read(&destination).unwrap(), plain.stdout);
        let diagnostics: Vec<Value> = String::from_utf8(output.stderr)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics.iter().all(|row| row["backend"] == "dotnet"));
    }
}

#[test]
fn dotnet_string_search_routes_directly_to_original_method_and_class() {
    let path = input();
    let rows = jsonl(&run("strings", &path, &["--contains", "世界"]));
    let hit = rows.iter().find(|row| row["record"] == "string").unwrap();
    assert_eq!(hit["detail"]["status"], "referenced");
    assert_eq!(hit["detail"]["il_offsets"], serde_json::json!([0]));
    let id = hit["context_id"]
        .as_str()
        .expect("search must return method FETCH context");
    let method = run("get-method", &path, &[id, "--il"]);
    success(&method);
    let method: Value = serde_json::from_slice(&method.stdout).unwrap();
    assert_eq!(
        method["detail"]["il_hex"],
        expected()["methods"]["Unicode"]["il_hex"]
    );
    let owner = hit["detail"]["declaring_type_id"]
        .as_str()
        .expect("search must return class FETCH context");
    let class = run("get-class", &path, &[owner, "--il"]);
    success(&class);
    let class: Value = serde_json::from_slice(&class.stdout).unwrap();
    assert!(
        class["detail"]["methods"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == id)
    );
}
