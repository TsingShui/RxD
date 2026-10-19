use std::{fs, path::PathBuf, process::Command};

use serde_json::Value;
use tempfile::TempDir;

struct Install {
    _directory: TempDir,
    executable: PathBuf,
    input: PathBuf,
    cache: PathBuf,
}

impl Install {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory
            .path()
            .join(format!("rxd{}", std::env::consts::EXE_SUFFIX));
        let input = directory.path().join("input with spaces");
        fs::copy(env!("CARGO_BIN_EXE_rxd"), &executable).unwrap();
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("third_party/kuna/decompiler/crates/kuna-analysis/tests/fixtures/fauxware"),
            &input,
        )
        .unwrap();
        let cache = directory.path().join("cache");
        Self {
            _directory: directory,
            executable,
            input,
            cache,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .arg("kuna")
            .env("RXD_CACHE_DIR", &self.cache)
            .env("KUNA_ROOT", "/does-not-exist")
            .env_remove("KUNA_DECOMP_DBG")
            .env_remove("KUNA_SLACOMP")
            .env_remove("KUNA_DECOMP_TEST")
            .current_dir(self.executable.parent().unwrap());
        command
    }

    fn runtime(&self) -> PathBuf {
        fs::read_dir(self.cache.join("rxd-kuna"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.join(".complete").is_file())
            .unwrap()
    }
}

#[test]
fn bundled_kuna_runs_from_a_copied_rxd_with_no_engine_installation() {
    let install = Install::new();
    let catalog = install
        .command()
        .args(["catalog", "--json", "--option", "loweredswitch"])
        .output()
        .unwrap();
    super::success(&catalog);
    let rows: Vec<Value> = serde_json::from_slice(&catalog.stdout).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["option"], "loweredswitch");
    let missing = install
        .command()
        .args(["catalog", "--option"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
    assert!(!install.cache.exists(), "catalog must use linked metadata");

    let wrapped = install
        .command()
        .arg("decompile")
        .arg(&install.input)
        .args(["main", "--json"])
        .output()
        .unwrap();
    super::success(&wrapped);
    let runtime = install.runtime();
    for name in ["kuna", "kuna.exe", "decomp_dbg", "slacomp", "catalog.json"] {
        assert!(
            !runtime.join(name).exists(),
            "unexpected packaged helper: {name}"
        );
    }
    assert!(
        runtime
            .join("specs/Ghidra/Processors/x86/data/languages/x86-64.sla")
            .is_file()
    );
    let json: Value = serde_json::from_slice(&wrapped.stdout).unwrap();
    let code = json["functions"][0]["code"].as_str().unwrap();
    assert!(!code.is_empty());
    let text = install
        .command()
        .arg("decompile")
        .arg(&install.input)
        .arg("main")
        .output()
        .unwrap();
    super::success(&text);
    assert_eq!(
        String::from_utf8(text.stdout).unwrap().trim_end(),
        code.trim_end()
    );
    let failed = install
        .command()
        .arg("unknown-kuna-command")
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(2));
    assert!(failed.stdout.is_empty());
}

#[test]
fn bundled_kuna_handles_simultaneous_cache_creation_and_parallel_workers() {
    let install = Install::new();
    let first = install
        .command()
        .arg("decompile-all")
        .arg(&install.input)
        .args(["--json", "--jobs", "2"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let second = install
        .command()
        .arg("functions")
        .arg(&install.input)
        .args(["--summary", "--json"])
        .output()
        .unwrap();
    super::success(&second);
    let out = first.wait_with_output().unwrap();
    super::success(&out);
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["count"].as_u64().unwrap() > 0);
    assert!(
        json["functions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["error"].is_null())
    );
}

#[test]
fn bundled_kuna_refuses_an_incomplete_resource_cache() {
    let install = Install::new();
    super::success(
        &install
            .command()
            .arg("functions")
            .arg(&install.input)
            .args(["--summary", "--json"])
            .output()
            .unwrap(),
    );
    let runtime = install.runtime();
    fs::write(runtime.join(".complete"), b"corrupt").unwrap();
    let out = install
        .command()
        .arg("functions")
        .arg(&install.input)
        .args(["--summary", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("incomplete cache"));
    assert!(out.stdout.is_empty());
    super::success(&install.command().arg("--version").output().unwrap());
}

#[test]
fn linked_kuna_streaming_project_workers_restart_the_copied_rxd() {
    let install = Install::new();
    let destination = install
        .executable
        .parent()
        .unwrap()
        .join("project with spaces");
    let out = install
        .command()
        .arg("decompile-project")
        .arg(&install.input)
        .args(["--stream", "--jobs", "2", "-o"])
        .arg(&destination)
        .output()
        .unwrap();
    super::success(&out);
    assert!(!destination.join(".streaming").exists());
    let rows: Vec<Value> = fs::read_to_string(destination.join("index.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(!rows.is_empty());
    assert!(fs::read_dir(&destination).unwrap().any(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|ext| ext == "c")
    }));
}
