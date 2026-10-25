use super::support::{Client, assert_schema};
use serde_json::{Value, json};
use std::{
    fs,
    time::{Duration, Instant},
};

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let large = root.path().join("slow.js");
    fs::write(
        &large,
        format!("function slow(){{\n{}}}", "var item = 1;\n".repeat(180_000)),
    )
    .unwrap();
    let small = root.path().join("small.js");
    fs::write(&small, "function ready(){return 7;}\n").unwrap();
    (root, large, small)
}
fn tool_call(id: u64, path: &std::path::Path) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"js_entities","arguments":{"input":{"path":path}}}})
}
fn error(response: &Value, code: &str) {
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert_eq!(
        response["result"]["structuredContent"]["error"]["code"], code,
        "{response}"
    );
}

#[test]
fn mcp_shared_inflight_budget_rejects_large_output_and_recovers() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.js");
    fs::write(
        &path,
        format!(r#"function large(){{return "{}";}}"#, "x".repeat(10_000)),
    )
    .unwrap();
    let mut client = Client::start(&[
        "--allow-path",
        root.path().to_str().unwrap(),
        "--max-inflight-bytes",
        "131072",
    ]);
    client.initialize();
    let entities = client.tool("js_entities", json!({"input":{"path":path}}));
    assert_ne!(entities["isError"], true, "{entities}");
    let at = entities["structuredContent"]["records"][0]["at"].clone();
    let fetched = client.tool("js_slice", json!({"input":{"path":path},"at":at}));
    assert_eq!(fetched["isError"], true, "{fetched}");
    assert_eq!(
        fetched["structuredContent"]["error"]["code"],
        "RESOURCE_LIMIT"
    );
    assert!(fetched["structuredContent"].get("detail").is_none());
    assert_ne!(
        client.tool("js_entities", json!({"input":{"path":path}}))["isError"],
        true
    );
}
#[test]
fn mcp_deadline_failure_leaves_the_protocol_process_usable() {
    let (root, large, _small) = fixture();
    let mut client = Client::start(&[
        "--allow-path",
        root.path().to_str().unwrap(),
        "--deadline-ms",
        "1",
    ]);
    client.initialize();
    let result = client.tool("js_entities", json!({"input":{"path":large}}));
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["structuredContent"]["error"]["code"], "TIMEOUT",
        "{result}"
    );
    assert!(client.request("tools/list", json!({}))["result"]["tools"].is_array());
}

#[test]
fn mcp_worker_admission_is_bounded_and_cancellation_releases_the_slot() {
    let (root, large, small) = fixture();
    let mut client = Client::start(&[
        "--allow-path",
        root.path().to_str().unwrap(),
        "--max-workers",
        "1",
        "--max-threads",
        "1",
    ]);
    client.initialize();
    client.send(tool_call(100, &large));
    client.send(tool_call(101, &large));
    let rejected = client.receive();
    error(&rejected, "RESOURCE_LIMIT");
    let accepted = if rejected["id"] == 100 { 101 } else { 100 };
    client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":accepted,"reason":"E2E cancellation"}}));
    // A cancelled response is suppressed. Poll through the public tool boundary until its worker allocation is released.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = client.tool("js_entities", json!({"input":{"path":small}}));
        if result["isError"] != true {
            assert_eq!(
                result["structuredContent"]["records"][1]["detail"]["label"],
                "ready"
            );
            break;
        }
        assert_eq!(
            result["structuredContent"]["error"]["code"], "RESOURCE_LIMIT",
            "{result}"
        );
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
pub(super) fn child_worker(parent: u32) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = std::process::Command::new("ps")
            .args(["-axo", "pid=,ppid="])
            .output()
            .unwrap();
        let child = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .find_map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields.next()?.parse::<u32>().ok()?;
                let ppid = fields.next()?.parse::<u32>().ok()?;
                (ppid == parent).then_some(pid)
            });
        if let Some(pid) = child {
            return pid;
        }
        assert!(Instant::now() < deadline, "analysis worker did not appear");
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
#[cfg(unix)]
fn mcp_killed_worker_reports_failure_then_subsequent_queries_succeed() {
    let (root, large, small) = fixture();
    let mut client = Client::start(&["--allow-path", root.path().to_str().unwrap()]);
    client.initialize();
    client.send(tool_call(100, &large));
    let worker = child_worker(client.pid());
    assert!(
        std::process::Command::new("kill")
            .args(["-KILL", &worker.to_string()])
            .status()
            .unwrap()
            .success()
    );
    let response = client.receive();
    assert_eq!(response["id"], 100);
    error(&response, "WORKER_ERROR");
    let recovered = client.tool("js_entities", json!({"input":{"path":small}}));
    assert_ne!(recovered["isError"], true, "{recovered}");
    let listing = client.request("tools/list", json!({}));
    let schema = &listing["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "js_entities")
        .unwrap()["outputSchema"];
    assert_schema(&response["result"]["structuredContent"], schema);
}
#[test]
#[cfg(unix)]
fn mcp_client_eof_reaps_active_workers_before_server_exit() {
    let (root, large, _small) = fixture();
    let mut client = Client::start(&["--allow-path", root.path().to_str().unwrap()]);
    client.initialize();
    client.send(tool_call(100, &large));
    let worker = child_worker(client.pid());
    client.disconnect();
    assert!(client.wait_exit().success());
    let status = std::process::Command::new("ps")
        .args(["-p", &worker.to_string(), "-o", "pid="])
        .output()
        .unwrap();
    assert!(
        status.stdout.is_empty(),
        "worker remained after EOF shutdown"
    );
}

#[test]
#[cfg(unix)]
fn mcp_client_eof_terminates_even_when_a_response_pipe_is_backpressured() {
    use std::{
        io::Write,
        os::fd::AsRawFd,
        process::{Command, Stdio},
    };
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("large.js");
    let source = format!(
        "function large(){{return \"{}\";}}",
        "x".repeat(2 * 1024 * 1024)
    );
    fs::write(&path, &source).unwrap();
    // The whole-file program coordinate is what a large `js_slice` fetch needs.
    let at = format!("program:0:{}", source.len());
    let mut child = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["mcp", "--allow-path", root.path().to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for request in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"backpressure-e2e","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"js_slice","arguments":{"input":{"path":path},"at":at}}}),
    ] {
        serde_json::to_writer(&mut input, &request).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let mut bytes = 0i32;
        // Observe public stdout delivery without draining it, exactly as a stalled MCP client behaves.
        assert_eq!(
            unsafe {
                libc::ioctl(
                    child.stdout.as_ref().unwrap().as_raw_fd(),
                    libc::FIONREAD,
                    &mut bytes,
                )
            },
            0
        );
        if bytes > 4096 {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("large MCP response never reached stdout pipe");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("MCP EOF shutdown waited indefinitely for an unread stdout pipe");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn mcp_inflight_budget_counts_rpc_identity_and_rolls_back_unreachable_targets() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.js");
    fs::write(
        &path,
        "function original(){}
",
    )
    .unwrap();
    let mut client = Client::start(&[
        "--allow-path",
        root.path().to_str().unwrap(),
        "--max-inflight-bytes",
        "65536",
        "--max-snapshot-bytes",
        "10000",
    ]);
    client.initialize();
    let long_id = "x".repeat(4500);
    client.send(json!({"jsonrpc":"2.0","id":long_id,"method":"tools/call","params":{"name":"rxd_open","arguments":{"path":path}}}));
    let response = client.receive();
    assert_eq!(response["id"], long_id);
    error(&response, "RESOURCE_LIMIT");
    // String IDs must release the same reservation/key as transport admission.
    for index in 0..20 {
        let id = format!("open-{index}");
        client.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"rxd_open","arguments":{"path":path}}}));
        let opened = client.receive();
        assert_eq!(opened["id"], id);
        assert_ne!(opened["result"]["isError"], true, "{opened}");
        assert_ne!(
            client.tool(
                "rxd_close",
                json!({"target_id":opened["result"]["structuredContent"]["target_id"]})
            )["isError"],
            true
        );
    }
    // Both ID types release admission correctly when reused after delivery.
    for id in [json!(400), json!("400")] {
        client.send(json!({"jsonrpc":"2.0","id":id,"method":"ping","params":{}}));
        let response = client.receive();
        assert_eq!(response["id"], id);
        assert!(response["result"].is_object(), "{response}");
    }
}
#[test]
#[cfg(target_os = "linux")]
fn mcp_unrepresentable_snapshot_metadata_does_not_leak_a_target_slot() {
    use std::os::unix::ffi::OsStringExt;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("directory");
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join(std::ffi::OsString::from_vec(vec![0xff, b'.', b'j', b's'])),
        "function original(){}\n",
    )
    .unwrap();
    let path = root.path().join("valid.js");
    fs::write(&path, "function valid(){}\n").unwrap();
    let mut client = Client::start(&[
        "--allow-path",
        root.path().to_str().unwrap(),
        "--max-snapshot-bytes",
        "10000",
    ]);
    client.initialize();
    let invalid = client.tool("rxd_open", json!({"path":directory,"backend":"js"}));
    assert_eq!(invalid["isError"], true, "{invalid}");
    let valid = client.tool("rxd_open", json!({"path":path}));
    assert_ne!(valid["isError"], true, "{valid}");
}
