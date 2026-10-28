use super::support::Client;
use serde_json::{Value, json};
use std::{path::Path, process::Command};

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("third_party/kuna/decompiler/crates/kuna-analysis/tests/fixtures/fauxware")
}

#[test]
fn mcp_native_functions_enumerate_exact_pinned_cli_facts_with_declared_schema() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = fixture();
    let mut client = Client::start(&["--allow-path", root.to_str().unwrap()]);
    client.initialize();
    let catalog = client.request("tools/list", json!({}));
    let definition = catalog["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "kuna_functions")
        .expect("compiled native functions tool is discoverable");
    let result = client.tool("kuna_functions", json!({"input":{"path":path}}));
    assert_ne!(result["isError"], true, "{result}");
    assert!(definition["outputSchema"].is_object());
    let cli = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args([
            "kuna",
            "functions",
            path.to_str().unwrap(),
            "--json",
            "--jobs",
            "1",
            "--option",
            "maxinstruction",
            "100000",
        ])
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let expected: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(result["structuredContent"], expected);
    assert_eq!(expected["count"], 21);
    assert!(
        expected["functions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|function| function["name"] == "main")
    );
}

#[test]
fn mcp_native_decompile_uses_returned_address_and_preserves_native_quality_and_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = fixture();
    let mut client = Client::start(&["--allow-path", root.to_str().unwrap()]);
    client.initialize();
    let functions = client.tool("kuna_functions", json!({"input":{"path":path}}));
    let main = functions["structuredContent"]["functions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|function| function["name"] == "main")
        .unwrap();
    let address = main["address_hex"].as_str().unwrap();
    let result = client.tool(
        "kuna_decompile",
        json!({"input":{"path":path},"address":address}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let cli = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args([
            "kuna",
            "decompile",
            path.to_str().unwrap(),
            address,
            "--addr",
            "--json",
            "--option",
            "maxinstruction",
            "100000",
            "--option",
            "errortoomanyinstructions",
            "on",
        ])
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let expected: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(result["structuredContent"], expected);
    assert_eq!(expected["count"], 1);
    assert_eq!(expected["functions"][0]["name"], "main");
    assert_eq!(expected["functions"][0]["unstructured_gotos"], 0);
    assert!(
        !expected["functions"][0]["line_mappings"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let catalog = client.request("tools/list", json!({}));
    let schema = &catalog["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "kuna_decompile")
        .unwrap()["outputSchema"];
    assert!(schema.is_object());
}

#[test]
fn mcp_native_worker_rss_limit_reaps_native_request_and_preserves_other_queries() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut client = Client::start(&[
        "--allow-path",
        root.to_str().unwrap(),
        "--max-native-worker-bytes",
        "1",
    ]);
    client.initialize();
    let result = client.tool("kuna_functions", json!({"input":{"path":fixture()}}));
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["structuredContent"]["error"]["code"], "RESOURCE_LIMIT",
        "{result}"
    );
    assert!(
        result["structuredContent"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("RSS")
    );
    client.disconnect();
    assert!(client.wait_exit().success());
}

#[test]
fn mcp_native_ambiguous_name_preserves_complete_native_error_and_address_navigation() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("duplicate-name.elf");
    // Reproducible boundary artifact from pinned fauxware: rename its sole
    // authenticate string-table entry to main, retaining two real definitions.
    let mut bytes = std::fs::read(fixture()).unwrap();
    let marker = b"authenticate\0";
    let positions: Vec<_> = bytes
        .windows(marker.len())
        .enumerate()
        .filter_map(|(i, value)| (value == marker).then_some(i))
        .collect();
    assert_eq!(positions.len(), 1);
    bytes[positions[0]..positions[0] + marker.len()].fill(0);
    bytes[positions[0]..positions[0] + 4].copy_from_slice(b"main");
    std::fs::write(&path, bytes).unwrap();
    let mut client = Client::start(&["--allow-path", scratch.path().to_str().unwrap()]);
    client.initialize();
    let result = client.tool(
        "kuna_decompile",
        json!({"input":{"path":path},"name":"main"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["structuredContent"]["error"]["code"], "AMBIGUOUS",
        "{result}"
    );
    let cli = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args([
            "kuna",
            "decompile",
            path.to_str().unwrap(),
            "main",
            "--json",
            "--option",
            "maxinstruction",
            "100000",
            "--option",
            "errortoomanyinstructions",
            "on",
        ])
        .output()
        .unwrap();
    assert!(!cli.status.success());
    let expected: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(result["structuredContent"]["error"]["candidates"], expected);
    assert!(
        expected["error"]
            .as_str()
            .unwrap()
            .contains("main at 0x400664")
    );
    assert!(
        expected["error"]
            .as_str()
            .unwrap()
            .contains("main at 0x40071d")
    );
    let inventory = client.tool("kuna_functions", json!({"input":{"path":path}}));
    let names: Vec<_> = inventory["structuredContent"]["functions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["name"] == "main")
        .collect();
    assert_eq!(names.len(), 2);
    let selected = client.tool(
        "kuna_decompile",
        json!({"input":{"path":path},"address":names[0]["address_hex"]}),
    );
    assert_ne!(selected["isError"], true, "{selected}");
    assert_eq!(selected["structuredContent"]["count"], 1);
}

#[test]
fn mcp_native_rejects_removed_instruction_limits_and_decompiles_completely() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = fixture();
    let mut client = Client::start(&["--allow-path", root.to_str().unwrap()]);
    client.initialize();
    let result = client.tool(
        "kuna_decompile",
        json!({"input":{"path":path},"name":"main","limits":{"instructions":1}}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["structuredContent"]["error"]["code"],
        "INVALID_QUERY"
    );
    let recovery = client.tool(
        "kuna_decompile",
        json!({"input":{"path":path},"name":"main"}),
    );
    assert_ne!(recovery["isError"], true, "{recovery}");
    assert_eq!(recovery["structuredContent"]["count"], 1);
    let tools = client.request("tools/list", json!({}));
    let tool = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "kuna_decompile")
        .unwrap();
    assert!(!tool["inputSchema"].to_string().contains("DecompileLimits"));
    assert!(tool["outputSchema"].is_object());
}
#[test]
fn mcp_native_inventory_has_no_ignored_byte_or_record_limit_parameters() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut client = Client::start(&["--allow-path", root.to_str().unwrap()]);
    client.initialize();
    for limits in [
        json!({"input_bytes":1}),
        json!({"output_bytes":128}),
        json!({"records":1}),
        json!({"entities":1}),
    ] {
        let result = client.tool(
            "kuna_functions",
            json!({"input":{"path":fixture()},"limits":limits}),
        );
        assert_eq!(result["isError"], true, "{result}");
        assert_eq!(
            result["structuredContent"]["error"]["code"],
            "INVALID_QUERY"
        );
        assert!(result["structuredContent"].get("functions").is_none());
    }
    let complete = client.tool("kuna_functions", json!({"input":{"path":fixture()}}));
    assert_ne!(complete["isError"], true, "{complete}");
    assert_eq!(complete["structuredContent"]["count"], 21);
    assert_eq!(complete["structuredContent"]["total"], 21);
}
#[test]
fn mcp_native_selectors_cannot_inject_flags_multi_selection_or_external_io() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut client = Client::start(&["--allow-path", root.to_str().unwrap()]);
    client.initialize();
    let cases = [
        json!({"name":"--engine=foreign"}),
        json!({"name":"main,authenticate"}),
        json!({"name":"main\u{0}"}),
        json!({"name":" main"}),
        json!({"name":"0x40071d"}),
        json!({"address":"--output"}),
        json!({"address":4196125}),
        json!({"address":"0x40071d;path"}),
        json!({"name":"main","address":"0x40071d"}),
        json!({"name":"main","argv":["--output","escaped"]}),
        json!({"name":"main","output":"escaped"}),
        json!({"name":"main","engine":"foreign"}),
        json!({"name":"main","limits":{"instructions":0}}),
    ];
    for mut args in cases {
        args["input"] = json!({"path":fixture()});
        let result = client.tool("kuna_decompile", args);
        assert_eq!(result["isError"], true, "{result}");
        assert_eq!(
            result["structuredContent"]["error"]["code"], "INVALID_QUERY",
            "{result}"
        );
    }
    let valid = client.tool(
        "kuna_decompile",
        json!({"input":{"path":fixture()},"name":"main"}),
    );
    assert_ne!(valid["isError"], true, "{valid}");
}

#[test]
fn mcp_native_target_keeps_original_bytes_and_logical_path_after_caller_mutation() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("原始-native.elf");
    std::fs::copy(fixture(), &path).unwrap();
    let expected = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args([
            "kuna",
            "decompile",
            path.to_str().unwrap(),
            "main",
            "--json",
            "--option",
            "maxinstruction",
            "100000",
            "--option",
            "errortoomanyinstructions",
            "on",
        ])
        .output()
        .unwrap();
    assert!(expected.status.success());
    let expected: Value = serde_json::from_slice(&expected.stdout).unwrap();
    let mut client = Client::start(&["--allow-path", scratch.path().to_str().unwrap()]);
    client.initialize();
    let opened = client.tool("rxd_open", json!({"path":path,"backend":"kuna"}));
    assert_ne!(opened["isError"], true, "{opened}");
    let target = opened["structuredContent"]["target_id"].clone();
    std::fs::write(&path, b"caller replaced this input").unwrap();
    let result = client.tool(
        "kuna_decompile",
        json!({"input":{"target_id":target},"name":"main"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"], expected);
    let miss = client.tool(
        "kuna_decompile",
        json!({"input":{"target_id":target},"name":"definitely_absent"}),
    );
    assert_eq!(miss["isError"], true, "{miss}");
    assert_eq!(miss["structuredContent"]["error"]["code"], "ANALYSIS_ERROR");
    let facts = &miss["structuredContent"]["error"]["candidates"];
    assert_eq!(facts["binary"], path.to_str().unwrap());
    assert_eq!(facts["count"], 0);
    assert!(
        facts["error"]
            .as_str()
            .unwrap()
            .contains(path.to_str().unwrap())
    );
    let fresh = client.tool("kuna_functions", json!({"input":{"path":path}}));
    assert_eq!(fresh["isError"], true, "{fresh}");
}

#[cfg(unix)]
#[test]
fn mcp_native_serial_thread_budget_overrides_inherited_decode_lanes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut client = Client::start_with_env(
        &[
            "--allow-path",
            root.to_str().unwrap(),
            "--max-workers",
            "1",
            "--max-threads",
            "1",
        ],
        &[("KUNA_DECODE_JOBS", "4"), ("KUNA_DECODE_MIN_BYTES", "0")],
    );
    client.initialize();
    client.send(json!({"jsonrpc":"2.0","id":100,"method":"tools/call","params":{"name":"kuna_functions","arguments":{"input":{"path":fixture()}}}}));
    let pid = super::support::child_worker(client.pid());
    let mut observed = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "owned worker thread sampling completed"
        );
        #[cfg(target_os = "macos")]
        let threads = std::process::Command::new("ps")
            .args(["-M", "-p", &pid.to_string(), "-o", "pid="])
            .output()
            .unwrap()
            .stdout;
        #[cfg(target_os = "macos")]
        let threads = String::from_utf8_lossy(&threads)
            .lines()
            .filter(|line| {
                line.split_whitespace()
                    .take(2)
                    .any(|value| value.parse::<u32>().ok() == Some(pid))
            })
            .count();
        #[cfg(target_os = "linux")]
        let threads = std::fs::read_to_string(format!("/proc/{pid}/status"))
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|line| line.starts_with("Threads:"))
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|n| n.parse::<usize>().ok())
            })
            .unwrap_or(0);
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let threads = 0usize;
        if threads == 0 {
            break;
        }
        observed = observed.max(threads);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let response = client.receive();
    assert_ne!(response["result"]["isError"], true, "{response}");
    assert!(observed > 0, "actual worker was observed");
    assert!(
        observed <= 2,
        "one waiting main + one analysis thread allowed; observed{observed}"
    );
    // The ordinary CLI retains its inherited decode policy; its native trace
    // reports actual admitted lanes, rather than merely echoing an env value.
    let cli = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args([
            "kuna",
            "functions",
            fixture().to_str().unwrap(),
            "--json",
            "--jobs",
            "1",
            "--option",
            "maxinstruction",
            "100000",
        ])
        .env("KUNA_DECODE_JOBS", "4")
        .env("KUNA_DECODE_MIN_BYTES", "0")
        .env("KUNA_DECODE_STATS", "1")
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    assert!(String::from_utf8_lossy(&cli.stderr).contains("decode stats: lanes=4"));
    assert_eq!(
        response["result"]["structuredContent"],
        serde_json::from_slice::<Value>(&cli.stdout).unwrap()
    );
}
