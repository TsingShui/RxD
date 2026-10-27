//! String discovery and navigation through the actual CLI.
use super::*;

fn string_input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/strings/Strings.dll")
}

fn string_expected() -> Value {
    serde_json::from_str(include_str!("../fixtures/dotnet/strings/expected.json")).unwrap()
}
/// Tokens name member identities; content matching searches decoded literals.
#[test]
fn dotnet_string_refs_match_every_containing_heap_entry_and_original_load_site() {
    let path = string_input();
    let oracle = string_expected();
    for value in ["shared 世界\nλ", "", "🙂世界", "SHARED 世界\nλ", "absent"] {
        let rows = jsonl(&run("xref", &path, &["-m", value, "--kind", "string"]));
        assert_eq!(
            rows,
            jsonl(&run("xref", &path, &["-m", value, "--kind", "string"]))
        );
        let mut expected = Vec::new();
        for entry in oracle["entries"]
            .as_array()
            .unwrap()
            .iter()
            // `-f` folds case by default, so the oracle folds with it.
            .filter(|entry| {
                entry["value"]
                    .as_str()
                    .is_some_and(|literal| literal.to_lowercase().contains(&value.to_lowercase()))
            })
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
        assert_eq!(rows.len(), expected.len());
        for (row, (method, offset, token)) in rows[..].iter().zip(expected) {
            assert_eq!(row["record"], "reference");
            assert_eq!(row["context_at"], format!("0x{method:08x}"));
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
                &["--at", row["context_at"].as_str().unwrap(), "--raw"],
            );
            success(&fetched);
            let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
            assert_eq!(fetched["detail"]["method"]["at"], format!("0x{method:08x}"));
            assert_eq!(fetched["parent_at"], row["parent_at"]);
        }
    }
    // Values not expressible as argv UTF-8 remain reachable by raw user-string token.
    for entry in oracle["entries"].as_array().unwrap() {
        let token = format!("0x{:08x}", entry["token"].as_u64().unwrap());
        let rows = jsonl(&run("xref", &path, &["--at", &token]));
        assert_eq!(rows.len(), entry["uses"].as_array().unwrap().len());
        for row in &rows[..] {
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
        ("strings", vec!["--filter", "shared"]),
        ("xref", vec!["-m", "shared 世界\nλ", "--kind", "string"]),
        ("xref", vec!["--at", "0x70000001"]),
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
            ("strings", vec!["--filter", "absent"]),
            ("xref", vec!["-m", "absent", "--kind", "string"]),
            ("xref", vec!["--at", "0x70000001"]),
        ] {
            let output = run(command, &path, &args);
            assert_eq!(output.status.code(), Some(1), "{name}: {command} {args:?}");
            assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_INPUT"));
            rejected(output);
        }
    }
    for args in [
        vec![],
        vec!["--at", "0x70000001", "-m", "shared", "--kind", "string"],
        vec!["--at", "0x70000002"],
        vec!["--at", "0x70000000"],
        vec!["--at", "0x70ffffff"],
        vec!["--at", "token:not-a-token"],
        vec!["--at", "bad-token"],
    ] {
        let output = run("xref", &string_input(), &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        rejected(output);
    }
}

#[test]
fn dotnet_string_scans_account_cumulative_work_and_fail_atomically_at_cli_limits() {
    let path = string_input();
    let oracle = string_expected();
    success(&run("methods", &path, &[]));
    for method in oracle["methods"].as_array().unwrap() {
        let id = format!("0x{:08x}", method["token"].as_u64().unwrap());
        success(&run("get-method", &path, &["--at", &id, "--raw"]));
    }
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("strings.jsonl");
    fs::write(&destination, b"previous result").unwrap();
    for (command, query) in [
        ("strings", vec!["--filter", "shared"]),
        ("xref", vec!["-m", "shared 世界\nλ", "--kind", "string"]),
        ("xref", vec!["--at", "0x70000001"]),
    ] {
        fs::write(&destination, b"previous result").unwrap();
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
fn dotnet_user_string_tokens_are_navigable_from_operands_and_xref_headers() {
    let path = string_input();
    let oracle = string_expected();
    let entries = oracle["entries"].as_array().unwrap();

    // ldstr operands expose both their #US coordinate and decoded literal.
    for entry in entries {
        for site in entry["uses"].as_array().unwrap() {
            let token = entry["token"].as_u64().unwrap();
            let at = format!("0x{:08x}", site["method_token"].as_u64().unwrap());
            let fetched: Value =
                serde_json::from_slice(&run("get-method", &path, &["--at", &at, "--raw"]).stdout)
                    .unwrap();
            let offset = site["il_offset"].as_u64().unwrap();
            let instruction = fetched["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|instruction| instruction["offset"] == offset)
                .unwrap_or_else(|| panic!("missing instruction at {offset}"));
            assert_eq!(instruction["mnemonic"], "ldstr");
            let operand = &instruction["operand"];
            assert_eq!(operand["value"], token);
            assert_eq!(operand["target_at"], format!("0x{token:08x}"));
            match &entry["value"] {
                Value::String(value) => assert_eq!(&operand["string"], value),
                _ => assert!(
                    operand["utf16_units"].is_array(),
                    "an undecodable heap entry keeps its raw UTF-16 units"
                ),
            }
        }
    }

    for entry in entries {
        if entry["uses"].as_array().unwrap().is_empty() {
            continue;
        }
        let token = entry["token"].as_u64().unwrap();
        let rows = jsonl(&run("xref", &path, &["--at", &format!("0x{token:08x}")]));
        let target = &rows[0]["detail"];
        assert_eq!(target["target_kind"], "string");
        assert_eq!(target["target_token"], token);
        if let Value::String(value) = &entry["value"] {
            assert_eq!(&target["target"], value);
            // argv cannot carry a NUL; those literals stay reachable by token only.
            if value.is_empty() || value.contains('\0') {
                continue;
            }
            let by_value = jsonl(&run("xref", &path, &["-m", value, "--kind", "string"]));
            assert_eq!(by_value[0]["detail"]["target_kind"], "string");
            assert_eq!(
                by_value[0]["detail"]["target"],
                serde_json::json!(value.as_str())
            );
        }
    }
}
