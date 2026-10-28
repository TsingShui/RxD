#[cfg(feature = "backend-dotnet")]
#[test]
fn mcp_shared_inflight_storage_can_bound_an_input_scaled_query() {
    use super::support::Client;
    use serde_json::json;
    use std::process::Command;

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/corpus/dotnet/newtonsoft-json-13.0.3/Newtonsoft.Json.dll");
    // CLI and MCP share the input-scaled analysis policy. A deliberately small
    // service-wide storage envelope rejects this result without narrowing its rows.
    let mut client = Client::start(&[
        "--allow-path",
        root.to_str().unwrap(),
        "--max-inflight-bytes",
        "16384",
    ]);
    client.initialize();
    let rejected = client.tool(
        "ilspy_methods",
        json!({
            "input": {"path": input}, "class": "Newtonsoft.Json.JsonReader", "name": "ReadAsInt32"
        }),
    );
    assert_eq!(rejected["isError"], true, "{rejected}");
    assert_eq!(
        rejected["structuredContent"]["error"]["code"], "RESOURCE_LIMIT",
        "{rejected}"
    );

    let cli = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["ilspy", "methods"])
        .arg(&input)
        .args([
            "--class",
            "Newtonsoft.Json.JsonReader",
            "--name",
            "ReadAsInt32",
        ])
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
}

#[cfg(feature = "backend-evm")]
#[cfg(feature = "backend-dex")]
#[test]
fn mcp_typed_reference_selectors_cannot_become_native_output_flags() {
    use super::super::dex::support as dex_inputs;
    use super::support::Client;
    use serde_json::json;
    use std::process::Command;

    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.dex");
    let victim = directory.path().join("victim.txt");
    std::fs::write(&input, dex_inputs::const_string_dex(1)).unwrap();
    let mut client = Client::start(&["--allow-path", directory.path().to_str().unwrap()]);
    client.initialize();
    let result=client.tool("rasc_xref",json!({"input":{"path":input},"kind":"method","class":"Fixture0","match":format!("--output={}",victim.display())}));
    assert!(
        !victim.exists(),
        "a typed filter was incorrectly interpreted as a host output destination: {result}"
    );
    let result = client.tool(
        "rasc_xref",
        json!({"input":{"path":input},"kind":"string","match":"-needle"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let cli = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["rasc", "xref"])
        .arg(&input)
        .args(["--kind=string", "--match=-needle"])
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let expected = if cli.stdout.is_empty() {
        json!({"records": []})
    } else {
        json!({"text": std::str::from_utf8(&cli.stdout).unwrap()})
    };
    assert_eq!(result["structuredContent"], expected);
}
