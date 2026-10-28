//! End-to-end tests only: every assertion exercises the actual CLI process.
#[path = "resource_policy/mod.rs"]
mod resource_policy;

#[cfg(feature = "backend-dex")]
#[path = "dex/mod.rs"]
mod dex;

#[cfg(feature = "backend-dotnet")]
#[path = "dotnet/mod.rs"]
mod dotnet;

#[cfg(feature = "backend-pyc")]
#[path = "pyc/mod.rs"]
mod pyc;

#[cfg(feature = "backend-wasm")]
#[path = "wasm/mod.rs"]
mod wasm;

#[cfg(feature = "backend-evm")]
#[path = "evm/mod.rs"]
mod evm;

#[cfg(feature = "backend-lua")]
#[path = "lua/mod.rs"]
mod lua;

#[cfg(feature = "backend-flutter")]
#[path = "flutter/mod.rs"]
mod flutter;

#[cfg(feature = "bundled-kuna")]
#[path = "kuna/mod.rs"]
mod kuna;

#[cfg(feature = "mcp")]
#[path = "mcp/mod.rs"]
mod mcp;

#[cfg(feature = "unpack")]
#[path = "unpack/mod.rs"]
mod unpack;

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;
use tempfile::TempDir;

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

// Independent handcrafted Chromium Pickles; no Electron/Node process runs input JS.
