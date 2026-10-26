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
