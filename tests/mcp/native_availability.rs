use super::support::Client;
use serde_json::json;

#[test]
fn mcp_native_catalog_reports_only_compiled_coverage_with_object_root_schemas() {
    let mut client = Client::start(&[]);
    client.initialize();
    let response = client.request("tools/list", json!({}));
    let native: Vec<_> = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| tool["_meta"]["backend"] == "kuna")
        .collect();
    assert_eq!(
        native.len(),
        if cfg!(feature = "bundled-kuna") { 2 } else { 0 }
    );
    for tool in native {
        assert!(matches!(
            tool["name"].as_str(),
            Some("kuna_functions" | "kuna_decompile")
        ));
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        assert_eq!(tool["outputSchema"]["type"], "object", "{tool}");
        let body = if tool["name"] == "kuna_functions" {
            "FunctionInventory"
        } else {
            "Decompilation"
        };
        assert!(
            tool["outputSchema"]["$defs"][body]["required"]
                .as_array()
                .unwrap()
                .contains(&json!("error"))
        );
        let capabilities = tool["_meta"]["capabilities"].as_array().unwrap();
        assert_eq!(capabilities.len(), 1);
        assert!(matches!(
            capabilities[0].as_str(),
            Some("ENUMERATE" | "FETCH")
        ));
        assert!(
            tool["description"]
                .as_str()
                .unwrap()
                .contains("Linked Kuna")
        );
    }
}

#[test]
fn mcp_native_worker_memory_limit_is_adjustable_without_a_fixed_ceiling() {
    let mut client = Client::start(&["--max-native-worker-bytes", "2147483648"]);
    client.initialize();
    assert!(client.request("tools/list", json!({}))["result"]["tools"].is_array());
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["mcp", "serve", "--max-native-worker-bytes", "0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}
