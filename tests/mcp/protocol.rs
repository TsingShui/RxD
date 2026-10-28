use super::support::Client;
use serde_json::json;
#[test]
fn mcp_legacy_and_modern_clients_discover_the_server() {
    let mut legacy = Client::start(&[]);
    legacy.initialize();
    let response = legacy.request("tools/list", json!({}));
    assert!(response["result"]["tools"].is_array(), "{response}");
    let mut modern = Client::start(&[]);
    let response=modern.request("server/discover",json!({"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"rxd-e2e","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}}));
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(
        response["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "rxd"
    );
    modern.disconnect();
    assert!(modern.wait_exit().success());
}

#[test]
fn mcp_modern_requests_validate_metadata_and_protocol_version() {
    let mut client = Client::start(&[]);
    let bad = client.request("server/discover", json!({}));
    assert_eq!(bad["error"]["code"], -32602, "{bad}");
    let meta = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"rxd-e2e","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
    let valid = client.request("server/discover", json!({"_meta":meta}));
    assert!(valid.get("error").is_none(), "{valid}");
    let missing = client.request("tools/list", json!({}));
    assert_eq!(missing["error"]["code"], -32602, "{missing}");
    let mut unsupported = meta.clone();
    unsupported["io.modelcontextprotocol/protocolVersion"] = json!("2099-01-01");
    let bad_version = client.request("tools/list", json!({"_meta":unsupported}));
    assert_eq!(bad_version["error"]["code"], -32022, "{bad_version}");
    assert!(client.request("tools/list", json!({"_meta":meta}))["result"]["tools"].is_array());
}
#[test]
fn mcp_oversized_unterminated_frame_is_rejected_before_allocation() {
    let mut client = Client::start(&["--max-frame-bytes", "512"]);
    client.initialize();
    client.send_raw(&vec![b'x'; 513]);
    client.disconnect();
    assert!(client.wait_exit().success());
}

#[test]
fn mcp_legacy_initialize_negotiates_a_supported_version() {
    let mut client = Client::start(&[]);
    let response=client.request("initialize",json!({"protocolVersion":"2099-01-01","capabilities":{},"clientInfo":{"name":"rxd-e2e","version":"1"}}));
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"]["protocolVersion"], "2099-01-01");
    assert!(
        !response["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("without executing")
    );
    client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    assert!(client.request("tools/list", json!({}))["result"]["tools"].is_array());
}
