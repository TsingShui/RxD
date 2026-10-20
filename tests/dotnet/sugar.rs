//! Compiler-generated sugar and state machines, checked against an independent PE reader.
use super::*;

fn input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/sugar/Sugar.dll")
}

fn expected() -> Value {
    serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/sugar/expected.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

#[cfg(feature = "csharp-decompiler")]
fn recovered(name: &str) -> String {
    let path = input();
    let id = method_id(&path, name);
    let output = run("get-method", &path, &["--at", &id]);
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_fetch_protocol(&row, "get-method");
    assert_eq!(row["detail"]["complete"], true);
    assert_eq!(row["detail"]["representation"], "csharp", "{name}: {row}");
    assert_eq!(row["detail"]["decompiled"], true);
    assert!(row["detail"]["reason"].is_null());
    let code = row["detail"]["code"].as_str().unwrap();
    assert!(!code.trim().is_empty());
    code.to_owned()
}

#[cfg(feature = "csharp-decompiler")]
#[test]
fn dotnet_sugar_async_await_is_recovered_as_csharp() {
    let code = recovered("AsyncAwait");
    assert!(code.contains("async ") && code.contains("await "), "{code}");
}

#[cfg(feature = "csharp-decompiler")]
#[test]
fn dotnet_sugar_iterator_is_recovered_as_csharp_yield() {
    let code = recovered("Iterator");
    assert!(code.contains("yield return "), "{code}");
}

#[cfg(feature = "csharp-decompiler")]
#[test]
fn dotnet_sugar_captured_lambda_and_local_function_are_recovered_as_csharp() {
    let code = recovered("ClosureAndLocalFunction");
    assert!(code.contains("=>") && code.contains("AddSeed("), "{code}");
}

#[cfg(feature = "csharp-decompiler")]
#[test]
fn dotnet_sugar_linq_is_recovered_as_csharp_query() {
    let code = recovered("Linq");
    for keyword in ["from ", "where ", "select "] {
        assert!(code.contains(keyword), "missing {keyword}: {code}");
    }
}

#[cfg(feature = "csharp-decompiler")]
#[test]
fn dotnet_sugar_common_idioms_are_recovered_as_csharp() {
    for (name, evidence) in [
        ("Initializers", "Value ="),
        ("Using", "using "),
        ("Lock", "lock ("),
        ("NullPropagation", "?."),
        ("Interpolation", "$\""),
    ] {
        let code = recovered(name);
        assert!(code.contains(evidence), "{name}: {code}");
    }
}

#[test]
fn dotnet_sugar_and_generated_methods_are_indexed_with_byte_exact_original_il_and_eh() {
    let path = input();
    let output = run("methods", &path, &[]);
    success(&output);
    let rows = jsonl(&output);
    // The joined view keeps the same records, in the same order.
    let joined: Vec<Value> = outline(&path, &[])
        .into_iter()
        .filter(|row| row["record"] == "method")
        .collect();
    let listed: Vec<Value> = rows
        .iter()
        .filter(|row| row["record"] == "method")
        .cloned()
        .collect();
    assert_eq!(listed, joined);
    let expected = expected();
    let methods = expected["methods"].as_object().unwrap();
    for name in [
        "AsyncAwait",
        "Iterator",
        "ClosureAndLocalFunction",
        "Linq",
        "Initializers",
        "Using",
        "Lock",
        "NullPropagation",
        "Interpolation",
    ] {
        assert!(methods.values().any(|method| method["name"] == name));
    }
    let indexed: Vec<_> = rows
        .iter()
        .filter(|row| row["record"] == "method")
        .collect();
    assert_eq!(indexed.len(), methods.len());
    for (token, method) in methods {
        let token_number = u32::from_str_radix(token.trim_start_matches("0x"), 16).unwrap();
        let id = format!("0x{token_number:08x}");
        let indexed = indexed
            .iter()
            .find(|row| row["at"] == id)
            .unwrap_or_else(|| panic!("missing method {token}: {method}"));
        assert_eq!(indexed["at"], id);
        assert_eq!(indexed["detail"]["name"], method["name"]);
        assert_eq!(
            indexed["detail"]["declaring_type"],
            method["declaring_type"]
        );
        assert_eq!(indexed["detail"]["has_body"], true);
        let output = run("get-method", &path, &["--at", &id, "--raw"]);
        success(&output);
        let row: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(row["detail"]["complete"], true);
        assert_eq!(row["at"], id);
        assert!(row["source_id"].is_null());
        assert_eq!(row["detail"]["method"]["at"], id);
        assert_eq!(row["detail"]["representation"], "il");
        assert_eq!(row["detail"]["decompiled"], false);
        assert!(row["detail"]["code"].is_null());
        for field in [
            "il_hex",
            "handlers",
            "code_size",
            "max_stack",
            "init_locals",
            "local_signature_token",
        ] {
            assert_eq!(row["detail"][field], method[field], "{token} {field}");
        }
        let mut end = 0;
        for instruction in row["detail"]["instructions"].as_array().unwrap() {
            assert_eq!(instruction["offset"], end, "{token}: instruction gap");
            let size = instruction["size"].as_u64().unwrap();
            assert!(size > 0);
            end += size;
        }
        assert_eq!(
            row["detail"]["code_size"], end,
            "{token}: incomplete instructions"
        );
    }
}

#[test]
fn dotnet_sugar_method_identity_is_path_independent() {
    let path = input();
    let directory = tempfile::tempdir().unwrap();
    let copy = directory.path().join("renamed.exe");
    fs::copy(&path, &copy).unwrap();
    for name in ["AsyncAwait", "Iterator", "ClosureAndLocalFunction", "Linq"] {
        let id = method_id(&path, name);
        assert_eq!(method_id(&copy, name), id);
        let original = run("get-method", &path, &["--at", &id, "--raw"]);
        let renamed = run("get-method", &copy, &["--at", &id, "--raw"]);
        success(&original);
        success(&renamed);
        assert_eq!(original.stdout, renamed.stdout);
    }
}
