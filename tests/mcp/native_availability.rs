use super::support::Client;
use serde_json::json;

#[test]
fn mcp_native_worker_memory_limit_is_adjustable_without_a_fixed_ceiling() {
    let mut client = Client::start(&["--max-native-worker-bytes", "2147483648"]);
    client.initialize();
    assert!(client.request("tools/list", json!({}))["result"]["tools"].is_array());
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["mcp", "--max-native-worker-bytes", "0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}
