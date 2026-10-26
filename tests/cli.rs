//! End-to-end tests only: every assertion exercises the actual CLI process.
#[cfg(all(
    feature = "backend-asar",
    any(feature = "backend-html", feature = "backend-wasm")
))]
#[path = "container_entry/mod.rs"]
mod container_entry;
#[cfg(feature = "backend-html")]
#[path = "html/mod.rs"]
mod html;
#[path = "resource_policy/mod.rs"]
mod resource_policy;
#[cfg(feature = "backend-zip")]
#[path = "zip/mod.rs"]
mod zip_backend;

#[cfg(feature = "backend-dex")]
#[path = "dex/mod.rs"]
mod dex;

#[cfg(feature = "backend-dotnet")]
#[path = "dotnet/mod.rs"]
mod dotnet;

#[cfg(feature = "backend-jvm")]
#[path = "jvm/mod.rs"]
mod jvm;

#[cfg(feature = "backend-pyc")]
#[path = "pyc/mod.rs"]
mod pyc;

#[cfg(feature = "backend-wasm")]
#[path = "wasm/mod.rs"]
mod wasm;

#[cfg(feature = "backend-evm")]
#[path = "evm/mod.rs"]
mod evm;

#[cfg(feature = "backend-wasm")]
#[path = "wasm_wabt/mod.rs"]
mod wasm_wabt;

#[cfg(feature = "bundled-kuna")]
#[path = "kuna/mod.rs"]
mod kuna;

#[cfg(feature = "mcp")]
#[path = "mcp/mod.rs"]
mod mcp;

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

#[test]
fn help_explains_parameter_sources_and_minimal_query_workflows() {
    let binary = env!("CARGO_BIN_EXE_rxd");
    let assert_query_focused = |help: &str| {
        for phrase in [
            "without executing",
            "never execute",
            "never runs input",
            "without instantiating",
            "no module instantiation",
            "without loading it",
        ] {
            assert!(!help.to_lowercase().contains(phrase), "{help}");
        }
    };
    let root = Command::new(binary).arg("--help").output().unwrap();
    success(&root);
    assert!(root.stderr.is_empty());
    let text = String::from_utf8(root.stdout).unwrap();
    assert!(text.contains("Workflow: list/search"));
    assert!(text.contains("COMMAND --help"));
    assert!(text.lines().next().unwrap().contains("Native binaries"));
    assert!(!text.contains("JavaScript"));
    assert_query_focused(&text);
    let routes: &[(&str, &str, &str)] = &[
        #[cfg(feature = "backend-html")]
        ("html", "extract", "entries.at"),
        #[cfg(feature = "backend-asar")]
        ("asar", "extract", "entries.at"),
        #[cfg(feature = "backend-zip")]
        ("zip", "extract", "entries.at"),
        #[cfg(feature = "backend-dex")]
        ("dex", "get-method", "methods.at"),
        #[cfg(feature = "backend-jvm")]
        ("jar", "get-method", "methods.at"),
        #[cfg(feature = "backend-dotnet")]
        ("dotnet", "get-method", "methods.at"),
        #[cfg(feature = "backend-wasm")]
        ("wasm", "get-function", "functions.at"),
        #[cfg(feature = "backend-pyc")]
        ("pyc", "get-code", "codes.at"),
        #[cfg(feature = "backend-evm")]
        ("evm", "get-function", "interface.at"),
    ];
    for (backend, command, source) in routes {
        let output = Command::new(binary)
            .args([backend, command, "--help"])
            .output()
            .unwrap();
        success(&output);
        assert!(output.stderr.is_empty());
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains("Examples:"), "{backend} {command}: {help}");
        assert!(help.contains(source), "{backend} {command}: {help}");
        assert!(help.contains("Output:"), "{backend} {command}: {help}");
        assert!(!help.contains("Not marked required by clap"));
        assert_query_focused(&help);
        let missing = Command::new(binary)
            .args([backend, command, "missing-input"])
            .output()
            .unwrap();
        assert_eq!(missing.status.code(), Some(2), "{missing:?}");
        assert!(missing.stdout.is_empty());
        assert!(String::from_utf8_lossy(&missing.stderr).contains("--at"));
    }
    assert!(
        !Command::new(binary)
            .args(["js", "--help"])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn skill_prints_the_embedded_skill_without_writing_files() {
    let home = TempDir::new().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .arg("skill")
        .env("HOME", home.path())
        .env_remove("CODEX_HOME")
        .output()
        .unwrap();
    success(&output);
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout, include_bytes!("../src/skills/rxd/SKILL.md"));
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
}

#[allow(dead_code)]
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[allow(dead_code)]
fn jsonl(output: &Output) -> Vec<Value> {
    success(output);
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[allow(dead_code)]
fn fixture(source: impl AsRef<[u8]>, filename: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(filename);
    fs::write(&path, source).unwrap();
    (dir, path)
}

#[allow(dead_code)]
fn perf_reports(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["record"] == "perf")
        .collect()
}

#[allow(dead_code)]
fn failure(output: Output, code: i32, diagnostic: &str) {
    // Query-class failures exit 2 even when a call site still passes 1.
    let expected = match diagnostic {
        "INVALID_QUERY" | "NOT_FOUND" | "AMBIGUOUS" => 2,
        _ => code,
    };
    assert_eq!(
        output.status.code(),
        Some(expected),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "failed queries must not emit partial stdout"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(diagnostic),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[allow(dead_code)]
fn refused(output: Output) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}

#[allow(dead_code)]
fn assert_record_keys(value: &Value, fields: &[&str]) {
    let mut expected = fields.to_vec();
    expected.sort_unstable();
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        expected
    );
}

// Independent handcrafted Chromium Pickles; no Electron/Node process runs input JS.
#[allow(dead_code)]
fn write_asar(path: &Path, header: &Value, data: &[u8]) {
    let json = serde_json::to_vec(header).unwrap();
    let payload = (json.len() + 4).next_multiple_of(4);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&4_u32.to_le_bytes());
    bytes.extend_from_slice(&((payload + 4) as u32).to_le_bytes());
    bytes.extend_from_slice(&(payload as u32).to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&json);
    bytes.resize(payload + 12, 0);
    bytes.extend_from_slice(data);
    fs::write(path, bytes).unwrap();
}

#[allow(dead_code)]
fn pack_asar(path: &Path, files: &[(&str, &[u8])]) {
    let mut header = serde_json::json!({"files": {}});
    let mut data = Vec::new();
    for (path, bytes) in files {
        let mut components = path.split('/').peekable();
        let mut parent = &mut header;
        while let Some(component) = components.next() {
            let files = parent["files"].as_object_mut().unwrap();
            if components.peek().is_none() {
                files.insert(
                    component.into(),
                    serde_json::json!({"size": bytes.len(), "offset": data.len().to_string()}),
                );
                break;
            }
            parent = files
                .entry(component)
                .or_insert_with(|| serde_json::json!({"files": {}}));
        }
        data.extend_from_slice(bytes);
    }
    write_asar(path, &header, &data);
}
