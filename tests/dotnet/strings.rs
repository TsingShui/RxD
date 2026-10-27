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
