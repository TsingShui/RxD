//! Type-selected C# FETCH retains the independently checked original class scope.
use super::*;

fn closure_input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/closure/Closure.dll")
}

fn type_id(path: &Path, name: &str) -> String {
    jsonl(&run("outline", path, &["--type", name]))
        .into_iter()
        .find(|row| row["record"] == "type")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn dotnet_class_csharp_recovers_generic_declarations_members_and_owned_nested_scope() {
    let path = closure_input();
    let id = type_id(&path, "Rxd.Closure.Root`1");
    let output = run("get-class", &path, &[&id]);
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
        serde_json::from_slice(&run("get-class", &path, &[&id, "--il"]).stdout).unwrap();
    for field in [
        "type",
        "types",
        "fields",
        "properties",
        "events",
        "methods",
        "work_used",
    ] {
        assert_eq!(row["detail"][field], raw["detail"][field], "{field}");
    }
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
    let mut options = vec![id];
    options.extend_from_slice(args);
    let output = run("get-class", path, &options);
    success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}

fn valid_mapping(row: &Value) {
    let lines = row["detail"]["code"].as_str().unwrap().lines().count() as u64;
    let methods = row["detail"]["methods"].as_array().unwrap();
    let mapping = row["detail"]["mapping"].as_array().unwrap();
    for point in mapping {
        let method = methods
            .iter()
            .find(|method| method["id"] == point["method_id"])
            .unwrap();
        assert_eq!(
            point["method_token"],
            method["detail"]["method"]["detail"]["token"]
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
    let row = fetched(&path, &type_id(&path, "Rxd.Closure.Root`1"), &[]);
    valid_mapping(&row);
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
    assert_eq!(
        row["detail"]["decompiled"], true,
        "{}",
        row["detail"]["reason"]
    );
    let code = row["detail"]["code"].as_str().unwrap();
    for evidence in ["async ", "await ", "yield return ", "=>"] {
        assert!(code.contains(evidence), "{code}");
    }
    valid_mapping(&row);
    let raw = fetched(&sugar, &id, &["--il"]);
    assert_eq!(row["detail"]["methods"], raw["detail"]["methods"]);
    assert_eq!(row["detail"]["types"], raw["detail"]["types"]);
    let generated = jsonl(&run("outline", &sugar, &[]))
        .into_iter()
        .find(|row| {
            row["record"] == "type"
                && row["detail"]["name"]
                    .as_str()
                    .unwrap()
                    .contains("DisplayClass")
        })
        .unwrap();
    let row = fetched(&sugar, generated["id"].as_str().unwrap(), &[]);
    assert_eq!(
        row["detail"]["decompiled"], true,
        "{}",
        row["detail"]["reason"]
    );
    assert_eq!(row["detail"]["type"], generated);
    valid_mapping(&row);
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
        assert_eq!(
            row["detail"]["methods"],
            fetched(&path, &id, &["--il"])["detail"]["methods"]
        );
    }
    let path = input();
    let id = type_id(&path, "Rxd.测试.Probe");
    let row = fetched(&path, &id, &[]);
    assert_eq!(row["detail"]["decompiled"], false);
    assert_eq!(row["detail"]["representation"], "il");
    assert!(!row["detail"]["reason"].as_str().unwrap().is_empty());
    assert!(row["detail"]["code"].is_null());
    assert_eq!(row["detail"]["mapping"], serde_json::json!([]));
    assert_eq!(row["detail"]["complete"], true);
    assert_eq!(
        row["detail"]["methods"],
        fetched(&path, &id, &["--il"])["detail"]["methods"]
    );
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
        .arg(&method)
        .env("RXD_DOTNET_NATIVE_LIBRARY", &library)
        .output()
        .unwrap();
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["detail"]["decompiled"], true);
    original_il(&row, "Add");
    let path = closure_input();
    let id = type_id(&path, "Rxd.Closure.Root`1");
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dotnet", "get-class"])
        .arg(&path)
        .arg(&id)
        .env("RXD_DOTNET_NATIVE_LIBRARY", &library)
        .output()
        .unwrap();
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["detail"]["reason"], "class_abi_unavailable");
    assert_eq!(row["detail"]["decompiled"], false);
    assert!(row["detail"]["code"].is_null());
    assert_eq!(
        row["detail"]["methods"],
        fetched(&path, &id, &["--il"])["detail"]["methods"]
    );
}

#[test]
fn dotnet_invalid_native_class_mappings_preserve_complete_original_scope() {
    let directory = tempfile::tempdir().unwrap();
    let library = native_fixture(directory.path(), false);
    let path = closure_input();
    let id = type_id(&path, "Rxd.Closure.Root`1");
    let raw = fetched(&path, &id, &["--il"]);
    let guard = raw["detail"]["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|method| method["detail"]["method"]["detail"]["name"] == "Guard")
        .unwrap()["detail"]["method"]["detail"]["token"]
        .as_u64()
        .unwrap();
    for mapping in [
        serde_json::json!([{"method_token":0x0600ffff,"il_offset":0,"line":1}]),
        serde_json::json!([{"method_token":guard,"il_offset":1,"line":1}]),
        serde_json::json!([{"method_token":guard,"il_offset":0,"line":99}]),
        serde_json::json!([{"il_offset":0,"line":1}]),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["dotnet", "get-class"]).arg(&path).arg(&id)
            .env("RXD_DOTNET_NATIVE_LIBRARY", &library)
            .env("RXD_TEST_NATIVE_PAYLOAD", serde_json::json!({"ok":true,"reason":null,"code":"class Root<T> {}","mapping":mapping}).to_string())
            .output().unwrap();
        success(&output);
        let row: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(row["detail"]["reason"], "invalid_engine_output");
        assert_eq!(row["detail"]["decompiled"], false);
        assert!(row["detail"]["code"].is_null());
        assert_eq!(row["detail"]["methods"], raw["detail"]["methods"]);
    }
}

#[test]
fn dotnet_class_reconstruction_charges_referenced_signatures_to_one_work_budget() {
    let path = closure_input();
    let id = type_id(&path, "Rxd.Closure.Root`1");
    let raw = fetched(&path, &id, &["--il"]);
    let instructions: usize = raw["detail"]["methods"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|method| method["detail"]["instructions"].as_array())
        .map(Vec::len)
        .sum();
    let limit = (raw["detail"]["work_used"].as_u64().unwrap() as usize + instructions).to_string();
    let output = run("get-class", &path, &[&id, "--max-work", &limit]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("RESOURCE_LIMIT"));
    rejected(output);
}
