use super::support::Client;
use serde_json::{Value, json};
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
        "--allow-path",
        root.path().to_str().unwrap(),
        "--max-snapshot-bytes",
        "30000",
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
#[test]
fn mcp_startup_rejects_capture_depth_above_the_safe_protocol_process_ceiling() {
    let root = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args([
            "mcp",
            "--allow-path",
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
    let mut client = Client::start(&["--allow-path", root.path().to_str().unwrap()]);
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
