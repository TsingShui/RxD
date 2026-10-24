//! Type-selected C# FETCH retains the independently checked original class scope.
use super::*;

fn closure_input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/closure/Closure.dll")
}

fn type_id(path: &Path, name: &str) -> String {
    outline(path, &["--class", name])
        .into_iter()
        .find(|row| row["record"] == "class")
        .unwrap()["at"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn dotnet_class_csharp_recovers_generic_declarations_members_and_owned_nested_scope() {
    let path = closure_input();
    let id = type_id(&path, "Rxd.Closure.Root`1");
    let output = run("get-class", &path, &["--at", &id]);
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["detail"]["decompiled"], true, "{row}");
    assert_eq!(row["detail"]["representation"], "csharp");
    assert!(row["detail"]["reason"].is_null());
    assert_eq!(row["detail"]["complete"], true);
    let code = row["detail"]["code"].as_str().unwrap();
    for evidence in [
        "Root<T>",
        "Value",
        "event Action",
        "Changed",
        "NoBody",
        "Nested",
        "Deep",
        "deep λ",
    ] {
        assert!(code.contains(evidence), "missing {evidence}: {code}");
    }
    assert!(!code.contains("Separate(") && !code.contains("Inherited("));
    let raw: Value =
        serde_json::from_slice(&run("get-class", &path, &["--at", &id, "--raw"]).stdout).unwrap();
    for field in ["type", "types", "fields", "properties", "events", "methods"] {
        assert!(
            row["detail"].get(field).is_none(),
            "default class body repeats {field}"
        );
        assert!(
            !raw["detail"][field].is_null(),
            "raw class body lost {field}"
        );
    }
    assert!(row["detail"]["work_used"].as_u64().unwrap() > 0);
}

fn native_fixture(directory: &Path, legacy: bool) -> PathBuf {
    let source = directory.join("engine.c");
    let library = directory.join(if cfg!(target_os = "macos") {
        "engine.dylib"
    } else {
        "engine.so"
    });
    fs::write(&source, r#"
#include <stdlib.h>
#include <string.h>
static unsigned char *payload(const char *value, int *length) {
    *length = (int)strlen(value);
    unsigned char *result = malloc((size_t)*length);
    memcpy(result, value, (size_t)*length);
    return result;
}
unsigned char *rxd_decompile_method(const unsigned char *input, int length, int token, int *out_length) {
    return payload("{\"ok\":true,\"reason\":null,\"code\":\"int Add(int a, int b) { return a + b; }\",\"mapping\":[{\"il_offset\":0,\"line\":1}]}", out_length);
}
#ifndef LEGACY
unsigned char *rxd_decompile_class(const unsigned char *input, int length, int token, int *out_length) {
    return payload(getenv("RXD_TEST_NATIVE_PAYLOAD"), out_length);
}
#endif
void rxd_free(unsigned char *value) { free(value); }
"#).unwrap();
    let mut command = Command::new("cc");
    command
        .args(if cfg!(target_os = "macos") {
            vec!["-dynamiclib", "-O2"]
        } else {
            vec!["-shared", "-fPIC", "-O2"]
        })
        .arg(&source)
        .arg("-o")
        .arg(&library);
    if legacy {
        command.arg("-DLEGACY");
    }
    let output = command.output().unwrap();
    success(&output);
    library
}

#[test]
fn dotnet_native_class_resource_failures_keep_the_previous_output_and_emit_no_success() {
    let directory = tempfile::tempdir().unwrap();
    let library = native_fixture(directory.path(), false);
    let path = closure_input();
    let id = type_id(&path, "Rxd.Closure.Root`1");
    let destination = directory.path().join("class.json");
    for reason in ["input_limit", "output_limit"] {
        fs::write(&destination, b"keep me").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["dotnet", "get-class"])
            .arg(&path)
            .arg("--at")
            .arg(&id)
            .arg("-o")
            .arg(&destination)
            .env("RXD_DOTNET_NATIVE_LIBRARY", &library)
            .env(
                "RXD_TEST_NATIVE_PAYLOAD",
                serde_json::json!({"ok":false,"reason":reason,"code":null,"mapping":[]})
                    .to_string(),
            )
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{reason}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("RESOURCE_LIMIT"));
        rejected(output);
        assert_eq!(fs::read(&destination).unwrap(), b"keep me");
    }
}

fn fetched(path: &Path, id: &str, args: &[&str]) -> Value {
    let mut options = vec!["--at", id];
    options.extend_from_slice(args);
    let output = run("get-class", path, &options);
    success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}

fn valid_mapping(row: &Value, inventory: &Value) {
    let lines = row["detail"]["code"].as_str().unwrap().lines().count() as u64;
    let methods = inventory["detail"]["methods"].as_array().unwrap();
    let mapping = row["detail"]["mapping"].as_array().unwrap();
    for point in mapping {
        let coordinate = format!("0x{:08x}", point["method_token"].as_u64().unwrap());
        let method = methods
            .iter()
            .find(|method| method["at"] == coordinate.as_str())
            .unwrap();
        assert_eq!(
            format!("0x{:08x}", point["method_token"].as_u64().unwrap()),
            method["detail"]["method"]["at"]
        );
        assert!(point["line"].as_u64().unwrap() > 0 && point["line"].as_u64().unwrap() <= lines);
        assert!(
            method["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|instruction| instruction["offset"] == point["il_offset"])
        );
    }
}

#[test]
fn dotnet_class_csharp_mappings_keep_method_domains_and_generated_types_are_directly_fetchable() {
    let path = closure_input();
    let root = type_id(&path, "Rxd.Closure.Root`1");
    let row = fetched(&path, &root, &[]);
    let inventory = fetched(&path, &root, &["--raw"]);
    valid_mapping(&row, &inventory);
    let mapping = row["detail"]["mapping"].as_array().unwrap();
    let domains: std::collections::BTreeSet<_> = mapping
        .iter()
        .filter(|point| point["il_offset"] == 0)
        .map(|point| point["method_token"].as_u64().unwrap())
        .collect();
    assert!(
        domains.len() >= 2,
        "distinct methods' offset zero mappings were lost"
    );
    let sugar = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/sugar/Sugar.dll");
    let id = type_id(&sugar, "Rxd.Sugar.Probe");
    let row = fetched(&sugar, &id, &[]);
    let inventory = fetched(&sugar, &id, &["--raw"]);
    assert_eq!(
        row["detail"]["decompiled"], true,
        "{}",
        row["detail"]["reason"]
    );
    let code = row["detail"]["code"].as_str().unwrap();
    for evidence in ["async ", "await ", "yield return ", "=>"] {
        assert!(code.contains(evidence), "{code}");
    }
    valid_mapping(&row, &inventory);
    assert!(row["detail"].get("methods").is_none());
    assert!(
        !inventory["detail"]["methods"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let generated = outline(&sugar, &[])
        .into_iter()
        .find(|row| {
            row["record"] == "class"
                && row["detail"]["name"]
                    .as_str()
                    .unwrap()
                    .contains("DisplayClass")
        })
        .unwrap();
    let generated_at = generated["at"].as_str().unwrap();
    let row = fetched(&sugar, generated_at, &[]);
    assert_eq!(
        row["detail"]["decompiled"], true,
        "{}",
        row["detail"]["reason"]
    );
    let inventory = fetched(&sugar, generated_at, &["--raw"]);
    assert_eq!(inventory["detail"]["type"], generated);
    valid_mapping(&row, &inventory);
}

#[test]
fn dotnet_class_csharp_retains_no_body_declarations_and_complete_failed_reconstruction() {
    let path = closure_input();
    for (name, evidence) in [
        ("Rxd.Closure.IContract`1", "interface IContract<T>"),
        ("Rxd.Closure.Base", "extern int Native("),
        ("Rxd.Closure.Value", "struct Value"),
        ("Rxd.Closure.Choice", "enum Choice"),
    ] {
        let id = type_id(&path, name);
        let row = fetched(&path, &id, &[]);
        assert_eq!(
            row["detail"]["decompiled"], true,
            "{name}: {}",
            row["detail"]["reason"]
        );
        assert!(row["detail"]["code"].as_str().unwrap().contains(evidence));
        assert!(row["detail"].get("methods").is_none());
        assert!(
            fetched(&path, &id, &["--raw"])["detail"]
                .get("methods")
                .is_some()
        );
    }
    // A body the engine reconstructs with caveats is still returned, and every caveat
    // travels with it: the warning names the member and the IL offset it applies to.
    let path = input();
    let id = type_id(&path, "Rxd.测试.Probe");
    let row = fetched(&path, &id, &[]);
    assert_eq!(
        row["detail"]["decompiled"], true,
        "{}",
        row["detail"]["reason"]
    );
    assert_eq!(row["detail"]["representation"], "csharp");
    assert!(row["detail"]["reason"].is_null());
    assert!(!row["detail"]["code"].as_str().unwrap().is_empty());
    let warnings = row["detail"]["warnings"].as_array().unwrap();
    assert!(!warnings.is_empty(), "{row}");
    for warning in warnings {
        let warning = warning.as_str().unwrap();
        assert!(
            warning.contains("IL_") || warning.contains(':'),
            "{warning}"
        );
    }
    assert_eq!(row["detail"]["complete"], true);
    assert!(row["detail"].get("methods").is_none());
    assert!(
        fetched(&path, &id, &["--raw"])["detail"]
            .get("methods")
            .is_some()
    );
    // The same bodies requested explicitly still carry no reconstruction at all.
    let raw = fetched(&path, &id, &["--raw"]);
    assert_eq!(raw["detail"]["decompiled"], false);
    assert!(raw["detail"]["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn dotnet_legacy_method_engine_remains_usable_and_class_fetch_has_complete_abi_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let library = native_fixture(directory.path(), true);
    let path = input();
    let method = method_id(&path, "Add");
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dotnet", "get-method"])
        .arg(&path)
        .arg("--at")
        .arg(&method)
        .env("RXD_DOTNET_NATIVE_LIBRARY", &library)
        .output()
        .unwrap();
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["detail"]["decompiled"], true);
    let raw = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dotnet", "get-method"])
        .arg(&path)
        .arg("--at")
        .arg(&method)
        .arg("--raw")
        .env("RXD_DOTNET_NATIVE_LIBRARY", &library)
        .output()
        .unwrap();
    success(&raw);
    let raw: Value = serde_json::from_slice(&raw.stdout).unwrap();
    original_il(&raw, "Add");
    let path = closure_input();
    let id = type_id(&path, "Rxd.Closure.Root`1");
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dotnet", "get-class"])
        .arg(&path)
        .arg("--at")
        .arg(&id)
        .env("RXD_DOTNET_NATIVE_LIBRARY", &library)
        .output()
        .unwrap();
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["detail"]["reason"], "class_abi_unavailable");
    assert_eq!(row["detail"]["decompiled"], false);
    assert!(row["detail"]["code"].is_null());
    assert!(row["detail"].get("methods").is_none());
    assert!(
        !fetched(&path, &id, &["--raw"])["detail"]["methods"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn dotnet_unprovable_native_class_mappings_are_dropped_and_reported_not_fatal() {
    let directory = tempfile::tempdir().unwrap();
    let library = native_fixture(directory.path(), false);
    let path = closure_input();
    let id = type_id(&path, "Rxd.Closure.Root`1");
    let raw = fetched(&path, &id, &["--raw"]);
    let guard = raw["detail"]["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|method| method["detail"]["method"]["detail"]["name"] == "Guard")
        .unwrap()["detail"]["method"]["at"]
        .as_str()
        .unwrap();
    let guard = u32::from_str_radix(guard.strip_prefix("0x").unwrap(), 16).unwrap();
    for mapping in [
        serde_json::json!([{"method_token":0x0600ffff,"il_offset":0,"line":1}]),
        serde_json::json!([{"method_token":guard,"il_offset":1,"line":1}]),
        serde_json::json!([{"method_token":guard,"il_offset":0,"line":99}]),
        serde_json::json!([{"il_offset":0,"line":1}]),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["dotnet", "get-class"]).arg(&path).arg("--at").arg(&id)
            .env("RXD_DOTNET_NATIVE_LIBRARY", &library)
            .env("RXD_TEST_NATIVE_PAYLOAD", serde_json::json!({"ok":true,"reason":null,"code":"class Root<T> {}","mapping":mapping}).to_string())
            .output().unwrap();
        success(&output);
        let row: Value = serde_json::from_slice(&output.stdout).unwrap();
        // The engine's reconstruction is returned as produced; only the navigation
        // entry it cannot prove is dropped, and the drop is reported.
        assert_eq!(row["detail"]["decompiled"], true, "{row}");
        assert_eq!(row["detail"]["representation"], "csharp");
        assert_eq!(row["detail"]["reason"], serde_json::Value::Null);
        assert_eq!(row["detail"]["code"], "class Root<T> {}");
        assert_eq!(row["detail"]["mapping"], serde_json::json!([]));
        assert!(
            row["detail"]["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|warning| warning
                    .as_str()
                    .unwrap()
                    .contains("dropped 1 unprovable source entry")),
            "{row}"
        );
        assert!(row["detail"].get("methods").is_none());
        assert!(!raw["detail"]["methods"].as_array().unwrap().is_empty());
    }
}

#[test]
fn dotnet_class_reconstruction_charges_referenced_signatures_to_one_work_budget() {
    let path = closure_input();
    let id = type_id(&path, "Rxd.Closure.Root`1");
    let raw = fetched(&path, &id, &["--raw"]);
    let instructions: usize = raw["detail"]["methods"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|method| method["detail"]["instructions"].as_array())
        .map(Vec::len)
        .sum();
    // The work a class reconstruction spends is reported, so a caller can see the cost
    // without a budget knob to tune it.
    assert!(raw["detail"]["work_used"].as_u64().unwrap() as usize >= instructions);
}
