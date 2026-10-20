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

#[cfg(feature = "backend-js")]
#[test]
fn mcp_catalog_filters_backends_and_publishes_input_output_schemas() {
    let mut client = Client::start(&["--backends", "js"]);
    client.initialize();
    let response = client.request("tools/list", json!({}));
    let tools = response["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|tool| tool["name"] == "js_slice"));
    assert!(tools.iter().any(|tool| tool["name"] == "rxd_open"));
    assert!(
        !tools
            .iter()
            .any(|tool| tool["name"].as_str().unwrap().starts_with("jar_"))
    );
    for tool in tools {
        assert!(tool["inputSchema"].is_object(), "{tool}");
        assert!(tool["outputSchema"].is_object(), "{tool}");
    }
    let disabled = client.request("tools/call", json!({"name":"jar_entries","arguments":{}}));
    assert!(disabled["error"].is_object(), "{disabled}");
    client.send_raw(b"not-json\n");
    assert_eq!(client.receive()["error"]["code"], -32700);
    assert!(client.request("tools/list", json!({}))["result"]["tools"].is_array());
    client.disconnect();
    assert!(client.wait_exit().success());
}

#[cfg(feature = "backend-asar")]
#[test]
fn mcp_artifact_tools_are_annotated_as_creating_a_target() {
    let mut asar = Client::start(&["--backends", "asar"]);
    asar.initialize();
    let response = asar.request("tools/list", json!({}));
    let extraction = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "asar_extract")
        .unwrap();
    assert_eq!(extraction["annotations"]["readOnlyHint"], false);
    assert_eq!(extraction["annotations"]["idempotentHint"], false);
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
    client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    assert!(client.request("tools/list", json!({}))["result"]["tools"].is_array());
}
#[cfg(feature = "backend-js")]
#[test]
fn mcp_protocol_request_admission_is_bounded_and_recovers_after_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("slow.js");
    std::fs::write(
        &path,
        format!("function slow(){{{}}}", "var item=1;".repeat(180_000)),
    )
    .unwrap();
    let mut client = Client::start(&[
        "--root",
        root.path().to_str().unwrap(),
        "--max-active-requests",
        "1",
    ]);
    client.initialize();
    client.send(json!({"jsonrpc":"2.0","id":100,"method":"tools/call","params":{"name":"js_entities","arguments":{"input":{"path":path}}}}));
    client.send(json!({"jsonrpc":"2.0","id":101,"method":"tools/list","params":{}}));
    let denied = client.receive();
    assert_eq!(denied["id"], 101, "{denied}");
    assert_eq!(denied["error"]["code"], -32000, "{denied}");
    client.send(
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":100}}),
    );
    assert!(client.request("tools/list", json!({}))["result"]["tools"].is_array());
}
