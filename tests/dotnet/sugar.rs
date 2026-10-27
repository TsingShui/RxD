//! Compiler-generated sugar and state machines, checked against an independent PE reader.
#[cfg(feature = "csharp-decompiler")]
use super::*;

#[cfg(feature = "csharp-decompiler")]
fn recovered(name: &str) -> String {
    let path = input();
    let id = method_id(&path, name);
    let output = run("get-method", &path, &["--at", &id]);
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_fetch_protocol(&row, "get-method");
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
