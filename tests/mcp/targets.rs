use super::support::Client;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;

fn success(result: &Value) -> &Value {
    assert_ne!(result["isError"], true, "{result}");
    &result["structuredContent"]
}

fn error(result: &Value, code: &str) {
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["structuredContent"]["error"]["code"], code,
        "{result}"
    );
}

#[cfg(feature = "backend-js")]
#[test]
fn mcp_target_snapshot_retains_original_bytes_and_close_invalidates_future_calls() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("原始 source.js");
    let source = "function original(){return \"你好\";}\r\n";
    fs::write(&path, source).unwrap();
    let mut client = Client::start(&["--root", root.path().to_str().unwrap(), "--backends", "js"]);
    client.initialize();
    let opened = client.tool("rxd_open", json!({"path":path}));
    let info = success(&opened);
    assert_eq!(
        info["source_id"],
        format!("{:x}", Sha256::digest(source.as_bytes()))
    );
    assert_eq!(info["snapshot_id"], info["source_id"]);
    assert_eq!(info["input_bytes"], source.len());
    let target = info["target_id"].as_str().unwrap().to_owned();
    let input = json!({"target_id":target});
    fs::write(&path, "function replacement(){return 0;}\n").unwrap();
    let entities = client.tool("js_entities", json!({"input":input}));
    let entities = success(&entities);
    assert_eq!(entities["header"]["source_id"], info["source_id"]);
    assert_eq!(entities["header"]["detail"]["path"], json!(path));
    let entity = entities["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entity| entity["detail"]["name"] == "original")
        .unwrap();
    let fetched = client.tool("js_get", json!({"input":input,"at":entity["at"]}));
    assert_eq!(
        success(&fetched)["detail"]["source"],
        "function original(){return \"你好\";}"
    );
    let changed = client.tool("js_entities", json!({"input":{"path":path}}));
    assert_ne!(success(&changed)["header"]["source_id"], info["source_id"]);
    let wrong_identity = client.tool("js_get", json!({"input":{"path":path},"at":entity["at"]}));
    assert_eq!(wrong_identity["isError"], true, "{wrong_identity}");
    success(&client.tool("rxd_close", json!({"target_id":target})));
    error(
        &client.tool("js_entities", json!({"input":input})),
        "TARGET_INVALID",
    );
    error(
        &client.tool("rxd_close", json!({"target_id":target})),
        "TARGET_INVALID",
    );
    error(
        &client.tool(
            "js_entities",
            json!({"input":{"target_id":"another-process-handle"}}),
        ),
        "TARGET_INVALID",
    );
}

#[cfg(feature = "backend-js")]
#[test]
fn mcp_one_shot_queries_do_not_consume_durable_target_slots() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.js");
    let second = root.path().join("second.js");
    fs::write(&first, "function first(){}\n").unwrap();
    fs::write(&second, "function second(){}\n").unwrap();
    let mut client = Client::start(&[
        "--root",
        root.path().to_str().unwrap(),
        "--backends",
        "js",
        "--max-targets",
        "1",
    ]);
    client.initialize();
    let opened = client.tool("rxd_open", json!({"path":first}));
    let target = success(&opened)["target_id"].as_str().unwrap();
    error(
        &client.tool("rxd_open", json!({"path":second})),
        "RESOURCE_LIMIT",
    );
    for _ in 0..3 {
        let query = client.tool("js_entities", json!({"input":{"path":second}}));
        assert!(
            success(&query)["records"]
                .as_array()
                .unwrap()
                .iter()
                .any(|record| record["detail"]["name"] == "second")
        );
    }
    success(&client.tool("rxd_close", json!({"target_id":target})));
    success(&client.tool("rxd_open", json!({"path":second})));
}

#[cfg(feature = "backend-js")]
#[test]
fn mcp_snapshot_byte_quotas_fail_explicitly_and_release_after_close() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.js");
    let second = root.path().join("second.js");
    let large = root.path().join("large.js");
    fs::write(&first, "const x=1;").unwrap();
    fs::write(&second, "const y=2;").unwrap();
    fs::write(&large, [b' '; 17]).unwrap();
    let mut client = Client::start(&[
        "--root",
        root.path().to_str().unwrap(),
        "--backends",
        "js",
        "--max-capture-bytes",
        "16",
        "--max-snapshot-bytes",
        "16",
    ]);
    client.initialize();
    let opened = client.tool("rxd_open", json!({"path":first}));
    let target = success(&opened)["target_id"].as_str().unwrap();
    error(
        &client.tool("rxd_open", json!({"path":second})),
        "RESOURCE_LIMIT",
    );
    error(
        &client.tool("rxd_open", json!({"path":large})),
        "RESOURCE_LIMIT",
    );
    success(&client.tool("rxd_close", json!({"target_id":target})));
    success(&client.tool("rxd_open", json!({"path":second})));
}

#[cfg(feature = "backend-js")]
#[test]
fn mcp_expired_target_handles_fail_and_their_slot_can_be_reused() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("expiry.js");
    fs::write(&path, "function expiry(){}\n").unwrap();
    let mut client = Client::start(&[
        "--root",
        root.path().to_str().unwrap(),
        "--backends",
        "js",
        "--max-targets",
        "1",
        "--target-ttl-ms",
        "20",
    ]);
    client.initialize();
    let opened = client.tool("rxd_open", json!({"path":path}));
    let target = success(&opened)["target_id"].as_str().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(80));
    error(
        &client.tool("js_entities", json!({"input":{"target_id":target}})),
        "TARGET_INVALID",
    );
    let reopened = client.tool("rxd_open", json!({"path":path}));
    assert_ne!(success(&reopened)["target_id"], target);
}

#[cfg(feature = "backend-js")]
#[test]
fn mcp_recursive_target_freezes_selected_sources_and_preserves_file_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let first = root.path().join("first.js");
    let second = nested.join("second.ts");
    fs::write(&first, "function first(){}\n").unwrap();
    fs::write(&second, "function second(value: number){return value;}\n").unwrap();
    fs::write(root.path().join("ignored.txt"), "this is not JavaScript").unwrap();
    let mut client = Client::start(&["--root", root.path().to_str().unwrap(), "--backends", "js"]);
    client.initialize();
    let opened = client.tool("rxd_open", json!({"path":root.path(),"backend":"js"}));
    let info = success(&opened);
    assert_eq!(info["kind"], "directory");
    assert_eq!(info["inputs"].as_array().unwrap().len(), 2);
    let target = info["target_id"].as_str().unwrap();
    fs::write(&first, "function replacement(){}\n").unwrap();
    fs::write(root.path().join("added.js"), "function added(){}\n").unwrap();
    let entities = client.tool("js_entities", json!({"input":{"target_id":target}}));
    let entities = success(&entities);
    assert_eq!(entities["scan_summary"]["complete"], true);
    assert_eq!(entities["scan_summary"]["detail"]["files"], 2);
    let files = entities["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["header"]["detail"]["path"], json!(first));
    assert_eq!(files[1]["header"]["detail"]["path"], json!(second));
    assert!(
        files[0]["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["detail"]["name"] == "first")
    );
    assert!(
        files[1]["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["detail"]["name"] == "second")
    );
    let changed = client.tool("js_entities", json!({"input":{"path":root.path()}}));
    assert_eq!(success(&changed)["scan_summary"]["detail"]["files"], 3);
}

#[cfg(all(feature = "backend-dex", feature = "backend-js"))]
#[test]
fn mcp_input_bundle_preserves_supplied_order_and_duplicate_identity() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.dex");
    let second = root.path().join("second.dex");
    let first_bytes = b"first fixed input";
    let second_bytes = b"second fixed input";
    fs::write(&first, first_bytes).unwrap();
    fs::write(&second, second_bytes).unwrap();
    let mut client = Client::start(&[
        "--root",
        root.path().to_str().unwrap(),
        "--backends",
        "dex,js",
    ]);
    client.initialize();
    let opened = client.tool(
        "rxd_open",
        json!({"paths":[second,first,second],"backend":"dex"}),
    );
    let info = success(&opened);
    assert_eq!(info["kind"], "bundle");
    assert_eq!(
        info["input_bytes"],
        first_bytes.len() + 2 * second_bytes.len()
    );
    assert_eq!(
        info["captured_bytes"],
        first_bytes.len() + second_bytes.len()
    );
    let files = info["inputs"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(files[0]["original_path"], json!(second));
    assert_eq!(files[1]["original_path"], json!(first));
    assert_eq!(files[2], files[0]);
    assert_eq!(
        files[0]["source_id"],
        format!("{:x}", Sha256::digest(second_bytes))
    );
    assert_eq!(
        files[1]["source_id"],
        format!("{:x}", Sha256::digest(first_bytes))
    );
    error(
        &client.tool(
            "js_entities",
            json!({"input":{"target_id":info["target_id"]}}),
        ),
        "INVALID_QUERY",
    );
}

#[cfg(all(feature = "backend-asar", feature = "backend-js"))]
#[test]
fn mcp_asar_dependencies_and_child_targets_remain_stable_after_parent_changes_and_close() {
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("app.asar");
    let unpacked = root.path().join("app.asar.unpacked");
    fs::create_dir_all(unpacked.join("pkg")).unwrap();
    let source = "function unpacked(){return \"你好\";}\r\n";
    fs::write(unpacked.join("pkg/bundle.js"), source).unwrap();
    crate::write_asar(
        &archive,
        &json!({"files":{
            "pkg":{"files":{"bundle.js":{"size":source.len(),"unpacked":true}}},
            "alias.js":{"link":"pkg/bundle.js"}
        }}),
        b"",
    );
    let archive_bytes = fs::read(&archive).unwrap();
    let mut client = Client::start(&[
        "--root",
        root.path().to_str().unwrap(),
        "--backends",
        "asar,js",
    ]);
    client.initialize();
    let opened = client.tool("rxd_open", json!({"path":archive,"backend":"asar"}));
    let parent = success(&opened);
    assert_eq!(
        parent["source_id"],
        format!("{:x}", Sha256::digest(&archive_bytes))
    );
    assert_ne!(parent["snapshot_id"], parent["source_id"]);
    assert_eq!(parent["input_bytes"], archive_bytes.len());
    assert_eq!(parent["captured_bytes"], archive_bytes.len() + source.len());
    fs::write(&archive, b"changed invalid archive").unwrap();
    fs::write(unpacked.join("pkg/bundle.js"), "function changed(){}\n").unwrap();
    let input = json!({"target_id":parent["target_id"]});
    let extracted = client.tool("asar_extract", json!({"input":input,"at":"pkg/bundle.js"}));
    let child = &success(&extracted)["child_target"];
    assert_eq!(
        child["source_id"],
        format!("{:x}", Sha256::digest(source.as_bytes()))
    );
    assert_eq!(child["origin"]["container_source_id"], parent["source_id"]);
    assert_eq!(child["origin"]["entry"], "pkg/bundle.js");
    assert_eq!(child["origin"]["parent_target_id"], parent["target_id"]);
    error(
        &client.tool("asar_extract", json!({"input":input,"at":"alias.js"})),
        "INVALID_INPUT",
    );
    success(&client.tool("rxd_close", json!({"target_id":parent["target_id"]})));
    let entities = client.tool(
        "js_entities",
        json!({"input":{"target_id":child["target_id"]}}),
    );
    assert!(
        success(&entities)["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["detail"]["name"] == "unpacked")
    );
}

#[cfg(unix)]
#[cfg(feature = "backend-js")]
#[test]
fn mcp_root_checks_reject_symlink_escape_and_nonregular_inputs_without_blocking() {
    use std::{ffi::CString, os::unix::fs::symlink};
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let original = root.path().join("original.js");
    let external = outside.path().join("secret.js");
    fs::write(&original, "function allowed(){}\n").unwrap();
    fs::write(&external, "function outside(){}\n").unwrap();
    let alias = root.path().join("alias.js");
    let escape = root.path().join("escape.js");
    symlink(&original, &alias).unwrap();
    symlink(&external, &escape).unwrap();
    let fifo = root.path().join("pipe.js");
    let fifo_path = CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: fifo_path is a valid NUL-terminated path used only as a handcrafted input fixture.
    assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
    let mut client = Client::start(&["--root", root.path().to_str().unwrap(), "--backends", "js"]);
    client.initialize();
    error(
        &client.tool("rxd_open", json!({"path":escape})),
        "ACCESS_DENIED",
    );
    error(
        &client.tool("rxd_open", json!({"path":fifo})),
        "INVALID_INPUT",
    );
    let entities = client.tool("js_entities", json!({"input":{"path":alias}}));
    assert_eq!(success(&entities)["header"]["detail"]["path"], json!(alias));
    assert!(
        success(&entities)["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["detail"]["name"] == "allowed")
    );
}

#[cfg(unix)]
#[cfg(feature = "backend-js")]
#[test]
fn mcp_captures_never_follow_raced_directory_links_outside_allowed_roots() {
    use std::{
        os::unix::fs::symlink,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    struct Mutator {
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for Mutator {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
        }
    }
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let directory = root.path().join("moving");
    let parked = root.path().join("parked");
    fs::create_dir(&directory).unwrap();
    let source = "function allowed(){}\n";
    fs::write(directory.join("input.js"), source).unwrap();
    fs::write(outside.path().join("input.js"), "function leaked(){}\n").unwrap();
    let external = outside.path().to_owned();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let input_path = directory.join("input.js");
    let mut client = Client::start(&["--root", root.path().to_str().unwrap(), "--backends", "js"]);
    client.initialize();
    let mutator = Mutator {
        stop,
        thread: Some(std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                fs::rename(&directory, &parked).unwrap();
                symlink(&external, &directory).unwrap();
                std::thread::yield_now();
                fs::remove_file(&directory).unwrap();
                fs::rename(&parked, &directory).unwrap();
            }
        })),
    };
    for _ in 0..20 {
        let result = client.tool("js_entities", json!({"input":{"path":input_path}}));
        if result["isError"] != true {
            let result = success(&result);
            assert_eq!(
                result["header"]["source_id"],
                format!("{:x}", Sha256::digest(source.as_bytes()))
            );
            assert!(
                result["records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|record| record["detail"]["name"] == "allowed")
            );
        }
    }
    drop(mutator);
    let result = client.tool("js_entities", json!({"input":{"path":input_path}}));
    assert_eq!(
        success(&result)["header"]["source_id"],
        format!("{:x}", Sha256::digest(source.as_bytes()))
    );
}

#[cfg(unix)]
#[cfg(feature = "backend-asar")]
#[test]
fn mcp_asar_dependency_capture_rejects_missing_and_linked_sources_and_releases_failed_captures() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("app.asar");
    let unpacked = root.path().join("app.asar.unpacked");
    crate::write_asar(
        &archive,
        &json!({"files":{"entry":{"size":1,"unpacked":true}}}),
        b"",
    );
    let mut client = Client::start(&[
        "--root",
        root.path().to_str().unwrap(),
        "--backends",
        "asar",
        "--max-live-captures",
        "1",
    ]);
    client.initialize();
    error(
        &client.tool("rxd_open", json!({"path":archive,"backend":"asar"})),
        "IO_ERROR",
    );
    fs::create_dir(&unpacked).unwrap();
    let external = root.path().join("external");
    fs::write(&external, b"x").unwrap();
    symlink(&external, unpacked.join("entry")).unwrap();
    error(
        &client.tool("rxd_open", json!({"path":archive,"backend":"asar"})),
        "IO_ERROR",
    );
    fs::remove_file(unpacked.join("entry")).unwrap();
    fs::write(unpacked.join("entry"), b"x").unwrap();
    let opened = client.tool("rxd_open", json!({"path":archive,"backend":"asar"}));
    assert_eq!(success(&opened)["inputs"].as_array().unwrap().len(), 2);
}

#[cfg(unix)]
#[cfg(feature = "backend-js")]
#[test]
fn mcp_closing_a_target_keeps_already_acquired_worker_bytes_available() {
    struct StoppedWorker(u32);
    impl Drop for StoppedWorker {
        fn drop(&mut self) {
            let _ = std::process::Command::new("kill")
                .args(["-CONT", &self.0.to_string()])
                .status();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("inflight.js");
    let source = format!("function slow(){{\n{}}}", "var item=1;\n".repeat(180_000));
    fs::write(&path, &source).unwrap();
    let mut client = Client::start(&["--root", root.path().to_str().unwrap(), "--backends", "js"]);
    client.initialize();
    let opened = client.tool("rxd_open", json!({"path":path}));
    let target = success(&opened)["target_id"].as_str().unwrap();
    client.send(
        json!({"jsonrpc":"2.0","id":100,"method":"tools/call","params":{
            "name":"js_entities","arguments":{"input":{"target_id":target}}
        }}),
    );
    let worker = super::runtime::child_worker(client.pid());
    assert!(
        std::process::Command::new("kill")
            .args(["-STOP", &worker.to_string()])
            .status()
            .unwrap()
            .success()
    );
    let stopped = StoppedWorker(worker);
    success(&client.tool("rxd_close", json!({"target_id":target})));
    error(
        &client.tool("js_entities", json!({"input":{"target_id":target}})),
        "TARGET_INVALID",
    );
    drop(stopped);
    let completed = client.receive();
    assert_eq!(completed["id"], 100);
    assert_eq!(
        success(&completed["result"])["header"]["source_id"],
        format!("{:x}", Sha256::digest(source.as_bytes()))
    );
}

#[test]
fn mcp_startup_rejects_capture_depth_above_the_safe_protocol_process_ceiling() {
    let root = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args([
            "mcp",
            "serve",
            "--root",
            root.path().to_str().unwrap(),
            "--max-snapshot-depth",
            "257",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(
        diagnostic.contains("INVALID_QUERY") && diagnostic.contains("256"),
        "{diagnostic}"
    );
}

#[cfg(windows)]
#[cfg(feature = "backend-asar")]
#[test]
fn mcp_child_artifacts_reject_windows_device_and_aliased_filenames_before_writing() {
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("app.asar");
    let mut client = Client::start(&[
        "--root",
        root.path().to_str().unwrap(),
        "--backends",
        "asar",
    ]);
    client.initialize();
    for name in ["NUL.dll", "COM¹.bin", "LPT3.txt", "trailing.", "trailing "] {
        let mut files = serde_json::Map::new();
        files.insert(name.into(), json!({"size":4,"offset":"0"}));
        crate::write_asar(&archive, &json!({"files":files}), b"data");
        error(
            &client.tool("asar_extract", json!({"input":{"path":archive},"at":name})),
            "INVALID_INPUT",
        );
    }
    crate::write_asar(
        &archive,
        &json!({"files":{"safe.bin":{"size":4,"offset":"0"}}}),
        b"data",
    );
    let result = client.tool(
        "asar_extract",
        json!({"input":{"path":archive},"at":"safe.bin"}),
    );
    assert_eq!(
        success(&result)["child_target"]["source_id"],
        format!("{:x}", Sha256::digest(b"data"))
    );
}
