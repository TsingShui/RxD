//! Aggregate FETCH contracts through the real CLI and independent fixture IL.
use super::*;

fn sugar_input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/sugar/Sugar.dll")
}

fn type_id(path: &Path, name: &str) -> String {
    outline(path, &["--class", name])
        .into_iter()
        .find(|row| row["record"] == "class" && row["detail"]["full_name"] == name)
        .unwrap_or_else(|| panic!("missing type {name}"))["at"]
        .as_str()
        .expect("type must have a FETCH ID")
        .to_owned()
}

#[test]
fn dotnet_class_il_fetch_includes_all_owned_generated_methods_with_original_il_and_eh() {
    let path = sugar_input();
    let outline = outline(&path, &[]);
    let root = outline
        .iter()
        .find(|row| row["record"] == "class" && row["detail"]["full_name"] == "Rxd.Sugar.Probe")
        .unwrap();
    let id = root["at"].as_str().expect("type must have a FETCH ID");
    let output = run("get-class", &path, &["--at", id, "--raw"]);
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_fetch_protocol(&row, "get-class");
    assert_eq!(row["record"], "class_body");
    assert_eq!(row["at"], id);
    assert_eq!(row["detail"]["complete"], true);
    assert_eq!(row["detail"]["representation"], "il");
    assert_eq!(row["detail"]["decompiled"], false);
    assert!(row["detail"]["code"].is_null());
    let oracle: Value =
        serde_json::from_str(include_str!("../fixtures/dotnet/sugar/expected.json")).unwrap();
    let selected: Vec<_> = oracle["methods"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, method)| {
            method["declaring_type"]
                .as_str()
                .unwrap()
                .starts_with("Rxd.Sugar.Probe")
        })
        .collect();
    let methods = row["detail"]["methods"].as_array().unwrap();
    assert_eq!(methods.len(), selected.len());
    for (token, expected) in selected {
        let token = u32::from_str_radix(token.trim_start_matches("0x"), 16).unwrap();
        let method = methods
            .iter()
            .find(|method| method["detail"]["method"]["at"] == format!("0x{token:08x}"))
            .unwrap();
        assert_eq!(method["detail"]["il_hex"], expected["il_hex"]);
        assert_eq!(method["detail"]["handlers"], expected["handlers"]);
        assert_eq!(method["parent_at"], method["detail"]["method"]["parent_at"]);
        assert!(method["parent_at"].as_str().unwrap().starts_with("0x"));
    }
    assert_eq!(
        row,
        serde_json::from_slice::<Value>(&run("get-class", &path, &["--at", id, "--raw"]).stdout)
            .unwrap()
    );
}

fn closure_input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/closure/Closure.dll")
}

#[test]
fn dotnet_class_fetch_preserves_declarations_relationships_and_exact_nested_scope() {
    let path = closure_input();
    let oracle: Value =
        serde_json::from_str(include_str!("../fixtures/dotnet/closure/expected.json")).unwrap();
    for name in [
        "Rxd.Closure.Base",
        "Rxd.Closure.Root`1",
        "Rxd.Closure.IContract`1",
        "Rxd.Closure.Value",
        "Rxd.Closure.Choice",
        "Rxd.Closure.Root`1+Nested+Deep",
    ] {
        let id = type_id(&path, name);
        let output = run("get-class", &path, &["--at", &id, "--raw"]);
        success(&output);
        let row: Value = serde_json::from_slice(&output.stdout).unwrap();
        let types = row["detail"]["types"].as_array().unwrap();
        let selected: Vec<_> = oracle["types"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, ty)| {
                ty["name"] == name
                    || ty["name"]
                        .as_str()
                        .unwrap()
                        .starts_with(&format!("{name}+"))
            })
            .collect();
        assert_eq!(types.len(), selected.len());
        let kind = if name.contains("IContract") {
            "interface"
        } else if name == "Rxd.Closure.Value" {
            "struct"
        } else if name == "Rxd.Closure.Choice" {
            "enum"
        } else {
            "class"
        };
        assert_eq!(row["detail"]["type"]["detail"]["kind"], kind);
        for (token, expected) in selected {
            let ty = types.iter().find(|ty| ty["at"] == *token).unwrap();
            assert_eq!(ty["detail"]["full_name"], expected["name"]);
            assert_eq!(ty["detail"]["extends_token"], expected["extends_token"]);
            assert_eq!(ty["detail"]["interfaces"], expected["interfaces"]);
            assert_eq!(ty["detail"]["access_flags"], expected["flags"]);
            for parameter in expected["generic_parameters"].as_array().unwrap() {
                assert!(
                    ty["detail"]["generic_parameters"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|got| got["name"] == parameter["name"]
                            && got["number"] == parameter["number"])
                );
            }
        }
        let selected_methods: Vec<_> = oracle["methods"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, method)| {
                method["declaring_type"] == name
                    || method["declaring_type"]
                        .as_str()
                        .unwrap()
                        .starts_with(&format!("{name}+"))
            })
            .collect();
        let methods = row["detail"]["methods"].as_array().unwrap();
        assert_eq!(methods.len(), selected_methods.len());
        for (token, expected) in selected_methods {
            let got = methods
                .iter()
                .find(|got| got["detail"]["method"]["at"] == *token)
                .unwrap();
            assert_eq!(got["detail"]["body_present"], expected["rva"] != 0);
            assert_eq!(got["detail"]["il_hex"], expected["il_hex"]);
            if expected["rva"] != 0 {
                for field in [
                    "handlers",
                    "code_size",
                    "max_stack",
                    "init_locals",
                    "local_signature_token",
                ] {
                    assert_eq!(got["detail"][field], expected[field]);
                }
                let start = got["start"].as_u64().unwrap() as usize;
                let end = got["end"].as_u64().unwrap() as usize;
                let bytes = fs::read(&path).unwrap();
                assert_eq!(
                    bytes[start..end]
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>(),
                    expected["il_hex"].as_str().unwrap()
                );
            } else {
                assert!(got["start"].is_null() && got["end"].is_null());
            }
        }
        for category in ["properties", "events"] {
            let selected: Vec<_> = oracle[category]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(_, item)| types.iter().any(|ty| ty["at"] == item["owner_token"]))
                .collect();
            let got = row["detail"][category].as_array().unwrap();
            assert_eq!(got.len(), selected.len());
            for (token, expected) in selected {
                let got = got.iter().find(|got| got["at"] == *token).unwrap();
                assert_eq!(got["detail"]["name"], expected["name"]);
                assert_eq!(got["parent_at"], expected["owner_token"]);
                for accessor in expected["accessors"].as_array().unwrap() {
                    let found = got["detail"]["accessors"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|got| got["method_token"] == accessor["method_token"])
                        .unwrap();
                    assert_eq!(found["semantics"], accessor["semantics"]);
                    assert!(
                        methods
                            .iter()
                            .any(|method| method["at"] == found["method_id"])
                    );
                }
            }
        }
    }
}

#[test]
fn dotnet_class_fetch_validates_ids_and_fails_atomically_under_aggregate_budgets() {
    let path = closure_input();
    let id = type_id(&path, "Rxd.Closure.Root`1");
    let raw = run("get-class", &path, &["--at", &id, "--raw"]);
    success(&raw);
    let row: Value = serde_json::from_slice(&raw.stdout).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let copy = directory.path().join("copy.exe");
    fs::copy(&path, &copy).unwrap();
    assert_eq!(type_id(&copy, "Rxd.Closure.Root`1"), id);
    let destination = directory.path().join("class.json");
    let saved = destination.to_str().unwrap();
    let output = run(
        "get-class",
        &path,
        &["--at", &id, "--raw", "-o", saved, "--perf"],
    );
    success(&output);
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(&destination).unwrap(), raw.stdout);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .all(|line| serde_json::from_str::<Value>(line).unwrap()["record"] == "perf")
    );
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dotnet", "get-class"])
        .arg(&path)
        .args(["--at", &id, "--raw"])
        .env("RXD_DOTNET_NATIVE_LIBRARY", "/never/load/this")
        .output()
        .unwrap();
    success(&output);
    assert_eq!(output.stdout, raw.stdout);
    let fallback = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dotnet", "get-class"])
        .arg(&path)
        .arg("--at")
        .arg(&id)
        .env(
            "RXD_DOTNET_NATIVE_LIBRARY",
            directory.path().join("missing.dylib"),
        )
        .output()
        .unwrap();
    success(&fallback);
    let fallback: Value = serde_json::from_slice(&fallback.stdout).unwrap();
    assert_eq!(fallback["detail"]["decompiled"], false);
    assert_eq!(fallback["detail"]["reason"], "class_engine_unavailable");
    assert_eq!(fallback["detail"]["methods"], row["detail"]["methods"]);
    let module = type_id(&path, "<Module>");
    let invalid = [
        module,
        // a method token is not a type: the table byte decides
        "0x06000001".to_owned(),
        "0x02000000".to_owned(),
        "0x020000040".to_owned(),
        "02000002".to_owned() + "0",
        "func:02000002".to_owned(),
    ];
    for invalid in invalid {
        let output = run("get-class", &path, &["--at", &invalid, "--raw"]);
        assert_eq!(output.status.code(), Some(2));
        rejected(output);
    }
    // Budgets are internal floors now, so atomicity is proven with a failing fetch: a
    // copy of the previous result must survive it.
    fs::write(&destination, b"keep me").unwrap();
    let output = run(
        "get-class",
        &path,
        &["--at", "0x0ffffff0", "--raw", "-o", saved],
    );
    assert_eq!(output.status.code(), Some(2));
    rejected(output);
    assert_eq!(fs::read(&destination).unwrap(), b"keep me");
}

#[test]
fn dotnet_class_fetch_rejects_corrupt_ownership_and_original_method_bodies_without_partial_output()
{
    let path = closure_input();
    let oracle: Value =
        serde_json::from_str(include_str!("../fixtures/dotnet/closure/expected.json")).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let malformed = directory.path().join("malformed.dll");
    let destination = directory.path().join("class.json");
    let id = type_id(&path, "Rxd.Closure.Root`1");
    let offsets = &oracle["table_file_offsets"];
    let original = fs::read(&path).unwrap();
    let nested = offsets["nested_class"].as_u64().unwrap() as usize;
    let semantics = offsets["method_semantics"].as_u64().unwrap() as usize;
    let method_def = offsets["method_def"].as_u64().unwrap() as usize;
    let (guard_token, guard) = oracle["methods"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, method)| method["name"] == "Guard")
        .unwrap();
    let guard_rid =
        u32::from_str_radix(guard_token.trim_start_matches("0x"), 16).unwrap() & 0xffffff;
    let first_rva = guard["rva"].as_u64().unwrap() as usize;
    // Fixture PE .text has VA 0x2000 and raw offset 0x200; PEReader provides the RVA.
    let code_header = 0x200 + first_rva - 0x2000;
    let changes = [
        (nested + 2, original[nested..nested + 2].to_vec()), // nested type owns itself
        (semantics + 2, 1u16.to_le_bytes().to_vec()), // Root property accessor moved to interface method
        (
            method_def + (guard_rid as usize - 1) * 14,
            0xffff_ffffu32.to_le_bytes().to_vec(),
        ), // readable method with impossible RVA
        (code_header, vec![0]),                       // invalid original method header
    ];
    for (offset, replacement) in changes {
        let mut bytes = original.clone();
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        fs::write(&malformed, &bytes).unwrap();
        fs::write(&destination, b"keep me").unwrap();
        let modified_id = id.replace(
            oracle["sha256"].as_str().unwrap(),
            &format!("{:x}", Sha256::digest(&bytes)),
        );
        let output = run(
            "get-class",
            &malformed,
            &[
                "--at",
                &modified_id,
                "--raw",
                "-o",
                destination.to_str().unwrap(),
            ],
        );
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        rejected(output);
        assert_eq!(fs::read(&destination).unwrap(), b"keep me");
    }
}
