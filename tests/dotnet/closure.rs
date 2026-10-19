//! Complete Agent navigation uses returned IDs across real CLI processes.
use super::*;

fn fixture(directory: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/dotnet")
        .join(directory)
        .join(if directory == "sugar" {
            "Sugar.dll"
        } else {
            "Closure.dll"
        })
}

fn oracle(directory: &str) -> Value {
    serde_json::from_slice(&fs::read(fixture(directory).with_file_name("expected.json")).unwrap())
        .unwrap()
}

fn fetch(path: &Path, command: &str, id: &str) -> Value {
    let output = run(command, path, &["--at", id]);
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["at"], id);
    assert_eq!(row["detail"]["complete"], true);
    if cfg!(feature = "csharp-decompiler") {
        assert_eq!(row["detail"]["decompiled"], true, "{row}");
        assert_eq!(row["detail"]["representation"], "csharp");
        assert!(row["detail"]["reason"].is_null());
        assert!(!row["detail"]["code"].as_str().unwrap().is_empty());
    } else {
        assert_eq!(row["detail"]["decompiled"], false);
        assert_eq!(row["detail"]["representation"], "il");
        assert!(row["detail"]["code"].is_null());
        assert_eq!(
            row["detail"]["reason"],
            if command == "get-class" {
                "class_engine_unavailable"
            } else {
                "engine_unavailable"
            }
        );
    }
    row
}

fn assert_original_method(row: &Value, expected: &Value) {
    let token = row["detail"]["method"]["detail"]["token"].as_u64().unwrap();
    let method = &expected["methods"][format!("0x{token:08x}")];
    assert!(row["source_id"].is_null());
    assert_eq!(row["detail"]["il_hex"], method["il_hex"]);
    assert_eq!(row["detail"]["handlers"], method["handlers"]);
}

fn navigate(path: &Path, hit: &Value, expected: &Value, class_name: &str) -> (Value, Value) {
    let method = fetch(path, "get-method", hit["context_at"].as_str().unwrap());
    assert_original_method(&method, expected);
    let class = fetch(
        path,
        "get-class",
        hit["detail"]["declaring_type_id"].as_str().unwrap(),
    );
    assert_eq!(class["detail"]["type"]["detail"]["full_name"], class_name);
    assert!(
        class["detail"]["methods"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["at"] == method["at"])
    );
    let selected: Vec<_> = expected["methods"]
        .as_object()
        .unwrap()
        .values()
        .filter(|method| {
            method["declaring_type"] == class_name
                || method["declaring_type"]
                    .as_str()
                    .unwrap()
                    .starts_with(&format!("{class_name}+"))
        })
        .collect();
    assert_eq!(
        class["detail"]["methods"].as_array().unwrap().len(),
        selected.len()
    );
    for original in class["detail"]["methods"].as_array().unwrap() {
        assert_original_method(original, expected);
    }
    (method, class)
}

#[test]
fn dotnet_complete_symbol_and_string_workflows_route_returned_ids_to_exact_original_csharp_scopes()
{
    let directory = tempfile::tempdir().unwrap();
    let path = fixture("sugar");
    let copied = directory.path().join("renamed-assembly.bin");
    fs::copy(&path, &copied).unwrap();
    let outline = outline(
        &path,
        &["--type", "Rxd.Sugar.Item", "--method", "set_Value"],
    );
    let target = outline
        .iter()
        .find(|row| row["record"] == "method")
        .unwrap();
    let refs = jsonl(&run(
        "xref",
        &copied,
        &["--at", target["at"].as_str().unwrap()],
    ));
    assert_eq!(refs[0]["schema_version"], 2);
    assert_eq!(refs.last().unwrap()["complete"], true);
    assert_eq!(refs.last().unwrap()["records"], 3);
    assert_eq!(
        refs,
        jsonl(&run(
            "xref",
            &copied,
            &["--at", target["at"].as_str().unwrap()]
        ))
    );
    for (hit, (name, offset)) in refs[1..refs.len() - 1].iter().zip([
        ("Initializers", 32),
        ("Initializers", 52),
        ("Lock", 23),
    ]) {
        assert_eq!(hit["detail"]["canonical_target_id"], target["at"]);
        assert_eq!(hit["detail"]["resolution_status"], "resolved");
        assert_eq!(hit["detail"]["name"], name);
        assert_eq!(hit["detail"]["il_offset"], offset);
        navigate(&copied, hit, &oracle("sugar"), "Rxd.Sugar.Probe");
    }

    let path = fixture("closure");
    let strings = jsonl(&run("strings", &path, &["--filter", "deep λ"]));
    assert_eq!(strings[0]["schema_version"], 2);
    assert_eq!(strings.last().unwrap()["complete"], true);
    assert_eq!(strings.last().unwrap()["records"], 1);
    let hit = &strings[1];
    assert_eq!(hit["detail"]["value"], "deep λ");
    assert_eq!(hit["detail"]["status"], "referenced");
    let (method, class) = navigate(
        &path,
        hit,
        &oracle("closure"),
        "Rxd.Closure.Root`1+Nested+Deep",
    );
    assert_eq!(method["detail"]["method"]["detail"]["name"], "Text");
    assert_eq!(class["detail"]["types"].as_array().unwrap().len(), 1);
    for offset in hit["detail"]["il_offsets"].as_array().unwrap() {
        assert!(
            method["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|instruction| {
                    instruction["offset"] == *offset
                        && instruction["mnemonic"] == "ldstr"
                        && instruction["operand"]["value"] == hit["detail"]["token"]
                })
        );
    }
}

#[test]
fn dotnet_navigation_rejects_late_fetch_failures_without_partial_output() {
    let directory = tempfile::tempdir().unwrap();
    let path = fixture("closure");
    let copied = directory.path().join("same-content.dll");
    fs::copy(&path, &copied).unwrap();
    let strings = jsonl(&run("strings", &path, &["--filter", "deep λ"]));
    let hit = &strings[1];
    let method_id = hit["context_at"].as_str().unwrap();
    let type_id = hit["detail"]["declaring_type_id"].as_str().unwrap();
    success(&run("get-method", &copied, &["--at", method_id, "--raw"]));
    let destination = directory.path().join("fetch.json");
    for (command, id) in [("get-method", method_id), ("get-class", type_id)] {
        fs::write(&destination, b"previous complete result").unwrap();
        // An unusable coordinate is refused, and a refused fetch leaves the previous
        // result in place; budgets are internal floors, not CLI knobs.
        let output = run(
            command,
            &copied,
            &["--at", "0x0ffffff0", "-o", destination.to_str().unwrap()],
        );
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(fs::read(&destination).unwrap(), b"previous complete result");
    }
}
