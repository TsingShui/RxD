//! End-to-end tests only: every assertion exercises the actual CLI process.
#[cfg(feature = "backend-html")]
#[path = "html/mod.rs"]
mod html;
#[path = "resource_policy/mod.rs"]
mod resource_policy;

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

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

#[test]
fn skill_installs_the_embedded_rxd_skill_for_pi_and_prints_exact_bytes() {
    let home = TempDir::new().unwrap();
    let printed = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["skill", "install", "--print"])
        .env("HOME", home.path())
        .env_remove("CODEX_HOME")
        .output()
        .unwrap();
    success(&printed);
    assert!(printed.stderr.is_empty());
    assert!(printed.stdout.starts_with(b"---\nname: rxd\n"));
    assert_eq!(printed.stdout, include_bytes!("../src/skills/rxd/SKILL.md"));
    assert!(!home.path().join(".agents").exists());

    let installed = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["skill", "install", "-a", "pi"])
        .env("HOME", home.path())
        .env_remove("CODEX_HOME")
        .output()
        .unwrap();
    success(&installed);
    assert!(installed.stdout.is_empty());
    let path = home.path().join(".pi/agent/skills/rxd/SKILL.md");
    assert_eq!(fs::read(&path).unwrap(), printed.stdout);
    assert!(
        String::from_utf8_lossy(&installed.stderr)
            .contains(&format!("rxd skill installed at {}", path.display()))
    );
}

#[test]
fn skill_installs_from_a_relocated_single_executable_without_runtime_provisioning() {
    let isolated = TempDir::new().unwrap();
    let bin = isolated.path().join("bin");
    let home = isolated.path().join("home");
    let working = isolated.path().join("empty-working-directory");
    let cache = isolated.path().join("absent-runtime-cache");
    for directory in [&bin, &home, &working] {
        fs::create_dir(directory).unwrap();
    }
    let executable = bin.join(if cfg!(windows) { "rxd.exe" } else { "rxd" });
    fs::copy(env!("CARGO_BIN_EXE_rxd"), &executable).unwrap();
    let invoke = |args: &[&str]| {
        let mut command = Command::new(&executable);
        command
            .args(["skill", "install"])
            .args(args)
            .current_dir(&working)
            .env_clear()
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("PATH", &bin)
            .env("RXD_CACHE_DIR", &cache);
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system_root);
        }
        command.output().unwrap()
    };
    let printed = invoke(&["--print"]);
    success(&printed);
    assert!(printed.stderr.is_empty());
    assert!(printed.stdout.starts_with(b"---\nname: rxd\n"));
    assert_eq!(printed.stdout, include_bytes!("../src/skills/rxd/SKILL.md"));
    assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
    assert!(!cache.exists());

    let installed = invoke(&["-a", "pi"]);
    success(&installed);
    assert!(installed.stdout.is_empty());
    let target = home.join(".pi/agent/skills/rxd/SKILL.md");
    assert_eq!(fs::read(&target).unwrap(), printed.stdout);
    assert_eq!(
        String::from_utf8(installed.stderr).unwrap(),
        format!("rxd skill installed at {}\n", target.display())
    );
    assert!(!cache.exists());
    assert_eq!(fs::read_dir(&working).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&bin).unwrap().count(), 1);
}

#[test]
fn skill_updates_different_bytes_and_preserves_an_identical_file() {
    let dir = TempDir::new().unwrap();
    let skills_root = dir.path().join("技能 roots");
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["skill", "install", "--dir"])
            .arg(&skills_root)
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .env_remove("CODEX_HOME")
            .output()
            .unwrap()
    };
    let path = skills_root.join("rxd/SKILL.md");
    let installed = invoke();
    success(&installed);
    assert!(installed.stdout.is_empty());
    assert!(String::from_utf8_lossy(&installed.stderr).contains("installed at"));
    fs::write(&path, [0xff, 0x00]).unwrap();
    let updated = invoke();
    success(&updated);
    assert!(updated.stdout.is_empty());
    assert!(String::from_utf8_lossy(&updated.stderr).contains("updated at"));
    assert_eq!(
        fs::read(&path).unwrap(),
        include_bytes!("../src/skills/rxd/SKILL.md")
    );
    let timestamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(timestamp)
        .unwrap();
    let unchanged = invoke();
    success(&unchanged);
    assert!(unchanged.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unchanged.stderr).contains("unchanged at"));
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), timestamp);
    assert_eq!(
        fs::read(path).unwrap(),
        include_bytes!("../src/skills/rxd/SKILL.md")
    );
}

#[test]
fn skill_explicit_agents_expand_aliases_and_deduplicate_in_selection_order() {
    let home = TempDir::new().unwrap();
    let codex_home = home.path().join("custom-codex");
    let output = skill_command(home.path())
        .args([
            "-a",
            "Pi",
            "codex",
            "CLAUDE-CODE",
            "agents",
            "-a",
            "all",
            "pi",
        ])
        .env("CODEX_HOME", &codex_home)
        .output()
        .unwrap();
    success(&output);
    assert!(output.stdout.is_empty());
    let paths = [
        home.path().join(".pi/agent/skills/rxd/SKILL.md"),
        codex_home.join("skills/rxd/SKILL.md"),
        home.path().join(".claude/skills/rxd/SKILL.md"),
        home.path().join(".agents/skills/rxd/SKILL.md"),
    ];
    let diagnostics = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        diagnostics.lines().collect::<Vec<_>>(),
        paths
            .iter()
            .map(|path| format!("rxd skill installed at {}", path.display()))
            .collect::<Vec<_>>()
    );
    for path in paths {
        assert_eq!(
            fs::read(path).unwrap(),
            include_bytes!("../src/skills/rxd/SKILL.md")
        );
    }
    assert!(!home.path().join(".codex").exists());

    let other_home = TempDir::new().unwrap();
    let output = skill_command(other_home.path())
        .args(["--agent", "claude", "codex", "claude-code"])
        .env("CODEX_HOME", "")
        .output()
        .unwrap();
    success(&output);
    assert_eq!(String::from_utf8_lossy(&output.stderr).lines().count(), 2);
    assert!(
        other_home
            .path()
            .join(".claude/skills/rxd/SKILL.md")
            .is_file()
    );
    assert!(
        other_home
            .path()
            .join(".codex/skills/rxd/SKILL.md")
            .is_file()
    );
    assert!(!other_home.path().join(".pi").exists());
    assert!(!other_home.path().join(".agents").exists());
}

#[test]
fn skill_reports_unreadable_existing_targets_without_replacing_them() {
    let home = TempDir::new().unwrap();
    let path = home.path().join("skills/rxd/SKILL.md");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("keep"), "existing directory").unwrap();
    let output = skill_command(home.path())
        .arg("--dir")
        .arg(home.path().join("skills"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains(&format!("read {}", path.display())));
    assert_eq!(
        fs::read_to_string(path.join("keep")).unwrap(),
        "existing directory"
    );
}

#[test]
fn skill_defaults_detect_agent_directories_and_respect_codex_and_home_fallbacks() {
    let empty = TempDir::new().unwrap();
    let fallback = skill_command(empty.path()).output().unwrap();
    success(&fallback);
    assert!(empty.path().join(".agents/skills/rxd/SKILL.md").is_file());
    assert!(!empty.path().join(".pi").exists());
    assert!(!empty.path().join(".codex").exists());
    assert!(!empty.path().join(".claude").exists());

    let home = TempDir::new().unwrap();
    let codex = home.path().join("codex-override");
    for path in [
        home.path().join(".pi"),
        home.path().join(".claude"),
        home.path().join(".agents/skills"),
        codex.clone(),
    ] {
        fs::create_dir_all(path).unwrap();
    }
    let detected = skill_command(home.path())
        .env("CODEX_HOME", &codex)
        .output()
        .unwrap();
    success(&detected);
    assert_eq!(String::from_utf8_lossy(&detected.stderr).lines().count(), 3);
    for path in [
        home.path().join(".pi/agent/skills/rxd/SKILL.md"),
        codex.join("skills/rxd/SKILL.md"),
        home.path().join(".claude/skills/rxd/SKILL.md"),
    ] {
        assert_eq!(
            fs::read(path).unwrap(),
            include_bytes!("../src/skills/rxd/SKILL.md")
        );
    }
    assert!(!home.path().join(".agents/skills/rxd").exists());
    assert!(!home.path().join(".codex").exists());

    let home = TempDir::new().unwrap();
    fs::create_dir(home.path().join(".codex")).unwrap();
    let fallback = skill_command(home.path())
        .env("CODEX_HOME", "")
        .env_remove("HOME")
        .output()
        .unwrap();
    success(&fallback);
    assert_eq!(String::from_utf8_lossy(&fallback.stderr).lines().count(), 1);
    assert!(home.path().join(".codex/skills/rxd/SKILL.md").is_file());

    let home = TempDir::new().unwrap();
    fs::create_dir(home.path().join(".codex")).unwrap();
    let override_missing = skill_command(home.path())
        .env("CODEX_HOME", home.path().join("not-detected"))
        .output()
        .unwrap();
    success(&override_missing);
    assert!(home.path().join(".agents/skills/rxd/SKILL.md").is_file());
    assert!(!home.path().join(".codex/skills").exists());
    assert!(!home.path().join("not-detected").exists());
}

#[test]
fn skill_rejects_invalid_and_conflicting_targets_before_writing_files() {
    let home = TempDir::new().unwrap();
    for (args, status, diagnostic) in [
        (vec!["-a", "pi", "unknown"], 1, "unknown agent `unknown`"),
        (vec!["-a", "pi", "--dir", "unused"], 1, "mutually exclusive"),
        (vec!["--print", "-a", "codex"], 1, "drop --agent and --dir"),
        (
            vec!["--print", "--dir", "unused"],
            1,
            "drop --agent and --dir",
        ),
        (vec!["--invalid"], 2, "unexpected argument"),
        (vec!["-a"], 2, "a value is required"),
    ] {
        let output = skill_command(home.path()).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(status));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(diagnostic),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
    }
    let no_home = skill_command(home.path())
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .output()
        .unwrap();
    assert_eq!(no_home.status.code(), Some(1));
    assert!(no_home.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&no_home.stderr).contains("cannot determine the home directory")
    );
    let printed = skill_command(home.path())
        .arg("--print")
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .output()
        .unwrap();
    success(&printed);
    assert!(printed.stderr.is_empty());
    assert_eq!(printed.stdout, include_bytes!("../src/skills/rxd/SKILL.md"));
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn skill_reports_write_failures_at_the_target_path() {
    let home = TempDir::new().unwrap();
    let path = home.path().join("skills/rxd/SKILL.md");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(home.path().join("missing/target"), &path).unwrap();
    let output = skill_command(home.path())
        .arg("--dir")
        .arg(home.path().join("skills"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains(&format!("write {}", path.display())));
    assert!(!home.path().join("missing").exists());
    assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());

    let skills_root = home.path().join("other-skills");
    fs::create_dir(&skills_root).unwrap();
    let skill_directory = skills_root.join("rxd");
    std::os::unix::fs::symlink(home.path().join("missing-directory"), &skill_directory).unwrap();
    let output = skill_command(home.path())
        .arg("--dir")
        .arg(&skills_root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains(&format!("create {}", skill_directory.display()))
    );
    assert!(!home.path().join("missing-directory").exists());
}

fn skill_command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rxd"));
    command
        .args(["skill", "install"])
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env_remove("CODEX_HOME");
    command
}

fn invoke(command: &str, input: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["js", command])
        .arg(input)
        .args(args)
        .output()
        .unwrap()
}

/// The full structural catalog: every kind except the program root. Grouped listing
/// commands are views over exactly this stream, so assertions keep one entry point.
fn outline_rows(input: &Path, args: &[&str]) -> Vec<Value> {
    jsonl(&invoke("entities", input, args))
}

/// A budget flag no longer exists: the CLI refuses it (exit 2) without touching outputs.
fn refused(output: Output) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}

/// A coordinate is what a row prints: `<kind>:<start>:<end>`, with the input identity in
/// the header rather than inside every id.
fn js_full_id(_header: &Value, at: &str) -> String {
    at.to_owned()
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn jsonl(output: &Output) -> Vec<Value> {
    success(output);
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn assert_js_keys(value: &Value, fields: &[&str]) {
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

fn assert_js_record(row: &Value) {
    let mut outer = vec![
        "record",
        "at",
        // Present but null on every record: identity lives in the header.
        "source_id",
        "parent_at",
        "context_at",
        "start",
        "end",
        "detail",
    ];
    if row.get("command").is_some() {
        outer.extend(["backend", "command", "schema_version"]);
        assert_eq!(row["backend"], "js");
        assert_eq!(row["schema_version"], 2);
    }
    // Identity lives in the stream header; every other record leaves source_id null.
    if row["record"] == "header" {
        assert!(row["source_id"].is_string());
    } else {
        assert!(row["source_id"].is_null());
    }
    assert_js_keys(row, &outer);
    assert!(row["start"].is_number());
    assert!(row["end"].is_number());
    let fields: &[&str] = match row["record"].as_str().unwrap() {
        "entity" => &[
            "kind",
            "name",
            "label",
            "label_origin",
            "depth",
            "flags",
            "key_span",
        ],
        "match" => &["literal_kind", "value", "dynamic", "resolution", "raw"],
        "identifier" => &["name", "role", "syntax_kind"],
        "call" => &["callee", "call_kind", "optional", "argument_count"],
        "member" => &["name", "member_kind", "key_resolution", "read", "write"],
        "resource" => &[
            "kind",
            "syntax_kind",
            "name",
            "resolution",
            "unresolved_reason",
            "site_start",
            "site_end",
            "argument_index",
            "spread",
            "optional",
            "type_only",
            "attribute",
            "operator",
            "base",
            "bindings",
        ],
        "value" => &[
            "kind",
            "name",
            "syntax_kind",
            "owner_start",
            "owner_end",
            "index",
            "computed",
            "spread",
            "key_start",
            "key_end",
            "key_resolution",
            "value",
        ],
        "reference" => &[
            "target",
            "read",
            "write",
            "member_write",
            "type_reference",
            "value_reference",
        ],
        "symbol" => &["name", "relation", "space", "declaration_ids"],
        "unresolved_reference" => &["name", "type_reference", "value_reference"],
        "locate" => &[
            "complete",
            "entity",
            "ancestors",
            "syntax",
            "declaration_file",
            "origin",
        ],
        "source" => &[
            "complete",
            "representation",
            "source",
            "entity",
            "symbol",
            "declaration",
            "fragment",
            "syntax",
            "declaration_file",
            "origin",
        ],
        "binding" => &["name", "space", "declaration_ids"],
        "fragment" => &["kind", "entity_id"],
        other => panic!("unexpected JS record: {other}"),
    };
    assert_js_keys(&row["detail"], fields);
    if matches!(
        row["record"].as_str().unwrap(),
        "match" | "reference" | "unresolved_reference"
    ) {
        assert!(row["at"].is_null());
        assert!(row["parent_at"].is_string());
    }
    if matches!(
        row["record"].as_str().unwrap(),
        "entity" | "symbol" | "binding" | "fragment"
    ) {
        assert!(row["at"].is_string());
        assert!(row["context_at"].is_null());
    }
    for field in ["entity", "symbol", "fragment"] {
        if row["detail"][field].is_object() {
            assert_js_record(&row["detail"][field]);
        }
    }
    if let Some(ancestors) = row["detail"]["ancestors"].as_array() {
        for ancestor in ancestors {
            assert_js_record(ancestor);
        }
    }
}

fn js_object(bytes: &[u8]) -> Value {
    let value: Value = serde_json::from_slice(bytes).unwrap();
    if value["backend"] == "js" && matches!(value["command"].as_str(), Some("slice" | "locate")) {
        assert_js_record(&value);
    }
    value
}

fn fixture(source: impl AsRef<[u8]>, filename: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(filename);
    fs::write(&path, source).unwrap();
    (dir, path)
}

fn failure(output: Output, code: i32, diagnostic: &str) {
    // ADR 0015: query-class failures exit 2 even when a call site still passes 1.
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

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/example.js")
}

#[test]
fn syntax_modes_scan_jsx_ts_tsx_and_support_explicit_overrides_without_js_regressions() {
    let dir = tempfile::tempdir().unwrap();
    for (name, source, _syntax, _mode) in [
        ("a.js", "const plain=1;", None, "script"),
        (
            "b.jsx",
            "const view=<div>hello</div>;",
            Some("jsx"),
            "script",
        ),
        (
            "c.ts",
            "interface Item{name:string}; const item:Item={name:'x'};",
            Some("ts"),
            "script",
        ),
        (
            "d.tsx",
            "export const View=(props:{name:string})=><div>{props.name}</div>;",
            Some("tsx"),
            "module",
        ),
        ("e.mts", "export type Value=string;", Some("ts"), "module"),
        ("f.cts", "const value:number=1;", Some("ts"), "script"),
        (
            "g.d.ts",
            "declare function api(value:string):string;",
            Some("ts"),
            "script",
        ),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, source).unwrap();
        let rows = outline_rows(&path, &[]);
        let root = rows[0]["at"].as_str().unwrap();
        let raw = invoke("slice", &path, &["--at", root]);
        success(&raw);
        assert_eq!(raw.stdout, source.as_bytes());
    }
    let output = invoke("entities", dir.path(), &[]);
    success(&output);
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.iter().filter(|row| row["record"] == "file").count(), 7);
    let path = dir.path().join("b.jsx");
    failure(
        invoke("entities", &path, &["--syntax", "js"]),
        1,
        "INVALID_INPUT",
    );
    let path = dir.path().join("c.ts");
    failure(
        invoke("entities", &path, &["--syntax", "js"]),
        1,
        "INVALID_INPUT",
    );
    let (_tmp, path) = fixture("const v=<div/>;", "opaque.txt");
    failure(invoke("entities", &path, &[]), 1, "INVALID_INPUT");
    success(&invoke("entities", &path, &["--syntax", "jsx"]));
    success(&invoke("entities", &path, &["--syntax", "tsx"]));
    let (_tmp, path) = fixture("const id=<T,>(value:T):T=>value;", "generic.tsx");
    let rows = outline_rows(&path, &[]);
    assert_eq!(
        rows.iter()
            .find(|r| r["detail"]["kind"] == "arrow_function")
            .unwrap()["detail"]["label"],
        "id"
    );
    let (_tmp, path) = fixture("const v=<number>value;", "assertion.ts");
    success(&invoke("entities", &path, &[]));
    failure(
        invoke("entities", &path, &["--syntax", "tsx"]),
        1,
        "INVALID_INPUT",
    );
    let (_tmp, path) = fixture("const f=(()=>1) satisfies (()=>number);", "wrapped.ts");
    let rows = outline_rows(&path, &[]);
    assert_eq!(
        rows.iter()
            .find(|r| r["detail"]["kind"] == "arrow_function")
            .unwrap()["detail"]["label"],
        "f"
    );
}

#[test]
fn jsx_search_decodes_entities_and_extracts_original_attributes_text_and_component_references() {
    let source = "\u{feff}// 中文\r\nconst Widget=()=>null; const UI={Widget}; let message='你好'; const view=<Widget title=\"A &amp; B\\n\">Hello &lt; 世界 &#x1F600; &unknown;{message}<UI.Widget/><div data-label=\"label\"/></Widget>;";
    let (_dir, path) = fixture(source, "view.jsx");
    for (query, kind, value, original) in [
        (
            "A & B",
            "jsx_attribute",
            "A & B\\n",
            "title=\"A &amp; B\\n\"",
        ),
        (
            "Hello <",
            "jsx_text",
            "Hello < 世界 😀 &unknown;",
            "Hello &lt; 世界 &#x1F600; &unknown;",
        ),
        (
            "unknown",
            "jsx_text",
            "Hello < 世界 😀 &unknown;",
            "Hello &lt; 世界 &#x1F600; &unknown;",
        ),
    ] {
        let rows = jsonl(&invoke("find", &path, &["-f", query]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["detail"]["literal_kind"], kind);
        assert_eq!(rows[0]["detail"]["value"], value);
        let id = rows[0]["context_at"].as_str().unwrap();
        let raw = invoke("slice", &path, &["--at", id]);
        success(&raw);
        assert_eq!(raw.stdout, original.as_bytes());
        let located = invoke("locate", &path, &["--byte", &rows[0]["start"].to_string()]);
        success(&located);
        assert_eq!(js_object(&located.stdout)["context_at"], id);
    }
    let declaration = source.find("Widget=()").unwrap();
    let rows = jsonl(&invoke(
        "xref",
        &path,
        &["--byte", &declaration.to_string()],
    ));
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .any(|row| row["start"] == source.rfind("Widget>").unwrap())
    );
    for row in &rows[..] {
        assert_eq!(row["detail"]["read"], true);
        assert!(row["detail"]["type_reference"].is_null());
    }
    let ui = source.find("UI={").unwrap();
    let rows = jsonl(&invoke("xref", &path, &["--byte", &ui.to_string()]));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["start"], source.find("UI.Widget").unwrap());
    for marker in ["div data-label", "data-label=", "Widget/>"] {
        failure(
            invoke(
                "xref",
                &path,
                &["--byte", &source.find(marker).unwrap().to_string()],
            ),
            1,
            "NOT_FOUND",
        );
    }
    let symbols = jsonl(&invoke("symbols", &path, &[]));
    let id = symbols
        .iter()
        .find(|r| r["detail"]["name"] == "view")
        .unwrap()["at"]
        .as_str()
        .unwrap();
    let raw = invoke("slice", &path, &["--at", id]);
    success(&raw);
    assert_eq!(
        raw.stdout,
        &source.as_bytes()[source.find("view=<").unwrap()..source.len() - 1]
    );
}

#[test]
fn ts_symbols_references_and_get_preserve_type_value_merges_enums_namespaces_and_generics() {
    let source = "interface Model{name:string} interface Model{age:number} const Model={kind:'model'}; let item:Model={name:'x',age:1}; type Kind=typeof Model; Model.kind; enum E{A=1,B=A+1,\"文字\"=3}; namespace API{export type Item=Model; export const route='/api/login'}; function identity<T extends Model>(value:T):T{return value}";
    let (_dir, path) = fixture(source, "types.ts");
    let symbols = jsonl(&invoke("symbols", &path, &[]));
    let model = symbols
        .iter()
        .find(|r| r["detail"]["name"] == "Model")
        .unwrap();
    assert_eq!(model["detail"]["space"], "type_value");
    assert_eq!(
        model["detail"]["declaration_ids"].as_array().unwrap().len(),
        3
    );
    for (id, expected) in model["detail"]["declaration_ids"]
        .as_array()
        .unwrap()
        .iter()
        .zip([
            "interface Model{name:string}",
            "interface Model{age:number}",
            "Model={kind:'model'}",
        ])
    {
        let raw = invoke("slice", &path, &["--at", id.as_str().unwrap()]);
        success(&raw);
        assert_eq!(raw.stdout, expected.as_bytes());
    }
    let rows = jsonl(&invoke(
        "xref",
        &path,
        &["--byte", &model["start"].to_string()],
    ));
    assert!(
        rows[..]
            .iter()
            .any(|r| r["detail"]["type_reference"] == true && r["detail"]["read"] == false)
    );
    assert!(
        rows[..]
            .iter()
            .any(|r| r["detail"]["value_reference"] == true && r["detail"]["read"] == true)
    );
    for (name, space, expected) in [
        ("Model", "type_value", "interface Model{name:string}"),
        ("Kind", "type", "type Kind=typeof Model;"),
        ("A", "type_value", "A=1"),
        ("文字", "type_value", "\"文字\"=3"),
        ("T", "type", "T extends Model"),
        ("value", "value", "value:T"),
    ] {
        let symbol = symbols
            .iter()
            .find(|r| r["record"] == "symbol" && r["detail"]["name"] == name)
            .unwrap();
        assert_eq!(symbol["detail"]["space"], space);
        let raw = invoke("slice", &path, &["--at", symbol["at"].as_str().unwrap()]);
        success(&raw);
        assert_eq!(raw.stdout, expected.as_bytes());
    }
    let a = symbols.iter().find(|r| r["detail"]["name"] == "A").unwrap();
    let rows = jsonl(&invoke("xref", &path, &["--byte", &a["start"].to_string()]));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["detail"]["value_reference"], true);
    let outlines = outline_rows(&path, &[]);
    let namespace = outlines
        .iter()
        .find(|r| r["detail"]["kind"] == "namespace")
        .unwrap();
    let scoped = jsonl(&invoke(
        "symbols",
        &path,
        &["--within", namespace["at"].as_str().unwrap()],
    ));
    assert_eq!(
        scoped
            .iter()
            .find(|r| r["detail"]["name"] == "Model")
            .unwrap()["detail"]["relation"],
        "external"
    );
    refused(invoke(
        "slice",
        &path,
        &["--at", model["at"].as_str().unwrap(), "--max-records", "1"],
    ));
    let source =
        "import type {Only} from 'lib'; type Missing=typeof missing; Only; let named:Only;";
    let (_tmp, path) = fixture(source, "imports.ts");
    let rows = jsonl(&invoke("symbols", &path, &[]));
    assert_eq!(
        rows.iter()
            .find(|r| r["record"] == "symbol" && r["detail"]["name"] == "Only")
            .unwrap()["detail"]["space"],
        "type"
    );
    let missing = rows
        .iter()
        .find(|r| r["detail"]["name"] == "missing")
        .unwrap();
    assert_eq!(missing["detail"]["type_reference"], true);
    assert_eq!(missing["detail"]["value_reference"], false);
    let value = rows
        .iter()
        .find(|r| r["record"] == "unresolved_reference" && r["detail"]["name"] == "Only")
        .unwrap();
    assert_eq!(value["detail"]["value_reference"], true);
    let refs = jsonl(&invoke(
        "xref",
        &path,
        &["--byte", &source.find("missing;").unwrap().to_string()],
    ));
    assert!(refs.iter().all(|row| row["detail"]["resolution"].is_null()));
}

#[test]
fn typed_ambient_signatures_type_literals_and_source_identity_remain_exact_and_bounded() {
    let source = "\u{feff}declare namespace SDK { interface Options { endpoint: '/api/login'; run<T>(value:T):T; } function send(value:Options):string; function send(value:string):string; }\r\ndeclare module 'legacy' { export type Value=string; }";
    let (dir, path) = fixture(source, "sdk.d.ts");
    let outlines = outline_rows(&path, &[]);
    assert!(
        outlines
            .iter()
            .any(|r| r["detail"]["kind"] == "method_signature")
    );
    assert!(
        outlines
            .iter()
            .any(|r| r["detail"]["kind"] == "external_module")
    );
    let symbols = jsonl(&invoke("symbols", &path, &[]));
    let send = symbols
        .iter()
        .find(|r| r["detail"]["name"] == "send")
        .unwrap();
    assert_eq!(
        send["detail"]["declaration_ids"].as_array().unwrap().len(),
        2
    );
    for id in send["detail"]["declaration_ids"].as_array().unwrap() {
        success(&invoke("slice", &path, &["--at", id.as_str().unwrap()]));
    }
    for symbol in symbols.iter().filter(|r| r["record"] == "symbol") {
        let output = invoke(
            "slice",
            &path,
            &["--at", symbol["at"].as_str().unwrap(), "--json"],
        );
        success(&output);
        let output: Value = js_object(&output.stdout);
        let a = output["detail"]["declaration"]["start"].as_u64().unwrap() as usize;
        let b = output["detail"]["declaration"]["end"].as_u64().unwrap() as usize;
        assert_eq!(
            output["detail"]["source"].as_str().unwrap().as_bytes(),
            &source.as_bytes()[a..b]
        );
        assert_eq!(output["detail"]["syntax"], "ts");
        assert_eq!(output["detail"]["declaration_file"], true);
    }
    let found = jsonl(&invoke("find", &path, &["-f", "/api/login"]));
    assert_eq!(found[0]["detail"]["literal_kind"], "type_string");
    let output = invoke(
        "slice",
        &path,
        &["--at", found[0]["context_at"].as_str().unwrap()],
    );
    success(&output);
    assert_eq!(output.stdout, b"endpoint: '/api/login';");
    let destination = dir.path().join("saved.jsonl");
    fs::write(&destination, "preserved").unwrap();
    refused(invoke(
        "symbols",
        &path,
        &[
            "--max-semantic-nodes",
            "1",
            "-o",
            destination.to_str().unwrap(),
        ],
    ));
    assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    // A coordinate names a span, not an input: the same span of a rewritten file still
    // resolves.
    let id = js_full_id(&found[0], send["at"].as_str().unwrap());
    fs::write(&path, format!("{source}\n// changed")).unwrap();
    success(&invoke("slice", &path, &["--at", &id]));
    let (_tmp, path) = fixture("type Route=`/api/${'login'|'logout'}`;", "template.ts");
    let rows = jsonl(&invoke("find", &path, &["-f", "/api/"]));
    assert_eq!(rows[0]["detail"]["literal_kind"], "type_template");
    assert_eq!(rows[0]["detail"]["dynamic"], true);
}

#[test]
fn published_types_and_jsx_tsx_sources_match_independent_babel_expectations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/typed");
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let refs_oracle: Value =
        serde_json::from_slice(&fs::read(root.join("refs-expectations.json")).unwrap()).unwrap();
    let contexts: Value =
        serde_json::from_slice(&fs::read(root.join("contexts-expectations.json")).unwrap())
            .unwrap();
    assert_eq!(
        manifest["generator"]["sha256"],
        format!(
            "{:x}",
            Sha256::digest(fs::read(root.join("../generate-typed.mjs")).unwrap())
        )
    );
    assert_eq!(manifest["generator"]["babel"], "7.29.9");
    for case in manifest["cases"].as_array().unwrap() {
        let path = root.join(case["file"].as_str().unwrap());
        let bytes = fs::read(&path).unwrap();
        assert_eq!(case["sha256"], format!("{:x}", Sha256::digest(&bytes)));
        let outline = outline_rows(&path, &[]);
        let entities = &outline[..];
        assert_eq!(
            entities.len(),
            case["expected"]["entity_count"].as_u64().unwrap() as usize,
            "{}",
            case["name"]
        );
        let by_id: std::collections::HashMap<_, _> = entities
            .iter()
            .map(|e| (e["at"].as_str().unwrap(), e))
            .collect();
        let rows: Vec<_> = entities
            .iter()
            .map(|e| {
                serde_json::json!([
                    e["detail"]["kind"],
                    e["start"],
                    e["end"],
                    e["parent_at"]
                        .as_str()
                        .map(|id| entity_reference(by_id[id])),
                    e["detail"]["depth"],
                    e["detail"]["name"],
                    e["detail"]["flags"]["async"],
                    e["detail"]["flags"]["generator"],
                    e["detail"]["flags"]["static"]
                ])
            })
            .collect();
        assert_eq!(
            format!("{:x}", Sha256::digest(serde_json::to_vec(&rows).unwrap())),
            case["expected"]["structure_sha256"],
            "{}",
            case["name"]
        );
        for entity in case["expected"]["sample_entities"].as_array().unwrap() {
            let record = entities
                .iter()
                .find(|e| {
                    entity_reference(e)
                        == serde_json::json!([entity["kind"], entity["start"], entity["end"]])
                })
                .unwrap();
            let raw = invoke("slice", &path, &["--at", record["at"].as_str().unwrap()]);
            success(&raw);
            assert_eq!(
                raw.stdout,
                bytes[entity["start"].as_u64().unwrap() as usize
                    ..entity["end"].as_u64().unwrap() as usize]
            );
        }
        let found = jsonl(&invoke(
            "find",
            &path,
            &["-f", case["needle"].as_str().unwrap()],
        ));
        let actual:Vec<_>=found[..].iter().map(|m|serde_json::json!({"literal_kind":m["detail"]["literal_kind"],"value":m["detail"]["value"],"start":m["start"],"end":m["end"],"dynamic":m["detail"]["dynamic"],"entity":entity_reference(by_id[m["parent_at"].as_str().unwrap()])})).collect();
        assert_eq!(
            serde_json::json!(actual),
            case["expected"]["matches"],
            "{}",
            case["name"]
        );
        let contexts = contexts["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|q| q["name"] == case["name"])
            .unwrap();
        let mut context_kinds = std::collections::HashSet::new();
        for (row, expected) in found[..]
            .iter()
            .zip(contexts["matches"].as_array().unwrap())
        {
            if expected["context"].is_null() {
                assert!(row["context_at"].is_null());
            } else {
                let c = &expected["context"];
                assert_eq!(
                    row["context_at"],
                    format!(
                        "fragment.{}:{}:{}",
                        c["kind"].as_str().unwrap(),
                        c["start"],
                        c["end"]
                    )
                );
            }
            if !context_kinds.insert(expected["context"]["kind"].as_str().unwrap_or("structure")) {
                continue;
            }
            let output = invoke("locate", &path, &["--byte", &row["start"].to_string()]);
            success(&output);
            assert_eq!(js_object(&output.stdout)["context_at"], row["context_at"]);
            if expected["context"].is_null() {
                assert!(row["context_at"].is_null());
                continue;
            }
            let context = &expected["context"];
            let expected_id = format!(
                "fragment.{}:{}:{}",
                context["kind"].as_str().unwrap(),
                context["start"],
                context["end"]
            );
            assert_eq!(row["context_at"], expected_id);
            let raw = invoke("slice", &path, &["--at", &expected_id]);
            success(&raw);
            assert_eq!(
                raw.stdout,
                bytes[context["start"].as_u64().unwrap() as usize
                    ..context["end"].as_u64().unwrap() as usize]
            );
        }
        let query = refs_oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|q| q["name"] == case["name"])
            .unwrap();
        let refs = jsonl(&invoke(
            "xref",
            &path,
            &["--byte", &query["byte"].to_string()],
        ));
        let actual:Vec<_>=refs[..].iter().map(|r|{let mut value=serde_json::json!({"start":r["start"],"end":r["end"],"read":r["detail"]["read"],"write":r["detail"]["write"],"member_write":r["detail"]["member_write"]});if let Some(t)=r["detail"].get("type_reference").filter(|v| !v.is_null()){value["type_reference"]=t.clone();value["value_reference"]=r["detail"]["value_reference"].clone();}value}).collect();
        assert_eq!(
            serde_json::json!(actual),
            query["references"],
            "{}",
            case["name"]
        );
        let symbols = jsonl(&invoke("symbols", &path, &[]));
        assert!(!symbols.is_empty());
    }
}

#[test]
fn local_contexts_extract_properties_fields_and_assignments_without_large_containers() {
    let padding = "/* unrelated */".repeat(5000);
    let source = format!(
        "\u{feff}// 中文\r\nconst config={{endpoint:'/api/local',retry:3}}; class Settings{{endpoint='/api/field'; method(){{{padding}return this.endpoint;}}}}\nfunction boot(){{config={{endpoint:'/api/write',retry:4}}; {padding}config.retry++; return config;}}"
    );
    let (_dir, path) = fixture(&source, "contexts.js");
    for (needle, expected, kind) in [
        ("/api/local", "endpoint:'/api/local'", "object_property"),
        ("/api/field", "endpoint='/api/field';", "class_field"),
        ("/api/write", "endpoint:'/api/write'", "object_property"),
    ] {
        let rows = jsonl(&invoke("find", &path, &["-f", needle]));
        let found = &rows[0];
        let id = found["context_at"].as_str().unwrap();
        let raw = invoke("slice", &path, &["--at", id]);
        success(&raw);
        assert_eq!(raw.stdout, expected.as_bytes());
        let located = invoke("locate", &path, &["--byte", &found["start"].to_string()]);
        success(&located);
        let located: Value = js_object(&located.stdout);
        assert_eq!(located["context_at"], id);
        let full = invoke(
            "slice",
            &path,
            &["--at", found["parent_at"].as_str().unwrap()],
        );
        success(&full);
        assert!(full.stdout.len() > expected.len() * 1000);
        let output = invoke("slice", &path, &["--at", id, "--json"]);
        success(&output);
        let output: Value = js_object(&output.stdout);
        assert_eq!(output["detail"]["representation"], "original");
        assert_eq!(output["detail"]["source"], expected);
        assert_eq!(output["detail"]["fragment"]["detail"]["kind"], kind);
        assert_eq!(
            output["detail"]["fragment"]["detail"]["entity_id"],
            found["parent_at"]
        );
        let a = output["detail"]["fragment"]["start"].as_u64().unwrap() as usize;
        let b = output["detail"]["fragment"]["end"].as_u64().unwrap() as usize;
        assert_eq!(&source.as_bytes()[a..b], raw.stdout);
        if needle == "/api/local" {
            let parent = invoke(
                "slice",
                &path,
                &[
                    "--at",
                    output["detail"]["fragment"]["parent_at"].as_str().unwrap(),
                    "--json",
                ],
            );
            success(&parent);
            let parent: Value = js_object(&parent.stdout);
            assert_eq!(
                parent["detail"]["source"],
                "config={endpoint:'/api/local',retry:3}"
            );
            let declaration = invoke(
                "slice",
                &path,
                &[
                    "--at",
                    parent["detail"]["fragment"]["parent_at"].as_str().unwrap(),
                ],
            );
            success(&declaration);
            assert_eq!(
                declaration.stdout,
                b"const config={endpoint:'/api/local',retry:3};"
            );
        }
    }
    let rows = jsonl(&invoke(
        "xref",
        &path,
        &["--byte", &source.find("config={").unwrap().to_string()],
    ));
    for (flag, expected, kind) in [
        (
            "write",
            "config={endpoint:'/api/write',retry:4}",
            "assignment_expression",
        ),
        ("member_write", "config.retry++", "update_expression"),
    ] {
        let reference = rows
            .iter()
            .find(|row| row["record"] == "reference" && row["detail"][flag] == true)
            .unwrap();
        let id = reference["context_at"].as_str().unwrap();
        let raw = invoke("slice", &path, &["--at", id]);
        success(&raw);
        assert_eq!(raw.stdout, expected.as_bytes());
        let output = invoke("slice", &path, &["--at", id, "--json"]);
        success(&output);
        let output: Value = js_object(&output.stdout);
        assert_eq!(output["detail"]["fragment"]["detail"]["kind"], kind);
        let located = invoke(
            "locate",
            &path,
            &["--byte", &reference["start"].to_string()],
        );
        success(&located);
        assert_eq!(js_object(&located.stdout)["context_at"], id);
    }
}

#[test]
fn local_contexts_preserve_function_boundaries_parameters_templates_and_control_syntax() {
    for (source, needle, expected) in [
        (
            "const x='/api/simple';",
            "/api/simple",
            Some("x='/api/simple'"),
        ),
        (
            "function f(x='/api/default'){return x}",
            "/api/default",
            Some("x='/api/default'"),
        ),
        (
            "function f(){if(flag){throw new Error('/api/error')}}",
            "/api/error",
            Some("throw new Error('/api/error')"),
        ),
        (
            "function f(){if(test('/api/condition')){work()}}",
            "/api/condition",
            Some("if(test('/api/condition')){work()}"),
        ),
        (
            "const x=`/api/${value}/end`;",
            "/api/",
            Some("x=`/api/${value}/end`"),
        ),
        (
            "import x from '/api/module';",
            "/api/module",
            Some("import x from '/api/module';"),
        ),
        (
            "const obj={['/api/key']:1};",
            "/api/key",
            Some("['/api/key']:1"),
        ),
        ("const f=()=>'/api/arrow';", "/api/arrow", None),
        (
            "const obj={method(){return '/api/method'}};",
            "/api/method",
            Some("return '/api/method'"),
        ),
        (
            "const f=function(){return '/api/function'};",
            "/api/function",
            Some("return '/api/function'"),
        ),
        ("'/api/directive';", "/api/directive", None),
    ] {
        let (_dir, path) = fixture(source, "syntax.js");
        let rows = jsonl(&invoke("find", &path, &["-f", needle]));
        let found = &rows[0];
        assert_eq!(
            found["context_at"].is_null(),
            expected.is_none(),
            "{source}"
        );
        let output = invoke("locate", &path, &["--byte", &found["start"].to_string()]);
        success(&output);
        let located: Value = js_object(&output.stdout);
        assert_eq!(located["context_at"], found["context_at"], "{source}");
        if let Some(expected) = expected {
            let raw = invoke(
                "slice",
                &path,
                &["--at", found["context_at"].as_str().unwrap()],
            );
            success(&raw);
            assert_eq!(raw.stdout, expected.as_bytes());
        }
    }
    let source = "const f=()=>f();";
    let (_dir, path) = fixture(source, "arrow.js");
    let rows = jsonl(&invoke("xref", &path, &["--byte", "6"]));
    assert!(rows[0]["context_at"].is_null());
}

#[test]
fn local_context_ids_are_content_bound_parser_only_bounded_and_atomic() {
    let source = "const obj={endpoint:'/api/check'}; function later(){}";
    let (dir, path) = fixture(source, "check.js");
    let found = jsonl(&invoke("find", &path, &["-f", "/api/check"]));
    let id = found[0]["context_at"].as_str().unwrap();
    let destination = dir.path().join("context.js");
    let written = invoke(
        "slice",
        &path,
        &["--at", id, "-o", destination.to_str().unwrap()],
    );
    success(&written);
    assert!(written.stdout.is_empty());
    assert_eq!(fs::read(&destination).unwrap(), b"endpoint:'/api/check'");
    for flag in [
        "--max-output-bytes",
        "--max-input-bytes",
        "--max-entities",
        "--max-ast-depth",
    ] {
        fs::write(&destination, "preserved").unwrap();
        refused(invoke(
            "slice",
            &path,
            &["--at", id, flag, "1", "-o", destination.to_str().unwrap()],
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
    for forged in [
        id.replace("fragment.object_property", "fragment.block_statement"),
        format!("{id}:extra"),
        "fragment.object_property:0:1".to_owned(),
        "fragment.object_property:011:32".to_owned(),
        "fragment.object_property:32:11".to_owned(),
    ] {
        refused(invoke("slice", &path, &["--at", &forged]));
    }
    for command in ["entities", "symbols"] {
        refused(invoke(command, &path, &["--within", id]));
    }
    for (command, args) in [
        ("find", vec!["-f", "/api/check"]),
        ("locate", vec!["--byte", "21"]),
        ("slice", vec!["--at", id]),
    ] {
        let plain = invoke(command, &path, &args);
        success(&plain);
        let mut args = args;
        args.push("--perf");
        let output = invoke(command, &path, &args);
        success(&output);
        assert_eq!(plain.stdout, output.stdout);
        assert!(
            perf_reports(&output)[0]["stages_ns"]
                .get("semantic")
                .is_none()
        );
    }
    let copy = path.with_file_name("copy.js");
    fs::copy(&path, &copy).unwrap();
    assert_eq!(
        invoke("slice", &copy, &["--at", id]).stdout,
        b"endpoint:'/api/check'"
    );
    // Rewriting the file keeps the span, so the coordinate still names it.
    fs::write(&path, "const obj={changed:true};").unwrap();
    refused(invoke(
        "slice",
        &path,
        &["--at", "fragment.object_property:12:20"],
    ));
    let path = dir.path().join("late.js");
    fs::write(&path, "const =;").unwrap();
    failure(
        invoke(
            "find",
            dir.path(),
            &["-f", "/api/check", "-o", destination.to_str().unwrap()],
        ),
        1,
        "INVALID_INPUT",
    );
    assert_eq!(fs::read(&destination).unwrap(), b"preserved");
}

#[test]
fn local_contexts_match_independent_acorn_literals_references_and_parent_ranges() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let oracle: Value =
        serde_json::from_slice(&fs::read(root.join("contexts-expectations.json")).unwrap())
            .unwrap();
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(oracle["contexts_version"], 1);
    assert_eq!(
        oracle["generator"]["sha256"],
        format!(
            "{:x}",
            Sha256::digest(fs::read(root.join("generate-contexts.mjs")).unwrap())
        )
    );
    assert_eq!(
        oracle["generator"]["refs_sha256"],
        format!(
            "{:x}",
            Sha256::digest(fs::read(root.join("refs-expectations.json")).unwrap())
        )
    );
    for case in oracle["cases"].as_array().unwrap() {
        let path = root.join(case["file"].as_str().unwrap());
        let bytes = fs::read(&path).unwrap();
        assert_eq!(case["source_id"], rxd_core::identity::input_id(&bytes));
        let input = manifest["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|input| input["name"] == case["name"])
            .unwrap();
        let found = jsonl(&invoke(
            "find",
            &path,
            &["-f", input["needle"].as_str().unwrap()],
        ));
        let refs = jsonl(&invoke(
            "xref",
            &path,
            &["--byte", &case["selected"]["start"].to_string()],
        ));
        let checks = found[..]
            .iter()
            .zip(case["matches"].as_array().unwrap())
            .chain(refs[..].iter().zip(case["references"].as_array().unwrap()));
        assert_eq!(found.len(), case["matches"].as_array().unwrap().len());
        assert_eq!(refs.len(), case["references"].as_array().unwrap().len());
        for (row, expected) in checks {
            assert_eq!(row["start"], expected["start"]);
            assert_eq!(row["end"], expected["end"]);
            let located = invoke("locate", &path, &["--byte", &row["start"].to_string()]);
            success(&located);
            let located: Value = js_object(&located.stdout);
            assert_eq!(located["context_at"], row["context_at"]);
            if expected["context"].is_null() {
                assert!(row["context_at"].is_null());
                continue;
            }
            let context = &expected["context"];
            let id = format!(
                "fragment.{}:{}:{}",
                context["kind"].as_str().unwrap(),
                context["start"],
                context["end"]
            );
            assert_eq!(row["context_at"], id);
            let raw = invoke("slice", &path, &["--at", &id]);
            success(&raw);
            let a = context["start"].as_u64().unwrap() as usize;
            let b = context["end"].as_u64().unwrap() as usize;
            assert_eq!(raw.stdout, bytes[a..b]);
            let output = invoke("slice", &path, &["--at", &id, "--json"]);
            success(&output);
            let output: Value = js_object(&output.stdout);
            assert_eq!(
                output["detail"]["fragment"]["detail"]["entity_id"],
                *row.get("parent_at").unwrap_or(&row["entity_id"])
            );
            assert_eq!(
                output["detail"]["source"].as_str().unwrap().as_bytes(),
                &bytes[a..b]
            );
            if context["parent"].is_null() {
                assert!(output["detail"]["fragment"]["parent_at"].is_null());
            } else {
                let parent = &context["parent"];
                let id = format!(
                    "fragment.{}:{}:{}",
                    parent["kind"].as_str().unwrap(),
                    parent["start"],
                    parent["end"]
                );
                assert_eq!(output["detail"]["fragment"]["parent_at"], id);
                let raw = invoke("slice", &path, &["--at", &id]);
                success(&raw);
                assert_eq!(
                    raw.stdout,
                    bytes[parent["start"].as_u64().unwrap() as usize
                        ..parent["end"].as_u64().unwrap() as usize]
                );
            }
        }
    }
}

#[test]
fn get_symbol_extracts_declarators_without_siblings_or_large_enclosing_functions() {
    let padding = "/* unrelated */".repeat(5000);
    let source = format!(
        "\u{feff}// 中文\r\n(function(){{const ignored='x', config /* 保留 */ = {{url:'/api/login', nested:{{x:1}}}}, another=2;\r\nconst {{key: renamed = 3, ...rest}}=config;\nlet 登录=1; const \\u0061=(2); function nested(){{const config={{inner:true}}; return config}}\n{padding}\nreturn config}})();"
    );
    let (_dir, path) = fixture(&source, "definitions.js");
    let rows = jsonl(&invoke("symbols", &path, &[]));
    for (marker, name, expected, variable_kind, initializer) in [
        (
            "config /*",
            "config",
            "config /* 保留 */ = {url:'/api/login', nested:{x:1}}",
            "const",
            Some("{url:'/api/login', nested:{x:1}}"),
        ),
        (
            "renamed =",
            "renamed",
            "{key: renamed = 3, ...rest}=config",
            "const",
            Some("config"),
        ),
        (
            "rest}",
            "rest",
            "{key: renamed = 3, ...rest}=config",
            "const",
            Some("config"),
        ),
        ("登录=", "登录", "登录=1", "let", Some("1")),
        ("\\u0061=", "a", "\\u0061=(2)", "const", Some("2")),
        (
            "config={inner",
            "config",
            "config={inner:true}",
            "const",
            Some("{inner:true}"),
        ),
    ] {
        let start = source.find(marker).unwrap();
        let symbol = rows
            .iter()
            .find(|r| r["record"] == "symbol" && r["start"] == start)
            .unwrap();
        let id = symbol["at"].as_str().unwrap();
        let raw = invoke("slice", &path, &["--at", id]);
        success(&raw);
        assert_eq!(raw.stdout, expected.as_bytes());
        let output = invoke("slice", &path, &["--at", id, "--json"]);
        success(&output);
        let extracted: Value = js_object(&output.stdout);
        assert_eq!(extracted["detail"]["representation"], "original");
        assert!(extracted["detail"]["entity"].is_null());
        assert_eq!(extracted["detail"]["symbol"]["at"], id);
        assert_eq!(extracted["detail"]["symbol"]["detail"]["name"], name);
        for field in ["start", "end", "entity_id"] {
            assert_eq!(extracted["detail"]["symbol"][field], symbol[field]);
        }
        assert_eq!(extracted["detail"]["source"], expected);
        let declaration = &extracted["detail"]["declaration"];
        assert_eq!(declaration["kind"], "variable_declarator");
        assert_eq!(declaration["variable_kind"], variable_kind);
        let a = declaration["start"].as_u64().unwrap() as usize;
        let b = declaration["end"].as_u64().unwrap() as usize;
        assert_eq!(&source.as_bytes()[a..b], raw.stdout);
        if let Some(initializer) = initializer {
            let a = declaration["initializer"]["start"].as_u64().unwrap() as usize;
            let b = declaration["initializer"]["end"].as_u64().unwrap() as usize;
            // Parentheses may belong to the initializer node's span.
            assert_eq!(source[a..b].trim_matches(['(', ')']), initializer);
        }
        let containing = invoke(
            "slice",
            &path,
            &["--at", symbol["parent_at"].as_str().unwrap()],
        );
        success(&containing);
        assert!(containing.stdout.len() > raw.stdout.len());
        if marker == "config /*" {
            assert!(containing.stdout.len() > raw.stdout.len() * 1000);
        }
    }
}

#[test]
fn get_symbol_preserves_complete_patterns_parameters_imports_and_canonical_declarations() {
    for (source, name, expected, kind, variable_kind) in [
        (
            "let x; x=123;",
            "x",
            "x",
            "variable_declarator",
            Some("let"),
        ),
        (
            "var x=1; var x=2; x=3;",
            "x",
            "x=1",
            "variable_declarator",
            Some("var"),
        ),
        (
            "function f(x){var x=2; return x}",
            "x",
            "x",
            "parameter",
            None,
        ),
        (
            "const [a, {b: local=2}, ...tail]=input;",
            "local",
            "[a, {b: local=2}, ...tail]=input",
            "variable_declarator",
            Some("const"),
        ),
        (
            "function f({key: x=1, ...rest}={key:2}, other=3){return rest}",
            "rest",
            "{key: x=1, ...rest}={key:2}",
            "parameter",
            None,
        ),
        (
            "function f(...[first,...tail]){}",
            "tail",
            "...[first,...tail]",
            "rest_parameter",
            None,
        ),
        (
            "try{}catch({message: reason, ...rest}){consume(reason)}",
            "reason",
            "{message: reason, ...rest}",
            "catch_parameter",
            None,
        ),
        (
            "import defaultName,{remote as local} from 'mod';",
            "local",
            "import defaultName,{remote as local} from 'mod';",
            "import_declaration",
            None,
        ),
        (
            "import * as ns from 'mod';",
            "ns",
            "import * as ns from 'mod';",
            "import_declaration",
            None,
        ),
        (
            "export const config={a:1};",
            "config",
            "config={a:1}",
            "variable_declarator",
            Some("const"),
        ),
        (
            "export default function named(){}",
            "named",
            "function named(){}",
            "function_declaration",
            None,
        ),
        (
            "const f=function inner(){return inner};",
            "inner",
            "function inner(){return inner}",
            "function_expression",
            None,
        ),
        (
            "class C{method(){}}",
            "C",
            "class C{method(){}}",
            "class_declaration",
            None,
        ),
        (
            "const value=class Named{};",
            "Named",
            "class Named{}",
            "class_expression",
            None,
        ),
        (
            "for(const {key: item} of items){consume(item)}",
            "item",
            "{key: item}",
            "variable_declarator",
            Some("const"),
        ),
        (
            "for(let i=0,j=1;i<2;i++){}",
            "i",
            "i=0",
            "variable_declarator",
            Some("let"),
        ),
    ] {
        let (_dir, path) = fixture(source, "bindings.js");
        let rows = jsonl(&invoke("symbols", &path, &[]));
        let id = rows
            .iter()
            .find(|r| r["record"] == "symbol" && r["detail"]["name"] == name)
            .unwrap()["at"]
            .as_str()
            .unwrap();
        let raw = invoke("slice", &path, &["--at", id]);
        success(&raw);
        assert_eq!(raw.stdout, expected.as_bytes(), "{source}");
        let output = invoke("slice", &path, &["--at", id, "--json"]);
        success(&output);
        let extracted: Value = js_object(&output.stdout);
        assert_eq!(extracted["detail"]["declaration"]["kind"], kind);
        assert_eq!(
            extracted["detail"]["declaration"]["variable_kind"].as_str(),
            variable_kind
        );
        let a = extracted["detail"]["declaration"]["start"]
            .as_u64()
            .unwrap() as usize;
        let b = extracted["detail"]["declaration"]["end"].as_u64().unwrap() as usize;
        assert_eq!(&source.as_bytes()[a..b], raw.stdout);
        if source.starts_with("function f({") {
            let init = &extracted["detail"]["declaration"]["initializer"];
            assert_eq!(
                &source[init["start"].as_u64().unwrap() as usize
                    ..init["end"].as_u64().unwrap() as usize],
                "{key:2}"
            );
        }
        if source.starts_with("let x;") {
            assert!(extracted["detail"]["declaration"]["initializer"].is_null());
        }
    }
}

#[test]
fn get_symbol_published_and_obfuscated_declarations_match_independent_acorn_ranges() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let oracle: Value =
        serde_json::from_slice(&fs::read(root.join("definitions-expectations.json")).unwrap())
            .unwrap();
    assert_eq!(oracle["definitions_version"], 1);
    assert_eq!(
        oracle["generator"]["sha256"],
        format!(
            "{:x}",
            Sha256::digest(fs::read(root.join("generate-definitions.mjs")).unwrap())
        )
    );
    for case in oracle["cases"].as_array().unwrap() {
        let path = root.join(case["file"].as_str().unwrap());
        let source = fs::read(&path).unwrap();
        assert_eq!(case["source_id"], rxd_core::identity::input_id(&source));
        let id = format!(
            "symbol:{}:{}",
            case["identifier"]["start"], case["identifier"]["end"]
        );
        let symbols = jsonl(&invoke("symbols", &path, &[]));
        let symbol = symbols.iter().find(|r| r["at"] == id).unwrap();
        assert_eq!(symbol["detail"]["name"], case["binding_name"]);
        let output = invoke("slice", &path, &["--at", &id, "--json"]);
        success(&output);
        let extracted: Value = js_object(&output.stdout);
        assert_eq!(
            extracted["detail"]["declaration"], case["declaration"],
            "{}",
            case["name"]
        );
        assert_eq!(
            extracted["detail"]["symbol"]["start"],
            case["identifier"]["start"]
        );
        assert_eq!(
            extracted["detail"]["symbol"]["end"],
            case["identifier"]["end"]
        );
        assert_eq!(
            extracted["detail"]["symbol"]["parent_at"],
            symbol["parent_at"]
        );
        let a = case["declaration"]["start"].as_u64().unwrap() as usize;
        let b = case["declaration"]["end"].as_u64().unwrap() as usize;
        let raw = invoke("slice", &path, &["--at", &id]);
        success(&raw);
        assert_eq!(raw.stdout, source[a..b]);
        assert_eq!(
            extracted["detail"]["source"].as_str().unwrap().as_bytes(),
            &source[a..b]
        );
        let containing = invoke(
            "slice",
            &path,
            &["--at", symbol["parent_at"].as_str().unwrap()],
        );
        success(&containing);
        assert!(containing.stdout.len() > raw.stdout.len());
    }
}

#[test]
fn get_symbol_validates_identity_budgets_and_atomic_outputs_without_changing_structure_get() {
    let source =
        "const config={url:'/api/login'}; var again=1; var again=2; function f(){return config}";
    let (dir, path) = fixture(source, "binding.js");
    let rows = jsonl(&invoke("symbols", &path, &[]));
    let symbol = &rows[0];
    let id = symbol["at"].as_str().unwrap();
    let expected = b"config={url:'/api/login'}";
    let destination = dir.path().join("definition.js");
    let written = invoke(
        "slice",
        &path,
        &["--at", id, "-o", destination.to_str().unwrap()],
    );
    success(&written);
    assert!(written.stdout.is_empty());
    assert_eq!(fs::read(&destination).unwrap(), expected);
    for flag in [
        "--max-semantic-nodes",
        "--max-output-bytes",
        "--max-input-bytes",
        "--max-entities",
        "--max-ast-depth",
    ] {
        fs::write(&destination, "preserved").unwrap();
        refused(invoke(
            "slice",
            &path,
            &["--at", id, flag, "1", "-o", destination.to_str().unwrap()],
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
    for (a, b) in [
        (0, source.len()),
        (6, 7),
        (30, 35),
        (
            source.find("again=2").unwrap(),
            source.find("again=2").unwrap() + 5,
        ),
        (usize::MAX, usize::MAX),
    ] {
        refused(invoke(
            "slice",
            &path,
            &["--at", &format!("symbol:{a}:{b}")],
        ));
    }
    for forged in [
        id.replace("symbol:", "variable:"),
        format!("{id}:extra"),
        "symbol:06:12".to_owned(),
        "symbol:12:6".to_owned(),
    ] {
        refused(invoke("slice", &path, &["--at", &forged]));
    }
    for command in ["entities", "symbols"] {
        refused(invoke(command, &path, &["--within", id]));
    }
    let profile = invoke("slice", &path, &["--at", id, "--perf"]);
    success(&profile);
    assert_eq!(profile.stdout, expected);
    let reports = perf_reports(&profile);
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["counters"]["records"], 1);
    assert!(reports[0]["stages_ns"]["semantic"].as_u64().unwrap() > 0);
    // The semantic index is still built on demand and still counted; its ceiling is not a
    // CLI knob, so the test asserts the reported work instead of tuning it.
    assert!(reports[0]["counters"]["semantic_nodes"].as_u64().unwrap() > 0);
    let legacy = invoke(
        "slice",
        &path,
        &["--at", symbol["parent_at"].as_str().unwrap(), "--perf"],
    );
    success(&legacy);
    assert_eq!(legacy.stdout, source.as_bytes());
    assert!(
        perf_reports(&legacy)[0]["stages_ns"]
            .get("semantic")
            .is_none()
    );
    let copied = path.with_file_name("copy.js");
    fs::copy(&path, &copied).unwrap();
    assert_eq!(invoke("slice", &copied, &["--at", id]).stdout, expected);
    fs::write(&path, "const config={changed:true};").unwrap();
    // The coordinate is file-scoped: 6:12 is a real binding in the rewritten file.
    let changed = invoke("slice", &path, &["--at", "symbol:6:12"]);
    success(&changed);
    assert_eq!(changed.stdout, b"config={changed:true}");
    // Syntax extraction remains available in a dynamic file; it claims no runtime value.
    let dynamic = "let x=1; eval('x=2');";
    fs::write(&path, dynamic).unwrap();
    let extracted = invoke("slice", &path, &["--at", "symbol:4:5"]);
    success(&extracted);
    assert_eq!(extracted.stdout, b"x=1");
    fs::write(&path, "let x; let x;").unwrap();
    failure(
        invoke("slice", &path, &["--at", "symbol:4:5"]),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn symbols_discovers_distinct_bindings_with_content_ids_and_original_spans() {
    let source = "\u{feff}// 中文\r\nimport {remote as alias} from 'mod';\r\nvar repeat=1; var repeat=2;\nconst {key: local, shorthand, ...rest}=alias;\nlet 登录=1; const \\u0061=2;\nfunction outer(local, {item}, ...args){let local2; {let local=3; local;} try{}catch(error){error;} return ()=>local+登录;}\nconst f=function inner(){return inner}; class C{method(p){return p}}\n";
    let (_dir, path) = fixture(source, "symbols.mjs");
    let rows = jsonl(&invoke("symbols", &path, &[]));
    let expected = [
        ("alias} from", "alias"),
        ("repeat=1", "repeat"),
        ("local, shorthand", "local"),
        ("shorthand,", "shorthand"),
        ("rest}", "rest"),
        ("登录=1", "登录"),
        ("\\u0061=2", "a"),
        ("outer(local", "outer"),
        ("local, {item}", "local"),
        ("item}", "item"),
        ("args)", "args"),
        ("local2;", "local2"),
        ("local=3", "local"),
        ("error)", "error"),
        ("f=function", "f"),
        ("inner(){", "inner"),
        ("C{", "C"),
        ("p){", "p"),
    ];
    assert_eq!(rows.len(), expected.len());
    let mut ids = std::collections::HashSet::new();
    for (row, (marker, name)) in rows[..].iter().zip(expected) {
        let start = source.find(marker).unwrap();
        let raw = if name == "a" { "\\u0061" } else { name };
        assert_eq!(row["record"], "symbol");
        assert_eq!(row["detail"]["name"], name);
        assert_eq!(row["detail"]["relation"], "declared");
        assert_eq!(row["start"], start);
        assert_eq!(row["end"], start + raw.len());
        assert_eq!(row["at"], format!("symbol:{start}:{}", start + raw.len()));
        assert!(ids.insert(row["at"].clone().to_string()));
        let extracted = invoke(
            "slice",
            &path,
            &["--at", row["parent_at"].as_str().unwrap(), "--json"],
        );
        success(&extracted);
        let extracted: Value = js_object(&extracted.stdout);
        assert!(extracted["detail"]["entity"]["start"].as_u64().unwrap() <= start as u64);
        assert!(
            extracted["detail"]["entity"]["end"].as_u64().unwrap() >= (start + raw.len()) as u64
        );
    }
    failure(
        invoke("symbols", &path, &["--jsonl"]),
        2,
        "unexpected argument",
    );
    let duplicate = path.with_file_name("duplicate.mjs");
    fs::copy(&path, &duplicate).unwrap();
    assert_eq!(jsonl(&invoke("symbols", &duplicate, &[]))[1..], rows[1..]);
}

#[test]
fn symbols_within_includes_nested_declarations_and_external_bindings_without_name_merging() {
    let source = "let x=0; let unused=1; function outer(x){const inner=()=>{let y=x; const nested=()=>x+y+host+host; return nested}; return inner;} const recur=()=>recur; var repeated; const redeclare=()=>{var repeated; return repeated}; function twice(){} function twice(){return twice};";
    let (_dir, path) = fixture(source, "closures.js");
    let entities = outline_rows(&path, &[]);
    let id = entities
        .iter()
        .find(|row| row["detail"]["label"] == "inner")
        .unwrap()["at"]
        .as_str()
        .unwrap();
    let rows = jsonl(&invoke("symbols", &path, &["--within", id]));
    let records = &rows[..];
    assert_eq!(records.len(), 5);
    assert_eq!(
        records
            .iter()
            .map(|r| r["detail"]["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["x", "y", "nested", "host", "host"]
    );
    assert_eq!(records[0]["detail"]["relation"], "external");
    assert_eq!(records[0]["start"], source.find("outer(x)").unwrap() + 6);
    assert_eq!(records[1]["detail"]["relation"], "declared");
    assert_eq!(records[2]["detail"]["relation"], "declared");
    for row in &records[3..] {
        assert_eq!(row["record"], "unresolved_reference");
        assert!(row["at"].is_null());
        assert!(row["detail"].get("relation").is_none());
        assert_eq!(
            row["end"].as_u64().unwrap() - row["start"].as_u64().unwrap(),
            4
        );
    }
    // A redeclaration inside the region can have its canonical declaration outside it.
    let second = entities
        .iter()
        .rfind(|row| row["detail"]["name"] == "twice")
        .unwrap();
    let repeated = jsonl(&invoke(
        "symbols",
        &path,
        &["--within", second["at"].as_str().unwrap()],
    ));
    assert_eq!(repeated.len(), 1);
    assert_eq!(repeated[0]["detail"]["relation"], "declared");
    assert_eq!(repeated[0]["start"], source.find("twice(){}").unwrap());
    assert!(repeated[0]["start"].as_u64().unwrap() < second["start"].as_u64().unwrap());
    let all = jsonl(&invoke("symbols", &path, &[]));
    let parameter = all
        .iter()
        .find(|r| r["start"] == records[0]["start"])
        .unwrap();
    assert_eq!(parameter["at"], records[0]["at"]);
    assert_eq!(parameter["detail"]["relation"], "declared");
    for (label, name, relation) in [
        ("recur", "recur", "external"),
        ("redeclare", "repeated", "declared"),
    ] {
        let id = entities
            .iter()
            .find(|row| row["detail"]["label"] == label)
            .unwrap()["at"]
            .as_str()
            .unwrap();
        let rows = jsonl(&invoke("symbols", &path, &["--within", id]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["detail"]["name"], name);
        assert_eq!(rows[0]["detail"]["relation"], relation);
    }
    let program_id = entities[0]["at"].as_str().unwrap();
    let explicit = jsonl(&invoke("symbols", &path, &["--within", program_id]));
    assert_eq!(explicit, all);
}

#[test]
fn symbols_is_bounded_atomic_single_file_and_rejects_invalid_structure_ids() {
    let (dir, path) = fixture("let x=0; function f(p){return x+p+host}", "symbols.js");
    let plain = invoke("symbols", &path, &[]);
    let rows = jsonl(&plain);
    let destination = dir.path().join("symbols.jsonl");
    let written = invoke("symbols", &path, &["-o", destination.to_str().unwrap()]);
    success(&written);
    assert!(written.stdout.is_empty());
    assert_eq!(fs::read(&destination).unwrap(), plain.stdout);
    for flag in [
        "--max-semantic-nodes",
        "--max-records",
        "--max-output-bytes",
        "--max-entities",
        "--max-ast-depth",
        "--max-input-bytes",
    ] {
        fs::write(&destination, "preserved").unwrap();
        refused(invoke(
            "symbols",
            &path,
            &[flag, "1", "-o", destination.to_str().unwrap()],
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
    failure(invoke("symbols", dir.path(), &[]), 1, "INVALID_INPUT");
    failure(invoke("symbols", &path, &["-r"]), 2, "unexpected argument");
    // A ceiling is not a CLI knob any more; the flag is simply unknown.
    failure(
        invoke("symbols", &path, &["--max-semantic-nodes", "0"]),
        2,
        "unexpected argument",
    );
    for id in [
        "garbage".to_owned(),
        rows[1]["at"].as_str().unwrap().to_owned(),
        "function_declaration:0:1".to_owned(),
    ] {
        refused(invoke("symbols", &path, &["--within", &id]));
    }
    fs::write(&path, "let changed=0;").unwrap();
    // A coordinate names a span in the current input, so read the rewritten file's root.
    let changed = outline_rows(&path, &[]);
    let root = changed[0]["at"].as_str().unwrap();
    success(&invoke("symbols", &path, &["--within", root]));
    let profiled = invoke("symbols", &path, &["--perf"]);
    success(&profiled);
    assert_eq!(profiled.stdout, invoke("symbols", &path, &[]).stdout);
    let reports: Vec<Value> = String::from_utf8(profiled.stderr)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["command"], "symbols");
    assert_eq!(reports[0]["counters"]["records"], 1);
    assert_eq!(reports[0]["counters"]["semantic_symbols"], 1);
    assert!(reports[0]["counters"]["semantic_nodes"].as_u64().unwrap() > 0);
    for (source, error) in [
        ("let x; let x;", "INVALID_INPUT"),
        ("const =;", "INVALID_INPUT"),
    ] {
        fs::write(&path, source).unwrap();
        failure(invoke("symbols", &path, &[]), 1, error);
    }
    fs::write(&path, [0xff]).unwrap();
    failure(invoke("symbols", &path, &[]), 1, "INVALID_INPUT");
}

#[test]
fn symbols_distinguishes_empty_static_dynamic_and_unresolved_results() {
    for (source, _resolution, records) in [
        ("", "static", 0),
        ("// no bindings\n({key: 'value'});", "static", 0),
        ("host.foo; host['key'];", "static", 2),
        ("let x=0; with(obj){x}", "dynamic", 0),
        ("let x=0; eval('x');", "dynamic", 0),
        ("let x=0; function eval(){} eval('x');", "dynamic", 0),
        ("let x=0; eval?.('x');", "static", 2),
    ] {
        let (_dir, path) = fixture(source, "edge.js");
        let rows = jsonl(&invoke("symbols", &path, &[]));
        assert_eq!(rows.len(), records);
    }
}

#[test]
fn refs_tracks_shadowing_closures_and_read_write_access() {
    let source = "let count=0;\nfunction outer(count){count++; const next=()=>count; return next();}\ncount=1; count+=2; count;\n";
    let (_dir, path) = fixture(source, "bindings.js");
    let global = jsonl(&invoke(
        "xref",
        &path,
        &["--byte", &source.find("count=0").unwrap().to_string()],
    ));
    let expected = [
        ("count=1", false, true),
        ("count+=2", true, true),
        ("count;", true, false),
    ];
    assert_eq!(global.len(), 3);
    for (row, (marker, read, write)) in global.iter().zip(expected) {
        assert_eq!(row["start"], source.rfind(marker).unwrap());
        assert_eq!(row["detail"]["read"], read);
        assert_eq!(row["detail"]["write"], write);
        assert_eq!(row["detail"]["member_write"], false);
        assert_eq!(row["parent_at"], global[0]["parent_at"]);
    }
    let declaration = source.find("outer(count)").unwrap() + "outer(".len();
    let inner = jsonl(&invoke(
        "xref",
        &path,
        &["--byte", &declaration.to_string()],
    ));
    assert_eq!(inner.len(), 2);
    assert_eq!(inner[0]["start"], source.find("count++").unwrap());
    assert_eq!(inner[0]["detail"]["read"], true);
    assert_eq!(inner[0]["detail"]["write"], true);
    let closure_byte = source.find("()=>count").unwrap() + "()=>".len();
    assert_eq!(inner[1]["start"], closure_byte);
    let from_reference = jsonl(&invoke(
        "xref",
        &path,
        &["--byte", &closure_byte.to_string()],
    ));
    assert_eq!(from_reference[1..], inner[1..]);
    let extracted = invoke(
        "slice",
        &path,
        &["--at", inner[1]["parent_at"].as_str().unwrap()],
    );
    success(&extracted);
    assert_eq!(extracted.stdout, b"()=>count");
}

#[test]
fn refs_handles_module_aliases_unicode_redeclarations_and_exact_entity_boundaries() {
    for (source, filename, selected, name, expected) in [
        (
            "function f(x){return x}",
            "equal-spans.js",
            "x){",
            "x",
            vec![("x}", true, false)],
        ),
        (
            "\u{feff}// 中文\r\nlet 登录=0; 登录++; ()=>登录;\r\n",
            "unicode.js",
            "登录=0",
            "登录",
            vec![("登录++", true, true), ("登录;", true, false)],
        ),
        (
            "const \\u0061=1; a;",
            "escaped.js",
            "\\u0061",
            "a",
            vec![("a;", true, false)],
        ),
        (
            "import {x as y} from 'mod'; export {y}; y();",
            "aliases.mjs",
            "y} from",
            "y",
            vec![("y};", true, false), ("y();", true, false)],
        ),
        (
            "var x=1; var x=2; x;",
            "redeclaration.js",
            "x=2",
            "x",
            vec![("x;", true, false)],
        ),
        (
            "use(); function use(){}",
            "hoisting.js",
            "use();",
            "use",
            vec![("use();", true, false)],
        ),
        (
            "const f=function inner(){return inner}; inner;",
            "named.js",
            "inner(){",
            "inner",
            vec![("inner}", true, false)],
        ),
        (
            "let {item:value}=input; value=2; value;",
            "destructure.js",
            "value}",
            "value",
            vec![("value=2", false, true), ("value;", true, false)],
        ),
        (
            "const x=0; try{x;}catch(x){x;} x;",
            "catch.js",
            "x){",
            "x",
            vec![("x;} x", true, false)],
        ),
    ] {
        let (_dir, path) = fixture(source, filename);
        let byte = source.find(selected).unwrap();
        // Selecting a byte inside a multi-byte identifier also selects that identifier.
        let offset = if name == "登录" { byte + 1 } else { byte };
        let output = invoke("xref", &path, &["--byte", &offset.to_string()]);
        let rows = jsonl(&output);
        assert_eq!(rows.len(), expected.len());
        for (row, (marker, read, write)) in rows[..].iter().zip(expected) {
            assert_eq!(row["start"], source.find(marker).unwrap(), "{filename}");
            assert_eq!(row["detail"]["read"], read);
            assert_eq!(row["detail"]["write"], write);
            let located = invoke("locate", &path, &["--byte", &row["start"].to_string()]);
            success(&located);
            let located: Value = js_object(&located.stdout);
            assert_eq!(
                row["parent_at"], located["detail"]["entity"]["at"],
                "{filename}"
            );
        }
    }
}

#[test]
fn refs_does_not_guess_properties_globals_or_dynamic_bindings() {
    let source = "const obj={}; obj.x=1; obj.x++; delete obj.x; obj[unknown];";
    let (_dir, path) = fixture(source, "properties.js");
    let rows = jsonl(&invoke("xref", &path, &["--byte", "6"]));
    assert_eq!(rows.len(), 4);
    for row in &rows[..3] {
        assert_eq!(row["detail"]["read"], true);
        assert_eq!(row["detail"]["write"], false);
        assert_eq!(row["detail"]["member_write"], true);
    }
    assert_eq!(rows[3]["detail"]["member_write"], false);
    failure(
        invoke(
            "xref",
            &path,
            &["--byte", &(source.find("obj.x").unwrap() + 4).to_string()],
        ),
        1,
        "NOT_FOUND",
    );
    let unknown = jsonl(&invoke(
        "xref",
        &path,
        &["--byte", &source.find("unknown").unwrap().to_string()],
    ));
    assert!(unknown.is_empty());
    for source in [
        "let x=1; with(obj){x;} x;",
        "let x=1; eval('x=2'); x;",
        "let x=1; (eval)('x=2'); x;",
        "let x=1; const eval=()=>0; eval('x=2'); x;",
    ] {
        let (_dir, path) = fixture(source, "dynamic.js");
        let rows = jsonl(&invoke("xref", &path, &["--byte", "4"]));
        assert!(rows.is_empty());
    }
    for source in ["let x=1; eval?.('x=2'); x;", "let x=1; obj.eval('x=2'); x;"] {
        let (_dir, path) = fixture(source, "not-direct-eval.js");
        let rows = jsonl(&invoke("xref", &path, &["--byte", "4"]));
        assert_eq!(rows.len(), 1);
    }
    let (_dir, path) = fixture("const unused=1;", "unused.js");
    let rows = jsonl(&invoke("xref", &path, &["--byte", "6"]));
    assert!(rows.is_empty());
}

#[test]
fn refs_is_single_file_bounded_atomic_and_semantic_only_on_demand() {
    let (dir, path) = fixture("let x=0; x++; x;", "bounded.js");
    let plain = invoke("xref", &path, &["--byte", "4"]);
    jsonl(&plain);
    assert!(plain.stderr.is_empty());
    failure(
        invoke("xref", &path, &["--byte", "4", "--jsonl"]),
        2,
        "unexpected argument",
    );
    let destination = dir.path().join("refs.jsonl");
    let saved = invoke(
        "xref",
        &path,
        &["--byte", "4", "-o", destination.to_str().unwrap()],
    );
    success(&saved);
    assert!(saved.stdout.is_empty());
    assert_eq!(fs::read(&destination).unwrap(), plain.stdout);
    // No CLI ceiling exists; each flag is refused and the atomic output is untouched.
    for flag in [
        "--max-semantic-nodes",
        "--max-records",
        "--max-output-bytes",
        "--max-input-bytes",
        "--max-ast-depth",
    ] {
        refused(invoke(
            "xref",
            &path,
            &[
                "--byte",
                "4",
                flag,
                "1",
                "-o",
                destination.to_str().unwrap(),
            ],
        ));
        assert_eq!(fs::read(&destination).unwrap(), plain.stdout);
    }
    for byte in ["16", "999"] {
        failure(invoke("xref", &path, &["--byte", byte]), 1, "INVALID_QUERY");
    }
    failure(invoke("xref", &path, &["--byte", "0"]), 1, "NOT_FOUND");
    failure(
        invoke("xref", dir.path(), &["--byte", "0"]),
        1,
        "INVALID_INPUT",
    );
    failure(
        invoke("xref", &path, &["--byte", "4", "-r"]),
        2,
        "unexpected argument",
    );
    // A ceiling is not a CLI knob; the flag is simply unknown.
    failure(
        invoke("xref", &path, &["--byte", "4", "--max-semantic-nodes", "0"]),
        2,
        "unexpected argument",
    );
    let profiled = invoke("xref", &path, &["--byte", "4", "--perf"]);
    success(&profiled);
    assert_eq!(profiled.stdout, plain.stdout);
    let reports = perf_reports(&profiled);
    assert_eq!(reports.len(), 2);
    assert!(reports[0]["stages_ns"]["semantic"].as_u64().unwrap() > 0);
    assert!(reports[0]["counters"]["semantic_nodes"].as_u64().unwrap() > 0);
    assert_eq!(reports[0]["counters"]["semantic_references"], 2);
    assert_eq!(
        reports[0]["limits"]["semantic_nodes"],
        rxd_core::Budget::for_input_bytes(fs::metadata(&path).unwrap().len() as usize)
            .unwrap()
            .semantic_nodes
    );
    assert!(reports[0]["counters"]["semantic_nodes"].as_u64().unwrap() > 0);
    let outlined = invoke("entities", &path, &["--perf"]);
    success(&outlined);
    assert!(
        perf_reports(&outlined)[0]["stages_ns"]
            .get("semantic")
            .is_none()
    );
    let (_dir, invalid) = fixture("let x; let x;", "redeclaration.js");
    success(&invoke("entities", &invalid, &[]));
    failure(
        invoke("xref", &invalid, &["--byte", "4"]),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn recursive_queries_preserve_sorted_per_file_streams_and_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/artifacts");
    let a = dir.path().join("a.js");
    let b = dir.path().join("nested").join("b.js");
    fs::copy(corpus.join("application-terser.js"), &a).unwrap();
    fs::copy(corpus.join("application-rc4.js"), &b).unwrap();
    // Invalid non-JS inputs must not be parsed.
    fs::write(dir.path().join("ignored.txt"), b"not JavaScript {{{").unwrap();
    for (command, args) in [("entities", vec![]), ("find", vec!["-f", "/api"])] {
        let mut expected = Vec::new();
        for path in [&a, &b] {
            expected.extend(jsonl(&invoke(command, path, &args)));
        }
        let mut recursive_args = args.clone();
        let output = invoke(command, dir.path(), &recursive_args);
        success(&output);
        assert!(output.stderr.is_empty());
        let rows = jsonl(&output);
        assert_eq!(rows.iter().filter(|row| row["record"] == "file").count(), 2);
        let data: Vec<_> = rows
            .iter()
            .filter(|row| row["record"] != "file")
            .cloned()
            .collect();
        assert_eq!(data, expected);
        assert_eq!(
            invoke(command, dir.path(), &recursive_args).stdout,
            output.stdout
        );
        let destination = dir.path().join("results.jsonl");
        recursive_args.extend(["-o", destination.to_str().unwrap()]);
        let saved = invoke(command, dir.path(), &recursive_args);
        success(&saved);
        assert!(saved.stdout.is_empty());
        assert_eq!(fs::read(destination).unwrap(), output.stdout);
    }
}

#[test]
fn recursive_scan_supports_module_modes_unicode_depth_and_perf_totals() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("子目录")).unwrap();
    let module = dir.path().join("a.mjs");
    let script = dir.path().join("子目录").join("b.cjs");
    fs::write(&module, "export function 登录(){return '你好'}").unwrap();
    fs::write(&script, "function outer(){return ()=>1}").unwrap();
    let output = invoke("entities", dir.path(), &["--depth", "0", "--perf"]);
    success(&output);
    let rows: Vec<Value> = String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["record"], "file");
    assert_eq!(rows[1]["detail"]["kind"], "program");
    assert_eq!(rows[2]["record"], "file");
    assert_eq!(rows[3]["detail"]["kind"], "program");
    let sidecars: Vec<Value> = String::from_utf8(output.stderr)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let analysis = sidecars
        .iter()
        .find(|row| row["scope"] == "analysis")
        .unwrap();
    assert!(analysis["source_id"].is_null());
    assert_eq!(analysis["counters"]["records"], 2);
    assert_eq!(analysis["counters"]["entities"], 5);
    assert_eq!(
        analysis["counters"]["input_bytes"],
        fs::metadata(&module).unwrap().len() + fs::metadata(&script).unwrap().len()
    );
    assert_eq!(analysis["counters"]["output_bytes"], output.stdout.len());
    let found = invoke("find", dir.path(), &["-f", "你好"]);
    success(&found);
    let rows: Vec<Value> = String::from_utf8(found.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let extracted = invoke(
        "slice",
        &module,
        &["--at", rows[1]["parent_at"].as_str().unwrap()],
    );
    success(&extracted);
    assert_eq!(
        extracted.stdout,
        "function 登录(){return '你好'}".as_bytes()
    );
}

#[test]
fn recursive_empty_scans_and_explicit_files_succeed() {
    let dir = tempfile::tempdir().unwrap();
    for command in ["entities", "find"] {
        let mut args = vec![];
        if command == "find" {
            args.extend(["-f", "absent"]);
        }
        let output = invoke(command, dir.path(), &args);
        success(&output);
        assert!(output.stdout.is_empty());
    }
    failure(invoke("find", dir.path(), &["-f", ""]), 2, "INVALID_QUERY");
    let file = dir.path().join("input.txt");
    fs::write(&file, "const x = 1").unwrap();
    let single = outline_rows(&file, &[]);
    assert!(!single.is_empty());
    // A single file is one input, with or without any flag: one stream, no wrappers.
    failure(invoke("entities", &file, &["-r"]), 2, "unexpected argument");
    failure(
        invoke("entities", dir.path(), &["--within", "dummy"]),
        2,
        "--within requires a single-file input",
    );
    fs::write(dir.path().join("valid.js"), "function f(){}").unwrap();
    failure(
        invoke("entities", dir.path(), &["--within", "dummy"]),
        2,
        "--within requires a single-file input",
    );
    failure(
        invoke("locate", dir.path(), &["--byte", "0"]),
        1,
        "INVALID_INPUT",
    );
    failure(
        invoke("slice", dir.path(), &["--at", "dummy"]),
        1,
        "INVALID_INPUT",
    );
    failure(
        invoke("locate", dir.path(), &["-r", "--byte", "0"]),
        2,
        "unexpected argument",
    );
    failure(
        invoke("slice", dir.path(), &["-r", "--at", "dummy"]),
        2,
        "unexpected argument",
    );
    failure(
        invoke("entities", dir.path(), &["--within", "dummy"]),
        2,
        "--within requires a single-file input",
    );
}

#[test]
fn recursive_budgets_and_late_errors_leave_output_untouched() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.js"), "const x = 'needle'").unwrap();
    let last = dir.path().join("b.js");
    fs::write(&last, "const y = 'needle'").unwrap();
    for (command, flag) in [
        ("entities", "--max-files"),
        ("entities", "--max-records"),
        ("find", "--max-records"),
        ("entities", "--max-input-bytes"),
    ] {
        let args: Vec<&str> = match command {
            "find" => vec!["-f", "needle", flag, "1"],
            _ => vec![flag, "1"],
        };
        refused(invoke(command, dir.path(), &args));
    }
    let first_bytes = invoke("entities", &dir.path().join("a.js"), &[])
        .stdout
        .len();
    let total_bytes = first_bytes + invoke("entities", &last, &[]).stdout.len();
    // The byte ceiling is internal now, so the recursive stream is measured, not limited.
    assert!(total_bytes > first_bytes);
    let destination = dir.path().join("output.jsonl");
    fs::write(&destination, "keep me").unwrap();
    fs::write(&last, "function broken( {").unwrap();
    failure(
        invoke(
            "entities",
            dir.path(),
            &["-o", destination.to_str().unwrap()],
        ),
        1,
        "b.js",
    );
    assert_eq!(fs::read(destination).unwrap(), b"keep me");
    failure(
        invoke("entities", &dir.path().join("missing"), &[]),
        1,
        "IO_ERROR",
    );
}

#[test]
fn recursive_scans_are_deterministic_across_worker_counts() {
    let dir = tempfile::tempdir().unwrap();
    for (index, name) in ["c.js", "a.js", "b.js"].iter().enumerate() {
        fs::write(
            dir.path().join(name),
            format!("function f{index}(){{ return 'needle'; }}"),
        )
        .unwrap();
    }
    // Discovery sorts by path and parallel workers restore that order, so the stream must not
    // depend on the worker count.
    let single = invoke("entities", dir.path(), &["--threads", "1"]);
    success(&single);
    assert!(!single.stdout.is_empty());
    for threads in ["2", "8", "16"] {
        let parallel = invoke("entities", dir.path(), &["--threads", threads]);
        success(&parallel);
        assert_eq!(
            parallel.stdout, single.stdout,
            "{threads} workers changed the stream"
        );
    }
    let find_single = invoke("find", dir.path(), &["-f", "needle", "--threads", "1"]);
    success(&find_single);
    for threads in ["2", "16"] {
        let parallel = invoke("find", dir.path(), &["-f", "needle", "--threads", threads]);
        success(&parallel);
        assert_eq!(
            parallel.stdout, find_single.stdout,
            "{threads} workers changed the stream"
        );
    }
    // Aggregate budgets and error attribution stay per-path, whatever the worker count.
    for threads in ["1", "16"] {
        refused(invoke(
            "entities",
            dir.path(),
            &["--max-records", "1", "--threads", threads],
        ));
    }
    let broken = dir.path().join("d.js");
    fs::write(&broken, "function broken( {").unwrap();
    for threads in ["1", "16"] {
        failure(
            invoke("entities", dir.path(), &["--threads", threads]),
            1,
            "d.js",
        );
    }
    // The worker count is validated like every other positive budget, and a single-file query
    // accepts and ignores it.
    failure(
        invoke("entities", dir.path(), &["--threads", "0"]),
        2,
        "expected an integer",
    );
    success(&invoke(
        "entities",
        &dir.path().join("a.js"),
        &["--threads", "16"],
    ));
}

#[cfg(unix)]
#[test]
fn recursive_scans_skip_symlinks_cycles_and_special_files() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.js"), "'needle'").unwrap();
    symlink(dir.path(), dir.path().join("cycle")).unwrap();
    symlink(dir.path().join("a.js"), dir.path().join("duplicate.js")).unwrap();
    symlink("missing", dir.path().join("dangling.js")).unwrap();
    let fifo = dir.path().join("pipe.js");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let output = invoke("find", dir.path(), &["-f", "needle"]);
    success(&output);
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["record"], "file");
    assert_eq!(rows[1]["record"], "match");
    failure(
        invoke("entities", &dir.path().join("cycle"), &[]),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn search_then_extract_and_locate_complete_function() {
    let path = example();
    let found = jsonl(&invoke("find", &path, &["-f", "/api/login"]));
    assert_eq!(found.len(), 2);
    let id = found[1]["parent_at"].as_str().unwrap();
    let original = invoke("slice", &path, &["--at", id]);
    success(&original);
    assert_eq!(
        original.stdout,
        b"async () => fetch(\"/api/login\", { method: \"POST\", body: user })"
    );
    assert!(original.stderr.is_empty());
    let located = invoke(
        "locate",
        &path,
        &["--byte", &found[1]["start"].to_string(), "--json"],
    );
    success(&located);
    let location: Value = js_object(&located.stdout);
    assert_eq!(location["detail"]["entity"]["at"], id);
    assert_eq!(
        location["detail"]["ancestors"][0]["detail"]["name"],
        "request"
    );
    assert_eq!(
        location["detail"]["ancestors"][1]["detail"]["kind"],
        "program"
    );
}

#[test]
fn outline_entities_have_exact_spans_and_no_duplicate_methods() {
    let path = example();
    let source = fs::read(&path).unwrap();
    let output = invoke("entities", &path, &[]);
    let records = jsonl(&output);
    assert!(output.stderr.is_empty());
    let entities = &records[..];
    // The full catalog includes the file root; the grouped commands do not.
    assert_eq!(entities.len(), 11);
    assert_eq!(
        entities
            .iter()
            .filter(|e| e["detail"]["kind"] == "function_expression")
            .count(),
        0
    );
    assert_eq!(
        entities
            .iter()
            .filter(|e| e["detail"]["kind"] == "arrow_function")
            .count(),
        2
    );
    assert_eq!(
        entities
            .iter()
            .filter(|e| e["detail"]["kind"] == "constructor")
            .count(),
        1
    );
    for entity in entities {
        let get = invoke("slice", &path, &["--at", entity["at"].as_str().unwrap()]);
        success(&get);
        assert_eq!(
            get.stdout,
            source[entity["start"].as_u64().unwrap() as usize
                ..entity["end"].as_u64().unwrap() as usize]
        );
    }
    let create = entities
        .iter()
        .find(|e| e["detail"]["name"] == "create")
        .unwrap();
    assert_eq!(create["detail"]["flags"]["async"], true);
    assert_eq!(create["detail"]["flags"]["static"], true);
    let events = entities
        .iter()
        .find(|e| e["detail"]["name"] == "events")
        .unwrap();
    assert_eq!(events["detail"]["flags"]["generator"], true);
    let send = entities
        .iter()
        .find(|e| e["detail"]["label"] == "send")
        .unwrap();
    assert_eq!(send["detail"]["name"], Value::Null);
    assert_eq!(send["detail"]["label_origin"], "binding");
}

#[test]
fn filtering_preserves_absolute_depth_and_parent_ids() {
    let (_dir, path) = fixture(
        "function outer(){function inner(){return ()=>1} return inner}",
        "bundle.js",
    );
    let all = outline_rows(&path, &[]);
    let outer = all.iter().find(|e| e["detail"]["name"] == "outer").unwrap();
    let id = outer["at"].as_str().unwrap();
    let filtered = outline_rows(&path, &["--within", id, "--depth", "1"]);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0]["detail"]["name"], "inner");
    assert_eq!(filtered[0]["detail"]["depth"], 2);
    assert_eq!(filtered[0]["parent_at"], id);
    assert_eq!(
        outline_rows(&path, &["--within", id, "--depth", "0"]).len(),
        0
    );
    let root_only = outline_rows(&path, &["--depth", "0"]);
    assert_eq!(root_only.len(), 1);
    assert_eq!(root_only[0]["detail"]["kind"], "program");
}

#[test]
fn get_json_preserves_original_source_for_functions_arrows_classes_and_members() {
    let path = example();
    let bytes = fs::read(&path).unwrap();
    let records = outline_rows(&path, &[]);
    for entity in &records[..] {
        let output = invoke(
            "slice",
            &path,
            &["--at", entity["at"].as_str().unwrap(), "--json"],
        );
        success(&output);
        let extracted: Value = js_object(&output.stdout);
        assert_eq!(extracted["detail"]["representation"], "original");
        assert_eq!(extracted["detail"]["entity"], *entity);
        assert_eq!(
            extracted["detail"]["source"].as_str().unwrap().as_bytes(),
            &bytes[entity["start"].as_u64().unwrap() as usize
                ..entity["end"].as_u64().unwrap() as usize]
        );
    }
}

#[test]
fn unicode_bom_crlf_and_no_added_newline_are_preserved() {
    let source = "\u{feff}// 中文\r\nfunction 登录(){return \"你好😀\";}\r\n";
    let (_dir, path) = fixture(source, "bundle.js");
    let found = jsonl(&invoke("find", &path, &["-f", "你好😀"]));
    let id = found[0]["parent_at"].as_str().unwrap();
    let get = invoke("slice", &path, &["--at", id]);
    success(&get);
    assert_eq!(get.stdout, "function 登录(){return \"你好😀\";}".as_bytes());
    let position = source.find("你好").unwrap() + 1;
    let located = invoke("locate", &path, &["--byte", &position.to_string()]);
    success(&located);
    assert_eq!(js_object(&located.stdout)["detail"]["entity"]["at"], id);
    let records = outline_rows(&path, &[]);
    assert_eq!(records[0]["start"], 0);
    assert_eq!(records[0]["end"], source.len());
    assert_eq!(
        invoke(
            "slice",
            &path,
            &["--at", records[1]["at"].as_str().unwrap()]
        )
        .stdout,
        "function 登录(){return \"你好😀\";}".as_bytes()
    );
}

#[test]
fn identifiers_find_lexical_occurrences_without_static_property_names() {
    let source = "let x=1; function f(x){ return x + obj.x + obj[x] + external; }";
    let (_dir, path) = fixture(source, "identifiers.js");
    let rows = jsonl(&invoke("identifiers", &path, &["--name", "x"]));
    let identifiers: Vec<_> = rows[..].to_vec();
    assert_eq!(identifiers.len(), 4);
    assert_eq!(identifiers[0]["detail"]["role"], "declaration");
    assert_eq!(identifiers[1]["detail"]["role"], "declaration");
    assert_eq!(identifiers[2]["detail"]["role"], "reference");
    assert_eq!(identifiers[3]["detail"]["role"], "reference");
    assert_eq!(
        identifiers[2]["detail"]["syntax_kind"],
        "identifier_reference"
    );
    let refs = jsonl(&invoke(
        "identifiers",
        &path,
        &["--name", "x", "--role", "reference"],
    ));
    assert_eq!(refs.len(), 2);
    let property = jsonl(&invoke("identifiers", &path, &["--name", "x"]));
    assert!(!property.iter().any(|row| row["start"] == 49));
    // Each occurrence reports its enclosing local syntax fragment when one exists.
    assert!(
        identifiers[2]["context_at"]
            .as_str()
            .is_some_and(|id| id.starts_with("fragment."))
    );
    // --within restricts occurrences to one structure and its descendants.
    let outline = outline_rows(&path, &["--name", "f"]);
    let function_id = outline.iter().find(|r| r["detail"]["name"] == "f").unwrap()["at"]
        .as_str()
        .unwrap()
        .to_owned();
    let within = jsonl(&invoke(
        "identifiers",
        &path,
        &["--name", "x", "--within", &function_id],
    ));
    assert_eq!(within.len(), 3);
}

#[test]
fn outline_filters_by_exact_kind_and_name() {
    let source = "function alpha(){} const beta=()=>1; class Gamma{ delta(){} }";
    let (_dir, path) = fixture(source, "filter.js");
    let functions = jsonl(&invoke(
        "entities",
        &path,
        &["--kind", "function_declaration"],
    ));
    let entities: Vec<_> = functions[..].to_vec();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0]["detail"]["name"], "alpha");
    let named = outline_rows(&path, &["--name", "beta"]);
    assert_eq!(named.len(), 1);
    assert_eq!(named[0]["detail"]["label"], "beta");
    let method = outline_rows(&path, &["--name", "delta"]);
    assert_eq!(method.len(), 1);
    assert_eq!(method[0]["detail"]["kind"], "class_method");
}

#[test]
fn calls_find_direct_and_member_call_sites_without_runtime_semantics() {
    let source = "function f(){ foo(1); obj.foo(2,3); new Foo(); bar?.(); }";
    let (_dir, path) = fixture(source, "calls.js");
    let foo = jsonl(&invoke("calls", &path, &["--name", "foo"]));
    let records: Vec<_> = foo[..].to_vec();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["detail"]["call_kind"], "direct");
    assert_eq!(records[0]["detail"]["argument_count"], 1);
    assert_eq!(records[1]["detail"]["call_kind"], "member");
    assert_eq!(records[1]["detail"]["argument_count"], 2);
    let ctor = jsonl(&invoke("calls", &path, &["--name", "Foo"]));
    assert_eq!(ctor.len(), 1);
    assert_eq!(ctor[0]["detail"]["call_kind"], "new");
    let optional = jsonl(&invoke("calls", &path, &["--name", "bar"]));
    assert_eq!(optional[0]["detail"]["optional"], true);
    for (row, expected) in [
        (&records[0], "foo(1);"),
        (&records[1], "obj.foo(2,3);"),
        (&ctor[0], "new Foo();"),
        (&optional[0], "bar?.();"),
    ] {
        let extracted = invoke(
            "slice",
            &path,
            &["--at", row["context_at"].as_str().unwrap()],
        );
        success(&extracted);
        assert_eq!(extracted.stdout, expected.as_bytes());
    }
    // A shorthand arrow must not borrow the outer variable initializer as its context.
    let (_dir, path) = fixture("const thunk=()=>foo();", "arrow-call.js");
    let arrow = jsonl(&invoke("calls", &path, &["--name", "foo"]));
    assert!(arrow[0]["context_at"].is_null());
    assert!(
        arrow[0]["parent_at"]
            .as_str()
            .unwrap()
            .starts_with("arrow_function:")
    );
}

#[test]
fn members_find_static_names_without_computed_dynamic_properties() {
    let source = r#"const obj={ alpha:1, beta(){}, get gamma(){return 1} }; obj.delta; obj[computed]; obj["theta"]; obj["io"+"ta"]; obj[1]; class C{ epsilon=1; zeta(){} } const el=<A eta="x" />;"#;
    let (_dir, path) = fixture(source, "members.jsx");

    let delta = jsonl(&invoke("members", &path, &["--name", "delta"]));
    assert_eq!(delta.len(), 1);
    assert_eq!(delta[0]["detail"]["member_kind"], "static_member");
    assert_eq!(delta[0]["detail"]["key_resolution"], Value::Null);
    assert_eq!(delta[0]["detail"]["read"], true);
    assert_eq!(delta[0]["detail"]["write"], false);
    let context = invoke(
        "slice",
        &path,
        &["--at", delta[0]["context_at"].as_str().unwrap()],
    );
    success(&context);
    assert_eq!(context.stdout, b"obj.delta;");

    let alpha = jsonl(&invoke("members", &path, &["--name", "alpha"]));
    assert_eq!(alpha[0]["detail"]["member_kind"], "property_key");
    assert!(alpha[0]["detail"]["read"].is_null());
    assert!(alpha[0]["detail"]["write"].is_null());
    let context = invoke(
        "slice",
        &path,
        &["--at", alpha[0]["context_at"].as_str().unwrap()],
    );
    success(&context);
    assert_eq!(context.stdout, b"alpha:1");

    let beta = jsonl(&invoke("members", &path, &["--name", "beta"]));
    assert_eq!(beta[0]["detail"]["member_kind"], "object_method_key");

    let gamma = jsonl(&invoke("members", &path, &["--name", "gamma"]));
    assert_eq!(gamma[0]["detail"]["member_kind"], "object_method_key");

    let epsilon = jsonl(&invoke("members", &path, &["--name", "epsilon"]));
    assert_eq!(epsilon[0]["detail"]["member_kind"], "class_field_key");

    let zeta = jsonl(&invoke("members", &path, &["--name", "zeta"]));
    assert_eq!(zeta[0]["detail"]["member_kind"], "class_method_key");

    let eta = jsonl(&invoke("members", &path, &["--name", "eta"]));
    assert_eq!(eta[0]["detail"]["member_kind"], "jsx_attribute_name");

    // A computed access with a literal or constant-folded key is a property key; the resolved
    // name is matched, and the record keeps the key resolution for the caller.
    let theta = jsonl(&invoke("members", &path, &["--name", "theta"]));
    assert_eq!(theta.len(), 1);
    assert_eq!(theta[0]["detail"]["member_kind"], "computed_key");
    assert_eq!(theta[0]["detail"]["key_resolution"], "literal");
    assert_eq!(theta[0]["detail"]["name"], "theta");
    let iota = jsonl(&invoke("members", &path, &["--name", "iota"]));
    assert_eq!(iota.len(), 1);
    assert_eq!(iota[0]["detail"]["member_kind"], "computed_key");
    assert_eq!(iota[0]["detail"]["key_resolution"], "static");
    assert_eq!(iota[0]["detail"]["name"], "iota");
    // `theta`/`iota` only ever appear as keys, so an unrelated name must not match them.
    let unrelated = jsonl(&invoke("members", &path, &["--name", "iota2"]));
    assert_eq!(unrelated.len(), 0);

    // Computed access `obj[computed]` is a lexical reference, not a static member name, and a
    // numeric index is not a property name either.
    let computed = jsonl(&invoke("members", &path, &["--name", "computed"]));
    assert_eq!(computed.len(), 0);
    let numeric = jsonl(&invoke("members", &path, &["--name", "1"]));
    assert_eq!(numeric.len(), 0);
}

#[test]
fn members_access_filters_real_targets_and_preserves_local_context() {
    // One p occurrence per statement; expectations are source facts, not runtime inference.
    let cases = [
        ("plain.p;", true, false),
        ("assigned.p = 1;", false, true),
        ("compound.p += 1;", true, true),
        ("logical.p ??= 1;", true, true),
        ("updated.p++;", true, true),
        ("delete removed?.p;", false, true),
        ("[array.p] = items;", false, true),
        ("({value: object.p = fallback} = input);", false, true),
        ("[...rest.p] = items;", false, true),
        ("for (loop.p of values) {}", false, true),
        ("for (keys.p in values) {}", false, true),
        ("base.p.child = 1;", true, false),
        ("obj[key.p] = 1;", true, false),
        ("[local = defaultValue.p] = values;", true, false),
        ("(asserted.p as number) = 1;", false, true),
        ("nullable.p!++;", true, true),
        ("computed[\"p\"] = 1;", false, true),
        ("folded[\"p\"+\"\"] += 1;", true, true),
        ("delete (0, detached.p);", true, false),
        ("delete (optional?.p!);", false, true),
    ];
    let source = format!(
        "// café 😀\r\nfunction load(){{\r\n{}\r\n}}\r\nconst definition={{p:0}}; outside.p=2;",
        cases
            .iter()
            .map(|case| case.0)
            .collect::<Vec<_>>()
            .join("\r\n")
    );
    let (_dir, path) = fixture(&source, "access.ts");
    let all = jsonl(&invoke("members", &path, &["--name", "p"]));
    let members = &all[..];
    assert_eq!(members.len(), cases.len() + 2);
    for (row, (_, read, write)) in members.iter().zip(&cases) {
        assert_eq!(row["detail"]["read"], *read, "{row}");
        assert_eq!(row["detail"]["write"], *write, "{row}");
        assert!(row["context_at"].as_str().unwrap().starts_with("fragment."));
    }
    assert!(members[cases.len()]["detail"]["read"].is_null());
    assert!(members[cases.len()]["detail"]["write"].is_null());
    let outline = outline_rows(&path, &["--name", "load"]);
    let within = outline[0]["at"].as_str().unwrap();
    for (access, expected) in [
        ("read", cases.iter().map(|case| case.1).collect::<Vec<_>>()),
        ("write", cases.iter().map(|case| case.2).collect::<Vec<_>>()),
    ] {
        let filtered = jsonl(&invoke(
            "members",
            &path,
            &["--name", "p", "--access", access, "--within", within],
        ));
        let expected: Vec<_> = members
            .iter()
            .zip(expected)
            .filter_map(|(row, keep)| keep.then_some(row))
            .collect();
        assert_eq!(filtered[..].iter().collect::<Vec<_>>(), expected);
    }
    let assignment = invoke(
        "slice",
        &path,
        &["--at", members[1]["context_at"].as_str().unwrap()],
    );
    success(&assignment);
    assert_eq!(assignment.stdout, b"assigned.p = 1");
    let bad = invoke("members", &path, &["--name", "p", "--access", "execute"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(bad.stdout.is_empty());
}

#[test]
fn outline_kind_accepts_a_comma_separated_exact_kind_list() {
    let source =
        "class C{ m(){} } function f(){} const g=()=>1; const o={ k:1, j(){} }; const v=1;";
    let (_dir, path) = fixture(source, "kinds.js");

    let kinds = |args: &[&str]| {
        outline_rows(&path, args)
            .into_iter()
            .filter(|row| row["record"] == "entity")
            .map(|row| row["detail"]["kind"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };

    let single = kinds(&["--kind", "class_method"]);
    assert_eq!(single, ["class_method"]);

    let pair = kinds(&["--kind", "class_method,function_declaration"]);
    assert_eq!(pair, ["class_method", "function_declaration"]);

    // Order in the list does not change the emitted order, and repeats do not duplicate.
    let reordered = kinds(&["--kind", "function_declaration,class_method"]);
    assert_eq!(reordered, pair);
    let repeated = kinds(&["--kind", "class_method,class_method"]);
    assert_eq!(repeated, single);

    let unknown = kinds(&["--kind", "not_a_kind"]);
    assert!(unknown.is_empty());

    // Every entry must be a real kind, so a stray separator is a query error, not a wildcard.
    for bad in [
        "class_method,",
        ",class_method",
        "class_method,,arrow_function",
        "",
    ] {
        let output = invoke("entities", &path, &["--kind", bad]);
        assert!(!output.status.success(), "{bad:?} was accepted");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("INVALID_QUERY"),
            "{bad:?}"
        );
    }
}

#[test]
fn static_resolution_matches_closed_ast_expressions_without_executing_bindings() {
    let source = r#"const a='set'+'Interval'; const b=`flag_${'correct'}`; const c=String.fromCharCode(115,101,116)+'Timeout'; const d=['window','document'][0]; const e='abc'.slice(1); const f=unknown+'unsafe'; const g=String.fromCodePoint(67,68); const h='abc'.toUpperCase(); const i=first(1)+'_joined'+second(2); const j=plain.toUpperCase();"#;
    let (_dir, path) = fixture(source, "static.js");

    let plain = jsonl(&invoke("find", &path, &["-f", "setInterval"]));
    assert_eq!(plain.len(), 0);

    let resolved = jsonl(&invoke(
        "find",
        &path,
        &["-f", "setInterval", "--resolve-static"],
    ));
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0]["detail"]["value"], "setInterval");
    assert_eq!(resolved[0]["detail"]["literal_kind"], "static_expression");
    assert_eq!(resolved[0]["detail"]["resolution"], "static");
    assert_eq!(resolved[0]["detail"]["raw"], "'set'+'Interval'");
    assert_eq!(resolved[0]["start"], 8);

    let flag = jsonl(&invoke(
        "find",
        &path,
        &["-f", "flag_correct", "--resolve-static"],
    ));
    assert_eq!(flag[0]["detail"]["value"], "flag_correct");
    let timeout = jsonl(&invoke(
        "find",
        &path,
        &["-f", "setTimeout", "--resolve-static"],
    ));
    assert_eq!(timeout[0]["detail"]["value"], "setTimeout");
    let array = jsonl(&invoke(
        "find",
        &path,
        &["-f", "window", "--resolve-static"],
    ));
    assert!(
        array[..]
            .iter()
            .any(|row| row["detail"]["literal_kind"] == "static_expression")
    );
    let unsafe_result = jsonl(&invoke(
        "find",
        &path,
        &["-f", "unsafe", "--resolve-static"],
    ));
    assert_eq!(unsafe_result[0]["detail"]["literal_kind"], "string");

    // The skip guards must keep every foldable form and reject unfoldable operands,
    // including a minified-style `+` chain whose spine ends in an opaque call.
    let codepoint = jsonl(&invoke("find", &path, &["-f", "CD", "--resolve-static"]));
    assert_eq!(codepoint.len(), 1);
    assert_eq!(codepoint[0]["detail"]["literal_kind"], "static_expression");
    assert_eq!(codepoint[0]["detail"]["value"], "CD");
    let method = jsonl(&invoke(
        "find",
        &path,
        &["-f", "ABC", "--resolve-static", "--case", "sensitive"],
    ));
    assert_eq!(method.len(), 1);
    assert_eq!(method[0]["detail"]["literal_kind"], "static_expression");
    assert_eq!(method[0]["detail"]["value"], "ABC");
    let chain = jsonl(&invoke(
        "find",
        &path,
        &["-f", "_joined", "--resolve-static"],
    ));
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0]["detail"]["literal_kind"], "string");
    let receiver = jsonl(&invoke("find", &path, &["-f", "PLAIN", "--resolve-static"]));
    assert!(receiver.is_empty());
}

#[test]
fn escaped_literals_and_templates_use_decoded_values_without_crossing_interpolation() {
    let source =
        r#"function f(){return ["\x2fapi\u002flogin", `/api/${user}/login`, `/api/login`]}"#;
    let (_dir, path) = fixture(source, "bundle.js");
    let found = jsonl(&invoke("find", &path, &["-f", "/api/login"]));
    assert_eq!(found.len(), 2);
    assert_eq!(found[0]["detail"]["value"], "/api/login");
    assert_eq!(found[1]["detail"]["dynamic"], false);
    let parts = jsonl(&invoke("find", &path, &["-f", "/api/"]));
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[1]["detail"]["dynamic"], true);
    assert_eq!(
        jsonl(&invoke(
            "find",
            &path,
            &["-f", "/API/", "--case", "sensitive"],
        ))
        .len(),
        0
    );
}

#[test]
fn lone_surrogates_are_preserved_and_do_not_match_replacement_characters() {
    let (_dir, path) = fixture(r#"const x = "prefix\uD800\uFFFDsuffix";"#, "bundle.js");
    let output = invoke("find", &path, &["-f", "prefix"]);
    success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert!(lines[0].contains("\"detail\":{\"literal_kind\":\"string\",\"value\":\"\\u0070\\u0072\\u0065\\u0066\\u0069\\u0078\\ud800\\ufffd\\u0073\\u0075\\u0066\\u0066\\u0069\\u0078\""));
    // A literal has no navigable identity of its own: `at` is null and the enclosing
    // structure is the copy-only short coordinate.
    assert!(lines[0].contains("\"at\":null"));
    assert!(lines[0].contains("\"parent_at\":\""));
    // A genuine replacement character remains independently searchable.
    let replacement = invoke("find", &path, &["-f", "�suffix"]);
    success(&replacement);
    assert_eq!(
        String::from_utf8(replacement.stdout)
            .unwrap()
            .matches("\"record\":\"match\"")
            .count(),
        1
    );
    let found = invoke("find", &path, &["-f", "�suffix"]);
    success(&found);
    assert!(
        String::from_utf8(found.stdout)
            .unwrap()
            .contains("\"record\":\"match\"")
    );
}

#[test]
fn computed_members_private_names_and_context_labels_are_explicit() {
    let (_dir, path) = fixture(
        "const f=(function named(){}); const C=class {#private(){} [key](){return 'needle'} field=()=>1}; const o={a:()=>2,['needle'](){}}; assigned=()=>3;",
        "bundle.js",
    );
    let records = outline_rows(&path, &[]);
    assert!(records.iter().any(|e| e["detail"]["name"] == "#private"));
    assert!(
        records
            .iter()
            .any(|e| e["detail"]["name"] == "named" && e["detail"]["kind"] == "function_expression")
    );
    assert!(
        records
            .iter()
            .any(|e| e["detail"]["label"] == "C" && e["detail"]["kind"] == "class_expression")
    );
    assert!(
        records
            .iter()
            .any(|e| e["detail"]["label"] == "field" && e["detail"]["label_origin"] == "property")
    );
    assert!(
        records
            .iter()
            .any(|e| e["detail"]["label"] == "assigned"
                && e["detail"]["label_origin"] == "assignment")
    );
    assert_eq!(
        records
            .iter()
            .filter(|e| !e["detail"]["key_span"].is_null() && e["detail"]["name"].is_null())
            .count(),
        2
    );
    let found = jsonl(&invoke("find", &path, &["-f", "needle"]));
    assert_eq!(found.len(), 2);
    assert_eq!(found[1]["detail"]["literal_kind"], "property_key");
}

#[test]
fn the_same_span_resolves_after_content_changes() {
    let (dir, path) = fixture("function a(){}", "one.js");
    let records = outline_rows(&path, &[]);
    let id = records[1]["at"].as_str().unwrap();
    let other = dir.path().join("two.js");
    fs::copy(&path, &other).unwrap();
    let copied = outline_rows(&other, &[]);
    assert_eq!(records[1]["at"], copied[1]["at"]);
    let same_span = js_full_id(&records[0], id);
    fs::write(&path, "function b(){}").unwrap();
    success(&invoke("slice", &path, &["--at", &same_span]));
}

#[test]
fn forged_entity_bounds_kinds_and_versions_are_rejected() {
    let (_dir, path) = fixture("function a(){}", "bundle.js");
    for id in [
        "function_declaration:0:3".to_owned(),
        "arrow_function:0:14".to_owned(),
        "garbage".into(),
    ] {
        refused(invoke("slice", &path, &["--at", &id]));
        refused(invoke("entities", &path, &["--within", &id]));
    }
}

#[test]
fn auto_and_explicit_source_modes_handle_esm_without_accepting_top_level_return() {
    let (_dir, path) = fixture("export const f=()=>1;", "bundle.js");
    failure(
        invoke("entities", &path, &["--source-type", "script"]),
        1,
        "INVALID_INPUT",
    );
    let (_dir, cjs) = fixture("export const f=()=>1;", "bundle.cjs");
    failure(invoke("entities", &cjs, &[]), 1, "INVALID_INPUT");
    let (_dir, _mjs) = fixture("const f=()=>1;", "bundle.mjs");
    let (_dir, invalid) = fixture("return 1;", "bundle.js");
    failure(invoke("entities", &invalid, &[]), 1, "INVALID_INPUT");
}

#[test]
fn malformed_utf8_syntax_and_unsupported_jsx_fail_atomically() {
    for source in [
        &b"function broken("[..],
        &b"const f = <div/>;"[..],
        &[0xff, 0xfe][..],
    ] {
        let (_dir, path) = fixture(source, "bundle.js");
        failure(invoke("entities", &path, &[]), 1, "INVALID_INPUT");
    }
}

#[test]
fn empty_files_zero_matches_and_invalid_offsets_have_distinct_results() {
    let (_dir, path) = fixture("", "bundle.js");
    let records = outline_rows(&path, &[]);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["start"], 0);
    assert_eq!(records[0]["end"], 0);
    assert_eq!(jsonl(&invoke("find", &path, &["-f", "absent"])).len(), 0);
    failure(
        invoke("locate", &path, &["--byte", "0"]),
        1,
        "INVALID_QUERY",
    );
    let (_dir, path) = fixture("()=>1", "bundle.js");
    failure(
        invoke("locate", &path, &["--byte", "5"]),
        1,
        "INVALID_QUERY",
    );
    let located = invoke("locate", &path, &["--byte", "4"]);
    success(&located);
    assert_eq!(
        js_object(&located.stdout)["detail"]["entity"]["detail"]["kind"],
        "arrow_function"
    );
}

#[test]
fn cli_argument_errors_exit_with_two() {
    failure(invoke("find", &example(), &["-f", ""]), 2, "INVALID_QUERY");
    for args in [
        &["--max-input-bytes", "0"][..],
        &["--source-type", "typescript"][..],
        &["--unknown"][..],
    ] {
        failure(invoke("entities", &example(), args), 2, "error:");
    }
    failure(invoke("slice", &example(), &[]), 2, "INVALID_QUERY");
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("source.js");
    fs::write(&destination, "keep me").unwrap();
    failure(
        invoke(
            "slice",
            &example(),
            &[
                "--at",
                "dummy",
                "--pretty",
                "--json",
                "-o",
                destination.to_str().unwrap(),
            ],
        ),
        2,
        "unexpected argument",
    );
    assert_eq!(fs::read(destination).unwrap(), b"keep me");
    let help = invoke("slice", &example(), &["--help"]);
    success(&help);
    assert!(!String::from_utf8_lossy(&help.stdout).contains("--pretty"));
}

#[test]
fn budgets_reject_results_without_partial_output() {
    // Ceilings live in rxd_core::Budget: there is no CLI knob and no partial stdout.
    for flag in [
        "--max-input-bytes",
        "--max-entities",
        "--max-records",
        "--max-output-bytes",
        "--max-ast-depth",
    ] {
        refused(invoke("entities", &example(), &[flag, "1"]));
    }
    refused(invoke(
        "find",
        &example(),
        &["-f", "guest", "--max-records", "1"],
    ));
}

#[test]
fn deep_and_wide_inputs_are_controlled_failures() {
    let source = format!("{}1{};", "(".repeat(800), ")".repeat(800));
    let (_dir, path) = fixture(source, "deep.js");
    failure(invoke("entities", &path, &[]), 1, "RESOURCE_LIMIT");
    let (_dir, wide) = fixture("(()=>1);".repeat(200), "wide.js");
    refused(invoke("entities", &wide, &["--max-entities", "100"]));
}

#[test]
fn output_files_match_stdout_and_keep_previous_contents_on_failure() {
    let (dir, path) = fixture("function a(){return 'needle'}", "bundle.js");
    let destination = dir.path().join("result.jsonl");
    fs::write(&destination, "old contents").unwrap();
    refused(invoke(
        "entities",
        &path,
        &[
            "-o",
            destination.to_str().unwrap(),
            "--max-output-bytes",
            "1",
        ],
    ));
    assert_eq!(fs::read(&destination).unwrap(), b"old contents");
    let stdout = invoke("entities", &path, &[]);
    success(&stdout);
    let saved = invoke("entities", &path, &["-o", destination.to_str().unwrap()]);
    success(&saved);
    assert!(saved.stdout.is_empty());
    assert_eq!(fs::read(&destination).unwrap(), stdout.stdout);
    failure(
        invoke("entities", &path, &["-o", dir.path().to_str().unwrap()]),
        1,
        "IO_ERROR",
    );
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        2,
        "temporary output files must be cleaned up"
    );
}

#[test]
fn output_order_is_deterministic_and_perf_stays_on_stderr() {
    let plain = invoke("entities", &example(), &[]);
    let profiled = invoke("entities", &example(), &["--perf"]);
    success(&plain);
    success(&profiled);
    assert_eq!(plain.stdout, profiled.stdout);
    let reports = perf_reports(&profiled);
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["scope"], "analysis");
    assert_eq!(reports[1]["scope"], "cli");
    assert!(reports[1]["stages_ns"]["spawn_wait"].as_u64().is_some());
}

#[test]
fn input_code_is_never_executed() {
    let (dir, path) = fixture("", "bundle.js");
    let marker = dir.path().join("executed");
    fs::write(
        &path,
        format!(
            "require('fs').writeFileSync({}, 'executed'); function f(){{throw Error('boom')}} f();",
            serde_json::to_string(&marker).unwrap()
        ),
    )
    .unwrap();
    outline_rows(&path, &[]);
    assert!(!marker.exists());
}

#[test]
fn raw_get_output_file_matches_stdout_without_an_added_newline() {
    let (dir, path) = fixture("const f=()=>42;", "bundle.js");
    let records = outline_rows(&path, &[]);
    let id = records[1]["at"].as_str().unwrap();
    let destination = dir.path().join("fragment.js");
    let saved = invoke(
        "slice",
        &path,
        &["--at", id, "-o", destination.to_str().unwrap()],
    );
    success(&saved);
    assert!(saved.stdout.is_empty());
    assert_eq!(fs::read(&destination).unwrap(), b"()=>42");
}

#[test]
fn tagged_templates_and_surrogates_do_not_invent_decoded_matches() {
    let (_dir, path) = fixture(
        r#"const x=tag`\unicode`; const y=`prefix\ud800suffix`; const z='prefix';"#,
        "bundle.js",
    );
    assert_eq!(jsonl(&invoke("find", &path, &["-f", "unicode"])).len(), 0);
    let output = invoke("find", &path, &["-f", "prefix"]);
    success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("\\ud800"));
    assert_eq!(text.matches("\"record\":\"match\"").count(), 2);
}

#[test]
fn missing_files_and_single_file_commands_produce_input_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    failure(
        invoke("entities", &dir.path().join("missing.js"), &[]),
        1,
        "IO_ERROR",
    );
    failure(
        invoke("locate", dir.path(), &["--byte", "0"]),
        1,
        "INVALID_INPUT",
    );
}

#[cfg(unix)]
#[test]
fn fifo_inputs_are_rejected_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("input.js");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    failure(invoke("entities", &fifo, &[]), 1, "INVALID_INPUT");
}

fn perf_reports(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["record"] == "perf")
        .collect()
}

#[test]
fn perf_is_opt_in_and_preserves_stdout_for_all_query_modes() {
    let path = example();
    let entities = outline_rows(&path, &[]);
    let id = entities
        .iter()
        .find(|e| e["detail"]["label"] == "send")
        .unwrap()["at"]
        .as_str()
        .unwrap();
    for (command, arguments) in [
        ("entities", vec![]),
        ("find", vec!["-f", "/api/login"]),
        ("locate", vec!["--byte", "100"]),
        ("slice", vec!["--at", id]),
        ("slice", vec!["--at", id, "--json"]),
    ] {
        let plain = invoke(command, &path, &arguments);
        success(&plain);
        assert!(plain.stderr.is_empty());
        let mut profiled = arguments.clone();
        profiled.push("--perf");
        let output = invoke(command, &path, &profiled);
        success(&output);
        assert_eq!(plain.stdout, output.stdout);
        let reports = perf_reports(&output);
        assert_eq!(reports.len(), 2);
        assert_eq!(String::from_utf8_lossy(&output.stderr).lines().count(), 2);
        let analysis = &reports[0];
        let cli = &reports[1];
        assert_eq!(analysis["scope"], "analysis");
        assert_eq!(cli["scope"], "cli");
        assert_ne!(analysis["pid"], cli["pid"]);
        for report in &reports {
            assert_eq!(report["perf_version"], 1);
            assert_eq!(report["backend"], "js");
            assert_eq!(report["command"], command);
            assert_eq!(report["status"], "ok");
            assert_eq!(report["counters"]["output_bytes"], output.stdout.len());
            let stages = report["stages_ns"].as_object().unwrap();
            let sum: u64 = stages.values().map(|v| v.as_u64().unwrap()).sum();
            assert!(report["total_ns"].as_u64().unwrap() >= sum);
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            assert!(report["peak_rss_bytes"].as_u64().unwrap() > 0);
        }
        assert_eq!(
            analysis["counters"]["input_bytes"],
            fs::metadata(&path).unwrap().len()
        );
        assert_eq!(analysis["counters"]["entities"], 11);
        assert_eq!(
            analysis["limits"]["input_bytes"],
            fs::metadata(&path).unwrap().len()
        );
        assert!(
            analysis["stages_ns"]
                .as_object()
                .unwrap()
                .get("pretty")
                .is_none()
        );
        if command == "find" {
            assert_eq!(analysis["counters"]["matches"], 2);
            assert_eq!(analysis["counters"]["records"], 2);
        }
    }
}

#[test]
fn perf_reports_failed_stages_without_partial_stdout() {
    // Ceilings are internal, so the failures a caller can still provoke are query failures;
    // each must still report its stage twice (analysis + write) with no partial stdout.
    for (command, arguments, error, stage) in [
        ("slice", vec!["--at", "invalid"], "INVALID_QUERY", "query"),
        ("locate", vec!["--byte", "999999"], "INVALID_QUERY", "query"),
    ] {
        let mut arguments = arguments;
        arguments.push("--perf");
        let output = invoke(command, &example(), &arguments);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let reports = perf_reports(&output);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0]["error_code"], error);
        assert!(reports[0]["stages_ns"][stage].as_u64().unwrap() > 0);
        assert!(reports.iter().all(|r| r["status"] == "error"));
        assert_eq!(reports[1]["counters"]["output_bytes"], 0);
    }
}

#[test]
fn perf_tracks_file_writes() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("result.jsonl");
    let output = invoke(
        "entities",
        &example(),
        &["-o", destination.to_str().unwrap(), "--perf"],
    );
    success(&output);
    assert!(output.stdout.is_empty());
    let reports = perf_reports(&output);
    assert_eq!(reports.len(), 2);
    assert_eq!(
        reports[0]["counters"]["output_bytes"],
        fs::metadata(destination).unwrap().len()
    );
    assert_eq!(reports[1]["counters"]["output_bytes"], 0);
    assert!(reports[0]["stages_ns"]["write"].as_u64().unwrap() > 0);
    // stderr carries the records and nothing else: one analysis report per backend stage,
    // one parent-process report.
    assert!(reports.iter().all(|report| report["record"] == "perf"));
    assert!(reports[1]["stages_ns"]["spawn_wait"].as_u64().is_some());
}

#[test]
fn benchmark_runs_actual_cli_and_produces_reproducible_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/cli.py"))
        .args([
            "--binary",
            env!("CARGO_BIN_EXE_rxd"),
            "--sizes",
            "0.02",
            "0.04",
            "--samples",
            "2",
            "--warmups",
            "1",
            "--profile-samples",
            "2",
            "--output",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    success(&output);
    let report: Value =
        serde_json::from_slice(&fs::read(dir.path().join("results.json")).unwrap()).unwrap();
    assert_eq!(report["benchmark_version"], 1);
    assert_eq!(report["status"], "ok");
    assert_eq!(report["build"]["profile"], "custom");
    assert_eq!(report["build"]["binary_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(report["method"]["latency_instrumented"], false);
    assert_eq!(report["fixtures"].as_array().unwrap().len(), 2);
    for fixture in report["fixtures"].as_array().unwrap() {
        assert_eq!(fixture["status"], "ok");
        assert_eq!(
            fixture["source_id"],
            rxd_core::identity::input_id(&fs::read(fixture["path"].as_str().unwrap()).unwrap())
        );
        assert_eq!(fixture["observed_entities"], fixture["expected_entities"]);
        for case in fixture["cases"].as_array().unwrap() {
            if case["status"] == "unavailable" {
                continue;
            }
            assert_eq!(case["status"], "ok");
            assert_eq!(case["summary"]["samples"], 2);
            assert!(case["summary"]["median_ns"].as_f64().unwrap() > 0.0);
            for sample in case["samples"].as_array().unwrap() {
                assert!(
                    sample
                        .get("perf")
                        .is_none_or(|v| v.as_array().unwrap().is_empty())
                );
            }
            if case["name"] != "rg" {
                assert_eq!(case["profiles"].as_array().unwrap().len(), 2);
                assert!(!case["profile_summary"].as_array().unwrap().is_empty());
            }
            if case["name"] == "find_slice" {
                let sample = &case["samples"][0];
                assert_eq!(sample["steps"].as_array().unwrap().len(), 2);
                assert!(
                    sample["duration_ns"].as_u64().unwrap()
                        >= sample["command_duration_ns"].as_u64().unwrap()
                );
            }
        }
    }
    assert!(
        fs::read_to_string(dir.path().join("results.md"))
            .unwrap()
            .contains("Instrumented phase medians")
    );
}

#[test]
fn benchmark_saves_failed_results_and_returns_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    // A ceiling is internal now, so the failure this test preserves is an unparsable input:
    // the benchmark must still persist the report and exit nonzero.
    let bad = dir.path().join("bad.js");
    fs::write(&bad, "const =;").unwrap();
    let output = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/cli.py"))
        .args(["--binary", env!("CARGO_BIN_EXE_rxd"), "--input"])
        .arg(&bad)
        .args(["--samples", "1", "--output"])
        .arg(dir.path().join("report"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value =
        serde_json::from_slice(&fs::read(dir.path().join("report/results.json")).unwrap()).unwrap();
    assert_eq!(report["status"], "failed");
    assert!(
        report["fixtures"][0]["error"]
            .as_str()
            .unwrap()
            .contains("command failed")
    );
    assert!(dir.path().join("report/results.md").exists());
}

fn entity_reference(entity: &Value) -> Value {
    serde_json::json!([entity["detail"]["kind"], entity["start"], entity["end"]])
}

#[test]
fn symbols_published_and_obfuscated_inputs_find_independent_acorn_parameter_bindings() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let oracle: Value =
        serde_json::from_slice(&fs::read(root.join("refs-expectations.json")).unwrap()).unwrap();
    for case in oracle["cases"].as_array().unwrap() {
        let path = root.join(case["file"].as_str().unwrap());
        let source_id = rxd_core::identity::input_id(&fs::read(&path).unwrap());
        assert_eq!(case["source_id"], source_id);
        let entity_id = format!(
            "{}:{}:{}",
            case["entity"]["kind"].as_str().unwrap(),
            case["entity"]["start"],
            case["entity"]["end"]
        );
        let all = jsonl(&invoke("symbols", &path, &[]));
        let within = jsonl(&invoke("symbols", &path, &["--within", &entity_id]));
        for rows in [&all, &within] {
            let symbol = rows
                .iter()
                .find(|r| {
                    r["record"] == "symbol"
                        && r["start"] == case["declaration"]["start"]
                        && r["end"] == case["declaration"]["end"]
                })
                .unwrap();
            assert_eq!(symbol["detail"]["name"], case["identifier"]);
            assert_eq!(symbol["parent_at"], entity_id);
            assert_eq!(symbol["detail"]["relation"], "declared");
            assert_eq!(
                symbol["at"],
                format!(
                    "symbol:{}:{}",
                    case["declaration"]["start"], case["declaration"]["end"]
                )
            );
            let keys: Vec<_> = rows[..]
                .iter()
                .map(|r| (r["start"].as_u64().unwrap(), r["end"].as_u64().unwrap()))
                .collect();
            assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
            let ids: Vec<_> = rows.iter().filter_map(|r| r["at"].as_str()).collect();
            assert_eq!(
                ids.iter().collect::<std::collections::HashSet<_>>().len(),
                ids.len()
            );
        }
    }
}

#[test]
fn refs_published_and_obfuscated_inputs_match_independent_acorn_expectations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let oracle: Value =
        serde_json::from_slice(&fs::read(root.join("refs-expectations.json")).unwrap()).unwrap();
    assert_eq!(oracle["refs_version"], 1);
    assert_eq!(
        oracle["generator"]["sha256"],
        format!(
            "{:x}",
            Sha256::digest(fs::read(root.join("generate-refs.mjs")).unwrap())
        )
    );
    for case in oracle["cases"].as_array().unwrap() {
        let path = root.join(case["file"].as_str().unwrap());
        let bytes = fs::read(&path).unwrap();
        assert_eq!(case["source_id"], rxd_core::identity::input_id(&bytes));
        let rows = jsonl(&invoke(
            "xref",
            &path,
            &["--byte", &case["byte"].to_string()],
        ));
        // Detail coordinates are the same copy-only form as row coordinates.
        let entity_id = format!(
            "{}:{}:{}",
            case["entity"]["kind"].as_str().unwrap(),
            case["entity"]["start"],
            case["entity"]["end"]
        );
        let actual: Vec<_> = rows[..].iter().map(|row| {
            assert_eq!(row["parent_at"], entity_id, "{}", case["name"]);
            serde_json::json!({"start": row["start"], "end": row["end"], "read": row["detail"]["read"], "write": row["detail"]["write"], "member_write": row["detail"]["member_write"]})
        }).collect();
        assert_eq!(
            serde_json::json!(actual),
            case["references"],
            "{}",
            case["name"]
        );
        let extracted = invoke("slice", &path, &["--at", &entity_id]);
        success(&extracted);
        assert_eq!(
            extracted.stdout,
            bytes[case["entity"]["start"].as_u64().unwrap() as usize
                ..case["entity"]["end"].as_u64().unwrap() as usize]
        );
    }
}

#[test]
fn published_and_obfuscated_corpus_matches_independent_acorn_expectations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    for case in manifest["cases"].as_array().unwrap() {
        let path = root.join(case["file"].as_str().unwrap());
        let bytes = fs::read(&path).unwrap();
        assert_eq!(
            case["sha256"],
            format!("{:x}", Sha256::digest(&bytes)),
            "{}",
            case["name"]
        );
        assert_eq!(case["input_bytes"], bytes.len());
        let outline = outline_rows(&path, &[]);
        let entities = &outline[..];
        assert_eq!(
            entities.len(),
            case["expected"]["entity_count"].as_u64().unwrap() as usize
        );
        let by_id: std::collections::HashMap<_, _> = entities
            .iter()
            .map(|e| (e["at"].as_str().unwrap(), e))
            .collect();
        let rows: Vec<_> = entities
            .iter()
            .map(|entity| {
                let parent = entity["parent_at"]
                    .as_str()
                    .map(|id| entity_reference(by_id[id]))
                    .unwrap_or(Value::Null);
                serde_json::json!([
                    entity["detail"]["kind"],
                    entity["start"],
                    entity["end"],
                    parent,
                    entity["detail"]["depth"],
                    entity["detail"]["name"],
                    entity["detail"]["flags"]["async"],
                    entity["detail"]["flags"]["generator"],
                    entity["detail"]["flags"]["static"]
                ])
            })
            .collect();
        assert_eq!(
            format!("{:x}", Sha256::digest(serde_json::to_vec(&rows).unwrap())),
            case["expected"]["structure_sha256"],
            "Acorn structure mismatch: {}",
            case["name"]
        );
        let found = jsonl(&invoke(
            "find",
            &path,
            &["-f", case["needle"].as_str().unwrap()],
        ));
        let actual_matches: Vec<_> = found[..].iter().map(|m| serde_json::json!({
            "literal_kind": m["detail"]["literal_kind"], "value": m["detail"]["value"], "start": m["start"], "end": m["end"],
            "dynamic": m["detail"]["dynamic"], "entity": entity_reference(by_id[m["parent_at"].as_str().unwrap()])
        })).collect();
        assert_eq!(
            serde_json::json!(actual_matches),
            case["expected"]["matches"],
            "Acorn find mismatch: {}",
            case["name"]
        );
        for expected in case["expected"]["sample_entities"].as_array().unwrap() {
            let entity = entities
                .iter()
                .find(|e| {
                    entity_reference(e)
                        == serde_json::json!([expected["kind"], expected["start"], expected["end"]])
                })
                .unwrap();
            let id = entity["at"].as_str().unwrap();
            let source = invoke("slice", &path, &["--at", id]);
            success(&source);
            let start = expected["start"].as_u64().unwrap() as usize;
            let end = expected["end"].as_u64().unwrap() as usize;
            assert_eq!(source.stdout, bytes[start..end], "{}", case["name"]);
        }
        for matched in &actual_matches {
            let located = invoke("locate", &path, &["--byte", &matched["start"].to_string()]);
            success(&located);
            let located: Value = js_object(&located.stdout);
            assert_eq!(
                entity_reference(&located["detail"]["entity"]),
                matched["entity"]
            );
            let extracted = invoke(
                "slice",
                &path,
                &[
                    "--at",
                    located["detail"]["entity"]["at"].as_str().unwrap(),
                    "--json",
                ],
            );
            success(&extracted);
            let extracted: Value = js_object(&extracted.stdout);
            assert_eq!(extracted["detail"]["entity"], located["detail"]["entity"]);
            assert_eq!(extracted["detail"]["representation"], "original");
            let start = located["detail"]["entity"]["start"].as_u64().unwrap() as usize;
            let end = located["detail"]["entity"]["end"].as_u64().unwrap() as usize;
            assert_eq!(
                extracted["detail"]["source"].as_str().unwrap().as_bytes(),
                &bytes[start..end]
            );
        }
    }
}

#[test]
fn corpus_benchmark_distinguishes_encrypted_strings_from_query_failures() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/cli.py"))
        .args(["--binary", env!("CARGO_BIN_EXE_rxd"), "--corpus"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/manifest.json"))
        .args([
            "--case",
            "application-renamed-escaped",
            "--case",
            "application-rc4",
            "--samples",
            "1",
            "--warmups",
            "0",
            "--profile-samples",
            "1",
            "--output",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    success(&output);
    let report: Value =
        serde_json::from_slice(&fs::read(dir.path().join("results.json")).unwrap()).unwrap();
    assert_eq!(report["status"], "ok");
    assert_eq!(report["method"]["input_mode"], "corpus");
    assert_eq!(report["fixtures"].as_array().unwrap().len(), 2);
    let encrypted = report["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "application-rc4")
        .unwrap();
    assert_eq!(encrypted["matches"], 0);
    let cases = encrypted["cases"].as_array().unwrap();
    assert_eq!(
        cases.iter().find(|c| c["name"] == "find_slice").unwrap()["status"],
        "not_applicable"
    );
    assert_eq!(
        cases.iter().find(|c| c["name"] == "find").unwrap()["samples"][0]["exit_code"],
        0
    );
    assert_eq!(
        cases.iter().find(|c| c["name"] == "entities").unwrap()["status"],
        "ok"
    );
}

fn asar(command: &str, input: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["asar", command])
        .arg(input)
        .args(args)
        .output()
        .unwrap()
}

// Independent handcrafted Chromium Pickles; no Electron/Node process runs input JS.
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

#[test]
fn asar_namespace_exposes_container_operations_without_electron_or_js_forwarding() {
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["asar", "--help"])
        .output()
        .unwrap();
    success(&output);
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("entries") && help.contains("extract"));
    assert!(!help.contains("entities") && !help.contains("symbols"));
    let root = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .arg("--help")
        .output()
        .unwrap();
    success(&root);
    assert!(!String::from_utf8(root.stdout).unwrap().contains("electron"));
    let removed = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["electron", "--help"])
        .output()
        .unwrap();
    failure(removed, 2, "unrecognized subcommand");
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("app.asar");
    pack_asar(&archive, &[("main.js", b"const f=()=>42;")]);
    failure(
        asar("extract", &archive, &["--at", "main.js"]),
        2,
        "--output",
    );
    failure(asar("entries", dir.path(), &[]), 1, "INVALID_INPUT");
}

#[test]
fn asar_lists_all_file_types_and_extracts_packed_and_unpacked_original_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("app.asar");
    let native = b"\x7fELF\0\xff\xfe\x80native";
    let bytecode = b"\xde\xad\0\xffbytecode";
    let source = "\u{feff}const 名称='你好';\r\nfunction run(value){return 名称+value;}".as_bytes();
    let mut data = source.to_vec();
    data.extend_from_slice(native);
    data.extend_from_slice(bytecode);
    let header = serde_json::json!({"files":{
        "main.cjs":{"size":source.len(),"offset":"0"},
        "addon.node":{"size":native.len(),"offset":source.len().to_string()},
        "main.jsc":{"size":bytecode.len(),"offset":(source.len()+native.len()).to_string()},
        "shared.so":{"size":native.len(),"offset":source.len().to_string()},
        "data.bin":{"size":3,"unpacked":true},
        "empty":{"size":0,"offset":data.len().to_string()},
        "alias":{"link":"main.cjs"},
    }});
    write_asar(&archive, &header, &data);
    let unpacked = dir.path().join("app.asar.unpacked");
    fs::create_dir(&unpacked).unwrap();
    fs::write(unpacked.join("data.bin"), [0, 255, 128]).unwrap();
    let rows = jsonl(&asar("entries", &archive, &[]));
    assert_eq!(rows.len(), 7);
    let paths: Vec<_> = rows[..]
        .iter()
        .map(|r| r["detail"]["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        [
            "addon.node",
            "alias",
            "data.bin",
            "empty",
            "main.cjs",
            "main.jsc",
            "shared.so"
        ]
    );
    assert!(
        rows.iter()
            .find(|r| r["detail"]["path"] == "data.bin")
            .unwrap()["detail"]
            .get("range_valid")
            .is_none()
    );
    assert_eq!(
        rows.iter()
            .find(|r| r["detail"]["path"] == "alias")
            .unwrap()["detail"]["storage"],
        "link"
    );
    let listing = dir.path().join("list.jsonl");
    let saved = asar("entries", &archive, &["-o", listing.to_str().unwrap()]);
    success(&saved);
    assert!(saved.stdout.is_empty());
    assert_eq!(
        fs::read(listing).unwrap(),
        asar("entries", &archive, &[]).stdout
    );
    for (name, expected, storage) in [
        ("main.cjs", source, "packed"),
        ("addon.node", native.as_slice(), "packed"),
        ("shared.so", native.as_slice(), "packed"),
        ("main.jsc", bytecode.as_slice(), "packed"),
        ("data.bin", &[0, 255, 128][..], "unpacked"),
        ("empty", &[][..], "packed"),
    ] {
        let destination = dir.path().join(format!("out-{name}"));
        let result = asar(
            "extract",
            &archive,
            &["--at", name, "-o", destination.to_str().unwrap(), "--perf"],
        );
        success(&result);
        assert_eq!(fs::read(&destination).unwrap(), expected);
        let receipt: Value = js_object(&result.stdout);
        assert_eq!(receipt["backend"], "asar");
        assert_eq!(receipt["command"], "extract");
        assert_eq!(receipt["detail"]["representation"], "original");
        assert_eq!(receipt["detail"]["storage"], storage);
        assert_eq!(receipt["detail"]["origin"]["entry"], name);
        assert_eq!(receipt["source_id"], rxd_core::identity::input_id(expected));
        let perf: Vec<Value> = String::from_utf8(result.stderr)
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(perf.len(), 2);
        assert!(
            perf.iter()
                .all(|p| p["backend"] == "asar" && p["status"] == "ok")
        );
        assert_eq!(perf[0]["counters"]["input_bytes"], expected.len());
        assert_eq!(
            perf[0]["counters"]["output_bytes"],
            expected.len() + result.stdout.len()
        );
    }
    failure(
        asar(
            "extract",
            &archive,
            &[
                "--at",
                "alias",
                "-o",
                dir.path().join("failed.bin").to_str().unwrap(),
            ],
        ),
        1,
        "INVALID_INPUT",
    );
    fs::write(unpacked.join("data.bin"), [0, 1]).unwrap();
    failure(
        asar(
            "extract",
            &archive,
            &[
                "--at",
                "data.bin",
                "-o",
                dir.path().join("failed.bin").to_str().unwrap(),
            ],
        ),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn asar_extract_then_js_queries_preserve_exact_navigation() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("app.asar");
    let source = "\u{feff}const 名称='你好';\r\nfunction run(value){return 名称+value;}".as_bytes();
    pack_asar(&archive, &[("main.cjs", source)]);
    let destination = dir.path().join("main.cjs");
    let extracted = asar(
        "extract",
        &archive,
        &["--at", "main.cjs", "-o", destination.to_str().unwrap()],
    );
    success(&extracted);
    let _receipt: Value = js_object(&extracted.stdout);
    let rows = outline_rows(&destination, &[]);
    let id = rows.iter().find(|r| r["detail"]["name"] == "run").unwrap()["at"]
        .as_str()
        .unwrap()
        .to_owned();
    let get = invoke("slice", &destination, &["--at", &id]);
    success(&get);
    assert_eq!(
        get.stdout,
        b"function run(value){return \xe5\x90\x8d\xe7\xa7\xb0+value;}"
    );
    assert_eq!(
        jsonl(&invoke("find", &destination, &["-f", "你好"])).len(),
        1
    );
    let byte = source
        .windows(5)
        .position(|s| s == b"value")
        .unwrap()
        .to_string();
    assert_eq!(
        jsonl(&invoke("xref", &destination, &["--byte", &byte])).len(),
        1
    );
    jsonl(&invoke("symbols", &destination, &[]));
    success(&invoke("locate", &destination, &["--byte", &byte]));
    let changed = String::from_utf8(source.to_vec())
        .unwrap()
        .replace("你好", "你们");
    pack_asar(&archive, &[("main.cjs", changed.as_bytes())]);
    success(&asar(
        "extract",
        &archive,
        &["--at", "main.cjs", "-o", destination.to_str().unwrap()],
    ));
    // The coordinate came from the previous content of this path; it names a span, so it
    // still resolves.
    success(&invoke("slice", &destination, &["--at", &id]));
}

#[test]
fn asar_selected_reads_survive_bloat_and_preserve_atomic_outputs_and_budgets() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("app.asar");
    let source = b"function run(value){return 'needle'+value;}";
    let fake = "f".repeat(60);
    // Handcrafted asarmor createBloatPatch shape; no protector or sample code runs.
    let mut header = serde_json::json!({"files":{
        "a.js":{"size":source.len(),"offset":"0"},
        "overflow.bin":{"size":0,"offset":u64::MAX.to_string()},
        "size-overflow.bin":{"size":u64::MAX,"offset":"0"},
    }});
    header["files"][&fake] =
        serde_json::json!({"size":1_u64<<30,"offset":((1_u64<<53)-1-(1_u64<<30)).to_string()});
    write_asar(&archive, &header, source);
    let listed = jsonl(&asar("entries", &archive, &[]));
    assert_eq!(listed.len(), 4);
    assert_eq!(
        listed.iter().find(|r| r["detail"]["path"] == fake).unwrap()["detail"]["range_error"],
        "out_of_bounds"
    );
    let destination = dir.path().join("a.js");
    success(&asar(
        "extract",
        &archive,
        &["--at", "a.js", "-o", destination.to_str().unwrap()],
    ));
    assert_eq!(fs::read(&destination).unwrap(), source);
    for name in [&fake, "overflow.bin", "size-overflow.bin"] {
        failure(
            asar(
                "extract",
                &archive,
                &["--at", name, "-o", destination.to_str().unwrap()],
            ),
            1,
            "INVALID_INPUT",
        );
        assert_eq!(fs::read(&destination).unwrap(), source);
    }
    // Every ceiling lives in rxd_core::Budget, so a budget flag is simply unknown.
    for flag in [
        "--max-input-bytes",
        "--max-output-bytes",
        "--max-header-bytes",
        "--max-entries",
        "--max-records",
    ] {
        let refused = asar(
            "extract",
            &archive,
            &[
                flag,
                "1",
                "--at",
                "a.js",
                "-o",
                destination.to_str().unwrap(),
            ],
        );
        assert_eq!(refused.status.code(), Some(2), "{flag}");
        assert_eq!(fs::read(&destination).unwrap(), source);
        assert_eq!(
            asar("entries", &archive, &[flag, "1"]).status.code(),
            Some(2)
        );
    }
    failure(
        asar(
            "extract",
            &archive,
            &["--at", "a.js", "-o", archive.to_str().unwrap()],
        ),
        2,
        "output must not replace an input file",
    );
    let missing_parent = dir.path().join("missing/a.js");
    failure(
        asar(
            "extract",
            &archive,
            &["--at", "a.js", "-o", missing_parent.to_str().unwrap()],
        ),
        1,
        "IO_ERROR",
    );
    assert_eq!(fs::read(&destination).unwrap(), source);
    header["files"]["empty"] = serde_json::json!({"size":0,"offset":(source.len()+1).to_string()});
    write_asar(&archive, &header, source);
    failure(
        asar(
            "extract",
            &archive,
            &["--at", "empty", "-o", destination.to_str().unwrap()],
        ),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn asar_rejects_unsafe_paths_corrupt_headers_and_index_budget_overruns() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("app.asar");
    for header in [
        serde_json::json!({"files":{"../outside":{"size":0,"offset":"0"}}}),
        serde_json::json!({"files":{"..":{"files":{}}}}),
        serde_json::json!({"files":{"C:evil":{"size":0,"offset":"0"}}}),
        serde_json::json!({"files":{"a":{"size":0,"offset":"-1"}}}),
        serde_json::json!({"files":{"a":{"size":0,"offset":"18446744073709551616"}}}),
        serde_json::json!({"files":{"a":{"size":0,"offset":"0","link":"a"}}}),
    ] {
        write_asar(&archive, &header, b"");
        failure(asar("entries", &archive, &[]), 1, "INVALID_INPUT");
    }
    let mut nested = serde_json::json!({"size":0,"offset":"0"});
    for _ in 0..3 {
        let mut parent = serde_json::json!({"files": {}});
        parent["files"]["x".repeat(1500)] = nested;
        nested = parent;
    }
    write_asar(&archive, &nested, b"");
    failure(asar("entries", &archive, &[]), 1, "RESOURCE_LIMIT");
    pack_asar(&archive, &[("a.js", b"const a=1;")]);
    let destination = dir.path().join("a.js");
    for name in ["../a.js", "/a.js", "a/../a.js", "a\\a.js", "C:/a.js"] {
        failure(
            asar(
                "extract",
                &archive,
                &["--at", name, "-o", destination.to_str().unwrap()],
            ),
            1,
            "INVALID_INPUT",
        );
    }
    let mut bytes = fs::read(&archive).unwrap();
    bytes[0] = 8;
    fs::write(&archive, bytes).unwrap();
    failure(asar("entries", &archive, &[]), 1, "INVALID_INPUT");
    fs::write(&archive, b"invalid").unwrap();
    failure(asar("entries", &archive, &[]), 1, "INVALID_INPUT");
    let json = br#"{"files":{"a":{"size":0,"offset":"0"},"a":{"size":0,"offset":"0"}}}"#;
    let payload = (json.len() + 4).next_multiple_of(4);
    let mut bytes = Vec::new();
    for n in [4, payload + 4, payload, json.len()] {
        bytes.extend_from_slice(&(n as u32).to_le_bytes());
    }
    bytes.extend_from_slice(json);
    bytes.resize(payload + 12, 0);
    fs::write(&archive, bytes).unwrap();
    failure(asar("entries", &archive, &[]), 1, "duplicate ASAR entry");
}

#[cfg(unix)]
#[test]
fn asar_does_not_follow_input_or_unpacked_entry_links() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("app.asar");
    write_asar(
        &archive,
        &serde_json::json!({"files":{"data":{"size":1,"unpacked":true}}}),
        b"",
    );
    let unpacked = dir.path().join("app.asar.unpacked");
    fs::create_dir(&unpacked).unwrap();
    let outside = dir.path().join("outside");
    fs::write(&outside, b"x").unwrap();
    symlink(&outside, unpacked.join("data")).unwrap();
    let destination = dir.path().join("result");
    failure(
        asar(
            "extract",
            &archive,
            &["--at", "data", "-o", destination.to_str().unwrap()],
        ),
        1,
        "IO_ERROR",
    );
    fs::remove_file(unpacked.join("data")).unwrap();
    fs::write(unpacked.join("data"), b"x").unwrap();
    failure(
        asar(
            "extract",
            &archive,
            &[
                "--at",
                "data",
                "-o",
                unpacked.join("data").to_str().unwrap(),
            ],
        ),
        2,
        "output must not replace an input file",
    );
    symlink(dir.path(), unpacked.join("linked")).unwrap();
    write_asar(
        &archive,
        &serde_json::json!({"files":{"linked":{"files":{"outside":{"size":1,"unpacked":true}}}}}),
        b"",
    );
    failure(
        asar(
            "extract",
            &archive,
            &[
                "--at",
                "linked/outside",
                "-o",
                destination.to_str().unwrap(),
            ],
        ),
        1,
        "IO_ERROR",
    );
    let link = dir.path().join("link.asar");
    symlink(&archive, &link).unwrap();
    failure(asar("entries", &link, &[]), 1, "INVALID_INPUT");
}

#[test]
fn asar_extracted_production_sources_match_independent_acorn_expectations() {
    let dir = tempfile::tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    for case in manifest["cases"].as_array().unwrap().iter().take(3) {
        let source = fs::read(root.join(case["file"].as_str().unwrap())).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&source)), case["sha256"]);
        let archive = dir.path().join("app.asar");
        pack_asar(&archive, &[("bundle.js", &source)]);
        let destination = dir.path().join("bundle.js");
        success(&asar(
            "extract",
            &archive,
            &["--at", "bundle.js", "-o", destination.to_str().unwrap()],
        ));
        assert_eq!(fs::read(&destination).unwrap(), source);
        let rows = outline_rows(&destination, &[]);
        assert_eq!(rows.len(), case["expected"]["entity_count"]);
        for expected in case["expected"]["sample_entities"].as_array().unwrap() {
            assert!(rows.iter().any(|r| r["detail"]["kind"] == expected["kind"]
                && r["start"] == expected["start"]
                && r["end"] == expected["end"]));
        }
    }
}

#[test]
fn js_parser_static_values_preserve_utf16_and_string_method_semantics() {
    let (_dir, path) = fixture(
        r#"const a=String.fromCharCode(0x1f600); const b=String.fromCodePoint(0x1f600);
        const c='prefix'+'😀x'[1]; const d='prefix'+String.fromCodePoint(0xd800);
        const e='prefix'+'abcdef'.slice(-2); const f='prefix'+'abcdef'.substring(4,1);
        const g='prefix'+'abcdef'.substr(2,3); const h='prefix'+'abc'.charAt(99);
        const i=('pre'+'fix')+3; const j=String.fromCodePoint(65.5);
        const k=(123).slice(1); const l='prefix'+('abc'[true]);"#,
        "static.mjs",
    );
    let emoji = jsonl(&invoke("find", &path, &["-f", "😀", "--resolve-static"]));
    let folded: Vec<_> = emoji
        .iter()
        .filter(|r| r["detail"]["resolution"] == "static")
        .collect();
    assert_eq!(folded.len(), 1);
    assert_eq!(folded[0]["detail"]["raw"], "String.fromCodePoint(0x1f600)");
    let unit = jsonl(&invoke(
        "find",
        &path,
        &["-f", "\u{f600}", "--resolve-static"],
    ));
    assert_eq!(unit[0]["detail"]["value"], "\u{f600}");
    let prefix = invoke("find", &path, &["-f", "prefix", "--resolve-static"]);
    success(&prefix);
    let raw = String::from_utf8(prefix.stdout).unwrap();
    assert!(raw.contains("\\ude00\""));
    assert!(raw.contains("\\ud800\""));
    for expected in ["prefixef", "prefixbcd", "prefixcde", "prefix3"] {
        assert!(raw.contains(&format!("\"value\":\"{expected}\"")), "{raw}");
    }
    assert!(!raw.contains("\"value\":\"prefixx\""));
    assert!(!raw.contains("\"value\":\"prefixb\""));
    let invalid = jsonl(&invoke(
        "find",
        &path,
        &["-f", "A", "--case", "sensitive", "--resolve-static"],
    ));
    assert_eq!(invalid.len(), 0);
}

#[test]
fn js_parser_name_filters_share_listing_case_and_owner_contracts() {
    let source = r#"const Load=()=>{}; const load=()=>{}; Load(); obj.LOAD(); obj["load"](); owner.load();
        const loadOwner={Other:"load"}; const İx=1; const 𐐀=2; const ΣΟΣ=3;
        obj["İ"]=0; obj["𐐀"](); const table={["lo"+"ad"]:"value", get ["Load"](){return 1;}};
        const lone={"prefix\uD800":0};"#;
    let (_dir, path) = fixture(source, "names.mjs");
    let calls = jsonl(&invoke("calls", &path, &["-f", "load"]));
    assert_eq!(calls.len(), 4);
    assert_eq!(
        jsonl(&invoke(
            "calls",
            &path,
            &["-f", "load", "--case", "sensitive"]
        ))
        .len(),
        2
    );
    assert_eq!(
        jsonl(&invoke("calls", &path, &["--name", "load", "-f", "LOAD"])).len(),
        2
    );
    assert_eq!(
        jsonl(&invoke(
            "calls",
            &path,
            &["--name", "load", "-f", "LOAD", "--case", "sensitive"]
        ))
        .len(),
        0
    );
    let members = jsonl(&invoke("members", &path, &["-f", "load"]));
    assert_eq!(members.len(), 5);
    assert_eq!(
        jsonl(&invoke(
            "values",
            &path,
            &["--kind", "property", "--name", "Other"]
        ))
        .len(),
        1
    );
    assert_eq!(jsonl(&invoke("members", &path, &["-f", "owner"])).len(), 0);
    assert_eq!(
        jsonl(&invoke("symbols", &path, &["--name", "Load"])).len(),
        1
    );
    for (command, needle) in [
        ("identifiers", "i\u{307}"),
        ("symbols", "σος"),
        ("members", "i\u{307}"),
        ("calls", "𐐨"),
    ] {
        assert_eq!(
            jsonl(&invoke(command, &path, &["-f", needle])).len(),
            1,
            "{command}"
        );
        assert_eq!(
            jsonl(&invoke(
                command,
                &path,
                &["-f", needle, "--case", "sensitive"]
            ))
            .len(),
            0,
            "{command}"
        );
    }
    let lone = invoke("members", &path, &["-f", "PREFIX"]);
    success(&lone);
    let output = String::from_utf8(lone.stdout).unwrap();
    assert!(output.contains("\\ud800"));
    assert!(!output.contains('�'));
    for command in ["calls", "members", "identifiers", "symbols"] {
        success(&invoke(command, &path, &[]));
        let empty = invoke(command, &path, &["--name", ""]);
        assert!(!empty.status.success());
        assert!(empty.stdout.is_empty());
    }
}

#[test]
fn js_parser_static_helpers_do_not_fold_visible_shadowing_or_dynamic_scope() {
    for source in [
        "const String={fromCharCode(){return 'other'}}; const a=String.fromCharCode(65);",
        "const a=String.fromCharCode(65); const String=opaque;",
        "String.fromCharCode=opaque; const a=String.fromCharCode(65);",
        "const a=String.fromCharCode(65); eval('opaque');",
        "globalThis.String=opaque; const a=String.fromCharCode(65);",
    ] {
        let (_dir, path) = fixture(source, "shadow.mjs");
        let rows = jsonl(&invoke(
            "find",
            &path,
            &["-f", "A", "--case", "sensitive", "--resolve-static"],
        ));
        assert_eq!(rows.len(), 0, "{source}");
    }
    let (_dir, path) = fixture(
        "const String=opaque; const a='pre'+'fix'; obj[String.fromCharCode(65)]();",
        "safe.mjs",
    );
    assert_eq!(
        jsonl(&invoke(
            "find",
            &path,
            &["-f", "prefix", "--resolve-static"]
        ))
        .len(),
        1
    );
    assert_eq!(jsonl(&invoke("calls", &path, &["--name", "A"])).len(), 0);
}

#[test]
fn js_parser_calls_include_static_computed_names_and_optional_paths() {
    let source = r#"loader.load('a'); loader['load']('b'); loader['lo'+'ad']('c');
        loader?.['load']?.('d'); new loader['load']('e'); loader[key]('skip');
        loader[('load' as const)]('f'); loader[`load`]('g');"#;
    let (_dir, path) = fixture(source, "calls.ts");
    let rows = jsonl(&invoke("calls", &path, &["--name", "load"]));
    assert_eq!(rows.len(), 7);
    let calls: Vec<_> = rows.iter().filter(|r| r["record"] == "call").collect();
    assert_eq!(calls[3]["detail"]["optional"], true);
    assert_eq!(calls[4]["detail"]["call_kind"], "new");
    for row in calls {
        let context = row["context_at"].as_str().unwrap();
        let extracted = invoke("slice", &path, &["--at", context]);
        success(&extracted);
        assert!(
            source
                .as_bytes()
                .windows(extracted.stdout.len())
                .any(|s| s == extracted.stdout)
        );
    }
}

#[test]
fn js_parser_values_preserve_slots_unknowns_selection_and_source_identity() {
    let source = "const table={Home:'a',Home:'b',nested:{items:['x',,...rest]},[key]:'v',['Guide']:'g',...more,get danger(){throw 1;}};\r\nsend(table,'/ready',...args);";
    let (_dir, path) = fixture(source, "values.mjs");
    let home = jsonl(&invoke(
        "values",
        &path,
        &["--kind", "property", "-f", "home"],
    ));
    assert_eq!(home.len(), 2);
    assert_eq!(home[0]["detail"]["value"]["value"], "a");
    assert_eq!(home[1]["detail"]["value"]["value"], "b");
    let context = home[0]["context_at"].as_str().unwrap();
    let one = jsonl(&invoke("values", &path, &["--within", context]));
    assert_eq!(one.len(), 1);
    let extracted = invoke("slice", &path, &["--at", context]);
    success(&extracted);
    assert_eq!(extracted.stdout, b"Home:'a'");
    let elements = jsonl(&invoke("values", &path, &["--kind", "element"]));
    assert_eq!(elements.len(), 3);
    assert_eq!(elements[1]["detail"]["value"]["resolution"], "hole");
    assert_eq!(elements[2]["detail"]["spread"], true);
    assert_eq!(
        elements[2]["detail"]["value"]["unresolved_reason"],
        "spread"
    );
    let args = jsonl(&invoke("values", &path, &["--kind", "argument"]));
    assert_eq!(args[0]["detail"]["value"]["resolution"], "unresolved");
    assert_eq!(args[1]["detail"]["value"]["value"], "/ready");
    assert_eq!(args[2]["detail"]["spread"], true);
    let properties = jsonl(&invoke("values", &path, &["--kind", "property"]));
    assert!(
        properties
            .iter()
            .any(|r| r["detail"]["key_resolution"] == "unresolved")
    );
    assert!(
        properties
            .iter()
            .any(|r| r["detail"]["syntax_kind"] == "get"
                && r["detail"]["value"]["unresolved_reason"] == "method_or_accessor")
    );
    let absent = invoke(
        "values",
        &path,
        &["--within", "fragment.object_property:1:2"],
    );
    assert!(!absent.status.success());
    assert!(absent.stdout.is_empty());
    let program = jsonl(&invoke("entities", &path, &["--kind", "program"]));
    let program_at = program
        .iter()
        .find(|row| row["record"] == "entity")
        .unwrap()["at"]
        .as_str()
        .unwrap();
    let original = invoke("slice", &path, &["--at", program_at]);
    success(&original);
    assert_eq!(original.stdout, source.as_bytes());
    let symbols = jsonl(&invoke("symbols", &path, &["--name", "table"]));
    let symbol_at = symbols
        .iter()
        .find(|row| row["record"] == "symbol")
        .unwrap()["at"]
        .as_str()
        .unwrap();
    success(&invoke("xref", &path, &["--at", symbol_at]));
    fs::write(&path, source.replacen("Home", "Hame", 1)).unwrap();
    // A coordinate names a span, not the file's bytes: the same span still resolves.
    success(&invoke("slice", &path, &["--at", program_at]));
    success(&invoke("xref", &path, &["--at", symbol_at]));
}

#[test]
fn js_parser_resources_follow_only_earlier_unwritten_same_scope_const_strings() {
    let source = r#"const base='/assets/'; const name='entry.js'; const path=`${base}${name}`; import(path);
        let mutable='skip'; import(mutable); { const base='/inside/'; import(base+'a.js'); }
        function nested(){import(base+'outside');} import(later); const later='late';
        const obj={src:'x'}; import(obj.src); const fn=()=> 'x'; import(fn());
        const changed='old'; changed='new'; import(changed);"#;
    let (_dir, path) = fixture(source, "bindings.mjs");
    let plain = jsonl(&invoke("resources", &path, &[]));
    assert!(
        plain
            .iter()
            .all(|row| row["detail"]["resolution"] != "binding")
    );
    let rows = jsonl(&invoke("resources", &path, &["--resolve-bindings"]));
    assert_eq!(
        rows.iter()
            .filter(|row| row["detail"]["resolution"] == "binding")
            .count(),
        2
    );
    let entry = rows
        .iter()
        .find(|row| row["detail"]["name"] == "/assets/entry.js")
        .unwrap();
    assert_eq!(entry["detail"]["resolution"], "binding");
    for proof in entry["detail"]["bindings"].as_array().unwrap() {
        success(&invoke(
            "slice",
            &path,
            &["--at", proof["at"].as_str().unwrap()],
        ));
    }
    assert!(
        rows.iter()
            .any(|r| r["detail"]["unresolved_reason"] == "binding_scope_boundary")
    );
    assert!(
        rows.iter()
            .any(|r| r["detail"]["unresolved_reason"] == "binding_before_initializer")
    );
    let (_dynamic_dir, dynamic) = fixture("const p='a'; eval('opaque'); import(p);", "dynamic.mjs");
    let rows = jsonl(&invoke("resources", &dynamic, &["--resolve-bindings"]));
    assert_eq!(rows[0]["detail"]["unresolved_reason"], "dynamic_scope");
}

#[cfg(feature = "backend-dex")]
#[path = "dex/mod.rs"]
mod dex;

#[cfg(feature = "mcp")]
#[path = "mcp/mod.rs"]
mod mcp;
