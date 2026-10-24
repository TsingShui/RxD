use super::support::Client;
use serde_json::{Value, json};
use std::process::Command;

#[cfg(feature = "backend-dotnet")]
#[test]
fn mcp_keeps_independent_fixed_service_budgets_after_cli_input_scaling() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/corpus/dotnet/newtonsoft-json-13.0.3/Newtonsoft.Json.dll");
    // The CLI scales its output envelope with the input. This server fixes a much
    // smaller response-derived projection budget, so the same request is admitted by
    // the CLI and rejected by the service.
    let mut client = Client::start(&[
        "--root",
        root.to_str().unwrap(),
        "--backends",
        "dotnet",
        "--max-response-bytes",
        "1024",
    ]);
    client.initialize();
    let rejected = client.tool(
        "dotnet_methods",
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
        .args(["dotnet", "methods"])
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
#[cfg(feature = "backend-js")]
#[test]
fn mcp_js_navigation_preserves_cli_identity_and_source() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = root.join("tests/fixtures/example.js");
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "js"]);
    client.initialize();
    let catalog = client.request("tools/list", json!({}));
    let entities_schema = catalog["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "js_entities")
        .unwrap()["outputSchema"]
        .clone();
    let result = client.tool("js_entities", json!({"input":{"path":fixture}}));
    assert_ne!(result["isError"], true, "{result}");
    let structured = &result["structuredContent"];
    super::support::assert_schema(structured, &entities_schema);
    let cli = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["js", "entities", fixture.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(cli.status.success());
    let rows: Vec<Value> = String::from_utf8(cli.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(structured["header"], rows[0]);
    assert_eq!(structured["records"], json!(&rows[1..rows.len() - 1]));
    assert_eq!(structured["summary"], rows[rows.len() - 1]);
    for (name, extra) in [
        ("js_entities", json!({})),
        ("js_functions", json!({})),
        ("js_classes", json!({})),
        ("js_methods", json!({})),
        ("js_find", json!({"filter":"/api/login"})),
        (
            "js_entities",
            json!({"kind":"function_declaration,arrow_function"}),
        ),
        ("js_identifiers", json!({"name":"user"})),
        ("js_calls", json!({"name":"fetch"})),
        ("js_members", json!({"name":"login"})),
        ("js_locate", json!({"byte":80})),
        ("js_xref", json!({"byte":6})),
        ("js_symbols", json!({})),
    ] {
        let mut arguments = extra;
        arguments["input"] = json!({"path":fixture});
        checked(&mut client, &catalog["result"]["tools"], name, arguments);
    }
    let entity = &structured["records"][1];
    let fetched = client.tool(
        "js_slice",
        json!({"input":{"path":fixture},"at":entity["at"]}),
    );
    assert_ne!(fetched["isError"], true, "{fetched}");
    compare_cli(
        "js_slice",
        &json!({"input":{"path":fixture},"at":entity["at"]}),
        &fetched["structuredContent"],
    );
    let original = std::fs::read_to_string(&fixture).unwrap();
    let start = entity["start"].as_u64().unwrap() as usize;
    let end = entity["end"].as_u64().unwrap() as usize;
    assert_eq!(
        fetched["structuredContent"]["detail"]["source"],
        original[start..end]
    );
}

#[cfg(feature = "backend-js")]
#[test]
fn mcp_js_member_writes_and_call_contexts_match_cli() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("loader.js");
    let source = "runtime.p; runtime.p=document.querySelector('meta[name=AppPublic]').content;";
    std::fs::write(&path, source).unwrap();
    let mut client = Client::start(&["--root", dir.path().to_str().unwrap(), "--backends", "js"]);
    client.initialize();
    let catalog = client.request("tools/list", json!({}));
    let tools = &catalog["result"]["tools"];
    let members = checked(
        &mut client,
        tools,
        "js_members",
        json!({
            "input":{"path":path}, "name":"p", "access":"write"
        }),
    );
    assert_eq!(members["records"].as_array().unwrap().len(), 1);
    assert_eq!(members["records"][0]["detail"]["write"], true);
    assert_eq!(members["records"][0]["detail"]["read"], false);
    let calls = checked(
        &mut client,
        tools,
        "js_calls",
        json!({
            "input":{"path":path}, "name":"querySelector"
        }),
    );
    assert_eq!(
        calls["records"][0]["context_at"],
        members["records"][0]["context_at"]
    );
    let context = checked(
        &mut client,
        tools,
        "js_slice",
        json!({
            "input":{"path":path}, "at":members["records"][0]["context_at"]
        }),
    );
    assert_eq!(
        context["detail"]["source"],
        "runtime.p=document.querySelector('meta[name=AppPublic]').content"
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
    ("js_entities", "js", "entities"),
    ("js_functions", "js", "functions"),
    ("js_classes", "js", "classes"),
    ("js_methods", "js", "methods"),
    ("js_find", "js", "find"),
    ("js_identifiers", "js", "identifiers"),
    ("js_calls", "js", "calls"),
    ("js_members", "js", "members"),
    ("js_resources", "js", "resources"),
    ("js_values", "js", "values"),
    ("js_locate", "js", "locate"),
    ("js_xref", "js", "xref"),
    ("js_symbols", "js", "symbols"),
    ("js_slice", "js", "slice"),
    ("asar_entries", "asar", "entries"),
    ("asar_extract", "asar", "extract"),
    ("html_entries", "html", "entries"),
    ("html_resources", "html", "resources"),
    ("html_extract", "html", "extract"),
    ("dex_entries", "dex", "entries"),
    ("dex_classes", "dex", "classes"),
    ("dex_fields", "dex", "fields"),
    ("dex_methods", "dex", "methods"),
    ("dex_strings", "dex", "strings"),
    ("dex_xref", "dex", "xref"),
    ("dex_manifest", "dex", "manifest"),
    ("dex_get_class", "dex", "get-class"),
    ("dex_get_method", "dex", "get-method"),
    ("jar_classes", "jar", "classes"),
    ("jar_entries", "jar", "entries"),
    ("jar_fields", "jar", "fields"),
    ("jar_methods", "jar", "methods"),
    ("jar_strings", "jar", "strings"),
    ("jar_xref", "jar", "xref"),
    ("jar_get_method", "jar", "get-method"),
    ("jar_get_class", "jar", "get-class"),
    ("dotnet_classes", "dotnet", "classes"),
    ("dotnet_fields", "dotnet", "fields"),
    ("dotnet_methods", "dotnet", "methods"),
    ("dotnet_strings", "dotnet", "strings"),
    ("dotnet_xref", "dotnet", "xref"),
    ("dotnet_get_method", "dotnet", "get-method"),
    ("dotnet_get_class", "dotnet", "get-class"),
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
    if matches!(name, "js_locate" | "js_slice" | "pyc_decompile") || name.starts_with("evm_") {
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

/// Mirror `query::reply`: a single JSON object is returned unchanged, a complete stream
/// becomes header/records/summary, and any other output (XML manifest, Java/C# source)
/// arrives as `text`.
fn cli_projection(name: &str, raw: &[u8]) -> Value {
    // Independent fixture-specific surrogate oracle: the CLI string has the
    // literal UTF-16 x,D800 units; preserve every other byte/record unchanged.
    let text = std::str::from_utf8(raw).unwrap();
    let text = if text.contains("\\ud800") || text.contains("\\uD800") {
        assert_eq!(name, "js_find");
        text.replace("\"\\u0078\\ud800\"", "{\"utf16_units\":[120,55296]}")
            .replace("\"\\u0078\\uD800\"", "{\"utf16_units\":[120,55296]}")
    } else {
        text.to_owned()
    };
    if let Ok(value) = serde_json::from_str::<Value>(&text) {
        // A zero-file scan is the one record that needs the recursive envelope.
        if value["record"] == "scan_summary" {
            return json!({"files":[],"scan_summary":value});
        }
        return value;
    }
    if text
        .lines()
        .filter(|line| !line.is_empty())
        .any(|line| serde_json::from_str::<Value>(line).is_err())
    {
        return json!({"text": text});
    }
    let mut rows: Vec<Value> = text
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let scan = rows
        .last()
        .is_some_and(|row| row["record"] == "scan_summary")
        .then(|| rows.pop().unwrap());
    let mut files = Vec::new();
    let mut file = Vec::new();
    for row in rows {
        let end = row["record"] == "summary";
        file.push(row);
        if end {
            let summary = file.pop().unwrap();
            let header = file.remove(0);
            files.push(
                json!({"header":header,"records":std::mem::take(&mut file),"summary":summary}),
            );
        }
    }
    assert!(file.is_empty(), "CLI output has complete envelopes");
    if let Some(scan) = scan {
        json!({"files":files,"scan_summary":scan})
    } else {
        assert_eq!(files.len(), 1);
        files.pop().unwrap()
    }
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

/// Both extract tools return the CLI receipt with its host destination blanked and the
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

#[cfg(feature = "backend-html")]
#[test]
fn mcp_html_entries_preserves_cli_catalog_and_immutable_target() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("page.html");
    std::fs::write(
        &input,
        "<!doctype html><DIV HREF='/a&amp;b'><span>old</span><!-- note --></DIV>",
    )
    .unwrap();
    let mut client = Client::start(&[
        "--root",
        directory.path().to_str().unwrap(),
        "--backends",
        "html",
    ]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let tool = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "html_entries")
        .unwrap();
    assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    assert_eq!(
        tool["inputSchema"]["properties"]["kind"]["enum"],
        json!(["element", "text", "comment", "doctype"])
    );
    assert_eq!(
        tool["inputSchema"]["properties"]["filter"]["type"],
        "string"
    );
    assert!(
        tool["inputSchema"]["properties"].get("at").is_none(),
        "a listing has no row selector: {tool}"
    );
    assert_eq!(tool["outputSchema"]["type"], "object");
    assert_eq!(
        tool["outputSchema"]["properties"]["records"]["type"],
        "array"
    );
    assert_eq!(tool["outputSchema"]["properties"]["text"]["type"], "string");
    assert!(
        tool["description"]
            .as_str()
            .unwrap()
            .contains("lexical_naive_v1")
    );
    let listing = checked(
        &mut client,
        &tools,
        "html_entries",
        json!({"input":{"path":input}}),
    );
    assert_eq!(listing["summary"]["complete"], true);
    assert_eq!(listing["records"][1]["at"], "element:15:71");
    assert_eq!(listing["records"][1]["detail"]["attrs"][0]["value"], "/a&b");
    let opened = client.tool("rxd_open", json!({"path":input,"backend":"html"}));
    assert_ne!(opened["isError"], true, "{opened}");
    let target = opened["structuredContent"]["target_id"].clone();
    std::fs::write(&input, "<changed/>").unwrap();
    let retained = client.tool("html_entries", json!({"input":{"target_id":target}}));
    assert_ne!(retained["isError"], true, "{retained}");
    super::support::assert_schema(&retained["structuredContent"], &tool["outputSchema"]);
    assert_eq!(retained["structuredContent"], listing);
}

#[cfg(feature = "backend-html")]
#[test]
fn mcp_html_entries_filters_follow_cli_semantics_and_the_response_budget_fails() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("filters.html");
    std::fs::write(
        &input,
        "<main><A HREF='/a&amp;b'>one</A><a href='/other'>two</a></main>",
    )
    .unwrap();
    let mut client = Client::start(&[
        "--root",
        directory.path().to_str().unwrap(),
        "--backends",
        "html",
    ]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let all = checked(
        &mut client,
        &tools,
        "html_entries",
        json!({"input":{"path":input}}),
    );
    let root = all["records"][0]["at"].clone();
    let filtered = checked(
        &mut client,
        &tools,
        "html_entries",
        json!({
            "input":{"path":input}, "kind":"element", "tag":"A", "attr":"HREF=/a&b",
            "within":root, "depth":1,
        }),
    );
    assert_eq!(filtered["records"].as_array().unwrap().len(), 1);
    assert_eq!(
        filtered["records"][0]["detail"]["attrs"][0]["value"],
        "/a&b"
    );
    let by_text = checked(
        &mut client,
        &tools,
        "html_entries",
        json!({"input":{"path":input},"filter":"two"}),
    );
    assert_eq!(by_text["summary"]["records"], 1);
    assert_eq!(by_text["records"][0]["detail"]["value"], "two");
    let by_tag = checked(
        &mut client,
        &tools,
        "html_entries",
        json!({"input":{"path":input},"filter":"MAI"}),
    );
    assert_eq!(by_tag["summary"]["records"], 1);
    assert_eq!(by_tag["records"][0]["detail"]["tag"], "main");
    let empty = checked(
        &mut client,
        &tools,
        "html_entries",
        json!({"input":{"path":input},"tag":"missing"}),
    );
    assert_eq!(empty["summary"]["records"], 0);
    assert_eq!(empty["summary"]["complete"], true);
    let shallow = checked(
        &mut client,
        &tools,
        "html_entries",
        json!({"input":{"path":input},"within":root,"depth":0}),
    );
    assert_eq!(shallow["records"].as_array().unwrap().len(), 1);
    for arguments in [
        json!({"input":{"path":input,"target_id":"invalid"}}),
        json!({"input":{"path":input},"kind":"invalid"}),
        json!({"input":{"path":input},"other":true}),
        // A backend ceiling has no CLI flag, so it cannot arrive as a tool argument either.
        json!({"input":{"path":input},"limits":{"records":1}}),
    ] {
        let rejected = client.tool("html_entries", arguments);
        assert_eq!(rejected["isError"], true, "{rejected}");
        assert_eq!(
            rejected["structuredContent"]["error"]["code"],
            "INVALID_QUERY"
        );
    }
    let invalid = client.tool("html_entries", json!({"input":{"target_id":"invalid"}}));
    assert_eq!(
        invalid["structuredContent"]["error"]["code"],
        "TARGET_INVALID"
    );
    let directory_input = client.tool("html_entries", json!({"input":{"path":directory.path()}}));
    assert_eq!(directory_input["isError"], true, "{directory_input}");
    // The MCP response budget, not a backend flag, is where an oversized result fails.
    let mut bounded = Client::start(&[
        "--root",
        directory.path().to_str().unwrap(),
        "--backends",
        "html",
        "--max-response-bytes",
        "1024",
    ]);
    bounded.initialize();
    let rejected = bounded.tool("html_entries", json!({"input":{"path":input}}));
    assert_eq!(rejected["isError"], true, "{rejected}");
    assert_eq!(
        rejected["structuredContent"]["error"]["code"], "RESOURCE_LIMIT",
        "{rejected}"
    );
}

#[cfg(all(feature = "backend-html", feature = "backend-js"))]
#[test]
fn mcp_html_extract_publishes_retained_child_target_with_cli_identity() {
    use base64::Engine;
    use sha2::{Digest, Sha256};

    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("page.html");
    let html = "<!doctype html><html><head><title>t</title></head><body><script>function hello(){return \"world\";}</script></body></html>";
    std::fs::write(&input, html).unwrap();
    let mut client = Client::start(&[
        "--root",
        directory.path().to_str().unwrap(),
        "--backends",
        "html,js",
    ]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let tool = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "html_extract")
        .unwrap();
    assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    assert_eq!(tool["inputSchema"]["required"], json!(["input"]));
    assert_eq!(tool["inputSchema"]["properties"]["at"]["type"], "string");
    assert_eq!(
        tool["inputSchema"]["properties"]["content"]["type"],
        "boolean"
    );
    assert_eq!(
        tool["inputSchema"]["properties"]["include_bytes"]["type"],
        "boolean"
    );
    assert_eq!(tool["outputSchema"]["type"], "object");
    assert_eq!(
        tool["outputSchema"]["properties"]["records"]["type"],
        "array"
    );

    let listing = checked(
        &mut client,
        &tools,
        "html_entries",
        json!({"input":{"path":input}}),
    );
    let script = listing["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["record"] == "element" && record["detail"]["tag"] == "script")
        .unwrap();
    let at = script["at"].as_str().unwrap();
    let start = script["start"].as_u64().unwrap() as usize;
    let end = script["end"].as_u64().unwrap() as usize;
    let outer = html.as_bytes()[start..end].to_vec();
    let content = b"function hello(){return \"world\";}".to_vec();

    // Default include_bytes is off and the receipt keeps a null host path.
    let extracted = client.tool("html_extract", json!({"input":{"path":input},"at":at}));
    assert_ne!(extracted["isError"], true, "{extracted}");
    super::support::assert_schema(&extracted["structuredContent"], &tool["outputSchema"]);
    let structured = &extracted["structuredContent"];
    assert_eq!(structured["detail"]["path"], Value::Null);
    assert_eq!(structured["detail"]["part"], "outer");
    assert_eq!(structured["detail"]["file_bytes"], outer.len());
    assert_eq!(structured["detail"]["origin"]["node_id"], json!(at));
    assert_eq!(structured["detail"]["origin"]["kind"], "element");
    assert_eq!(structured["detail"]["origin"]["part"], "outer");
    assert_eq!(structured["detail"]["origin"]["start"], start);
    assert_eq!(structured["detail"]["origin"]["end"], end);
    assert!(structured.get("bytes_base64").is_none());
    assert_eq!(
        structured["child_target"]["source_id"],
        format!("{:x}", Sha256::digest(&outer))
    );
    assert_eq!(structured["child_target"]["origin"]["entry"], json!(at));
    assert_eq!(
        structured["child_target"]["origin"]["parent_target_id"],
        Value::Null
    );

    // Artifact identity equals the equivalent CLI extract receipt.
    let destination = directory.path().join("outer.js");
    let cli = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args([
            "html",
            "extract",
            input.to_str().unwrap(),
            "--at",
            at,
            "--output",
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let mut expected: Value = serde_json::from_slice(&cli.stdout).unwrap();
    expected["detail"]["path"] = Value::Null;
    let mut receipt = structured.clone();
    let _ = receipt.as_object_mut().unwrap().remove("child_target");
    assert_eq!(receipt, expected, "complete original CLI receipt");
    assert_eq!(std::fs::read(&destination).unwrap(), outer);

    // include_bytes adds exact base64 only when requested; content selects inner bytes.
    let inner = client.tool(
        "html_extract",
        json!({"input":{"path":input},"at":at,"content":true,"include_bytes":true}),
    );
    assert_ne!(inner["isError"], true, "{inner}");
    super::support::assert_schema(&inner["structuredContent"], &tool["outputSchema"]);
    assert_eq!(inner["structuredContent"]["detail"]["part"], "content");
    assert_eq!(
        inner["structuredContent"]["detail"]["file_bytes"],
        content.len()
    );
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(inner["structuredContent"]["bytes_base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, content);
    assert_eq!(
        inner["structuredContent"]["child_target"]["source_id"],
        format!("{:x}", Sha256::digest(&content))
    );

    // A published child survives parent close and remains a usable JS artifact.
    let opened = client.tool("rxd_open", json!({"path":input,"backend":"html"}));
    assert_ne!(opened["isError"], true, "{opened}");
    let parent = opened["structuredContent"]["target_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let from_parent = client.tool(
        "html_extract",
        json!({"input":{"target_id":parent},"at":at,"content":true}),
    );
    assert_ne!(from_parent["isError"], true, "{from_parent}");
    let child_target = from_parent["structuredContent"]["child_target"]["target_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        from_parent["structuredContent"]["child_target"]["origin"]["parent_target_id"],
        json!(parent)
    );
    assert_eq!(
        from_parent["structuredContent"]["child_target"]["origin"]["entry"],
        json!(at)
    );
    let closed = client.tool("rxd_close", json!({"target_id":parent}));
    assert_ne!(closed["isError"], true, "{closed}");
    let entities = client.tool("js_entities", json!({"input":{"target_id":child_target}}));
    assert_ne!(entities["isError"], true, "{entities}");
    std::fs::write(directory.path().join("main.js"), &content).unwrap();
    let cli = cli_query("js_entities", &json!({}), "main.js", Some(directory.path()));
    assert!(cli.status.success());
    let expected = cli_projection("js_entities", &cli.stdout);
    assert_eq!(
        entities["structuredContent"]["records"],
        expected["records"]
    );
    assert_eq!(
        entities["structuredContent"]["summary"],
        expected["summary"]
    );
    assert!(
        entities["structuredContent"]["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["detail"]["name"] == "hello")
    );
}

#[cfg(feature = "backend-html")]
#[test]
fn mcp_html_extract_failure_paths_do_not_leak_children() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("page.html");
    std::fs::write(
        &input,
        "<!doctype html><html><body><script>function hello(){return 1;}</script></body></html>",
    )
    .unwrap();
    let cli = cli_query("html_entries", &json!({}), input.to_str().unwrap(), None);
    assert!(cli.status.success());
    let at = cli_projection("html_entries", &cli.stdout)["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["record"] == "element" && record["detail"]["tag"] == "script")
        .unwrap()["at"]
        .as_str()
        .unwrap()
        .to_owned();

    // Publication failure (target-handle budget) leaves no unreachable child.
    {
        let mut client = Client::start(&[
            "--root",
            directory.path().to_str().unwrap(),
            "--backends",
            "html",
            "--max-targets",
            "1",
        ]);
        client.initialize();
        let opened = client.tool("rxd_open", json!({"path":input,"backend":"html"}));
        assert_ne!(opened["isError"], true, "{opened}");
        let parent = opened["structuredContent"]["target_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let rejected = client.tool(
            "html_extract",
            json!({"input":{"target_id":parent},"at":at}),
        );
        assert_eq!(rejected["isError"], true, "{rejected}");
        assert_eq!(
            rejected["structuredContent"]["error"]["code"],
            "RESOURCE_LIMIT"
        );
        let closed = client.tool("rxd_close", json!({"target_id":parent}));
        assert_ne!(closed["isError"], true, "{closed}");
        let reopened = client.tool("rxd_open", json!({"path":input,"backend":"html"}));
        assert_ne!(reopened["isError"], true, "{reopened}");
    }

    // Response-budget rollback frees a just-published child handle.
    {
        let mut client = Client::start(&[
            "--root",
            directory.path().to_str().unwrap(),
            "--backends",
            "html",
            "--max-response-bytes",
            "16384",
            "--max-targets",
            "2",
        ]);
        client.initialize();
        let opened = client.tool("rxd_open", json!({"path":input,"backend":"html"}));
        assert_ne!(opened["isError"], true, "{opened}");
        let parent = opened["structuredContent"]["target_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let long_id = "x".repeat(15000);
        client.send(json!({"jsonrpc":"2.0","id":long_id,"method":"tools/call","params":{"name":"html_extract","arguments":{"input":{"target_id":parent},"at":at}}}));
        let response = client.receive();
        assert_eq!(response["id"], json!(long_id));
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert_eq!(
            response["result"]["structuredContent"]["error"]["code"],
            "RESOURCE_LIMIT"
        );
        // A leaked child would occupy the second slot and make the second reopen fail.
        let closed = client.tool("rxd_close", json!({"target_id":parent}));
        assert_ne!(closed["isError"], true, "{closed}");
        let first = client.tool("rxd_open", json!({"path":input,"backend":"html"}));
        assert_ne!(first["isError"], true, "{first}");
        let second = client.tool("rxd_open", json!({"path":input,"backend":"html"}));
        assert_ne!(second["isError"], true, "{second}");
    }
}

#[cfg(all(feature = "backend-asar", feature = "backend-js"))]
#[test]
fn mcp_asar_extraction_returns_original_bytes_for_js_navigation() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("app.asar");
    let source = b"const message='captured'; function read(){return message}\n";
    super::super::pack_asar(&archive, &[("main.js", source)]);
    let mut client = Client::start(&[
        "--root",
        directory.path().to_str().unwrap(),
        "--backends",
        "asar,js",
    ]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let listing = checked(
        &mut client,
        &tools,
        "asar_entries",
        json!({"input":{"path":archive}}),
    );
    assert_eq!(listing["records"][0]["detail"]["path"], "main.js");
    assert_eq!(listing["records"][0]["at"], "main.js");
    let extracted = checked(
        &mut client,
        &tools,
        "asar_extract",
        json!({"input":{"path":archive},"at":"main.js"}),
    );
    assert_eq!(extracted["detail"]["file_bytes"], source.len());
    // Child targets retain container-entry labels while CLI uses actual bytes.
    std::fs::write(directory.path().join("main.js"), source).unwrap();
    let target = &extracted["child_target"]["target_id"];
    let outlived = checked(
        &mut client,
        &tools,
        "js_entities",
        json!({"input":{"target_id":target}}),
    );
    let cli = cli_query("js_entities", &json!({}), "main.js", Some(directory.path()));
    assert!(cli.status.success());
    assert_eq!(outlived, cli_projection("js_entities", &cli.stdout));
    // The program coordinate fetches the whole captured file.
    let at = &outlived["records"][0]["at"];
    let fetched = checked(
        &mut client,
        &tools,
        "js_slice",
        json!({"input":{"target_id":target},"at":at}),
    );
    let cli = cli_query(
        "js_slice",
        &json!({"at":at}),
        "main.js",
        Some(directory.path()),
    );
    assert!(cli.status.success());
    assert_eq!(fetched, cli_projection("js_slice", &cli.stdout));
    assert_eq!(
        fetched["detail"]["source"],
        std::str::from_utf8(source).unwrap()
    );
}

#[cfg(feature = "backend-dex")]
use super::super::dex::support as dex_inputs;

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
    let mut client = Client::start(&[
        "--root",
        directory.path().to_str().unwrap(),
        "--backends",
        "dex",
    ]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let methods = checked(
        &mut client,
        &tools,
        "dex_methods",
        json!({"input":{"path":archive},"class":"LFixture0;"}),
    );
    let method = methods["records"][0]["at"].clone();
    for (name, extra) in [
        ("dex_entries", json!({})),
        ("dex_classes", json!({"filter":"Fixture"})),
        ("dex_fields", json!({"class":"LFixture0;","filter":"f"})),
        (
            "dex_strings",
            json!({"filter":"Authorization","limit":1,"offset":0}),
        ),
        ("dex_xref", json!({"kind":"string","match":"Authorization"})),
        ("dex_xref", json!({"kind":"type","match":"Fixture"})),
        (
            "dex_xref",
            json!({"kind":"method","match":"m0","class":"Fixture0"}),
        ),
        ("dex_xref", json!({"kind":"field","match":"missing"})),
        ("dex_manifest", json!({})),
        ("dex_get_class", json!({"at":"LFixture0;"})),
        ("dex_get_method", json!({"at":method})),
    ] {
        let mut arguments = extra;
        arguments["input"] = json!({"path":archive});
        checked(&mut client, &tools, name, arguments);
    }
    let bundle = client.tool(
        "rxd_open",
        json!({"paths":[archive,archive],"backend":"dex"}),
    );
    assert_ne!(bundle["isError"], true, "{bundle}");
    let wrong_scope = client.tool(
        "dex_classes",
        json!({"input":{"target_id":bundle["structuredContent"]["target_id"]}}),
    );
    assert_eq!(wrong_scope["isError"], true, "{wrong_scope}");
    assert_eq!(
        wrong_scope["structuredContent"]["error"]["code"],
        "INVALID_QUERY"
    );
    let absent = json!({"input":{"path":archive},"at":"LMissing;"});
    let missing = client.tool("dex_get_class", absent.clone());
    assert_eq!(missing["isError"], true, "{missing}");
    assert_eq!(missing["structuredContent"]["error"]["code"], "NOT_FOUND");
    let cli = cli_query("dex_get_class", &absent, archive.to_str().unwrap(), None);
    assert_eq!(cli.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&cli.stderr).contains("no class LMissing; in this input"),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
}

#[cfg(feature = "backend-jvm")]
#[test]
fn mcp_jvm_navigation_preserves_original_instruction_fetch() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/fixtures/jvm/fixture/FlowFixture.class");
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "jar"]);
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

#[cfg(feature = "backend-js")]
#[test]
fn mcp_js_literal_search_preserves_lone_surrogate_units() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("surrogate.js");
    std::fs::write(&input, b"const text='x\\uD800';").unwrap();
    let mut client = Client::start(&[
        "--root",
        directory.path().to_str().unwrap(),
        "--backends",
        "js",
    ]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let found = checked(
        &mut client,
        &tools,
        "js_find",
        json!({"input":{"path":input},"filter":"x"}),
    );
    assert_eq!(
        found["records"][0]["detail"]["value"]["utf16_units"],
        json!([120, 55296])
    );
}

#[cfg(feature = "backend-dotnet")]
#[test]
fn mcp_managed_metadata_navigation_keeps_original_il_evidence() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/fixtures/dotnet/Fixture.dll");
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "dotnet"]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let classes = checked(
        &mut client,
        &tools,
        "dotnet_classes",
        json!({"input":{"path":input}}),
    );
    assert!(
        classes["records"][0]["at"]
            .as_str()
            .unwrap()
            .starts_with("0x02")
    );
    let methods = checked(
        &mut client,
        &tools,
        "dotnet_methods",
        json!({"input":{"path":input}}),
    );
    let method = methods["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["record"] == "method" && row["detail"]["has_body"] == true)
        .unwrap();
    let declaring_type = method["parent_at"].clone();
    for (name, extra) in [
        ("dotnet_fields", json!({"filter":"Count"})),
        ("dotnet_strings", json!({"filter":""})),
        ("dotnet_xref", json!({"at":method["at"]})),
        ("dotnet_xref", json!({"match":"missing","kind":"string"})),
        ("dotnet_get_method", json!({"at":method["at"],"raw":true})),
        ("dotnet_get_class", json!({"at":declaring_type,"raw":true})),
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
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "wasm"]);
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
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "pyc"]);
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
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "evm"]);
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
        assert_eq!(listed["header"]["detail"]["carrier"]["encoding"], "hex");
        assert!(listed["header"]["detail"].get("abi_verified").is_some());
        assert_eq!(
            listed["header"]["detail"]["offset_unit"],
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
        "--root",
        root.to_str().unwrap(),
        "--backends",
        "evm",
        "--max-response-bytes",
        "1024",
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

#[cfg(feature = "backend-js")]
#[test]
fn mcp_recursive_js_keeps_each_file_boundary_and_empty_scan_summary() {
    let directory = tempfile::tempdir().unwrap();
    let sources = directory.path().join("sources");
    std::fs::create_dir(&sources).unwrap();
    std::fs::write(sources.join("a.js"), "const a='needle';").unwrap();
    std::fs::write(sources.join("b.ts"), "const b: string='needle';").unwrap();
    let empty = directory.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let mut client = Client::start(&[
        "--root",
        directory.path().to_str().unwrap(),
        "--backends",
        "js",
    ]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    for name in ["js_entities", "js_find"] {
        let mut arguments = json!({"input":{"path":sources}});
        if name == "js_find" {
            arguments["filter"] = json!("needle");
        }
        let result = checked(&mut client, &tools, name, arguments);
        assert_eq!(result["files"].as_array().unwrap().len(), 2);
        assert_eq!(result["scan_summary"]["detail"]["files"], 2);
        assert_eq!(result["scan_summary"]["complete"], true);
    }
    let result = checked(
        &mut client,
        &tools,
        "js_entities",
        json!({"input":{"path":empty}}),
    );
    assert_eq!(result["files"], json!([]));
    assert_eq!(result["scan_summary"]["detail"]["files"], 0);
}

#[cfg(feature = "backend-asar")]
#[test]
fn mcp_asar_optional_binary_bytes_are_recoverable_and_content_bound() {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("bytes.mcp-extracted");
    let bytes = [0, 255, 128, 1, 13, 10, 0, 42];
    super::super::pack_asar(&archive, &[("binary.dat", &bytes)]);
    let mut client = Client::start(&[
        "--root",
        directory.path().to_str().unwrap(),
        "--backends",
        "asar",
    ]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let extracted = checked(
        &mut client,
        &tools,
        "asar_extract",
        json!({"input":{"path":archive},"at":"binary.dat","include_bytes":true}),
    );
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(extracted["bytes_base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, bytes);
    assert_eq!(extracted["source_id"], rxd_core::identity::input_id(&bytes));
    assert_eq!(
        extracted["child_target"]["source_id"],
        format!("{:x}", Sha256::digest(bytes))
    );
}

#[cfg(feature = "backend-dex")]
#[test]
fn mcp_typed_reference_selectors_cannot_become_native_output_flags() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.dex");
    let victim = directory.path().join("victim.txt");
    std::fs::write(&input, dex_inputs::const_string_dex(1)).unwrap();
    let mut client = Client::start(&[
        "--root",
        directory.path().to_str().unwrap(),
        "--backends",
        "dex",
    ]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let result=client.tool("dex_xref",json!({"input":{"path":input},"kind":"method","class":"Fixture0","match":format!("--output={}",victim.display())}));
    assert!(
        !victim.exists(),
        "a typed filter was incorrectly interpreted as a host output destination: {result}"
    );
    checked(
        &mut client,
        &tools,
        "dex_xref",
        json!({"input":{"path":input},"kind":"string","match":"-needle"}),
    );
}

#[test]
fn mcp_builtin_catalog_has_exactly_the_public_cli_parity_matrix_operations() {
    let mut client = Client::start(&[]);
    client.initialize();
    let catalog = client.request("tools/list", json!({}));
    let actual: std::collections::BTreeSet<_> = catalog["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| {
            let name = tool["name"].as_str().unwrap();
            (!name.starts_with("rxd_") && !name.starts_with("kuna_")).then_some(name)
        })
        .collect();
    let enabled: std::collections::BTreeSet<_> = [
        ("js", cfg!(feature = "backend-js")),
        ("asar", cfg!(feature = "backend-asar")),
        ("html", cfg!(feature = "backend-html")),
        ("dex", cfg!(feature = "backend-dex")),
        ("jar", cfg!(feature = "backend-jvm")),
        ("dotnet", cfg!(feature = "backend-dotnet")),
        ("evm", cfg!(feature = "backend-evm")),
        ("wasm", cfg!(feature = "backend-wasm")),
        ("pyc", cfg!(feature = "backend-pyc")),
    ]
    .into_iter()
    .filter_map(|(name, enabled)| enabled.then_some(name))
    .collect();
    let expected: std::collections::BTreeSet<_> = CLI_ROUTES
        .iter()
        .filter(|route| enabled.contains(route.1))
        .map(|route| route.0)
        .collect();
    assert_eq!(actual, expected);
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
        .args(["dotnet", "methods", input.to_str().unwrap()])
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
            "dotnet",
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
        &["--root", root.to_str().unwrap()],
        &[("RXD_CACHE_DIR", mcp_cache.to_str().unwrap())],
        &["RXD_DOTNET_NATIVE_LIBRARY"],
    );
    client.initialize();
    let result = client.tool(
        "dotnet_get_method",
        json!({"input":{"path":input},"at":method}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"], expected);
    let manifest_path = if let Some(embed) = option_env!("RXD_ILSPY_EMBED_RS") {
        std::path::Path::new(embed)
            .parent()
            .unwrap()
            .join("manifest.json")
    } else {
        let target = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => "aarch64-apple-darwin",
            ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
            ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
            ("windows", "x86_64") => "x86_64-pc-windows-msvc",
            _ => panic!("no supported prebuilt target"),
        };
        root.join("prebuilt/ilspy")
            .join(target)
            .join("manifest.json")
    };
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
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "evm"]);
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
