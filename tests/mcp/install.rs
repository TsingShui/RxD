use super::support::Client;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

fn install(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["mcp", "install"])
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("PATH", home.join("empty-path"))
        .output()
        .unwrap()
}
fn ok(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(feature = "backend-js")]
#[test]
fn mcp_install_pi_recorded_launch_queries_from_another_directory() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("分析 roots");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("sample.js"),
        "function greeting() { return 42; }\n",
    )
    .unwrap();
    let outside = temp.path().join("outside.js");
    fs::write(&outside, "function forbidden() {}\n").unwrap();
    let result = install(&home, &root, &["-a", "pi", "--backends", "js"]);
    ok(&result);
    let config: Value =
        serde_json::from_slice(&fs::read(home.join(".pi/agent/mcp.json")).unwrap()).unwrap();
    let entry = &config["mcpServers"]["rxd"];
    assert_eq!(entry["type"], "stdio");
    assert_eq!(entry["args"][0], "mcp");
    assert_eq!(entry["args"][1], "serve");
    assert!(Path::new(entry["command"].as_str().unwrap()).is_absolute());
    let mut command = Command::new(entry["command"].as_str().unwrap());
    command
        .args(
            entry["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap()),
        )
        .current_dir(temp.path());
    let mut client = Client::start_command(command);
    client.initialize();
    let catalog = client.request("tools/list", json!({}));
    assert!(
        catalog["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "js_entities")
    );
    let query = client.tool("js_entities", json!({"input":{"path":"sample.js"}}));
    assert_ne!(query["isError"], true, "{query}");
    assert!(
        query["structuredContent"]["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["detail"]["name"] == "greeting")
    );
    let denied = client.tool("js_entities", json!({"input":{"path":outside}}));
    assert_eq!(
        denied["structuredContent"]["error"]["code"],
        "ACCESS_DENIED"
    );
}

#[test]
fn mcp_install_json_preserves_settings_previews_and_requires_conflict_replacement() {
    let temp = TempDir::new().unwrap();
    let home = &temp.path().join("home");
    let analysis = temp.path().join("analysis");
    fs::create_dir_all(home).unwrap();
    fs::create_dir(&analysis).unwrap();
    let path = home.join(".pi/agent/mcp.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = br#"{"preference":true,"credential":"dummy-secret","mcpServers":{"other":{"command":"other"},"rxd":{"type":"stdio","command":"unrelated","args":[],"timeout":123,"enabled":false,"env":{"TOKEN":"dummy-secret"}}}}"#;
    fs::write(&path, original).unwrap();
    let conflict = install(home, &analysis, &["-a", "pi"]);
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("conflict"));
    assert_eq!(fs::read(&path).unwrap(), original);
    let preview = install(home, &analysis, &["-a", "pi", "--replace", "--preview"]);
    ok(&preview);
    let report: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(report["targets"][0]["outcome"], "updated");
    assert_eq!(report["preview"], true);
    assert!(!String::from_utf8_lossy(&preview.stdout).contains("dummy-secret"));
    assert_eq!(fs::read(&path).unwrap(), original);
    ok(&install(home, &analysis, &["-a", "pi", "--replace"]));
    let after = fs::read(&path).unwrap();
    let config: Value = serde_json::from_slice(&after).unwrap();
    assert_eq!(config["credential"], "dummy-secret");
    assert_eq!(config["preference"], true);
    assert_eq!(config["mcpServers"]["other"]["command"], "other");
    assert_eq!(config["mcpServers"]["rxd"]["timeout"], 123);
    assert_eq!(config["mcpServers"]["rxd"]["enabled"], false);
    assert_eq!(config["mcpServers"]["rxd"]["env"]["TOKEN"], "dummy-secret");
    let again = install(home, &analysis, &["-a", "pi"]);
    ok(&again);
    assert!(String::from_utf8_lossy(&again.stdout).contains("unchanged"));
    assert_eq!(fs::read(&path).unwrap(), after);
}

#[test]
fn mcp_install_all_native_formats_aliases_overrides_and_cc_legacy_precedence() {
    let temp = TempDir::new().unwrap();
    let home = &temp.path().join("home");
    let analysis = temp.path().join("analysis");
    fs::create_dir_all(home).unwrap();
    fs::create_dir(&analysis).unwrap();
    let pi = home.join("pi override");
    let cc = home.join("cc override");
    let codex = home.join("codex override");
    for dir in [&pi, &cc, &codex] {
        fs::create_dir(dir).unwrap();
    }
    fs::write(
        cc.join(".config.json"),
        r#"{"legacy":true,"mcpServers":{"other":{"command":"kept"}}}"#,
    )
    .unwrap();
    fs::write(
        cc.join(".claude.json"),
        b"modern config must stay byte exact",
    )
    .unwrap();
    fs::write(codex.join("config.toml"), "# keep this comment\npreference = 'preserved'\n[credentials]\ntoken = 'dummy-secret'\n[mcp_servers.other]\ncommand = 'other'\n").unwrap();
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["mcp", "install"])
            .args(args)
            .current_dir(&analysis)
            .env_clear()
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("PATH", home.join("empty-path"))
            .env("PI_CODING_AGENT_DIR", &pi)
            .env("CLAUDE_CONFIG_DIR", &cc)
            .env("CODEX_HOME", &codex)
            .output()
            .unwrap()
    };
    let result = invoke(&[
        "-a",
        "all",
        "-a",
        "claude",
        "-a",
        "claude-code",
        "-a",
        "cc",
        "-a",
        "pi",
    ]);
    ok(&result);
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["targets"].as_array().unwrap().len(), 3);
    assert_eq!(report["targets"][0]["agent"], "pi");
    assert_eq!(report["targets"][1]["agent"], "cc");
    assert_eq!(report["targets"][2]["agent"], "codex");
    let pi_config: Value = serde_json::from_slice(&fs::read(pi.join("mcp.json")).unwrap()).unwrap();
    let cc_config: Value =
        serde_json::from_slice(&fs::read(cc.join(".config.json")).unwrap()).unwrap();
    assert_eq!(cc_config["legacy"], true);
    assert_eq!(cc_config["mcpServers"]["other"]["command"], "kept");
    assert_eq!(
        fs::read(cc.join(".claude.json")).unwrap(),
        b"modern config must stay byte exact"
    );
    let codex_bytes = fs::read_to_string(codex.join("config.toml")).unwrap();
    assert!(codex_bytes.starts_with("# keep this comment\n"));
    let codex_config: toml::Value = toml::from_str(&codex_bytes).unwrap();
    assert_eq!(codex_config["preference"].as_str(), Some("preserved"));
    assert_eq!(
        codex_config["credentials"]["token"].as_str(),
        Some("dummy-secret")
    );
    assert_eq!(
        codex_config["mcp_servers"]["other"]["command"].as_str(),
        Some("other")
    );
    assert_eq!(
        codex_config["mcp_servers"]["rxd"]["command"].as_str(),
        pi_config["mcpServers"]["rxd"]["command"].as_str()
    );
    assert_eq!(
        cc_config["mcpServers"]["rxd"]["args"],
        pi_config["mcpServers"]["rxd"]["args"]
    );
    assert!(!home.join(".pi").exists());
    assert!(!home.join(".codex").exists());
    assert!(!home.join(".claude.json").exists());
    ok(&invoke(&[])); // Existing override directories are detection evidence.
    assert_eq!(
        fs::read_to_string(codex.join("config.toml")).unwrap(),
        codex_bytes
    );
    fs::write(cc.join(".config.json"), "invalid legacy").unwrap();
    let invalid = invoke(&["-a", "all"]);
    assert!(!invalid.status.success());
    assert_eq!(
        fs::read_to_string(codex.join("config.toml")).unwrap(),
        codex_bytes
    );
    assert_eq!(
        fs::read(cc.join(".claude.json")).unwrap(),
        b"modern config must stay byte exact"
    );
}

#[test]
fn mcp_install_rejects_ambiguous_or_invalid_configs_and_preflights_all_targets() {
    let temp = TempDir::new().unwrap();
    let home = &temp.path().join("home");
    let analysis = temp.path().join("analysis");
    fs::create_dir_all(home).unwrap();
    fs::create_dir(&analysis).unwrap();
    let path = home.join(".pi/agent/mcp.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for invalid in [
        "not json",
        "[]",
        r#"{"mcpServers":[]}"#,
        r#"{"mcpServers":{"rxd":[]}}"#,
        r#"{"mcpServers":{"rxd":{"command":42,"args":[]}}}"#,
        r#"{"mcpServers":{"rxd":{"command":"rxd","args":[42]}}}"#,
        r#"{"mcpServers":{},"mcpServers":{}}"#,
    ] {
        fs::write(&path, invalid).unwrap();
        let output = install(home, &analysis, &["-a", "all", "--replace"]);
        assert!(!output.status.success(), "accepted {invalid}");
        assert_eq!(fs::read(&path).unwrap(), invalid.as_bytes());
        assert!(!home.join(".claude.json").exists());
        assert!(!home.join(".codex/config.toml").exists());
    }
    fs::remove_file(&path).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    for invalid in [
        "not toml {",
        "mcp_servers = []",
        "[mcp_servers]\nrxd = 1",
        "[mcp_servers.rxd]\nargs = [1]",
    ] {
        fs::write(home.join(".codex/config.toml"), invalid).unwrap();
        let output = install(home, &analysis, &["-a", "all"]);
        assert!(!output.status.success());
        assert!(!path.exists());
        assert!(!home.join(".claude.json").exists());
        assert_eq!(
            fs::read(home.join(".codex/config.toml")).unwrap(),
            invalid.as_bytes()
        );
    }
}

#[test]
fn mcp_install_filesystem_and_launch_argument_failures_do_not_write() {
    let temp = TempDir::new().unwrap();
    let home = &temp.path().join("home");
    let analysis = temp.path().join("analysis");
    fs::create_dir_all(home).unwrap();
    fs::create_dir(&analysis).unwrap();
    for args in [
        vec![],
        vec!["-a", "unknown"],
        vec!["-a", "all", "--root", "missing"],
        vec!["-a", "all", "--base", "missing"],
        vec!["-a", "all", "--backends", "not-a-backend"],
    ] {
        let result = install(home, &analysis, &args);
        assert!(!result.status.success());
        assert_eq!(fs::read_dir(home).unwrap().count(), 0);
    }
    let preview = install(home, &analysis, &["-a", "all", "--preview"]);
    ok(&preview);
    assert_eq!(fs::read_dir(home).unwrap().count(), 0);
    // A known invalid parent must be caught before the earlier Pi target is written.
    fs::write(home.join(".codex"), "not a directory").unwrap();
    let result = install(home, &analysis, &["-a", "all"]);
    assert!(!result.status.success());
    assert!(!home.join(".pi/agent/mcp.json").exists());
    assert!(!home.join(".claude.json").exists());
    assert_eq!(fs::read(home.join(".codex")).unwrap(), b"not a directory");
}

#[test]
fn mcp_install_detects_disabled_pi_native_mcp_without_enabling_it() {
    let temp = TempDir::new().unwrap();
    let home = &temp.path().join("home");
    let analysis = temp.path().join("analysis");
    fs::create_dir_all(home).unwrap();
    fs::create_dir(&analysis).unwrap();
    let dir = home.join(".pi/agent");
    fs::create_dir_all(&dir).unwrap();
    let settings =
        br#"{"extensions":["-builtin:mcp","/private/mcp-bridge.ts"],"secret":"dummy-secret"}"#;
    fs::write(dir.join("settings.json"), settings).unwrap();
    let result = install(home, &analysis, &["-a", "pi"]);
    ok(&result);
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        report["targets"][0]["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("native_mcp_disabled"))
    );
    assert!(
        report["targets"][0]["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("possible_mcp_extension_override"))
    );
    assert!(!String::from_utf8_lossy(&result.stdout).contains("dummy-secret"));
    assert_eq!(fs::read(dir.join("settings.json")).unwrap(), settings);
}

#[cfg(unix)]
#[test]
fn mcp_install_preflights_read_only_files_preserves_permissions_and_refuses_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = TempDir::new().unwrap();
    let home = &temp.path().join("home");
    let analysis = temp.path().join("analysis");
    fs::create_dir_all(home).unwrap();
    fs::create_dir(&analysis).unwrap();
    let pi = home.join(".pi/agent/mcp.json");
    fs::create_dir_all(pi.parent().unwrap()).unwrap();
    fs::write(&pi, "{}").unwrap();
    fs::set_permissions(&pi, fs::Permissions::from_mode(0o600)).unwrap();
    let cc = home.join(".claude.json");
    fs::write(&cc, "{}").unwrap();
    fs::set_permissions(&cc, fs::Permissions::from_mode(0o400)).unwrap();
    let failure = install(home, &analysis, &["-a", "pi", "-a", "cc"]);
    assert!(!failure.status.success());
    assert_eq!(fs::read(&pi).unwrap(), b"{}");
    assert_eq!(fs::read(&cc).unwrap(), b"{}");
    ok(&install(home, &analysis, &["-a", "pi"]));
    assert_eq!(
        fs::metadata(&pi).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let bytes = fs::read(&pi).unwrap();
    fs::remove_file(&pi).unwrap();
    symlink(&cc, &pi).unwrap();
    assert!(
        !install(home, &analysis, &["-a", "pi", "--replace"])
            .status
            .success()
    );
    assert_eq!(fs::read(&cc).unwrap(), b"{}");
    assert!(fs::symlink_metadata(&pi).unwrap().file_type().is_symlink());
    assert!(!bytes.is_empty());
}

#[cfg(feature = "backend-js")]
#[test]
fn mcp_install_relocation_all_recorded_launches_and_remote_conflict_replacement() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("分析 roots");
    let other_cwd = temp.path().join("different cwd");
    for dir in [&home, &root, &other_cwd] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(
        root.join("sample.js"),
        "function greeting() { return 42; }\n",
    )
    .unwrap();
    fs::write(other_cwd.join("outside.js"), "function forbidden() {}\n").unwrap();
    ok(&install(&home, &root, &["-a", "all", "--backends", "js"]));
    let relocated =
        temp.path()
            .join("binary 空间")
            .join(if cfg!(windows) { "rxd.exe" } else { "rxd" });
    fs::create_dir_all(relocated.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_rxd"), &relocated).unwrap();
    let output = Command::new(&relocated)
        .args(["mcp", "install", "-a", "all"])
        .current_dir(&other_cwd)
        .env_clear()
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("PATH", home.join("empty-path"))
        .env("RXD_CACHE_DIR", home.join("absent-engine-cache"))
        .output()
        .unwrap();
    ok(&output);
    assert!(!home.join("absent-engine-cache").exists());
    let configs = [
        home.join(".pi/agent/mcp.json"),
        home.join(".claude.json"),
        home.join(".codex/config.toml"),
    ];
    for (index, path) in configs.iter().enumerate() {
        let mut config: Value = if index == 2 {
            serde_json::to_value(
                toml::from_str::<toml::Value>(&fs::read_to_string(path).unwrap()).unwrap(),
            )
            .unwrap()
        } else {
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
        };
        let servers = if index == 2 {
            "mcp_servers"
        } else {
            "mcpServers"
        };
        let entry = &config[servers]["rxd"];
        assert_eq!(
            Path::new(entry["command"].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            relocated.canonicalize().unwrap()
        );
        assert!(
            entry["args"]
                .as_array()
                .unwrap()
                .contains(&json!(root.canonicalize().unwrap()))
        );
        assert!(entry["args"].as_array().unwrap().contains(&json!("js")));
        let mut command = Command::new(entry["command"].as_str().unwrap());
        command
            .args(
                entry["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap()),
            )
            .current_dir(&other_cwd);
        let mut client = Client::start_command(command);
        client.initialize();
        let catalog = client.request("tools/list", json!({}));
        assert!(
            catalog["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == "js_entities")
        );
        let result = client.tool("js_entities", json!({"input":{"path":"sample.js"}}));
        assert_ne!(result["isError"], true, "{result}");
        assert!(
            result["structuredContent"]["records"]
                .as_array()
                .unwrap()
                .iter()
                .any(|record| record["detail"]["name"] == "greeting")
        );
        assert_eq!(
            client.tool(
                "js_entities",
                json!({"input":{"path":other_cwd.join("outside.js")}})
            )["structuredContent"]["error"]["code"],
            "ACCESS_DENIED"
        );
        // An ambiguous URL definition must be replaced even when command/args match.
        config[servers]["rxd"]["command"] = json!(env!("CARGO_BIN_EXE_rxd"));
        config[servers]["rxd"]["url"] = json!("https://invalid.example/mcp");
        if index == 2 {
            fs::write(path, toml::to_string(&config).unwrap()).unwrap();
        } else {
            fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
        }
        let agent = ["pi", "cc", "codex"][index];
        assert!(!install(&home, &root, &["-a", agent]).status.success());
        let replaced = install(
            &home,
            &root,
            &["-a", agent, "--replace", "--backends", "js"],
        );
        ok(&replaced);
        let report: Value = serde_json::from_slice(&replaced.stdout).unwrap();
        assert_eq!(report["targets"][0]["outcome"], "updated");
        assert!(
            !fs::read_to_string(path)
                .unwrap()
                .contains("invalid.example")
        );
    }
}

#[test]
fn mcp_install_recognizes_current_renamed_executable_but_conflicts_on_old_custom_name() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    let analysis = temp.path().join("analysis");
    fs::create_dir(&analysis).unwrap();
    let custom = temp.path().join(if cfg!(windows) {
        "分析 rxd custom.exe"
    } else {
        "分析 rxd custom"
    });
    fs::copy(env!("CARGO_BIN_EXE_rxd"), &custom).unwrap();
    let invoke = |replace: bool| {
        let mut command = Command::new(&custom);
        command
            .args(["mcp", "install", "-a", "pi"])
            .current_dir(&analysis)
            .env_clear()
            .env("HOME", &home)
            .env("USERPROFILE", &home);
        if replace {
            command.arg("--replace");
        }
        command.output().unwrap()
    };
    ok(&invoke(false));
    let config = home.join(".pi/agent/mcp.json");
    let bytes = fs::read(&config).unwrap();
    let repeat = invoke(false);
    ok(&repeat);
    assert_eq!(fs::read(&config).unwrap(), bytes);
    assert!(!install(&home, &analysis, &["-a", "pi"]).status.success());
    assert_eq!(fs::read(&config).unwrap(), bytes);
    ok(&install(&home, &analysis, &["-a", "pi", "--replace"]));
    assert_ne!(fs::read(&config).unwrap(), bytes);
    for name in ["rxd.wrapper", "rxd.sh", "rxd.py"] {
        let original = serde_json::to_vec(&json!({"mcpServers":{"rxd":{
            "command":format!("/unrelated/{name}"), "args":["mcp", "serve"],
            "timeout":123
        }}}))
        .unwrap();
        fs::write(&config, &original).unwrap();
        let conflict = install(&home, &analysis, &["-a", "pi"]);
        assert!(!conflict.status.success(), "silently took over {name}");
        let report: Value = serde_json::from_slice(&conflict.stdout).unwrap();
        assert_eq!(report["targets"][0]["outcome"], "conflict");
        assert_eq!(fs::read(&config).unwrap(), original);
        ok(&install(&home, &analysis, &["-a", "pi", "--replace"]));
        let replaced: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
        assert_eq!(replaced["mcpServers"]["rxd"]["timeout"], 123);
    }
    for name in ["rxd", "rxd.exe"] {
        fs::write(
            &config,
            serde_json::to_vec(&json!({"mcpServers":{"rxd":{
                "command":format!("/old/{name}"), "args":["mcp", "serve"]
            }}}))
            .unwrap(),
        )
        .unwrap();
        ok(&install(&home, &analysis, &["-a", "pi"]));
    }
}

#[test]
fn mcp_install_never_defaults_to_home_ancestors_or_filesystem_roots() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    for base in [&home, temp.path(), home.ancestors().last().unwrap()] {
        let result = install(&home, base, &["-a", "pi"]);
        assert!(
            !result.status.success(),
            "implicitly authorized {}",
            base.display()
        );
        assert!(!home.join(".pi").exists());
    }
    let explicit_base = install(
        &home,
        temp.path(),
        &["-a", "pi", "--base", home.to_str().unwrap()],
    );
    assert!(!explicit_base.status.success());
    ok(&install(
        &home,
        &home,
        &["-a", "pi", "--root", home.to_str().unwrap()],
    ));
    // A recognized prior explicit root is retained, never silently narrowed/broadened.
    ok(&install(&home, &home, &["-a", "pi"]));
}

#[test]
fn mcp_install_reports_actual_partial_publication_for_overlapping_client_destinations() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    let analysis = temp.path().join("analysis");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&analysis).unwrap();
    let clients = temp.path().join("clients");
    let pi_dir = clients.join(".claude.json");
    let result = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["mcp", "install", "-a", "pi", "-a", "cc"])
        .current_dir(&analysis)
        .env_clear()
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("PI_CODING_AGENT_DIR", &pi_dir)
        .env("CLAUDE_CONFIG_DIR", &clients)
        .output()
        .unwrap();
    assert!(!result.status.success());
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["targets"][0]["agent"], "pi");
    assert_eq!(report["targets"][0]["outcome"], "installed");
    assert_eq!(report["targets"][1]["agent"], "cc");
    assert_eq!(report["targets"][1]["outcome"], "failed");
    let persisted: Value =
        serde_json::from_slice(&fs::read(pi_dir.join("mcp.json")).unwrap()).unwrap();
    assert_eq!(
        persisted["mcpServers"]["rxd"]["command"],
        env!("CARGO_BIN_EXE_rxd")
    );
    assert_eq!(
        persisted["mcpServers"]["rxd"]["args"][3],
        json!(analysis.canonicalize().unwrap())
    );
    assert!(clients.join(".claude.json").is_dir());
    assert!(String::from_utf8_lossy(&result.stderr).contains("no cross-client transaction"));
    assert_eq!(fs::read_dir(&clients).unwrap().count(), 1); // No leaked staging files.
}

#[test]
fn mcp_install_bounds_generated_configs_before_staging_any_target() {
    let temp = TempDir::new().unwrap();
    let analysis = temp.path().join("analysis");
    fs::create_dir(&analysis).unwrap();
    for format in ["json", "toml"] {
        let home = temp.path().join(format);
        let path = if format == "json" {
            home.join(".pi/agent/mcp.json")
        } else {
            home.join(".codex/config.toml")
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = if format == "json" {
            // Compact input fits; pretty serialization alone expands past 4 MiB.
            serde_json::to_vec(&json!({"padding":vec![0;700_000],"mcpServers":{}})).unwrap()
        } else {
            // Native TOML fits until adding the recorded launch registration.
            format!("padding = '{}'\n", "x".repeat(4 * 1024 * 1024 - 100)).into_bytes()
        };
        assert!(original.len() < 4 * 1024 * 1024);
        fs::write(&path, &original).unwrap();
        for args in [
            vec!["-a", "all"],
            vec!["-a", "all"],
            vec!["-a", "all", "--preview"],
        ] {
            let result = install(&home, &analysis, &args);
            assert!(
                !result.status.success(),
                "accepted oversized generated {format}"
            );
            assert!(
                String::from_utf8_lossy(&result.stderr)
                    .contains("generated configuration exceeds 4 MiB")
            );
            let report: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(report["targets"].as_array().unwrap().len(), 3);
            for target in report["targets"].as_array().unwrap() {
                assert_eq!(
                    target["outcome"],
                    if target["agent"] == if format == "json" { "pi" } else { "codex" } {
                        "failed"
                    } else {
                        "not-written"
                    }
                );
            }
            assert_eq!(fs::read(&path).unwrap(), original);
            assert!(!home.join(".claude.json").exists());
            if format == "json" {
                assert!(!home.join(".codex").exists());
            } else {
                assert!(!home.join(".pi").exists());
            }
            assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn mcp_install_refuses_existing_mounted_roots_unless_explicitly_authorized() {
    #[cfg(target_os = "macos")]
    let mount = {
        let output = Command::new("/sbin/mount").output().unwrap();
        assert!(output.status.success());
        let mounts = String::from_utf8(output.stdout).unwrap();
        ["/System/Volumes/Data", "/dev"]
            .into_iter()
            .find(|path| mounts.contains(&format!(" on {path} (")) && Path::new(path).is_dir())
    };
    #[cfg(target_os = "linux")]
    let mount = {
        let mounts = fs::read_to_string("/proc/self/mountinfo").unwrap();
        ["/proc", "/dev", "/sys"].into_iter().find(|path| {
            mounts
                .lines()
                .any(|line| line.split_whitespace().nth(4) == Some(*path))
                && Path::new(path).is_dir()
        })
    };
    let Some(mount) = mount else {
        eprintln!(
            "UNAVAILABLE: no existing supplemental directory mount for implicit-root check; not runtime mount coverage"
        );
        return;
    };
    eprintln!("Existing mounted-root CLI check: {mount}");
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    let analysis = temp.path().join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&analysis).unwrap();
    for args in [
        vec!["-a", "all", "--base", mount],
        vec!["-a", "all", "--base", mount, "--preview"],
    ] {
        let output = install(&home, &analysis, &args);
        assert!(
            !output.status.success(),
            "implicitly authorized mounted root {mount}"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("implicit home-wide/filesystem-wide")
        );
        assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
    }
    // cwd is also an implicit root. Nothing is written to the mount itself.
    assert!(
        !install(&home, Path::new(mount), &["-a", "pi"])
            .status
            .success()
    );
    assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
    ok(&install(
        &home,
        &analysis,
        &["-a", "all", "--base", mount, "--root", mount],
    ));
    let path = home.join(".pi/agent/mcp.json");
    let bytes = fs::read(&path).unwrap();
    ok(&install(&home, &analysis, &["-a", "all"])); // Preserve recognized explicit roots.
    assert_eq!(fs::read(&path).unwrap(), bytes);
    // A legitimate project cwd remains a valid default for a new registration.
    fs::remove_dir_all(home.join(".pi")).unwrap();
    ok(&install(&home, &analysis, &["-a", "pi"]));
    let config: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        config["mcpServers"]["rxd"]["args"][5],
        json!(analysis.canonicalize().unwrap())
    );
}
