use super::support::Client;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, process::Command};

fn opened(client: &mut Client, path: &std::path::Path, bytes: &[u8]) {
    let result = client.tool("rxd_open", json!({"path":path}));
    assert_ne!(result["isError"], true, "{result}");
    let info = &result["structuredContent"];
    assert_eq!(info["source_id"], format!("{:x}", Sha256::digest(bytes)));
    assert_eq!(info["inputs"][0]["original_path"], json!(path));
    let closed = client.tool("rxd_close", json!({"target_id":info["target_id"]}));
    assert_ne!(closed["isError"], true, "{closed}");
}

fn error(result: &Value, code: &str) {
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["structuredContent"]["error"]["code"], code,
        "{result}"
    );
}

#[test]
fn mcp_requires_absolute_inputs_and_defaults_to_unrestricted_paths() {
    let working = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let local = working.path().join("source.js");
    let external = outside.path().join("输入 source.js");
    let bytes = b"function source(){return 7;}\n";
    fs::write(&local, bytes).unwrap();
    fs::write(&external, bytes).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_rxd"));
    command.arg("mcp").current_dir(working.path());
    let mut client = Client::start_command(command);
    client.initialize();
    opened(&mut client, &local, bytes);
    opened(&mut client, &external, bytes);
    for path in ["source.js", "./source.js", "../source.js", ""] {
        error(
            &client.tool("rxd_open", json!({"path":path})),
            "INVALID_QUERY",
        );
    }
    #[cfg(feature = "backend-dex")]
    error(
        &client.tool(
            "rxd_open",
            json!({"paths":[local,"source.js"],"backend":"rasc"}),
        ),
        "INVALID_QUERY",
    );
    opened(&mut client, &external, bytes);

    for flag in ["--root", "--base"] {
        let rejected = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["mcp", flag])
            .arg(working.path())
            .output()
            .unwrap();
        assert_eq!(rejected.status.code(), Some(2));
        assert!(rejected.stdout.is_empty());
    }
    let relative_grant = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["mcp", "--allow-path", "."])
        .current_dir(working.path())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(relative_grant.status.code(), Some(2));
    assert!(relative_grant.stdout.is_empty());
    assert!(String::from_utf8_lossy(&relative_grant.stderr).contains("must be absolute"));
}

#[test]
fn mcp_allow_paths_grant_only_named_files_and_directory_descendants() {
    let scratch = tempfile::tempdir().unwrap();
    let directory = scratch.path().join("allowed");
    let nested = directory.join("nested");
    let prefix_sibling = scratch.path().join("allowed-other");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir(&prefix_sibling).unwrap();
    let local = nested.join("source.js");
    let selected = scratch.path().join("selected.js");
    let sibling = scratch.path().join("selected.js-other");
    let outside = prefix_sibling.join("source.js");
    let bytes = b"function source(){return 7;}\n";
    for path in [&local, &selected, &sibling, &outside] {
        fs::write(path, bytes).unwrap();
    }
    let mut client = Client::start(&[
        "--allow-path",
        directory.to_str().unwrap(),
        "--allow-path",
        selected.to_str().unwrap(),
    ]);
    client.initialize();
    opened(&mut client, &local, bytes);
    opened(&mut client, &selected, bytes);
    for path in [&sibling, &outside] {
        error(
            &client.tool("rxd_open", json!({"path":path})),
            "ACCESS_DENIED",
        );
    }
    opened(&mut client, &local, bytes);
}
