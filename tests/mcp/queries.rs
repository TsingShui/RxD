#[cfg(feature = "backend-dex")]
use super::super::dex::support as dex_inputs;
use super::support::Client;
use serde_json::{Value, json};
use std::process::Command;

#[cfg(feature = "backend-dotnet")]
#[test]
fn mcp_shared_inflight_storage_can_bound_an_input_scaled_query() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/corpus/dotnet/newtonsoft-json-13.0.3/Newtonsoft.Json.dll");
    // CLI and MCP share the input-scaled analysis policy. A deliberately small
    // service-wide storage envelope rejects this result without narrowing its rows.
    let mut client = Client::start(&[
        "--allow-path",
        root.to_str().unwrap(),
        "--max-inflight-bytes",
        "16384",
    ]);
    client.initialize();
    let rejected = client.tool(
        "ilspy_methods",
        json!({
            "input": {"path": input}, "class": "Newtonsoft.Json.JsonReader", "name": "ReadAsInt32"
        }),
    );
    assert_eq!(rejected["isError"], true, "{rejected}");
    assert_eq!(
        rejected["structuredContent"]["error"]["code"], "RESOURCE_LIMIT",
        "{rejected}"
    );

    // The same query through the CLI is complete under its input-scaled envelope.
    let cli = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["ilspy", "methods"])
        .arg(&input)
        .args([
            "--class",
            "Newtonsoft.Json.JsonReader",
            "--name",
            "ReadAsInt32",
        ])
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
}
#[cfg(feature = "backend-dotnet")]
#[test]
fn mcp_input_scaled_dotnet_xref_finishes_above_the_old_fixed_work_budget() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/corpus/dotnet/newtonsoft-json-13.0.3/Newtonsoft.Json.dll");
    let mut client = Client::start(&["--allow-path", root.to_str().unwrap()]);
    client.initialize();
    let result = client.tool(
        "ilspy_xref",
        json!({"input":{"path":input},"kind":"string","match":"__rxd_nonexistent_budget_probe__"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert!(
        result["structuredContent"]["records"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

fn checked(client: &mut Client, tools: &Value, name: &str, arguments: Value) -> Value {
    let result = client.tool(name, arguments.clone());
    assert_ne!(result["isError"], true, "{name}: {result}");
    let schema = &tools
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap()["outputSchema"];
    super::support::assert_schema(&result["structuredContent"], schema);
    compare_cli(name, &arguments, &result["structuredContent"]);
    result["structuredContent"].clone()
}

/// Public CLI command matrix for every built-in tool. No internal argv/lift code
/// participates in the oracle; only complete stdout at the other public entry.
const CLI_ROUTES: &[(&str, &str, &str)] = &[
    ("rasc_entries", "rasc", "entries"),
    ("rasc_classes", "rasc", "classes"),
    ("rasc_fields", "rasc", "fields"),
    ("rasc_methods", "rasc", "methods"),
    ("rasc_strings", "rasc", "strings"),
    ("rasc_xref", "rasc", "xref"),
    ("rasc_manifest", "rasc", "manifest"),
    ("rasc_get_class", "rasc", "get-class"),
    ("rasc_get_method", "rasc", "get-method"),
    ("jar_classes", "jar", "classes"),
    ("jar_entries", "jar", "entries"),
    ("jar_fields", "jar", "fields"),
    ("jar_methods", "jar", "methods"),
    ("jar_strings", "jar", "strings"),
    ("jar_xref", "jar", "xref"),
    ("jar_get_method", "jar", "get-method"),
    ("jar_get_class", "jar", "get-class"),
    ("ilspy_classes", "ilspy", "classes"),
    ("ilspy_fields", "ilspy", "fields"),
    ("ilspy_methods", "ilspy", "methods"),
    ("ilspy_strings", "ilspy", "strings"),
    ("ilspy_xref", "ilspy", "xref"),
    ("ilspy_get_method", "ilspy", "get-method"),
    ("ilspy_get_class", "ilspy", "get-class"),
    ("wasm_types", "wasm", "types"),
    ("wasm_imports", "wasm", "imports"),
    ("wasm_exports", "wasm", "exports"),
    ("wasm_functions", "wasm", "functions"),
    ("wasm_tables", "wasm", "tables"),
    ("wasm_memories", "wasm", "memories"),
    ("wasm_globals", "wasm", "globals"),
    ("wasm_data", "wasm", "data"),
    ("wasm_elements", "wasm", "elements"),
    ("wasm_strings", "wasm", "strings"),
    ("wasm_xref", "wasm", "xref"),
    ("wasm_get_function", "wasm", "get-function"),
    ("pyc_codes", "pyc", "codes"),
    ("pyc_strings", "pyc", "strings"),
    ("pyc_xref", "pyc", "xref"),
    ("pyc_get_code", "pyc", "get-code"),
    ("pyc_decompile", "pyc", "decompile"),
    ("evm_strings", "evm", "strings"),
    ("evm_xref", "evm", "xref"),
    ("evm_get_block", "evm", "get-block"),
    ("evm_get_function", "evm", "get-function"),
    ("evm_analyze", "evm", "analyze"),
    ("evm_interface", "evm", "interface"),
    ("evm_storage", "evm", "storage"),
    ("evm_disassemble", "evm", "disassemble"),
    ("evm_basic_blocks", "evm", "basic-blocks"),
    ("evm_cfg", "evm", "cfg"),
    ("evm_metadata", "evm", "metadata"),
    ("evm_blocks", "evm", "blocks"),
];

/// The CLI spelling of the same call: flags are derived from the argument names exactly
/// as `reflect::argv` does, and reference-kind positionals come after `--`.
fn cli_query(
    name: &str,
    arguments: &Value,
    path: &str,
    cwd: Option<&std::path::Path>,
) -> std::process::Output {
    let (_, backend, command) = CLI_ROUTES.iter().find(|route| route.0 == name).unwrap();
    let mut argv = vec![backend.to_string(), command.to_string(), path.to_owned()];
    let tail = Vec::new();
    for (field, value) in arguments.as_object().unwrap() {
        if matches!(field.as_str(), "input" | "include_bytes") {
            continue;
        }
        let flag = field.replace('_', "-");
        if value == true {
            argv.push(format!("--{flag}"));
        } else if value == false {
            continue;
        } else {
            argv.push(format!(
                "--{flag}={}",
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            ));
        }
    }
    // `reflect::argv` always requests the machine-readable switch when the CLI declares one.
    if matches!(name, "pyc_decompile") || name.starts_with("evm_") {
        argv.push("--json".into());
    }
    if !tail.is_empty() {
        argv.push("--".into());
        argv.extend(tail);
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_rxd"));
    command.args(argv);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.output().unwrap()
}

/// Compare MCP projection: JSON object, JSONL records, or source text.
fn cli_projection(_name: &str, raw: &[u8]) -> Value {
    let text = std::str::from_utf8(raw).unwrap().to_owned();
    if let Ok(value) = serde_json::from_str::<Value>(&text)
        && ((value.get("backend").is_some() && value.get("command").is_some())
            || (value.get("columns").is_some() && value.get("rows").is_some()))
    {
        return value;
    }
    if text
        .lines()
        .filter(|line| !line.is_empty())
        .any(|line| serde_json::from_str::<Value>(line).is_err())
    {
        return json!({"text": text});
    }
    let rows: Vec<Value> = text
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    json!({"records": rows})
}

fn compare_cli(name: &str, arguments: &Value, actual: &Value) {
    let Some(path) = arguments["input"]["path"].as_str() else {
        return;
    };
    if name.ends_with("_extract") {
        compare_extract(name, arguments, path, actual);
        return;
    }
    let cli = cli_query(name, arguments, path, None);
    assert!(
        cli.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let expected = cli_projection(name, &cli.stdout);
    assert_eq!(
        *actual, expected,
        "{name}: selected content/order/identity/units/representation/completeness equals CLI"
    );
}

/// Extract tools return the CLI receipt with its host destination blanked and the
/// extracted artifact published as `child_target`; everything else must match byte-exact.
fn compare_extract(name: &str, arguments: &Value, path: &str, actual: &Value) {
    let (_, backend, command) = CLI_ROUTES.iter().find(|route| route.0 == name).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let output = temporary.path().join("original-bytes");
    let mut invocation = Command::new(env!("CARGO_BIN_EXE_rxd"));
    invocation.args([
        backend,
        command,
        path,
        "--at",
        arguments["at"].as_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);
    if arguments["content"] == true {
        invocation.arg("--content");
    }
    let cli = invocation.output().unwrap();
    assert!(
        cli.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let mut expected: Value = serde_json::from_slice(&cli.stdout).unwrap();
    expected["detail"]["path"] = Value::Null;
    let mut receipt = actual.clone();
    let _ = receipt.as_object_mut().unwrap().remove("child_target");
    let bytes = std::fs::read(output).unwrap();
    if let Some(encoded) = receipt.as_object_mut().unwrap().remove("bytes_base64") {
        use base64::Engine;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(encoded.as_str().unwrap())
                .unwrap(),
            bytes
        );
    }
    assert_eq!(receipt, expected, "{name}: complete original CLI receipt");
}

#[cfg(feature = "backend-dex")]
#[test]
fn mcp_dex_preserves_typed_queries_and_missing_selectors() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("app.apk");
    let dex = dex_inputs::const_string_dex(2);
    let manifest = dex_inputs::binary_manifest_with_cdata("evidence");
    std::fs::write(
        &archive,
        dex_inputs::stored_zip(&[("classes.dex", &dex), ("AndroidManifest.xml", &manifest)]),
    )
    .unwrap();
    let mut client = Client::start(&["--allow-path", directory.path().to_str().unwrap()]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let methods = checked(
        &mut client,
        &tools,
        "rasc_methods",
        json!({"input":{"path":archive},"class":"LFixture0;"}),
    );
    let method = methods["records"][0]["at"].clone();
    for (name, extra) in [
        ("rasc_entries", json!({})),
        ("rasc_classes", json!({"filter":"Fixture"})),
        ("rasc_fields", json!({"class":"LFixture0;","filter":"f"})),
        (
            "rasc_strings",
            json!({"filter":"Authorization","limit":1,"offset":0}),
        ),
        (
            "rasc_xref",
            json!({"kind":"string","match":"Authorization"}),
        ),
        ("rasc_xref", json!({"kind":"type","match":"Fixture"})),
        (
            "rasc_xref",
            json!({"kind":"method","match":"m0","class":"Fixture0"}),
        ),
        ("rasc_xref", json!({"kind":"field","match":"missing"})),
        ("rasc_manifest", json!({})),
        ("rasc_get_class", json!({"at":"LFixture0;"})),
        ("rasc_get_method", json!({"at":method})),
    ] {
        let mut arguments = extra;
        arguments["input"] = json!({"path":archive});
        checked(&mut client, &tools, name, arguments);
    }
    let bundle = client.tool(
        "rxd_open",
        json!({"paths":[archive,archive],"backend":"rasc"}),
    );
    assert_ne!(bundle["isError"], true, "{bundle}");
    let wrong_scope = client.tool(
        "rasc_classes",
        json!({"input":{"target_id":bundle["structuredContent"]["target_id"]}}),
    );
    assert_eq!(wrong_scope["isError"], true, "{wrong_scope}");
    assert_eq!(
        wrong_scope["structuredContent"]["error"]["code"],
        "INVALID_QUERY"
    );
    let absent = json!({"input":{"path":archive},"at":"LMissing;"});
    let missing = client.tool("rasc_get_class", absent.clone());
    assert_eq!(missing["isError"], true, "{missing}");
    assert_eq!(missing["structuredContent"]["error"]["code"], "NOT_FOUND");
    let cli = cli_query("rasc_get_class", &absent, archive.to_str().unwrap(), None);
    assert_eq!(cli.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&cli.stderr).contains("no class LMissing; in this input"),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
}

#[cfg(feature = "backend-dex")]
#[test]
fn mcp_dex_chunked_output_uses_the_explicit_sink_budget_and_recovers() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("classes.dex");
    std::fs::write(&input, dex_inputs::const_string_dex(9000)).unwrap();
    let cli = cli_query(
        "rasc_classes",
        &json!({"threads":2}),
        input.to_str().unwrap(),
        None,
    );
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let expected = cli_projection("rasc_classes", &cli.stdout);
    assert_eq!(expected["records"].as_array().unwrap().len(), 9000);
    let mut client = Client::start(&["--allow-path", directory.path().to_str().unwrap()]);
    client.initialize();
    for threads in [1, 2] {
        let result = client.tool(
            "rasc_classes",
            json!({"input":{"path":input},"threads":threads}),
        );
        assert_ne!(result["isError"], true, "DEX chunked output failed");
        assert_eq!(result["structuredContent"], expected);
    }
    let mut bounded = Client::start(&[
        "--allow-path",
        directory.path().to_str().unwrap(),
        "--max-inflight-bytes",
        "131072",
    ]);
    bounded.initialize();
    let rejected = bounded.tool("rasc_classes", json!({"input":{"path":input},"threads":2}));
    assert_eq!(rejected["isError"], true, "{rejected}");
    assert_eq!(
        rejected["structuredContent"]["error"]["code"],
        "RESOURCE_LIMIT"
    );
    assert!(rejected["structuredContent"].get("records").is_none());
    let recovered = bounded.tool(
        "rasc_classes",
        json!({"input":{"path":input},"class":"LFixture0;"}),
    );
    assert_ne!(recovered["isError"], true, "{recovered}");
    assert_eq!(
        recovered["structuredContent"]["records"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[cfg(feature = "backend-jvm")]
#[test]
fn mcp_jvm_navigation_preserves_original_instruction_fetch() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/fixtures/jvm/fixture/FlowFixture.class");
    let mut client = Client::start(&["--allow-path", root.to_str().unwrap()]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let classes = checked(
        &mut client,
        &tools,
        "jar_classes",
        json!({"input":{"path":input}}),
    );
    assert_eq!(classes["records"][0]["at"], "Lfixture/FlowFixture;");
    let methods = checked(
        &mut client,
        &tools,
        "jar_methods",
        json!({"input":{"path":input},"class":"Lfixture/FlowFixture;"}),
    );
    let method = methods["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["record"] == "method")
        .unwrap();
    for (name, extra) in [
        ("jar_entries", json!({})),
        (
            "jar_fields",
            json!({"class":"Lfixture/FlowFixture;","filter":"v"}),
        ),
        ("jar_strings", json!({"filter":""})),
        ("jar_xref", json!({"kind":"string","match":""})),
        ("jar_xref", json!({"kind":"type","match":"Object"})),
        ("jar_xref", json!({"kind":"method","match":"println"})),
        ("jar_xref", json!({"kind":"field","match":"counter"})),
        ("jar_get_method", json!({"at":method["at"],"raw":true})),
        ("jar_get_method", json!({"at":method["at"]})),
        ("jar_get_class", json!({"at":"Lfixture/FlowFixture;"})),
    ] {
        let mut arguments = extra;
        arguments["input"] = json!({"path":input});
        checked(&mut client, &tools, name, arguments);
    }
}

#[cfg(feature = "backend-dotnet")]
#[test]
fn mcp_managed_metadata_navigation_keeps_original_il_evidence() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/fixtures/dotnet/Fixture.dll");
    let mut client = Client::start(&["--allow-path", root.to_str().unwrap()]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let classes = checked(
        &mut client,
        &tools,
        "ilspy_classes",
        json!({"input":{"path":input}}),
    );
    // Listings are one table document: [at, full_name] per row.
    assert!(classes["rows"][0][0].as_str().unwrap().starts_with("0x02"));
    let methods = checked(
        &mut client,
        &tools,
        "ilspy_methods",
        json!({"input":{"path":input}}),
    );
    let method_at = methods["rows"][0][0].clone();
    let declaring_type = classes["rows"][1][0].clone();
    for (name, extra) in [
        ("ilspy_fields", json!({"filter":"Count"})),
        ("ilspy_strings", json!({"filter":""})),
        ("ilspy_xref", json!({"at":method_at})),
        ("ilspy_xref", json!({"match":"missing","kind":"string"})),
        ("ilspy_get_method", json!({"at":method_at,"raw":true})),
        ("ilspy_get_class", json!({"at":declaring_type,"raw":true})),
    ] {
        let mut arguments = extra;
        arguments["input"] = json!({"path":input});
        checked(&mut client, &tools, name, arguments);
    }
}

#[cfg(feature = "backend-wasm")]
#[test]
fn mcp_wasm_navigation_retains_raw_body_and_explicit_references() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/fixtures/wasm/fixture.wasm");
    let mut client = Client::start(&["--allow-path", root.to_str().unwrap()]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let functions = checked(
        &mut client,
        &tools,
        "wasm_functions",
        json!({"input":{"path":input}}),
    );
    let function = functions["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["record"] == "function")
        .unwrap();
    assert_eq!(function["at"], "func:1");
    for (name, extra) in [
        ("wasm_types", json!({})),
        ("wasm_imports", json!({})),
        ("wasm_exports", json!({})),
        ("wasm_tables", json!({})),
        ("wasm_memories", json!({})),
        ("wasm_globals", json!({})),
        ("wasm_data", json!({})),
        ("wasm_elements", json!({})),
        ("wasm_strings", json!({"filter":""})),
        ("wasm_xref", json!({"at":function["at"]})),
        ("wasm_get_function", json!({"at":function["at"],"raw":true})),
        ("wasm_get_function", json!({"at":function["at"]})),
    ] {
        let mut arguments = extra;
        arguments["input"] = json!({"path":input});
        checked(&mut client, &tools, name, arguments);
    }
}

#[cfg(feature = "backend-pyc")]
#[test]
fn mcp_python_navigation_preserves_code_tables_and_source_completeness() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/fixtures/pyc/hello.3.12.pyc");
    let mut client = Client::start(&["--allow-path", root.to_str().unwrap()]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let codes = checked(
        &mut client,
        &tools,
        "pyc_codes",
        json!({"input":{"path":input}}),
    );
    let code = &codes["records"][0];
    assert_eq!(code["at"], "code:0");
    for (name, extra) in [
        ("pyc_codes", json!({"name":"Greeter"})),
        ("pyc_strings", json!({"filter":""})),
        ("pyc_strings", json!({"filter_hex":"47"})),
        ("pyc_xref", json!({"kind":"name","at":"print"})),
        ("pyc_get_code", json!({"at":code["at"]})),
        ("pyc_decompile", json!({})),
    ] {
        let mut arguments = extra;
        arguments["input"] = json!({"path":input});
        checked(&mut client, &tools, name, arguments);
    }
}

#[cfg(feature = "backend-evm")]
#[test]
fn mcp_evm_operations_keep_explicit_encoding_and_inference_metadata() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/fixtures/evm/storage-runtime.hex");
    let mut client = Client::start(&["--allow-path", root.to_str().unwrap()]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    // Single-object analyses keep their detail at the top level.
    for name in [
        "evm_analyze",
        "evm_storage",
        "evm_disassemble",
        "evm_basic_blocks",
        "evm_cfg",
        "evm_metadata",
    ] {
        let result = checked(
            &mut client,
            &tools,
            name,
            json!({"input":{"path":input},"encoding":"hex","include_bytecode":true}),
        );
        assert_eq!(result["detail"]["carrier"]["encoding"], "hex", "{name}");
        assert_eq!(result["detail"]["abi_verified"], false, "{name}");
        assert_eq!(
            result["detail"]["offset_unit"], "decoded_bytecode_byte",
            "{name}"
        );
    }
    // The function and block listings are streams, so their facts are in the header.
    for (name, record) in [("evm_interface", "function"), ("evm_blocks", "basic_block")] {
        let listed = checked(
            &mut client,
            &tools,
            name,
            json!({"input":{"path":input},"encoding":"hex","include_bytecode":true}),
        );
        assert_eq!(listed["records"][0]["detail"]["carrier"]["encoding"], "hex");
        assert!(listed["records"][0]["detail"].get("abi_verified").is_some());
        assert_eq!(
            listed["records"][0]["detail"]["offset_unit"],
            "decoded_bytecode_byte"
        );
        assert!(
            listed["records"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["record"] == record)
        );
    }
    // A backend ceiling has no CLI flag, so a caller cannot pass one either.
    for arguments in [
        json!({"input":{"path":input},"encoding":"hex","limits":{"work":1}}),
        json!({"input":{"path":input},"encoding":"hex","max_work":1}),
    ] {
        let unsupported = client.tool("evm_cfg", arguments);
        assert_eq!(unsupported["isError"], true, "{unsupported}");
        assert_eq!(
            unsupported["structuredContent"]["error"]["code"],
            "INVALID_QUERY"
        );
    }
    // Oversized results fail at the MCP response budget, never at a backend flag.
    let mut bounded = Client::start(&[
        "--allow-path",
        root.to_str().unwrap(),
        "--max-inflight-bytes",
        "16384",
    ]);
    bounded.initialize();
    let rejected = bounded.tool(
        "evm_analyze",
        json!({"input":{"path":input},"encoding":"hex"}),
    );
    assert_eq!(rejected["isError"], true, "{rejected}");
    assert_eq!(
        rejected["structuredContent"]["error"]["code"], "RESOURCE_LIMIT",
        "{rejected}"
    );
}

#[cfg(feature = "backend-dex")]
#[test]
fn mcp_typed_reference_selectors_cannot_become_native_output_flags() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.dex");
    let victim = directory.path().join("victim.txt");
    std::fs::write(&input, dex_inputs::const_string_dex(1)).unwrap();
    let mut client = Client::start(&["--allow-path", directory.path().to_str().unwrap()]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let result=client.tool("rasc_xref",json!({"input":{"path":input},"kind":"method","class":"Fixture0","match":format!("--output={}",victim.display())}));
    assert!(
        !victim.exists(),
        "a typed filter was incorrectly interpreted as a host output destination: {result}"
    );
    checked(
        &mut client,
        &tools,
        "rasc_xref",
        json!({"input":{"path":input},"kind":"string","match":"-needle"}),
    );
}

#[cfg(feature = "bundled-ilspy")]
#[test]
fn mcp_cold_relocated_ilspy_matches_cli_after_complete_verified_publication() {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = tempfile::tempdir().unwrap();
    let binary = scratch
        .path()
        .join(if cfg!(windows) { "rxd.exe" } else { "rxd" });
    std::fs::copy(env!("CARGO_BIN_EXE_rxd"), &binary).unwrap();
    let input = root.join("tests/fixtures/dotnet/sugar/Sugar.dll");
    let listing = Command::new(&binary)
        .args(["ilspy", "methods", input.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(listing.status.success());
    let rows: Vec<Value> = std::str::from_utf8(&listing.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let method = rows
        .iter()
        .find(|row| row["record"] == "method" && row["at"] == "0x06000001")
        .unwrap()["at"]
        .as_str()
        .unwrap();
    let cli_cache = scratch.path().join("fresh-cli-cache");
    let cli = Command::new(&binary)
        .args([
            "ilspy",
            "get-method",
            input.to_str().unwrap(),
            "--at",
            method,
        ])
        .env_remove("RXD_DOTNET_NATIVE_LIBRARY")
        .env("RXD_CACHE_DIR", &cli_cache)
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let expected: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(expected["detail"]["representation"], "csharp");
    assert_eq!(expected["detail"]["decompiled"], true);
    let mcp_cache = scratch.path().join("fresh-mcp-cache");
    // Client executable selection is a public process fixture, never an in-process handler.
    let mut client = Client::start_binary_with_env(
        &binary,
        &["--allow-path", root.to_str().unwrap()],
        &[("RXD_CACHE_DIR", mcp_cache.to_str().unwrap())],
        &["RXD_DOTNET_NATIVE_LIBRARY"],
    );
    client.initialize();
    let result = client.tool(
        "ilspy_get_method",
        json!({"input":{"path":input},"at":method}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"], expected);
    let embed = env!("RXD_ILSPY_EMBED_RS");
    let manifest_path = std::path::Path::new(embed)
        .parent()
        .unwrap()
        .join("manifest.json");
    let manifest: Value = serde_json::from_slice(&std::fs::read(manifest_path).unwrap()).unwrap();
    for cache in [&cli_cache, &mcp_cache] {
        let published = cache
            .join("rxd-ilspy")
            .join(manifest["bundle_sha256"].as_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(published.join(".complete")).unwrap(),
            manifest["bundle_sha256"].as_str().unwrap()
        );
        for file in manifest["files"].as_array().unwrap() {
            let bytes = std::fs::read(published.join(file["path"].as_str().unwrap())).unwrap();
            assert_eq!(bytes.len() as u64, file["size"].as_u64().unwrap());
            assert_eq!(
                format!("{:x}", Sha256::digest(bytes)),
                file["sha256"].as_str().unwrap()
            );
        }
    }
}

#[cfg(feature = "backend-evm")]
#[test]
fn mcp_latest_evm_navigation_routes_returned_ids_to_exact_cli_fetch_and_references() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/corpus/evm/authored-navigation-legacy.runtime.hex");
    let mut client = Client::start(&["--allow-path", root.to_str().unwrap()]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let interface = checked(
        &mut client,
        &tools,
        "evm_interface",
        json!({"input":{"path":input},"encoding":"hex"}),
    );
    let function = interface["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["record"] == "function")
        .unwrap();
    let blocks = checked(
        &mut client,
        &tools,
        "evm_blocks",
        json!({"input":{"path":input},"encoding":"hex"}),
    );
    let block = blocks["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["record"] == "basic_block")
        .unwrap();
    assert!(block["at"].is_string(), "{block}");
    assert!(function["at"].is_string(), "{function}");
    for (name, extra) in [
        ("evm_get_block", json!({"at":block["at"]})),
        ("evm_get_function", json!({"at":function["at"],"raw":true})),
        (
            "evm_get_function",
            json!({"selector":function["detail"]["selector"]}),
        ),
        ("evm_xref", json!({"at":block["at"]})),
    ] {
        let mut args = extra;
        args["input"] = json!({"path":input});
        args["encoding"] = json!("hex");
        checked(&mut client, &tools, name, args);
    }
    for name in ["evm_strings", "evm_xref"] {
        let extra = if name == "evm_strings" {
            json!({"kind":"text","query":"absent-final-mcp"})
        } else {
            json!({"at":function["at"]})
        };
        let mut args = extra;
        args["input"] = json!({"path":input});
        args["encoding"] = json!("hex");
        checked(&mut client, &tools, name, args);
    }
    // An unknown identity is a backend failure with the same code through both surfaces.
    let absent = client.tool(
        "evm_get_block",
        json!({"input":{"path":input},"encoding":"hex","at":"hex:block:9999:10000"}),
    );
    assert_eq!(absent["isError"], true, "{absent}");
    let absent_cli = cli_query(
        "evm_get_block",
        &json!({"at":"hex:block:9999:10000","encoding":"hex"}),
        input.to_str().unwrap(),
        None,
    );
    assert!(!absent_cli.status.success(), "{absent_cli:?}");
    assert_eq!(
        absent["structuredContent"]["error"]["code"],
        "INVALID_QUERY"
    );
    assert!(
        String::from_utf8_lossy(&absent_cli.stderr).contains(
            absent["structuredContent"]["error"]["message"]
                .as_str()
                .unwrap()
        ),
        "{}",
        String::from_utf8_lossy(&absent_cli.stderr)
    );
    for (kind, query) in [
        ("number", "7"),
        ("bytes", "07"),
        ("text", "RXD_DATA_NAVIGATION"),
    ] {
        let literals = checked(
            &mut client,
            &tools,
            "evm_strings",
            json!({"input":{"path":input},"encoding":"hex","kind":kind,"query":query}),
        );
        for literal in literals["records"].as_array().unwrap().iter().take(1) {
            if literal["at"].is_null() {
                continue;
            }
            let refs = checked(
                &mut client,
                &tools,
                "evm_xref",
                json!({"input":{"path":input},"encoding":"hex","at":literal["at"]}),
            );
            if let Some(context) = refs["records"]
                .as_array()
                .unwrap()
                .iter()
                .find_map(|row| row["context_at"].as_str())
            {
                let tool = if context.contains(":function:") {
                    "evm_get_function"
                } else {
                    "evm_get_block"
                };
                checked(
                    &mut client,
                    &tools,
                    tool,
                    json!({"input":{"path":input},"encoding":"hex","at":context}),
                );
            }
        }
    }
}
