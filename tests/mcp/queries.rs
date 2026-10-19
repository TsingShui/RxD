use super::support::Client;
use serde_json::{Value, json};
use std::process::Command;

#[test]
fn mcp_js_navigation_preserves_cli_identity_and_source() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = root.join("tests/fixtures/example.js");
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "js"]);
    client.initialize();
    let catalog = client.request("tools/list", json!({}));
    let outline_schema = catalog["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "js_outline")
        .unwrap()["outputSchema"]
        .clone();
    let result = client.tool("js_outline", json!({"input":{"path":fixture}}));
    assert_ne!(result["isError"], true, "{result}");
    let structured = &result["structuredContent"];
    super::support::assert_schema(structured, &outline_schema);
    let cli = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["js", "outline", fixture.to_str().unwrap()])
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
        ("js_find", json!({"string":"/api/login"})),
        (
            "js_outline",
            json!({"kind":"function_declaration,arrow_function"}),
        ),
        ("js_identifiers", json!({"name":"user"})),
        ("js_calls", json!({"name":"fetch"})),
        ("js_members", json!({"name":"login"})),
        ("js_locate", json!({"byte":80})),
        ("js_refs", json!({"byte":6})),
        ("js_symbols", json!({})),
    ] {
        let mut arguments = extra;
        arguments["input"] = json!({"path":fixture});
        checked(&mut client, &catalog["result"]["tools"], name, arguments);
    }
    let entity = &structured["records"][1];
    let fetched = client.tool(
        "js_get",
        json!({"input":{"path":fixture},"id":entity["id"]}),
    );
    assert_ne!(fetched["isError"], true, "{fetched}");
    compare_cli(
        "js_get",
        &json!({"input":{"path":fixture},"id":entity["id"]}),
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
    ("js_outline", "js", "outline"),
    ("js_find", "js", "find"),
    ("js_identifiers", "js", "identifiers"),
    ("js_calls", "js", "calls"),
    ("js_members", "js", "members"),
    ("js_locate", "js", "locate"),
    ("js_refs", "js", "refs"),
    ("js_symbols", "js", "symbols"),
    ("js_get", "js", "get"),
    ("asar_list", "asar", "list"),
    ("asar_extract", "asar", "extract"),
    ("html_list", "html", "list"),
    ("html_extract", "html", "extract"),
    ("dex_entries", "dex", "entries"),
    ("dex_classes", "dex", "classes"),
    ("dex_strings", "dex", "strings"),
    ("dex_findrefs", "dex", "findrefs"),
    ("dex_manifest", "dex", "manifest"),
    ("dex_get_class", "dex", "get-class"),
    ("dex_fields_plan", "dex", "fields-plan"),
    ("dex_member_by_index", "dex", "member-by-index"),
    ("jar_outline", "jar", "outline"),
    ("jar_strings", "jar", "strings"),
    ("jar_findrefs", "jar", "findrefs"),
    ("jar_get_method", "jar", "get-method"),
    ("jar_get_class", "jar", "get-class"),
    ("dotnet_outline", "dotnet", "outline"),
    ("dotnet_strings", "dotnet", "strings"),
    ("dotnet_refs", "dotnet", "refs"),
    ("dotnet_get_method", "dotnet", "get-method"),
    ("dotnet_get_class", "dotnet", "get-class"),
    ("wasm_outline", "wasm", "outline"),
    ("wasm_strings", "wasm", "strings"),
    ("wasm_refs", "wasm", "refs"),
    ("wasm_get_function", "wasm", "get-function"),
    ("pyc_outline", "pyc", "outline"),
    ("pyc_strings", "pyc", "strings"),
    ("pyc_refs", "pyc", "refs"),
    ("pyc_get_code", "pyc", "get-code"),
    ("pyc_decompile", "pyc", "decompile"),
    ("evm_outline", "evm", "outline"),
    ("evm_strings", "evm", "strings"),
    ("evm_refs", "evm", "refs"),
    ("evm_get_block", "evm", "get-block"),
    ("evm_get_function", "evm", "get-function"),
    ("evm_analyze", "evm", "analyze"),
    ("evm_interface", "evm", "interface"),
    ("evm_storage", "evm", "storage"),
    ("evm_disassemble", "evm", "disassemble"),
    ("evm_basic_blocks", "evm", "basic-blocks"),
    ("evm_cfg", "evm", "cfg"),
    ("evm_metadata", "evm", "metadata"),
];

fn cli_query(
    name: &str,
    arguments: &Value,
    path: &str,
    cwd: Option<&std::path::Path>,
) -> std::process::Output {
    let (_, backend, command) = CLI_ROUTES.iter().find(|route| route.0 == name).unwrap();
    let mut argv = vec![backend.to_string(), command.to_string()];
    let resolution = matches!(name, "dex_fields_plan" | "dex_member_by_index");
    if resolution {
        argv.push("--apk".into());
        if let Some(inputs) = arguments["inputs"].as_array() {
            argv.extend(
                inputs
                    .iter()
                    .map(|input| input["path"].as_str().unwrap().to_owned()),
            );
        } else {
            argv.push(path.to_owned());
        }
    } else {
        argv.push(path.to_owned());
    }
    let refs = matches!(name, "dex_findrefs" | "jar_findrefs");
    if refs {
        argv.push(arguments["kind"].as_str().unwrap().to_owned());
    }
    let positional = match name {
        "jar_get_method" | "dotnet_get_method" | "dotnet_get_class" | "wasm_get_function" => {
            Some("id")
        }
        "evm_get_block" | "evm_get_function" => Some("id"),
        "pyc_get_code" => Some("code_id"),
        "dex_get_class" | "jar_get_class" => Some("class"),
        _ => None,
    };
    let mut tail = Vec::new();
    for (field, value) in arguments.as_object().unwrap() {
        if matches!(
            field.as_str(),
            "input" | "inputs" | "limits" | "include_bytes"
        ) {
            continue;
        }
        if refs && field == "kind" {
            continue;
        }
        if Some(field.as_str()) == positional {
            tail.push(value.as_str().unwrap().to_owned());
            continue;
        }
        if refs && (field == "value" || field == "name") {
            tail.push(value.as_str().unwrap().to_owned());
            continue;
        }
        let flag = field.replace('_', "-");
        if value == true {
            argv.push(format!("--{flag}"));
        } else if value == false {
            continue;
        } else if field == "token" {
            argv.push(format!("--token=0x{:x}", value.as_u64().unwrap()));
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
    if matches!(name, "js_get" | "pyc_decompile") {
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

fn cli_projection(name: &str, arguments: &Value, raw: &[u8]) -> Value {
    if matches!(name, "dex_get_class" | "jar_get_class" | "dex_manifest") {
        return json!({"representation":if name=="dex_manifest" {"xml"} else {"java"},"source":std::str::from_utf8(raw).unwrap(),"complete":true});
    }
    if name == "jar_get_method" && arguments["bytecode"] != true {
        if let Ok(value) = serde_json::from_slice::<Value>(raw) {
            return value;
        }
        return json!({"representation":"java","source":std::str::from_utf8(raw).unwrap(),"complete":true});
    }
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
        if value["record"] == "scan_summary" {
            return json!({"files":[],"scan_summary":value});
        }
        return value;
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
    if name == "asar_extract" {
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("original-bytes");
        let cli = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args([
                "asar",
                "extract",
                path,
                "--file",
                arguments["file"].as_str().unwrap(),
                "--output",
                output.to_str().unwrap(),
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
        let mut receipt = actual.clone();
        let child = receipt
            .as_object_mut()
            .unwrap()
            .remove("child_target")
            .unwrap();
        assert_eq!(child["source_id"], expected["source_id"]);
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
        return;
    }
    let cli = cli_query(name, arguments, path, None);
    assert!(
        cli.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let mut expected = cli_projection(name, arguments, &cli.stdout);
    if expected.get("representation").is_some() && expected.get("source_id").is_none() {
        use sha2::{Digest, Sha256};
        expected["source_id"] = json!(format!(
            "{:x}",
            Sha256::digest(std::fs::read(path).unwrap())
        ));
    }
    assert_eq!(
        *actual, expected,
        "{name}: selected content/order/identity/units/representation/completeness equals CLI"
    );
}

#[test]
fn mcp_html_list_preserves_cli_catalog_and_immutable_target() {
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
        .find(|tool| tool["name"] == "html_list")
        .unwrap();
    assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    assert_eq!(
        tool["inputSchema"]["properties"]["kind"]["enum"],
        json!(["element", "text", "comment", "doctype"])
    );
    assert_eq!(
        tool["inputSchema"]["properties"]["limits"]["properties"]["ast_depth"]["minimum"],
        1
    );
    assert_eq!(tool["outputSchema"]["type"], "object");
    assert!(
        tool["outputSchema"]["$defs"]["HtmlElement"]["properties"]["detail"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("content_end"))
    );
    assert!(
        tool["outputSchema"]["$defs"]["HtmlAttr"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("value"))
    );
    assert!(
        tool["description"]
            .as_str()
            .unwrap()
            .contains("lexical_naive_v1")
    );
    let listing = checked(
        &mut client,
        &tools,
        "html_list",
        json!({"input":{"path":input}}),
    );
    assert_eq!(listing["summary"]["complete"], true);
    assert_eq!(listing["records"][1]["detail"]["attrs"][0]["value"], "/a&b");
    let opened = client.tool("rxd_open", json!({"path":input,"backend":"html"}));
    assert_ne!(opened["isError"], true, "{opened}");
    let target = opened["structuredContent"]["target_id"].clone();
    std::fs::write(&input, "<changed/>").unwrap();
    let retained = client.tool("html_list", json!({"input":{"target_id":target}}));
    assert_ne!(retained["isError"], true, "{retained}");
    super::support::assert_schema(&retained["structuredContent"], &tool["outputSchema"]);
    assert_eq!(retained["structuredContent"], listing);
}

#[test]
fn mcp_html_list_filters_and_budgets_follow_cli_semantics() {
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
        "html_list",
        json!({"input":{"path":input}}),
    );
    let root = all["records"][0]["id"].clone();
    let filtered = checked(
        &mut client,
        &tools,
        "html_list",
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
    let empty = checked(
        &mut client,
        &tools,
        "html_list",
        json!({"input":{"path":input},"tag":"missing"}),
    );
    assert_eq!(empty["summary"]["records"], 0);
    assert_eq!(empty["summary"]["complete"], true);
    let shallow = checked(
        &mut client,
        &tools,
        "html_list",
        json!({"input":{"path":input},"within":root,"depth":0}),
    );
    assert_eq!(shallow["records"].as_array().unwrap().len(), 1);
    for limits in [
        json!({"records":1}),
        json!({"ast_depth":1}),
        json!({"input_bytes":1}),
        json!({"output_bytes":1}),
    ] {
        let rejected = client.tool(
            "html_list",
            json!({"input":{"path":input},"tag":"missing","limits":limits}),
        );
        assert_eq!(rejected["isError"], true, "{rejected}");
        assert_eq!(
            rejected["structuredContent"]["error"]["code"], "RESOURCE_LIMIT",
            "{rejected}"
        );
    }
    for arguments in [
        json!({"input":{"path":input,"target_id":"invalid"}}),
        json!({"input":{"path":input},"kind":"invalid"}),
        json!({"input":{"path":input},"other":true}),
        json!({"input":{"path":input},"limits":{"ast_depth":0}}),
    ] {
        let rejected = client.tool("html_list", arguments);
        assert_eq!(rejected["isError"], true, "{rejected}");
        assert_eq!(
            rejected["structuredContent"]["error"]["code"],
            "INVALID_QUERY"
        );
    }
    let invalid = client.tool("html_list", json!({"input":{"target_id":"invalid"}}));
    assert_eq!(
        invalid["structuredContent"]["error"]["code"],
        "TARGET_INVALID"
    );
    let directory_input = client.tool("html_list", json!({"input":{"path":directory.path()}}));
    assert_eq!(directory_input["isError"], true, "{directory_input}");
}

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
    assert_eq!(tool["inputSchema"]["required"], json!(["input", "id"]));
    assert_eq!(
        tool["inputSchema"]["properties"]["content"]["type"],
        "boolean"
    );
    assert_eq!(
        tool["inputSchema"]["properties"]["include_bytes"]["type"],
        "boolean"
    );
    assert!(
        tool["outputSchema"]["$defs"]["HtmlOrigin"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("node_id"))
    );
    assert!(
        tool["outputSchema"]["$defs"]["HtmlOrigin"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("start"))
    );

    let listing = checked(
        &mut client,
        &tools,
        "html_list",
        json!({"input":{"path":input}}),
    );
    let script = listing["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["record"] == "element" && record["detail"]["tag"] == "script")
        .unwrap();
    let id = script["id"].as_str().unwrap();
    let start = script["start"].as_u64().unwrap() as usize;
    let end = script["end"].as_u64().unwrap() as usize;
    let outer = html.as_bytes()[start..end].to_vec();
    let content = b"function hello(){return \"world\";}".to_vec();

    // Default include_bytes is off and the receipt keeps a null host path.
    let extracted = client.tool("html_extract", json!({"input":{"path":input},"id":id}));
    assert_ne!(extracted["isError"], true, "{extracted}");
    super::support::assert_schema(&extracted["structuredContent"], &tool["outputSchema"]);
    let structured = &extracted["structuredContent"];
    assert_eq!(structured["detail"]["path"], Value::Null);
    assert_eq!(structured["detail"]["part"], "outer");
    assert_eq!(structured["detail"]["file_bytes"], outer.len());
    assert_eq!(structured["detail"]["origin"]["node_id"], json!(id));
    assert_eq!(structured["detail"]["origin"]["kind"], "element");
    assert_eq!(structured["detail"]["origin"]["part"], "outer");
    assert_eq!(structured["detail"]["origin"]["start"], start);
    assert_eq!(structured["detail"]["origin"]["end"], end);
    assert!(structured.get("bytes_base64").is_none());
    assert_eq!(
        structured["child_target"]["source_id"],
        format!("{:x}", Sha256::digest(&outer))
    );
    assert_eq!(structured["child_target"]["origin"]["entry"], json!(id));
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
            "--id",
            id,
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
    let child = receipt
        .as_object_mut()
        .unwrap()
        .remove("child_target")
        .unwrap();
    assert_eq!(child["source_id"], expected["source_id"]);
    assert_eq!(receipt, expected, "complete original CLI receipt");
    assert_eq!(std::fs::read(&destination).unwrap(), outer);

    // include_bytes adds exact base64 only when requested; content selects inner bytes.
    let inner = client.tool(
        "html_extract",
        json!({"input":{"path":input},"id":id,"content":true,"include_bytes":true}),
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
        json!({"input":{"target_id":parent},"id":id,"content":true}),
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
        json!(id)
    );
    let closed = client.tool("rxd_close", json!({"target_id":parent}));
    assert_ne!(closed["isError"], true, "{closed}");
    let outlined = client.tool("js_outline", json!({"input":{"target_id":child_target}}));
    assert_ne!(outlined["isError"], true, "{outlined}");
    std::fs::write(directory.path().join("main.js"), &content).unwrap();
    let cli = cli_query("js_outline", &json!({}), "main.js", Some(directory.path()));
    assert!(cli.status.success());
    let expected = cli_projection("js_outline", &json!({}), &cli.stdout);
    assert_eq!(
        outlined["structuredContent"]["records"],
        expected["records"]
    );
    assert_eq!(
        outlined["structuredContent"]["summary"],
        expected["summary"]
    );
    assert!(
        outlined["structuredContent"]["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["detail"]["name"] == "hello")
    );
}

#[test]
fn mcp_html_extract_failure_paths_do_not_leak_children() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("page.html");
    std::fs::write(
        &input,
        "<!doctype html><html><body><script>function hello(){return 1;}</script></body></html>",
    )
    .unwrap();
    let cli = cli_query("html_list", &json!({}), input.to_str().unwrap(), None);
    assert!(cli.status.success());
    let id = cli_projection("html_list", &json!({}), &cli.stdout)["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["record"] == "element" && record["detail"]["tag"] == "script")
        .unwrap()["id"]
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
            json!({"input":{"target_id":parent},"id":id}),
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
        client.send(json!({"jsonrpc":"2.0","id":long_id,"method":"tools/call","params":{"name":"html_extract","arguments":{"input":{"target_id":parent},"id":id}}}));
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
        "asar_list",
        json!({"input":{"path":archive}}),
    );
    assert_eq!(listing["records"][0]["detail"]["path"], "main.js");
    let extracted = checked(
        &mut client,
        &tools,
        "asar_extract",
        json!({"input":{"path":archive},"file":"main.js"}),
    );
    assert_eq!(extracted["detail"]["file_bytes"], source.len());
    // Child targets retain container-entry labels while CLI uses actual bytes.
    std::fs::write(directory.path().join("main.js"), source).unwrap();
    let target = &extracted["child_target"]["target_id"];
    let outlined = checked(
        &mut client,
        &tools,
        "js_outline",
        json!({"input":{"target_id":target}}),
    );
    let cli = cli_query("js_outline", &json!({}), "main.js", Some(directory.path()));
    assert!(cli.status.success());
    assert_eq!(
        outlined,
        cli_projection("js_outline", &json!({}), &cli.stdout)
    );
    let id = &outlined["records"][0]["id"];
    let fetched = checked(
        &mut client,
        &tools,
        "js_get",
        json!({"input":{"target_id":target},"id":id}),
    );
    let cli = cli_query(
        "js_get",
        &json!({"id":id}),
        "main.js",
        Some(directory.path()),
    );
    assert!(cli.status.success());
    assert_eq!(
        fetched,
        cli_projection("js_get", &json!({"id":id}), &cli.stdout)
    );
    assert_eq!(
        fetched["detail"]["source"],
        std::str::from_utf8(source).unwrap()
    );
}

#[cfg(feature = "backend-dex")]
use super::super::dex::support as dex_inputs;

#[cfg(feature = "backend-dex")]
#[test]
fn mcp_dex_preserves_typed_queries_and_resolution_verdicts() {
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
    for (name, extra) in [
        ("dex_entries", json!({})),
        ("dex_classes", json!({"filter":"Fixture"})),
        (
            "dex_strings",
            json!({"filter":"Authorization","limit":1,"offset":0}),
        ),
        (
            "dex_findrefs",
            json!({"kind":"string","value":"Authorization"}),
        ),
        ("dex_findrefs", json!({"kind":"type","value":"Fixture"})),
        (
            "dex_findrefs",
            json!({"kind":"method","name":"m0","class":"Fixture0"}),
        ),
        ("dex_findrefs", json!({"kind":"field","name":"missing"})),
        ("dex_manifest", json!({})),
        ("dex_get_class", json!({"class":"Fixture0","members":true})),
        ("dex_fields_plan", json!({"descriptor":"Fixture0"})),
        (
            "dex_member_by_index",
            json!({"descriptor":"Fixture0","method_index":0}),
        ),
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
    for name in ["dex_fields_plan", "dex_member_by_index"] {
        let mut absent = json!({"input":{"path":archive},"descriptor":"Missing"});
        if name == "dex_member_by_index" {
            absent["method_index"] = json!(0);
        }
        let missing = client.tool(name, absent.clone());
        let cli = cli_query(name, &absent, archive.to_str().unwrap(), None);
        assert_eq!(cli.status.code(), Some(3));
        if name == "dex_fields_plan" {
            assert!(cli.stdout.is_empty());
            assert!(String::from_utf8_lossy(&cli.stderr).contains("not defined"));
            assert_eq!(
                missing["structuredContent"]["error"]["candidates"],
                json!({"definitions":[]})
            );
        } else {
            assert_eq!(
                missing["structuredContent"]["error"]["candidates"],
                cli_projection(name, &absent, &cli.stdout)
            );
        }
        assert_eq!(missing["isError"], true);
        assert_eq!(missing["structuredContent"]["error"]["code"], "NOT_FOUND");
        let mut ambiguous =
            json!({"inputs":[{"path":archive},{"path":archive}],"descriptor":"Fixture0"});
        if name == "dex_member_by_index" {
            ambiguous["method_index"] = json!(0);
        }
        let duplicate = client.tool(name, ambiguous.clone());
        let cli = cli_query(name, &ambiguous, archive.to_str().unwrap(), None);
        assert_eq!(cli.status.code(), Some(4));
        if name == "dex_fields_plan" {
            assert!(cli.stdout.is_empty());
            let diagnostic = String::from_utf8_lossy(&cli.stderr);
            let definitions = diagnostic
                .split(" times (")
                .nth(1)
                .unwrap()
                .split("); a plan")
                .next()
                .unwrap()
                .split(", ")
                .collect::<Vec<_>>();
            assert_eq!(
                duplicate["structuredContent"]["error"]["candidates"],
                json!({"definitions":definitions})
            );
        } else {
            assert_eq!(
                duplicate["structuredContent"]["error"]["candidates"],
                cli_projection(name, &ambiguous, &cli.stdout)
            );
        }
        assert_eq!(duplicate["isError"], true, "{duplicate}");
        assert_eq!(
            duplicate["structuredContent"]["error"]["code"], "AMBIGUOUS",
            "{duplicate}"
        );
        let candidates = &duplicate["structuredContent"]["error"]["candidates"];
        assert_eq!(
            if name == "dex_fields_plan" {
                candidates["definitions"].as_array().unwrap().len()
            } else {
                candidates["records"].as_array().unwrap().len()
            },
            2
        );
    }
}

#[test]
fn mcp_jvm_navigation_preserves_original_instruction_fetch() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/fixtures/jvm/fixture/FlowFixture.class");
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "jar"]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let outline = checked(
        &mut client,
        &tools,
        "jar_outline",
        json!({"input":{"path":input}}),
    );
    let method = outline["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["record"] == "method" && record["detail"]["has_code"] == true)
        .unwrap();
    for (name, extra) in [
        ("jar_strings", json!({"filter":""})),
        ("jar_findrefs", json!({"kind":"string","value":""})),
        ("jar_findrefs", json!({"kind":"type","name":"Object"})),
        ("jar_findrefs", json!({"kind":"method","name":"println"})),
        ("jar_findrefs", json!({"kind":"field","name":"counter"})),
        ("jar_get_method", json!({"id":method["id"],"bytecode":true})),
        ("jar_get_method", json!({"id":method["id"]})),
        ("jar_get_class", json!({"class":"fixture.FlowFixture"})),
    ] {
        let mut arguments = extra;
        arguments["input"] = json!({"path":input});
        checked(&mut client, &tools, name, arguments);
    }
}

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
        json!({"input":{"path":input},"string":"x"}),
    );
    assert_eq!(
        found["records"][0]["detail"]["value"]["utf16_units"],
        json!([120, 55296])
    );
}

#[test]
fn mcp_managed_metadata_navigation_keeps_original_il_evidence() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/fixtures/dotnet/Fixture.dll");
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "dotnet"]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let outline = checked(
        &mut client,
        &tools,
        "dotnet_outline",
        json!({"input":{"path":input}}),
    );
    let rows = outline["records"].as_array().unwrap();
    let method = rows
        .iter()
        .find(|row| row["record"] == "method" && row["detail"]["has_body"] == true)
        .unwrap();
    let ty = rows
        .iter()
        .find(|row| row["record"] == "type" && row["id"] == method["parent_id"])
        .unwrap();
    for (name, extra) in [
        ("dotnet_strings", json!({"contains":""})),
        ("dotnet_refs", json!({"token":method["detail"]["token"]})),
        ("dotnet_refs", json!({"id":method["id"]})),
        ("dotnet_refs", json!({"string":"missing"})),
        ("dotnet_get_method", json!({"id":method["id"],"il":true})),
        ("dotnet_get_class", json!({"id":ty["id"],"il":true})),
    ] {
        let mut arguments = extra;
        arguments["input"] = json!({"path":input});
        checked(&mut client, &tools, name, arguments);
    }
}

#[test]
fn mcp_wasm_navigation_retains_raw_body_and_explicit_references() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/fixtures/wasm/fixture.wasm");
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "wasm"]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let outline = checked(
        &mut client,
        &tools,
        "wasm_outline",
        json!({"input":{"path":input}}),
    );
    let function = outline["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["record"] == "function")
        .unwrap();
    for (name, extra) in [
        ("wasm_strings", json!({"contains":""})),
        (
            "wasm_refs",
            json!({"func":function["detail"]["func_index"]}),
        ),
        ("wasm_get_function", json!({"id":function["id"],"raw":true})),
        ("wasm_get_function", json!({"id":function["id"]})),
    ] {
        let mut arguments = extra;
        arguments["input"] = json!({"path":input});
        checked(&mut client, &tools, name, arguments);
    }
}

#[test]
fn mcp_python_navigation_preserves_code_tables_and_source_completeness() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/fixtures/pyc/hello.3.12.pyc");
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "pyc"]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let outline = checked(
        &mut client,
        &tools,
        "pyc_outline",
        json!({"input":{"path":input}}),
    );
    let code = &outline["records"][0];
    for (name, extra) in [
        ("pyc_strings", json!({"contains":""})),
        ("pyc_refs", json!({"name":"print"})),
        ("pyc_get_code", json!({"code_id":code["id"]})),
        ("pyc_decompile", json!({})),
    ] {
        let mut arguments = extra;
        arguments["input"] = json!({"path":input});
        checked(&mut client, &tools, name, arguments);
    }
}

#[test]
fn mcp_evm_operations_keep_explicit_encoding_and_inference_metadata() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/fixtures/evm/storage-runtime.hex");
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "evm"]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    for name in [
        "evm_analyze",
        "evm_interface",
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
        assert_eq!(result["detail"]["carrier"]["encoding"], "hex");
        assert_eq!(result["detail"]["abi_verified"], false);
        assert_eq!(result["detail"]["offset_unit"], "decoded_bytecode_byte");
    }
    let unsupported = client.tool(
        "evm_cfg",
        json!({"input":{"path":input},"encoding":"hex","limits":{"work":1}}),
    );
    assert_eq!(unsupported["isError"], true);
    assert_eq!(
        unsupported["structuredContent"]["error"]["code"],
        "INVALID_QUERY"
    );
}

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
    for name in ["js_outline", "js_find"] {
        let mut arguments = json!({"input":{"path":sources}});
        if name == "js_find" {
            arguments["string"] = json!("needle");
        }
        let result = checked(&mut client, &tools, name, arguments);
        assert_eq!(result["files"].as_array().unwrap().len(), 2);
        assert_eq!(result["scan_summary"]["detail"]["files"], 2);
        assert_eq!(result["scan_summary"]["complete"], true);
    }
    let result = checked(
        &mut client,
        &tools,
        "js_outline",
        json!({"input":{"path":empty}}),
    );
    assert_eq!(result["files"], json!([]));
    assert_eq!(result["scan_summary"]["detail"]["files"], 0);
}

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
        json!({"input":{"path":archive},"file":"binary.dat","include_bytes":true}),
    );
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(extracted["bytes_base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, bytes);
    assert_eq!(
        extracted["source_id"],
        format!("{:x}", Sha256::digest(bytes))
    );
    assert_eq!(
        extracted["child_target"]["source_id"],
        extracted["source_id"]
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
    let result=client.tool("dex_findrefs",json!({"input":{"path":input},"kind":"method","class":"Fixture0","name":format!("--output={}",victim.display())}));
    assert!(
        !victim.exists(),
        "typed name was incorrectly interpreted as a host output destination: {result}"
    );
    checked(
        &mut client,
        &tools,
        "dex_findrefs",
        json!({"input":{"path":input},"kind":"string","value":"-needle"}),
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
    let outline = Command::new(&binary)
        .args(["dotnet", "outline", input.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(outline.status.success());
    let rows: Vec<Value> = std::str::from_utf8(&outline.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let method = rows
        .iter()
        .find(|row| row["record"] == "method" && row["detail"]["token"] == 0x06000001u32)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let cli_cache = scratch.path().join("fresh-cli-cache");
    let cli = Command::new(&binary)
        .args(["dotnet", "get-method", input.to_str().unwrap(), method])
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
        json!({"input":{"path":input},"id":method}),
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

#[test]
fn mcp_latest_evm_navigation_routes_returned_ids_to_exact_cli_fetch_and_references() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/corpus/evm/authored-navigation-legacy.runtime.hex");
    let mut client = Client::start(&["--root", root.to_str().unwrap(), "--backends", "evm"]);
    client.initialize();
    let tools = client.request("tools/list", json!({}))["result"]["tools"].clone();
    let outline = checked(
        &mut client,
        &tools,
        "evm_outline",
        json!({"input":{"path":input},"encoding":"hex"}),
    );
    let function = outline["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["record"] == "function")
        .unwrap();
    let block = outline["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["record"] == "basic_block")
        .unwrap();
    for (name, extra) in [
        ("evm_get_block", json!({"id":block["id"]})),
        ("evm_get_function", json!({"id":function["id"],"raw":true})),
        (
            "evm_get_function",
            json!({"selector":function["detail"]["selector"]}),
        ),
        ("evm_refs", json!({"id":block["id"]})),
    ] {
        let mut args = extra;
        args["input"] = json!({"path":input});
        args["encoding"] = json!("hex");
        checked(&mut client, &tools, name, args);
    }
    for name in ["evm_strings", "evm_refs"] {
        let extra = if name == "evm_strings" {
            json!({"kind":"text","query":"absent-final-mcp"})
        } else {
            json!({"id":function["id"]})
        };
        let mut args = extra;
        args["input"] = json!({"path":input});
        args["encoding"] = json!("hex");
        checked(&mut client, &tools, name, args);
    }
    for key in [
        "work",
        "states",
        "stack_cells",
        "reconstruction_work",
        "values",
    ] {
        let mut limits = json!({});
        limits[key] = json!(1);
        let failed = client.tool(
            "evm_get_function",
            json!({"input":{"path":input},"encoding":"hex","id":function["id"],"limits":limits}),
        );
        assert_eq!(failed["isError"], true, "{key}: {failed}");
        assert_eq!(
            failed["structuredContent"]["error"]["code"], "RESOURCE_LIMIT",
            "{key}: {failed}"
        );
    }
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
                "evm_refs",
                json!({"input":{"path":input},"encoding":"hex","id":literal["id"]}),
            );
            if let Some(context) = refs["records"]
                .as_array()
                .unwrap()
                .iter()
                .find_map(|row| row["context_id"].as_str())
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
                    json!({"input":{"path":input},"encoding":"hex","id":context}),
                );
            }
        }
    }
}
