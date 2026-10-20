//! End-to-end tests only: every assertion exercises the actual CLI process.
#[cfg(feature = "backend-html")]
#[path = "html/mod.rs"]
mod html;

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

/// A row prints a copy-only coordinate; a stale-input check needs the content-bound form,
/// which a caller rebuilds from the header identity.
fn js_full_id(header: &Value, at: &str) -> String {
    format!("rxd1:js:{}:{at}", header["source_id"].as_str().unwrap())
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
    let records: Vec<Value> = String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.first().unwrap()["record"], "header");
    assert_eq!(records.last().unwrap()["record"], "summary");
    assert_eq!(records.last().unwrap()["complete"], true);
    assert_eq!(
        records.last().unwrap()["records"].as_u64().unwrap() as usize,
        records.len() - 2
    );
    if records[0]["backend"] == "js" {
        assert_js_stream(&records);
    }
    records
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
        "member" => &["name", "member_kind", "key_resolution"],
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

fn assert_js_stream(rows: &[Value]) {
    assert_js_keys(
        &rows[0],
        &[
            "record",
            "backend",
            "command",
            "source_id",
            "input_bytes",
            "schema_version",
            "query",
            "detail",
        ],
    );
    assert_eq!(rows[0]["schema_version"], 2);
    assert!(rows[0]["input_bytes"].is_number());
    let mut fields = vec![
        "path",
        "root_id",
        "source_type",
        "analysis",
        "syntax",
        "declaration_file",
        "origin",
    ];
    match rows[0]["command"].as_str().unwrap() {
        "xref" => fields.extend(["resolution", "selected", "declaration", "declaration_ids"]),
        "symbols" => fields.extend(["resolution", "within_id"]),
        _ => {}
    }
    assert_js_keys(&rows[0]["detail"], &fields);
    for row in &rows[1..rows.len() - 1] {
        assert_js_record(row);
    }
    assert_js_keys(
        rows.last().unwrap(),
        &["record", "records", "complete", "detail"],
    );
    assert_js_keys(&rows.last().unwrap()["detail"], &[]);
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
    assert_eq!(
        output.status.code(),
        Some(code),
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
    for (name, source, syntax, mode) in [
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
        assert_eq!(rows[0]["detail"]["syntax"].as_str(), syntax);
        assert_eq!(rows[0]["detail"]["source_type"], mode);
        assert_eq!(
            rows[0]["detail"]["declaration_file"].as_bool(),
            syntax.map(|_| name == "g.d.ts")
        );
        let root = rows[0]["detail"]["root_id"].as_str().unwrap();
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
    assert_eq!(rows.last().unwrap()["detail"]["files"], 7);
    assert_eq!(rows.last().unwrap()["complete"], true);
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
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1]["detail"]["literal_kind"], kind);
        assert_eq!(rows[1]["detail"]["value"], value);
        let id = rows[1]["context_at"].as_str().unwrap();
        let raw = invoke("slice", &path, &["--at", id]);
        success(&raw);
        assert_eq!(raw.stdout, original.as_bytes());
        let located = invoke("locate", &path, &["--byte", &rows[1]["start"].to_string()]);
        success(&located);
        assert_eq!(js_object(&located.stdout)["context_at"], id);
    }
    let declaration = source.find("Widget=()").unwrap();
    let rows = jsonl(&invoke(
        "xref",
        &path,
        &["--byte", &declaration.to_string()],
    ));
    assert_eq!(rows[0]["detail"]["resolution"], "resolved");
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[3]["start"], source.rfind("Widget>").unwrap());
    for row in &rows[1..rows.len() - 1] {
        assert_eq!(row["detail"]["read"], true);
        assert!(row["detail"]["type_reference"].is_null());
    }
    let ui = source.find("UI={").unwrap();
    let rows = jsonl(&invoke("xref", &path, &["--byte", &ui.to_string()]));
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1]["start"], source.find("UI.Widget").unwrap());
    for marker in ["div data-label", "data-label=", "Widget/>"] {
        failure(
            invoke(
                "xref",
                &path,
                &["--byte", &source.find(marker).unwrap().to_string()],
            ),
            1,
            "INVALID_TARGET",
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
    assert_eq!(
        rows[0]["detail"]["declaration_ids"],
        model["detail"]["declaration_ids"]
    );
    assert!(
        rows[1..rows.len() - 1]
            .iter()
            .any(|r| r["detail"]["type_reference"] == true && r["detail"]["read"] == false)
    );
    assert!(
        rows[1..rows.len() - 1]
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
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1]["detail"]["value_reference"], true);
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
    assert_eq!(refs[0]["detail"]["resolution"], "unresolved");
    assert_eq!(refs[0]["detail"]["selected"]["type_reference"], true);
}

#[test]
fn typed_ambient_signatures_type_literals_and_source_identity_remain_exact_and_bounded() {
    let source = "\u{feff}declare namespace SDK { interface Options { endpoint: '/api/login'; run<T>(value:T):T; } function send(value:Options):string; function send(value:string):string; }\r\ndeclare module 'legacy' { export type Value=string; }";
    let (dir, path) = fixture(source, "sdk.d.ts");
    let outlines = outline_rows(&path, &[]);
    assert_eq!(outlines[0]["detail"]["declaration_file"], true);
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
    assert_eq!(found[1]["detail"]["literal_kind"], "type_string");
    let output = invoke(
        "slice",
        &path,
        &["--at", found[1]["context_at"].as_str().unwrap()],
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
    let id = js_full_id(&found[0], send["at"].as_str().unwrap());
    fs::write(&path, format!("{source}\n// changed")).unwrap();
    failure(invoke("slice", &path, &["--at", &id]), 1, "SOURCE_MISMATCH");
    let (_tmp, path) = fixture("type Route=`/api/${'login'|'logout'}`;", "template.ts");
    let rows = jsonl(&invoke("find", &path, &["-f", "/api/"]));
    assert_eq!(rows[1]["detail"]["literal_kind"], "type_template");
    assert_eq!(rows[1]["detail"]["dynamic"], true);
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
        let entities = &outline[1..outline.len() - 1];
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
        let actual:Vec<_>=found[1..found.len()-1].iter().map(|m|serde_json::json!({"literal_kind":m["detail"]["literal_kind"],"value":m["detail"]["value"],"start":m["start"],"end":m["end"],"dynamic":m["detail"]["dynamic"],"entity":entity_reference(by_id[m["parent_at"].as_str().unwrap()])})).collect();
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
        for (row, expected) in found[1..found.len() - 1]
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
        assert_eq!(refs[0]["detail"]["resolution"], "resolved");
        assert_eq!(
            refs[0]["detail"]["declaration"]["start"],
            query["declaration"]["start"]
        );
        assert_eq!(
            refs[0]["detail"]["declaration"]["end"],
            query["declaration"]["end"]
        );
        let actual:Vec<_>=refs[1..refs.len()-1].iter().map(|r|{let mut value=serde_json::json!({"start":r["start"],"end":r["end"],"read":r["detail"]["read"],"write":r["detail"]["write"],"member_write":r["detail"]["member_write"]});if let Some(t)=r["detail"].get("type_reference").filter(|v| !v.is_null()){value["type_reference"]=t.clone();value["value_reference"]=r["detail"]["value_reference"].clone();}value}).collect();
        assert_eq!(
            serde_json::json!(actual),
            query["references"],
            "{}",
            case["name"]
        );
        let symbols = jsonl(&invoke("symbols", &path, &[]));
        assert!(symbols.len() > 2);
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
        let found = &rows[1];
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
        assert_eq!(output["detail"]["complete"], true);
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
        let found = &rows[1];
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
    assert!(rows[1]["context_at"].is_null());
    assert!(
        rows[0]["detail"]["selected"]["context_at"]
            .as_str()
            .unwrap()
            .contains("fragment.variable_declarator:")
    );
    let source = "host(value);";
    let (_dir, path) = fixture(source, "host.js");
    let rows = jsonl(&invoke("xref", &path, &["--byte", "0"]));
    assert_eq!(rows[0]["detail"]["resolution"], "unresolved");
    let context = invoke(
        "slice",
        &path,
        &[
            "--at",
            rows[0]["detail"]["selected"]["context_at"]
                .as_str()
                .unwrap(),
        ],
    );
    success(&context);
    assert_eq!(context.stdout, source.as_bytes());
}

#[test]
fn local_context_ids_are_content_bound_parser_only_bounded_and_atomic() {
    let source = "const obj={endpoint:'/api/check'}; function later(){}";
    let (dir, path) = fixture(source, "check.js");
    let found = jsonl(&invoke("find", &path, &["-f", "/api/check"]));
    let id = found[1]["context_at"].as_str().unwrap();
    let fingerprint = found[0]["source_id"].as_str().unwrap();
    let full_id = format!("rxd1:js:{fingerprint}:{id}");
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
        full_id.replacen("rxd1", "rxd2", 1),
        full_id.replace("fragment.object_property", "fragment.block_statement"),
        format!("{full_id}:extra"),
        format!("rxd1:js:{fingerprint}:fragment.object_property:0:1"),
        format!("rxd1:js:{fingerprint}:fragment.object_property:011:32"),
        format!("rxd1:js:{fingerprint}:fragment.object_property:32:11"),
    ] {
        failure(invoke("slice", &path, &["--at", &forged]), 1, "INVALID_ID");
    }
    for command in ["entities", "symbols"] {
        failure(invoke(command, &path, &["--within", id]), 1, "INVALID_ID");
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
    let stale = js_full_id(&found[0], id);
    fs::write(&path, "const obj={changed:true};").unwrap();
    failure(
        invoke("slice", &path, &["--at", &stale]),
        1,
        "SOURCE_MISMATCH",
    );
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
        assert_eq!(case["source_id"], format!("{:x}", Sha256::digest(&bytes)));
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
        let checks = found[1..found.len() - 1]
            .iter()
            .zip(case["matches"].as_array().unwrap())
            .chain(std::iter::once((
                &refs[0]["detail"]["selected"],
                &case["selected"],
            )))
            .chain(
                refs[1..refs.len() - 1]
                    .iter()
                    .zip(case["references"].as_array().unwrap()),
            );
        assert_eq!(found.len() - 2, case["matches"].as_array().unwrap().len());
        assert_eq!(refs.len() - 2, case["references"].as_array().unwrap().len());
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
        assert_eq!(extracted["detail"]["complete"], true);
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
        assert_eq!(case["source_id"], format!("{:x}", Sha256::digest(&source)));
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
    let symbol = &rows[1];
    let id = symbol["at"].as_str().unwrap();
    let fingerprint = rows[0]["source_id"].as_str().unwrap();
    // Malformed-input cases need the content-bound spelling; a short coordinate has no
    // schema prefix to corrupt.
    let full = format!("rxd1:js:{fingerprint}:{id}");
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
        failure(
            invoke(
                "slice",
                &path,
                &["--at", &format!("rxd1:js:{fingerprint}:symbol:{a}:{b}")],
            ),
            1,
            "INVALID_ID",
        );
    }
    for forged in [
        full.replacen("rxd1", "rxd2", 1),
        full.replace(":symbol:", ":variable:"),
        format!("{full}:extra"),
        format!("rxd1:js:{fingerprint}:symbol:06:12"),
        format!("rxd1:js:{fingerprint}:symbol:12:6"),
    ] {
        failure(invoke("slice", &path, &["--at", &forged]), 1, "INVALID_ID");
    }
    for command in ["entities", "symbols"] {
        failure(invoke(command, &path, &["--within", id]), 1, "INVALID_ID");
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
    let stale = js_full_id(&rows[0], id);
    fs::write(&path, "const config={changed:true};").unwrap();
    failure(
        invoke("slice", &path, &["--at", &stale]),
        1,
        "SOURCE_MISMATCH",
    );
    // Syntax extraction remains available in a dynamic file; it claims no runtime value.
    let dynamic = "let x=1; eval('x=2');";
    fs::write(&path, dynamic).unwrap();
    let fingerprint = format!("{:x}", Sha256::digest(dynamic.as_bytes()));
    let extracted = invoke(
        "slice",
        &path,
        &["--at", &format!("rxd1:js:{fingerprint}:symbol:4:5")],
    );
    success(&extracted);
    assert_eq!(extracted.stdout, b"x=1");
    fs::write(&path, "let x; let x;").unwrap();
    let fingerprint = format!("{:x}", Sha256::digest(b"let x; let x;"));
    failure(
        invoke(
            "slice",
            &path,
            &["--at", &format!("rxd1:js:{fingerprint}:symbol:4:5")],
        ),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn symbols_discovers_distinct_bindings_with_content_ids_and_original_spans() {
    let source = "\u{feff}// 中文\r\nimport {remote as alias} from 'mod';\r\nvar repeat=1; var repeat=2;\nconst {key: local, shorthand, ...rest}=alias;\nlet 登录=1; const \\u0061=2;\nfunction outer(local, {item}, ...args){let local2; {let local=3; local;} try{}catch(error){error;} return ()=>local+登录;}\nconst f=function inner(){return inner}; class C{method(p){return p}}\n";
    let (_dir, path) = fixture(source, "symbols.mjs");
    let rows = jsonl(&invoke("symbols", &path, &[]));
    assert_eq!(rows[0]["command"], "symbols");
    assert_eq!(rows[0]["detail"]["analysis"], "lexical");
    assert_eq!(rows[0]["detail"]["resolution"], "static");
    assert_eq!(rows[0]["detail"]["within_id"], rows[0]["detail"]["root_id"]);
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
    assert_eq!(rows.len(), expected.len() + 2);
    let mut ids = std::collections::HashSet::new();
    for (row, (marker, name)) in rows[1..rows.len() - 1].iter().zip(expected) {
        let start = source.find(marker).unwrap();
        let raw = if name == "a" { "\\u0061" } else { name };
        assert_eq!(row["record"], "symbol");
        assert_eq!(row["detail"]["name"], name);
        assert_eq!(row["detail"]["relation"], "declared");
        assert_eq!(row["start"], start);
        assert_eq!(row["end"], start + raw.len());
        assert_eq!(row["at"], format!("symbol:{start}:{}", start + raw.len()));
        assert!(ids.insert(row["at"].clone().to_string()));
        let refs = jsonl(&invoke("xref", &path, &["--byte", &start.to_string()]));
        assert_eq!(refs[0]["detail"]["selected"]["name"], name);
        assert_eq!(refs[0]["detail"]["declaration"]["start"], row["start"]);
        assert_eq!(refs[0]["detail"]["declaration"]["end"], row["end"]);
        assert_eq!(
            refs[0]["detail"]["declaration"]["entity_id"],
            row["parent_at"]
        );
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
    assert_eq!(
        invoke("symbols", &path, &["--jsonl"]).stdout,
        invoke("symbols", &path, &[]).stdout
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
    assert_eq!(rows[0]["detail"]["within_id"], id);
    let records = &rows[1..rows.len() - 1];
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
    assert_eq!(repeated.len(), 3);
    assert_eq!(repeated[1]["detail"]["relation"], "declared");
    assert_eq!(repeated[1]["start"], source.find("twice(){}").unwrap());
    assert!(repeated[1]["start"].as_u64().unwrap() < second["start"].as_u64().unwrap());
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
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1]["detail"]["name"], name);
        assert_eq!(rows[1]["detail"]["relation"], relation);
    }
    let program_id = entities[0]["detail"]["root_id"].as_str().unwrap();
    let explicit = jsonl(&invoke("symbols", &path, &["--within", program_id]));
    assert_eq!(explicit[0]["query"]["within"], program_id);
    assert_eq!(all[0]["query"]["within"], Value::Null);
    assert_eq!(&explicit[1..], &all[1..]);
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
        format!(
            "rxd1:js:{}:function_declaration:0:1",
            rows[0]["source_id"].as_str().unwrap()
        ),
    ] {
        failure(
            invoke("symbols", &path, &["--within", &id]),
            1,
            "INVALID_ID",
        );
    }
    let root = js_full_id(&rows[0], rows[0]["detail"]["root_id"].as_str().unwrap());
    fs::write(&path, "let changed=0;").unwrap();
    failure(
        invoke("symbols", &path, &["--within", &root]),
        1,
        "SOURCE_MISMATCH",
    );
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
    for (source, resolution, records) in [
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
        assert_eq!(rows[0]["detail"]["resolution"], resolution);
        assert_eq!(rows.last().unwrap()["records"], records);
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
    assert_eq!(global[0]["detail"]["resolution"], "resolved");
    assert_eq!(global[0]["detail"]["analysis"], "lexical");
    let expected = [
        ("count=1", false, true),
        ("count+=2", true, true),
        ("count;", true, false),
    ];
    assert_eq!(global.len(), 5);
    for (row, (marker, read, write)) in global[1..4].iter().zip(expected) {
        assert_eq!(row["start"], source.rfind(marker).unwrap());
        assert_eq!(row["detail"]["read"], read);
        assert_eq!(row["detail"]["write"], write);
        assert_eq!(row["detail"]["member_write"], false);
        assert_eq!(row["parent_at"], global[0]["detail"]["root_id"]);
    }
    let declaration = source.find("outer(count)").unwrap() + "outer(".len();
    let inner = jsonl(&invoke(
        "xref",
        &path,
        &["--byte", &declaration.to_string()],
    ));
    assert_eq!(inner[0]["detail"]["declaration"]["start"], declaration);
    assert_eq!(inner[0]["detail"]["selected"]["role"], "declaration");
    assert_eq!(inner.len(), 4);
    assert_eq!(inner[1]["start"], source.find("count++").unwrap());
    assert_eq!(inner[1]["detail"]["read"], true);
    assert_eq!(inner[1]["detail"]["write"], true);
    let closure_byte = source.find("()=>count").unwrap() + "()=>".len();
    assert_eq!(inner[2]["start"], closure_byte);
    let from_reference = jsonl(&invoke(
        "xref",
        &path,
        &["--byte", &closure_byte.to_string()],
    ));
    assert_eq!(from_reference[0]["detail"]["selected"]["role"], "reference");
    assert_eq!(
        from_reference[0]["detail"]["declaration"],
        inner[0]["detail"]["declaration"]
    );
    assert_eq!(from_reference[1..], inner[1..]);
    let extracted = invoke(
        "slice",
        &path,
        &["--at", inner[2]["parent_at"].as_str().unwrap()],
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
        assert_eq!(rows[0]["detail"]["resolution"], "resolved", "{filename}");
        assert_eq!(rows[0]["detail"]["selected"]["name"], name);
        assert_eq!(rows.len(), expected.len() + 2);
        for (row, (marker, read, write)) in rows[1..rows.len() - 1].iter().zip(expected) {
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
        if filename == "redeclaration.js" {
            assert_eq!(rows[0]["detail"]["declaration"]["start"], 4);
            assert_eq!(rows[0]["detail"]["selected"]["start"], 13);
        }
    }
}

#[test]
fn refs_does_not_guess_properties_globals_or_dynamic_bindings() {
    let source = "const obj={}; obj.x=1; obj.x++; delete obj.x; obj[unknown];";
    let (_dir, path) = fixture(source, "properties.js");
    let rows = jsonl(&invoke("xref", &path, &["--byte", "6"]));
    assert_eq!(rows.len(), 6);
    for row in &rows[1..4] {
        assert_eq!(row["detail"]["read"], true);
        assert_eq!(row["detail"]["write"], false);
        assert_eq!(row["detail"]["member_write"], true);
    }
    assert_eq!(rows[4]["detail"]["member_write"], false);
    failure(
        invoke(
            "xref",
            &path,
            &["--byte", &(source.find("obj.x").unwrap() + 4).to_string()],
        ),
        1,
        "INVALID_TARGET",
    );
    let unknown = jsonl(&invoke(
        "xref",
        &path,
        &["--byte", &source.find("unknown").unwrap().to_string()],
    ));
    assert_eq!(unknown[0]["detail"]["resolution"], "unresolved");
    assert!(unknown[0]["detail"]["declaration"].is_null());
    assert_eq!(unknown.len(), 2);
    for source in [
        "let x=1; with(obj){x;} x;",
        "let x=1; eval('x=2'); x;",
        "let x=1; (eval)('x=2'); x;",
        "let x=1; const eval=()=>0; eval('x=2'); x;",
    ] {
        let (_dir, path) = fixture(source, "dynamic.js");
        let rows = jsonl(&invoke("xref", &path, &["--byte", "4"]));
        assert_eq!(rows[0]["detail"]["resolution"], "dynamic");
        assert!(rows[0]["detail"]["declaration"].is_null());
        assert_eq!(rows.len(), 2);
    }
    for source in ["let x=1; eval?.('x=2'); x;", "let x=1; obj.eval('x=2'); x;"] {
        let (_dir, path) = fixture(source, "not-direct-eval.js");
        let rows = jsonl(&invoke("xref", &path, &["--byte", "4"]));
        assert_eq!(rows[0]["detail"]["resolution"], "resolved");
        assert_eq!(rows.len(), 3);
    }
    let (_dir, path) = fixture("const unused=1;", "unused.js");
    let rows = jsonl(&invoke("xref", &path, &["--byte", "6"]));
    assert_eq!(rows[0]["detail"]["resolution"], "resolved");
    assert_eq!(rows.len(), 2);
}

#[test]
fn refs_is_single_file_bounded_atomic_and_semantic_only_on_demand() {
    let (dir, path) = fixture("let x=0; x++; x;", "bounded.js");
    let plain = invoke("xref", &path, &["--byte", "4"]);
    jsonl(&plain);
    assert!(plain.stderr.is_empty());
    assert_eq!(
        invoke("xref", &path, &["--byte", "4", "--jsonl"]).stdout,
        plain.stdout
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
        failure(
            invoke("xref", &path, &["--byte", byte]),
            1,
            "INVALID_OFFSET",
        );
    }
    failure(invoke("xref", &path, &["--byte", "0"]), 1, "INVALID_TARGET");
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
    assert_eq!(reports[0]["limits"]["semantic_nodes"], 1_000_000);
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
fn recursive_queries_preserve_sorted_per_file_streams_and_content_ids() {
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
        let mut count = 0;
        for path in [&a, &b] {
            let single = invoke(command, path, &args);
            let records = jsonl(&single);
            count += records.last().unwrap()["records"].as_u64().unwrap();
            expected.extend_from_slice(&single.stdout);
        }
        let mut recursive_args = args.clone();
        let output = invoke(command, dir.path(), &recursive_args);
        let mut explicit_args = args.clone();
        explicit_args.push("-r");
        let explicit = invoke(command, dir.path(), &explicit_args);
        success(&explicit);
        assert_eq!(explicit.stdout, output.stdout);
        success(&output);
        assert!(output.stderr.is_empty());
        assert!(output.stdout.starts_with(&expected));
        let summary: Value = serde_json::from_slice(&output.stdout[expected.len()..]).unwrap();
        assert_eq!(summary["record"], "scan_summary");
        assert_eq!(summary["detail"]["files"], 2);
        assert_eq!(summary["records"], count);
        assert_eq!(summary["complete"], true);
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
    assert_eq!(rows.len(), 7);
    assert_eq!(rows[0]["detail"]["source_type"], "module");
    assert_eq!(rows[1]["detail"]["kind"], "program");
    assert_eq!(rows[3]["detail"]["source_type"], "script");
    assert_eq!(rows[4]["detail"]["kind"], "program");
    assert_eq!(rows[6]["records"], 2);
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
fn recursive_empty_scans_and_explicit_files_are_complete() {
    let dir = tempfile::tempdir().unwrap();
    for command in ["entities", "find"] {
        let mut args = vec![];
        if command == "find" {
            args.extend(["-f", "absent"]);
        }
        let output = invoke(command, dir.path(), &args);
        success(&output);
        let summary: Value = js_object(&output.stdout);
        assert_eq!(summary["record"], "scan_summary");
        assert_eq!(summary["detail"]["files"], 0);
        assert_eq!(summary["records"], 0);
        assert_eq!(summary["complete"], true);
    }
    failure(invoke("find", dir.path(), &["-f", ""]), 2, "INVALID_QUERY");
    let file = dir.path().join("input.txt");
    fs::write(&file, "const x = 1").unwrap();
    let single = outline_rows(&file, &[]);
    assert_eq!(single[0]["detail"]["path"], file.to_str().unwrap());
    let output = invoke("entities", &file, &["-r"]);
    success(&output);
    let summary: Value =
        serde_json::from_slice(output.stdout.split(|&b| b == b'\n').rev().nth(1).unwrap()).unwrap();
    assert_eq!(summary["detail"]["files"], 1);
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
        invoke("entities", dir.path(), &["-r", "--within", "dummy"]),
        2,
        "cannot be used",
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
        let mut explicit = args.clone();
        explicit.push("-r");
        refused(invoke(command, dir.path(), &explicit));
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
        invoke("entities", &dir.path().join("missing"), &["-r"]),
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
    let explicit = invoke("find", dir.path(), &["--recursive", "-f", "needle"]);
    success(&explicit);
    assert_eq!(explicit.stdout, output.stdout);
    success(&output);
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[3]["detail"]["files"], 1);
    assert_eq!(rows[3]["records"], 1);
    failure(
        invoke("entities", &dir.path().join("cycle"), &[]),
        1,
        "INVALID_INPUT",
    );
    failure(
        invoke("entities", &dir.path().join("cycle"), &["-r"]),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn search_then_extract_and_locate_complete_function() {
    let path = example();
    let found = jsonl(&invoke("find", &path, &["-f", "/api/login", "--jsonl"]));
    assert_eq!(found.len(), 4);
    assert_eq!(found[1]["parent_at"], found[0]["detail"]["root_id"]);
    let id = found[2]["parent_at"].as_str().unwrap();
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
        &["--byte", &found[2]["start"].to_string(), "--json"],
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
    assert_eq!(
        records[0]["source_id"],
        format!("{:x}", Sha256::digest(&source))
    );
    let entities = &records[1..records.len() - 1];
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
    assert_eq!(filtered.len(), 3);
    assert_eq!(filtered[1]["detail"]["name"], "inner");
    assert_eq!(filtered[1]["detail"]["depth"], 2);
    assert_eq!(filtered[1]["parent_at"], id);
    assert_eq!(
        outline_rows(&path, &["--within", id, "--depth", "0"]).len(),
        2
    );
    let root_only = outline_rows(&path, &["--depth", "0"]);
    assert_eq!(root_only.len(), 3);
    assert_eq!(root_only[1]["detail"]["kind"], "program");
}

#[test]
fn get_json_preserves_original_source_for_functions_arrows_classes_and_members() {
    let path = example();
    let bytes = fs::read(&path).unwrap();
    let records = outline_rows(&path, &[]);
    for entity in &records[1..records.len() - 1] {
        let output = invoke(
            "slice",
            &path,
            &["--at", entity["at"].as_str().unwrap(), "--json"],
        );
        success(&output);
        let extracted: Value = js_object(&output.stdout);
        assert_eq!(extracted["detail"]["representation"], "original");
        assert_eq!(extracted["detail"]["entity"], *entity);
        assert_eq!(extracted["detail"]["complete"], true);
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
    let id = found[1]["parent_at"].as_str().unwrap();
    let get = invoke("slice", &path, &["--at", id]);
    success(&get);
    assert_eq!(get.stdout, "function 登录(){return \"你好😀\";}".as_bytes());
    let position = source.find("你好").unwrap() + 1;
    let located = invoke("locate", &path, &["--byte", &position.to_string()]);
    success(&located);
    assert_eq!(js_object(&located.stdout)["detail"]["entity"]["at"], id);
    let records = outline_rows(&path, &[]);
    assert_eq!(records[1]["start"], 0);
    assert_eq!(records[1]["end"], source.len());
    assert_eq!(
        invoke(
            "slice",
            &path,
            &["--at", records[1]["at"].as_str().unwrap()]
        )
        .stdout,
        source.as_bytes()
    );
}

#[test]
fn identifiers_find_lexical_occurrences_without_static_property_names() {
    let source = "let x=1; function f(x){ return x + obj.x + obj[x] + external; }";
    let (_dir, path) = fixture(source, "identifiers.js");
    let rows = jsonl(&invoke("identifiers", &path, &["--name", "x"]));
    let identifiers: Vec<_> = rows[1..rows.len() - 1].to_vec();
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
    assert_eq!(refs.last().unwrap()["records"], 2);
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
    assert_eq!(within.last().unwrap()["records"], 3);
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
    let entities: Vec<_> = functions[1..functions.len() - 1].to_vec();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0]["detail"]["name"], "alpha");
    let named = outline_rows(&path, &["--name", "beta"]);
    assert_eq!(named.last().unwrap()["records"], 1);
    assert_eq!(named[1]["detail"]["label"], "beta");
    let method = outline_rows(&path, &["--name", "delta"]);
    assert_eq!(method.last().unwrap()["records"], 1);
    assert_eq!(method[1]["detail"]["kind"], "class_method");
}

#[test]
fn calls_find_direct_and_member_call_sites_without_runtime_semantics() {
    let source = "function f(){ foo(1); obj.foo(2,3); new Foo(); bar?.(); }";
    let (_dir, path) = fixture(source, "calls.js");
    let foo = jsonl(&invoke("calls", &path, &["--name", "foo"]));
    let records: Vec<_> = foo[1..foo.len() - 1].to_vec();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["detail"]["call_kind"], "direct");
    assert_eq!(records[0]["detail"]["argument_count"], 1);
    assert_eq!(records[1]["detail"]["call_kind"], "member");
    assert_eq!(records[1]["detail"]["argument_count"], 2);
    let ctor = jsonl(&invoke("calls", &path, &["--name", "Foo"]));
    assert_eq!(ctor.last().unwrap()["records"], 1);
    assert_eq!(ctor[1]["detail"]["call_kind"], "new");
    let optional = jsonl(&invoke("calls", &path, &["--name", "bar"]));
    assert_eq!(optional[1]["detail"]["optional"], true);
}

#[test]
fn members_find_static_names_without_computed_dynamic_properties() {
    let source = r#"const obj={ alpha:1, beta(){}, get gamma(){return 1} }; obj.delta; obj[computed]; obj["theta"]; obj["io"+"ta"]; obj[1]; class C{ epsilon=1; zeta(){} } const el=<A eta="x" />;"#;
    let (_dir, path) = fixture(source, "members.jsx");

    let delta = jsonl(&invoke("members", &path, &["--name", "delta"]));
    assert_eq!(delta.len(), 3);
    assert_eq!(delta[1]["detail"]["member_kind"], "static_member");
    assert_eq!(delta[1]["detail"]["key_resolution"], Value::Null);

    let alpha = jsonl(&invoke("members", &path, &["--name", "alpha"]));
    assert_eq!(alpha[1]["detail"]["member_kind"], "property_key");

    let beta = jsonl(&invoke("members", &path, &["--name", "beta"]));
    assert_eq!(beta[1]["detail"]["member_kind"], "object_method_key");

    let gamma = jsonl(&invoke("members", &path, &["--name", "gamma"]));
    assert_eq!(gamma[1]["detail"]["member_kind"], "object_method_key");

    let epsilon = jsonl(&invoke("members", &path, &["--name", "epsilon"]));
    assert_eq!(epsilon[1]["detail"]["member_kind"], "class_field_key");

    let zeta = jsonl(&invoke("members", &path, &["--name", "zeta"]));
    assert_eq!(zeta[1]["detail"]["member_kind"], "class_method_key");

    let eta = jsonl(&invoke("members", &path, &["--name", "eta"]));
    assert_eq!(eta[1]["detail"]["member_kind"], "jsx_attribute_name");

    // A computed access with a literal or constant-folded key is a property key; the resolved
    // name is matched, and the record keeps the key resolution for the caller.
    let theta = jsonl(&invoke("members", &path, &["--name", "theta"]));
    assert_eq!(theta.len(), 3);
    assert_eq!(theta[1]["detail"]["member_kind"], "computed_key");
    assert_eq!(theta[1]["detail"]["key_resolution"], "literal");
    assert_eq!(theta[1]["detail"]["name"], "theta");
    let iota = jsonl(&invoke("members", &path, &["--name", "iota"]));
    assert_eq!(iota.len(), 3);
    assert_eq!(iota[1]["detail"]["member_kind"], "computed_key");
    assert_eq!(iota[1]["detail"]["key_resolution"], "static");
    assert_eq!(iota[1]["detail"]["name"], "iota");
    // `theta`/`iota` only ever appear as keys, so an unrelated name must not match them.
    let unrelated = jsonl(&invoke("members", &path, &["--name", "iota2"]));
    assert_eq!(unrelated.last().unwrap()["records"], 0);

    // Computed access `obj[computed]` is a lexical reference, not a static member name, and a
    // numeric index is not a property name either.
    let computed = jsonl(&invoke("members", &path, &["--name", "computed"]));
    assert_eq!(computed.last().unwrap()["records"], 0);
    let numeric = jsonl(&invoke("members", &path, &["--name", "1"]));
    assert_eq!(numeric.last().unwrap()["records"], 0);
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
    assert_eq!(plain.last().unwrap()["records"], 0);

    let resolved = jsonl(&invoke(
        "find",
        &path,
        &["-f", "setInterval", "--resolve-static"],
    ));
    assert_eq!(resolved.len(), 3);
    assert_eq!(resolved[1]["detail"]["value"], "setInterval");
    assert_eq!(resolved[1]["detail"]["literal_kind"], "static_expression");
    assert_eq!(resolved[1]["detail"]["resolution"], "static");
    assert_eq!(resolved[1]["detail"]["raw"], "'set'+'Interval'");
    assert_eq!(resolved[1]["start"], 8);

    let flag = jsonl(&invoke(
        "find",
        &path,
        &["-f", "flag_correct", "--resolve-static"],
    ));
    assert_eq!(flag[1]["detail"]["value"], "flag_correct");
    let timeout = jsonl(&invoke(
        "find",
        &path,
        &["-f", "setTimeout", "--resolve-static"],
    ));
    assert_eq!(timeout[1]["detail"]["value"], "setTimeout");
    let array = jsonl(&invoke(
        "find",
        &path,
        &["-f", "window", "--resolve-static"],
    ));
    assert!(
        array[1..array.len() - 1]
            .iter()
            .any(|row| row["detail"]["literal_kind"] == "static_expression")
    );
    let unsafe_result = jsonl(&invoke(
        "find",
        &path,
        &["-f", "unsafe", "--resolve-static"],
    ));
    assert_eq!(unsafe_result[1]["detail"]["literal_kind"], "string");

    // The skip guards must keep every foldable form and reject unfoldable operands,
    // including a minified-style `+` chain whose spine ends in an opaque call.
    let codepoint = jsonl(&invoke("find", &path, &["-f", "CD", "--resolve-static"]));
    assert_eq!(codepoint.len(), 3);
    assert_eq!(codepoint[1]["detail"]["literal_kind"], "static_expression");
    assert_eq!(codepoint[1]["detail"]["value"], "CD");
    let method = jsonl(&invoke("find", &path, &["-f", "ABC", "--resolve-static"]));
    assert_eq!(method.len(), 3);
    assert_eq!(method[1]["detail"]["literal_kind"], "static_expression");
    assert_eq!(method[1]["detail"]["value"], "ABC");
    let chain = jsonl(&invoke(
        "find",
        &path,
        &["-f", "_joined", "--resolve-static"],
    ));
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[1]["detail"]["literal_kind"], "string");
    let receiver = jsonl(&invoke("find", &path, &["-f", "PLAIN", "--resolve-static"]));
    assert_eq!(receiver.len(), 2);
}

#[test]
fn escaped_literals_and_templates_use_decoded_values_without_crossing_interpolation() {
    let source =
        r#"function f(){return ["\x2fapi\u002flogin", `/api/${user}/login`, `/api/login`]}"#;
    let (_dir, path) = fixture(source, "bundle.js");
    let found = jsonl(&invoke("find", &path, &["-f", "/api/login"]));
    assert_eq!(found.len(), 4);
    assert_eq!(found[1]["detail"]["value"], "/api/login");
    assert_eq!(found[2]["detail"]["dynamic"], false);
    let parts = jsonl(&invoke("find", &path, &["-f", "/api/"]));
    assert_eq!(parts.len(), 5);
    assert_eq!(parts[2]["detail"]["dynamic"], true);
    assert_eq!(jsonl(&invoke("find", &path, &["-f", "/API/"])).len(), 2);
}

#[test]
fn lone_surrogates_are_preserved_and_do_not_match_replacement_characters() {
    let (_dir, path) = fixture(r#"const x = "prefix\uD800\uFFFDsuffix";"#, "bundle.js");
    let output = invoke("find", &path, &["-f", "prefix"]);
    success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    let header: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(header["schema_version"], 2);
    assert!(lines[1].contains("\"detail\":{\"literal_kind\":\"string\",\"value\":\"\\u0070\\u0072\\u0065\\u0066\\u0069\\u0078\\ud800\\ufffd\\u0073\\u0075\\u0066\\u0066\\u0069\\u0078\""));
    // A literal has no navigable identity of its own: `at` is null and the enclosing
    // structure is the copy-only short coordinate.
    assert!(lines[1].contains("\"at\":null"));
    assert!(lines[1].contains("\"parent_at\":\""));
    let summary: Value = serde_json::from_str(lines[2]).unwrap();
    assert_eq!(summary["records"], 1);
    assert_eq!(summary["complete"], true);
    // A genuine replacement character remains independently searchable.
    assert_eq!(jsonl(&invoke("find", &path, &["-f", "prefix�"])).len(), 2);
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
    assert_eq!(found.len(), 4);
    assert_eq!(found[2]["detail"]["literal_kind"], "property_key");
}

#[test]
fn same_content_has_same_identity_and_changed_content_rejects_old_ids() {
    let (dir, path) = fixture("function a(){}", "one.js");
    let records = outline_rows(&path, &[]);
    let id = records[2]["at"].as_str().unwrap();
    let other = dir.path().join("two.js");
    fs::copy(&path, &other).unwrap();
    let copied = outline_rows(&other, &[]);
    assert_eq!(records[2]["at"], copied[2]["at"]);
    let stale = js_full_id(&records[0], id);
    fs::write(&path, "function b(){}").unwrap();
    failure(
        invoke("slice", &path, &["--at", &stale]),
        1,
        "SOURCE_MISMATCH",
    );
}

#[test]
fn forged_entity_bounds_kinds_and_versions_are_rejected() {
    let (_dir, path) = fixture("function a(){}", "bundle.js");
    let records = outline_rows(&path, &[]);
    let source_id = records[0]["source_id"].as_str().unwrap();
    for id in [
        format!("rxd1:js:{source_id}:function_declaration:0:3"),
        format!("rxd1:js:{source_id}:arrow_function:0:14"),
        format!("rxd2:js:{source_id}:program:0:14"),
        "garbage".into(),
    ] {
        failure(invoke("slice", &path, &["--at", &id]), 1, "INVALID_ID");
        failure(
            invoke("entities", &path, &["--within", &id]),
            1,
            "INVALID_ID",
        );
    }
}

#[test]
fn auto_and_explicit_source_modes_handle_esm_without_accepting_top_level_return() {
    let (_dir, path) = fixture("export const f=()=>1;", "bundle.js");
    assert_eq!(
        outline_rows(&path, &[])[0]["detail"]["source_type"],
        "module"
    );
    failure(
        invoke("entities", &path, &["--source-type", "script"]),
        1,
        "INVALID_INPUT",
    );
    let (_dir, cjs) = fixture("export const f=()=>1;", "bundle.cjs");
    failure(invoke("entities", &cjs, &[]), 1, "INVALID_INPUT");
    assert_eq!(
        outline_rows(&cjs, &["--source-type", "module"])[0]["detail"]["source_type"],
        "module"
    );
    let (_dir, mjs) = fixture("const f=()=>1;", "bundle.mjs");
    assert_eq!(
        outline_rows(&mjs, &[])[0]["detail"]["source_type"],
        "module"
    );
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
    assert_eq!(records.len(), 3);
    assert_eq!(records[1]["start"], 0);
    assert_eq!(records[1]["end"], 0);
    assert_eq!(jsonl(&invoke("find", &path, &["-f", "absent"])).len(), 2);
    failure(
        invoke("locate", &path, &["--byte", "0"]),
        1,
        "INVALID_OFFSET",
    );
    let (_dir, path) = fixture("()=>1", "bundle.js");
    failure(
        invoke("locate", &path, &["--byte", "5"]),
        1,
        "INVALID_OFFSET",
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
    failure(invoke("slice", &example(), &[]), 2, "error:");
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
    let id = records[2]["at"].as_str().unwrap();
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
    assert_eq!(jsonl(&invoke("find", &path, &["-f", "unicode"])).len(), 2);
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
        assert_eq!(analysis["source_id"], entities[0]["source_id"]);
        assert_eq!(
            analysis["counters"]["input_bytes"],
            fs::metadata(&path).unwrap().len()
        );
        assert_eq!(analysis["counters"]["entities"], 11);
        assert_eq!(analysis["limits"]["input_bytes"], 8u64 << 30);
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
        ("slice", vec!["--at", "invalid"], "INVALID_ID", "query"),
        (
            "locate",
            vec!["--byte", "999999"],
            "INVALID_OFFSET",
            "query",
        ),
    ] {
        let mut arguments = arguments;
        arguments.push("--perf");
        let output = invoke(command, &example(), &arguments);
        assert_eq!(output.status.code(), Some(1));
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
            format!(
                "{:x}",
                Sha256::digest(fs::read(fixture["path"].as_str().unwrap()).unwrap())
            )
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
            if case["name"] == "find_get" {
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
        let source_id = format!("{:x}", Sha256::digest(fs::read(&path).unwrap()));
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
            assert_eq!(rows[0]["detail"]["resolution"], "static");
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
            let keys: Vec<_> = rows[1..rows.len() - 1]
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
        assert_eq!(case["source_id"], format!("{:x}", Sha256::digest(&bytes)));
        let rows = jsonl(&invoke(
            "xref",
            &path,
            &["--byte", &case["byte"].to_string()],
        ));
        assert_eq!(rows[0]["source_id"], case["source_id"]);
        assert_eq!(rows[0]["detail"]["resolution"], case["resolution"]);
        assert_eq!(rows[0]["detail"]["selected"]["name"], case["identifier"]);
        assert_eq!(
            rows[0]["detail"]["selected"]["start"],
            case["declaration"]["start"]
        );
        assert_eq!(
            rows[0]["detail"]["selected"]["end"],
            case["declaration"]["end"]
        );
        // Detail coordinates are the same copy-only form as row coordinates.
        let entity_id = format!(
            "{}:{}:{}",
            case["entity"]["kind"].as_str().unwrap(),
            case["entity"]["start"],
            case["entity"]["end"]
        );
        if case["resolution"] == "resolved" {
            let mut declaration = case["declaration"].clone();
            declaration["entity_id"] = serde_json::json!(entity_id);
            assert_eq!(rows[0]["detail"]["declaration"], declaration);
        }
        let actual: Vec<_> = rows[1..rows.len()-1].iter().map(|row| {
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
        if let Some(reference) = case["references"].as_array().unwrap().first() {
            let selected = jsonl(&invoke(
                "xref",
                &path,
                &["--byte", &reference["start"].to_string()],
            ));
            assert_eq!(selected[0]["detail"]["selected"]["role"], "reference");
            assert_eq!(
                selected[0]["detail"]["declaration"],
                rows[0]["detail"]["declaration"]
            );
            assert_eq!(selected[1..], rows[1..]);
        }
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
        let entities = &outline[1..outline.len() - 1];
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
        let actual_matches: Vec<_> = found[1..found.len() - 1].iter().map(|m| serde_json::json!({
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
        cases.iter().find(|c| c["name"] == "find_get").unwrap()["status"],
        "not_applicable"
    );
    assert_eq!(
        cases.iter().find(|c| c["name"] == "find").unwrap()["samples"][0]["exit_code"],
        0
    );
    assert_eq!(
        cases.iter().find(|c| c["name"] == "get").unwrap()["status"],
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

/// Every fetch takes `--at` or `--from`: the same selection, read from the previous stream.
/// One table for the backends whose fetch takes a single unit, so the loop is checked once
/// per backend instead of once per spelling.
#[test]
fn fetch_from_a_stream_matches_at_for_each_single_unit_backend() {
    use std::process::Stdio;
    let cases: Vec<(&str, Vec<&str>, Vec<&str>, String)> = vec![
        (
            "wasm",
            vec!["wasm", "functions"],
            vec!["wasm", "get-function"],
            "tests/fixtures/wasm/fixture.wasm".to_owned(),
        ),
        (
            "pyc",
            vec!["pyc", "codes"],
            vec!["pyc", "get-code"],
            "tests/fixtures/pyc/hello.3.12.pyc".to_owned(),
        ),
        (
            "evm",
            vec!["evm", "interface", "ENC", "hex"],
            vec!["evm", "get-function", "ENC", "hex"],
            "tests/fixtures/evm/storage-runtime.hex".to_owned(),
        ),
        (
            "html",
            vec!["html", "entries"],
            vec!["html", "extract", "-o", "OUT"],
            "tests/fixtures/html/catalog.html".to_owned(),
        ),
    ];
    for (name, listing, fetch, fixture) in cases {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(&fixture);
        let path = path.to_str().unwrap();
        let listed = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(
                listing
                    .iter()
                    .map(|arg| if *arg == "ENC" { "--encoding" } else { arg }),
            )
            .arg(path)
            .output()
            .unwrap();
        success(&listed);
        let rows = jsonl(&listed);
        let at = rows[1]["at"].as_str().unwrap().to_owned();

        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("unit.bin");
        let args: Vec<String> = fetch
            .iter()
            .map(|arg| match *arg {
                "ENC" => "--encoding".to_owned(),
                "OUT" => destination.to_str().unwrap().to_owned(),
                other => other.to_string(),
            })
            .collect();
        let mut child = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(&args)
            .arg(path)
            .args(["--from", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(&listed.stdout)
                .unwrap();
        }
        drop(child.stdin.take());
        let streamed = child.wait_with_output().unwrap();
        success(&streamed);

        let direct = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(&args)
            .arg(path)
            .args(["--at", &at])
            .output()
            .unwrap();
        success(&direct);
        assert_eq!(streamed.stdout, direct.stdout, "{name}");
    }
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
    let rows = jsonl(&asar("entries", &archive, &["--jsonl"]));
    assert_eq!(rows[0]["backend"], "asar");
    assert_eq!(rows[0]["detail"]["analysis"], "metadata");
    assert_eq!(rows.last().unwrap()["records"], 7);
    assert_eq!(rows.last().unwrap()["detail"]["invalid_ranges"], 0);
    let paths: Vec<_> = rows[1..rows.len() - 1]
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
        assert_eq!(receipt["detail"]["complete"], true);
        assert_eq!(receipt["detail"]["representation"], "original");
        assert_eq!(receipt["detail"]["storage"], storage);
        assert_eq!(receipt["detail"]["origin"]["entry"], name);
        assert_eq!(
            receipt["source_id"],
            format!("{:x}", Sha256::digest(expected))
        );
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
fn asar_extract_then_js_queries_preserve_source_identity_and_exact_navigation() {
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
    let receipt: Value = js_object(&extracted.stdout);
    let rows = outline_rows(&destination, &[]);
    assert_eq!(rows[0]["source_id"], receipt["source_id"]);
    let id = rows.iter().find(|r| r["detail"]["name"] == "run").unwrap()["at"]
        .as_str()
        .unwrap()
        .to_owned();
    // The short coordinate a row prints is content-agnostic; a stale-input check needs the
    // content-bound form, which the header identity lets a caller rebuild.
    let id_full = format!("rxd1:js:{}:{id}", rows[0]["source_id"].as_str().unwrap());
    let get = invoke("slice", &destination, &["--at", &id]);
    success(&get);
    assert_eq!(
        get.stdout,
        b"function run(value){return \xe5\x90\x8d\xe7\xa7\xb0+value;}"
    );
    assert_eq!(
        jsonl(&invoke("find", &destination, &["-f", "你好"]))
            .last()
            .unwrap()["records"],
        1
    );
    let byte = source
        .windows(5)
        .position(|s| s == b"value")
        .unwrap()
        .to_string();
    assert_eq!(
        jsonl(&invoke("xref", &destination, &["--byte", &byte]))
            .last()
            .unwrap()["records"],
        1
    );
    jsonl(&invoke("symbols", &destination, &[]));
    success(&invoke("locate", &destination, &["--byte", &byte]));
    pack_asar(
        &archive,
        &[("main.cjs", b"function run(value){return value+1;}")],
    );
    success(&asar(
        "extract",
        &archive,
        &["--at", "main.cjs", "-o", destination.to_str().unwrap()],
    ));
    failure(
        invoke("slice", &destination, &["--at", &id_full]),
        1,
        "SOURCE_MISMATCH",
    );
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
    assert_eq!(listed.last().unwrap()["records"], 4);
    assert_eq!(listed.last().unwrap()["detail"]["invalid_ranges"], 3);
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
        assert_eq!(
            rows.last().unwrap()["records"],
            case["expected"]["entity_count"]
        );
        for expected in case["expected"]["sample_entities"].as_array().unwrap() {
            assert!(rows.iter().any(|r| r["detail"]["kind"] == expected["kind"]
                && r["start"] == expected["start"]
                && r["end"] == expected["end"]));
        }
    }
}

#[cfg(feature = "backend-dex")]
#[path = "dex/mod.rs"]
mod dex;

#[cfg(feature = "mcp")]
#[path = "mcp/mod.rs"]
mod mcp;
