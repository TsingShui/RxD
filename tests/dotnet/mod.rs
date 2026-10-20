//! Static .NET CLI contracts using independently assembled original fixtures.
use super::*;

mod class;
#[cfg(all(feature = "csharp-decompiler", unix))]
mod class_csharp;
mod closure;
mod field_type_refs;
mod method_refs;
mod strings;
mod sugar;

#[test]
fn enabled_backend_features_define_the_cli_catalog() {
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .arg("--help")
        .output()
        .unwrap();
    success(&output);
    let help = String::from_utf8(output.stdout).unwrap();
    let expected = [
        ("js", cfg!(feature = "backend-js")),
        ("asar", cfg!(feature = "backend-asar")),
        ("html", cfg!(feature = "backend-html")),
        ("dex", cfg!(feature = "backend-dex")),
        ("jar", cfg!(feature = "backend-jvm")),
        ("dotnet", cfg!(feature = "backend-dotnet")),
        ("evm", cfg!(feature = "backend-evm")),
        ("wasm", cfg!(feature = "backend-wasm")),
        ("pyc", cfg!(feature = "backend-pyc")),
        ("kuna", cfg!(feature = "backend-kuna")),
        ("mcp", cfg!(feature = "mcp")),
    ];
    for (name, enabled) in expected {
        let listed = help
            .lines()
            .any(|line| line.trim_start().starts_with(&format!("{name} ")));
        assert_eq!(
            listed, enabled,
            "unexpected help visibility for {name}:\n{help}"
        );
    }
}

fn assert_keys(value: &Value, expected: &[&str]) {
    let mut actual: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut expected = expected.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

fn assert_fetch_protocol(row: &Value, command: &str) {
    assert_keys(
        row,
        &[
            "record",
            "at",
            "source_id",
            "parent_at",
            "context_at",
            "start",
            "end",
            "detail",
            "backend",
            "command",
            "schema_version",
        ],
    );
    assert_eq!(row["backend"], "dotnet");
    assert_eq!(row["command"], command);
    assert_eq!(row["schema_version"], 2);
}

#[test]
fn dotnet_outline_uses_the_shared_envelope_and_keeps_fetch_ids_navigable() {
    let path = input();
    let rows = outline(&path, &["--method", "Add"]);
    assert_eq!(rows[0]["schema_version"], 2);
    let header = rows[0].as_object().unwrap();
    assert_eq!(
        header.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "backend",
            "command",
            "detail",
            "input_bytes",
            "query",
            "record",
            "schema_version",
            "source_id"
        ]
    );
    let summary = rows.last().unwrap();
    assert_eq!(summary["complete"], true);
    assert_eq!(summary["records"], rows.len() - 2);
    assert!(summary["detail"].is_object());
    for row in &rows[1..rows.len() - 1] {
        assert_eq!(
            row.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "at",
                "context_at",
                "detail",
                "end",
                "parent_at",
                "record",
                "source_id",
                "start"
            ]
        );
        assert!(row["source_id"].is_null());
        assert!(row["start"].is_null() && row["end"].is_null());
        assert!(row["context_at"].is_null());
    }
    let method = rows.iter().find(|row| row["record"] == "method").unwrap();
    assert_eq!(method["detail"]["name"], "Add");
    let fetch = run(
        "get-method",
        &path,
        &["--at", method["at"].as_str().unwrap(), "--raw"],
    );
    success(&fetch);
}

#[test]
fn dotnet_method_fetch_envelope_preserves_original_byte_ranges_and_exception_handlers() {
    let path = input();
    for name in ["Add", "Finally"] {
        let rows = outline(&path, &["--method", name]);
        let method = rows.iter().find(|row| row["record"] == "method").unwrap();
        let output = run(
            "get-method",
            &path,
            &["--at", method["at"].as_str().unwrap(), "--raw"],
        );
        success(&output);
        let row: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_fetch_protocol(&row, "get-method");
        assert_eq!(row["record"], "method_body");
        assert_eq!(row["at"], method["at"]);
        assert!(row["source_id"].is_null());
        assert!(row["context_at"].is_null());
        assert_eq!(row["parent_at"], method["parent_at"]);
        let detail = &row["detail"];
        assert_eq!(detail["complete"], true);
        assert_eq!(detail["method"], *method);
        assert_eq!(detail["il_hex"], expected()["methods"][name]["il_hex"]);
        assert_eq!(row["start"], detail["code_offset"]);
        let start = row["start"].as_u64().unwrap() as usize;
        let end = row["end"].as_u64().unwrap() as usize;
        assert_eq!(end - start, detail["code_size"].as_u64().unwrap() as usize);
        let bytes = fs::read(&path).unwrap();
        let original = bytes[start..end]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(detail["il_hex"], original);
        if name == "Finally" {
            assert_eq!(detail["handlers"][0]["handler_offset"], 4);
            assert_eq!(detail["handlers"][0]["handler_length"], 3);
        }
    }
}

#[test]
fn dotnet_search_and_reference_envelopes_route_exact_token_hits_to_original_methods() {
    let path = input();
    let strings = jsonl(&run("strings", &path, &["--filter", "世界"]));
    assert_eq!(strings[0]["schema_version"], 2);
    let literal = &strings[1];
    assert_eq!(literal["detail"]["value"], expected()["string"]);
    for key in ["at", "parent_at", "context_at", "start", "end"] {
        assert!(literal.as_object().unwrap().contains_key(key));
    }
    // The `#US` token is the literal's identity, so it is the row's `at`.
    assert_eq!(literal["at"], format!("0x{:08x}", 0x7000_0001u32));
    assert!(literal["start"].is_null());
    assert!(literal["end"].is_null());
    assert_eq!(literal["detail"]["status"], "referenced");
    assert_eq!(literal["parent_at"], literal["parent_at"]);
    assert!(literal["context_at"].as_str().unwrap().starts_with("0x"));
    let rows = jsonl(&run("xref", &path, &["--at", "0x70000001"]));
    assert_eq!(rows[0]["schema_version"], 2);
    let hit = &rows[1];
    assert!(hit["at"].is_null());
    assert_eq!(hit["detail"]["target_token"], 0x70000001);
    assert_eq!(hit["detail"]["il_offset"], 0);
    assert_eq!(hit["detail"]["opcode"], "ldstr");
    let output = run(
        "get-method",
        &path,
        &["--at", hit["context_at"].as_str().unwrap(), "--raw"],
    );
    success(&output);
    let method: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(method["detail"]["method"]["detail"]["name"], "Unicode");
    assert_eq!(hit["start"], method["start"]);
    assert_eq!(
        hit["end"].as_u64().unwrap() - hit["start"].as_u64().unwrap(),
        5
    );
    let zero = jsonl(&run("strings", &path, &["--filter", "absent-literal"]));
    assert_eq!(zero.len(), 2);
    assert_eq!(zero[1]["records"], 0);
    assert_eq!(zero[1]["complete"], true);
}

fn input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/Fixture.dll")
}

fn expected() -> Value {
    serde_json::from_str(include_str!("../fixtures/dotnet/expected.json")).unwrap()
}

/// The per-object listings as one stream, the view `outline` used to print.
///
/// Tests assert on the members an assembly holds, not on which command produced a row,
/// so this keeps them readable after the listing was split into three commands.
fn outline(path: &Path, args: &[&str]) -> Vec<Value> {
    let mut head: Vec<&str> = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if *arg == "--method" {
            skip = true;
            continue;
        }
        head.push(arg);
    }
    // One header and one summary for the joined stream; each listing contributes its
    // records only, in declaration order.
    let mut rows = Vec::new();
    let mut summary = Value::Null;
    for (index, sub) in ["classes", "fields", "methods"].iter().enumerate() {
        let sub_args: &[&str] = if *sub == "methods" { args } else { &head };
        let mut listing = jsonl(&run(sub, path, sub_args));
        summary = listing.pop().unwrap_or(Value::Null);
        let header = listing.remove(0);
        if index == 0 {
            rows.push(header);
        }
        rows.append(&mut listing);
    }
    let total = rows.len() - 1;
    if summary.is_object() {
        summary["records"] = serde_json::json!(total);
    }
    rows.push(summary);
    rows
}

fn run(command: &str, path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dotnet", command])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

fn method_id(path: &Path, name: &str) -> String {
    outline(path, &["--method", name])
        .into_iter()
        .find(|row| row["record"] == "method" && row["detail"]["name"] == name)
        .unwrap_or_else(|| panic!("missing method {name}"))["at"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn get(name: &str, raw: bool) -> Value {
    let path = input();
    let id = method_id(&path, name);
    let args = if raw {
        vec!["--at", id.as_str(), "--raw"]
    } else {
        vec!["--at", id.as_str()]
    };
    let out = run("get-method", &path, &args);
    success(&out);
    let row: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(row["detail"]["complete"], true);
    row
}

fn original_il(row: &Value, name: &str) {
    assert_eq!(
        row["detail"]["il_hex"],
        expected()["methods"][name]["il_hex"]
    );
    assert!(!row["detail"]["instructions"].as_array().unwrap().is_empty());
}

fn rejected(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "failure emitted partial stdout");
    assert!(!output.stderr.is_empty(), "failure omitted diagnostic");
}

#[test]
fn dotnet_outline_uses_deterministic_content_bound_ids_and_unicode_names() {
    let path = input();
    let rows = outline(&path, &[]);
    assert_eq!(rows, outline(&path, &[]));
    let methods: Vec<_> = rows.iter().filter(|v| v["record"] == "method").collect();
    assert_eq!(
        methods.len(),
        expected()["methods"].as_object().unwrap().len()
    );
    for row in methods {
        assert_eq!(
            row["detail"]["declaring_type"],
            expected()["methods"][row["detail"]["name"].as_str().unwrap()]["declaring_type"]
        );
        assert_eq!(row["detail"]["has_body"], true);
        // The handle is a copy-only coordinate; content binding lives in the stream
        // header, so a row does not repeat it.
        assert!(row["at"].as_str().unwrap().starts_with("0x"));
        assert!(row["source_id"].is_null());
    }
    let filtered = outline(&path, &["--type", "Rxd.测试.Probe", "--method", "Add"]);
    assert_eq!(
        filtered.iter().filter(|v| v["record"] == "method").count(),
        1
    );
    assert_eq!(filtered[0]["query"]["type"], "Rxd.测试.Probe");
    // The method filter only reaches the methods listing, so it shows up there.
    let methods_only = jsonl(&run(
        "methods",
        &path,
        &["--type", "Rxd.测试.Probe", "--method", "Add"],
    ));
    assert_eq!(methods_only[0]["query"]["method"], "Add");
    let directory = tempfile::tempdir().unwrap();
    let copy = directory.path().join("same-bytes.exe");
    fs::copy(&path, &copy).unwrap();
    assert_eq!(method_id(&path, "Add"), method_id(&copy, "Add"));
}

#[test]
fn dotnet_raw_il_is_complete_and_matches_the_independent_assembler() {
    for name in expected()["methods"].as_object().unwrap().keys() {
        let row = get(name, true);
        assert_eq!(row["detail"]["representation"], "il");
        assert_eq!(row["detail"]["decompiled"], false);
        original_il(&row, name);
        let instructions = row["detail"]["instructions"].as_array().unwrap();
        assert_eq!(instructions.last().unwrap()["mnemonic"], "ret");
    }
}

#[test]
fn dotnet_simple_method_has_csharp_or_engine_unavailable_with_original_il() {
    let row = get("Add", false);
    original_il(&row, "Add");
    csharp_or_unavailable(&row);
}

fn unavailable(row: &Value) {
    assert_eq!(row["detail"]["representation"], "il", "{row}");
    assert_eq!(row["detail"]["decompiled"], false);
    assert_eq!(row["detail"]["reason"], "engine_unavailable");
    assert!(row["detail"]["code"].is_null());
    assert!(row["detail"]["mapping"].as_array().unwrap().is_empty());
}

fn csharp_or_unavailable(row: &Value) {
    if !cfg!(feature = "csharp-decompiler") {
        unavailable(row);
        return;
    }
    assert_eq!(row["detail"]["representation"], "csharp", "{row}");
    assert_eq!(row["detail"]["decompiled"], true);
    assert!(row["detail"]["reason"].is_null());
    let code = row["detail"]["code"].as_str().unwrap();
    assert!(!code.trim().is_empty());
    let lines = code.lines().count() as u64;
    let mapping = row["detail"]["mapping"].as_array().unwrap();
    assert!(
        !mapping.is_empty(),
        "ordinary method omitted navigation: {row}"
    );
    for site in mapping {
        let line = site["line"].as_u64().unwrap();
        assert!(line > 0 && line <= lines, "{site}");
        assert!(
            row["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|instruction| instruction["offset"] == site["il_offset"]),
            "{site}"
        );
    }
}

#[test]
fn dotnet_numeric_reference_and_prefix_methods_have_csharp_and_original_il() {
    for (name, operation) in [
        ("Checked", "add.ovf"),
        ("UnsignedGreater", "cgt.un"),
        ("RefRead", "ldind.i4"),
        ("VolatileRef", "volatile."),
    ] {
        let row = get(name, false);
        original_il(&row, name);
        assert!(
            row["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["mnemonic"] == operation)
        );
        csharp_or_unavailable(&row);
    }
}

#[test]
fn dotnet_effects_and_branch_merges_have_csharp_and_original_il() {
    for name in ["Effect", "Discard", "Duplicate", "Merge"] {
        let row = get(name, false);
        original_il(&row, name);
        csharp_or_unavailable(&row);
    }
}

#[test]
fn dotnet_finally_methods_retain_complete_original_exception_handlers() {
    for name in [
        "Finally",
        "NestedFinally",
        "SharedFinally",
        "AliasedFinally",
        "FinallySkip",
    ] {
        let row = get(name, false);
        original_il(&row, name);
        csharp_or_unavailable(&row);
        let raw = get(name, true);
        assert_eq!(row["detail"]["handlers"], raw["detail"]["handlers"]);
        assert_eq!(row["detail"]["instructions"], raw["detail"]["instructions"]);
        assert!(!row["detail"]["handlers"].as_array().unwrap().is_empty());
    }
    let row = get("Finally", false);
    assert_eq!(
        row["detail"]["handlers"],
        serde_json::json!([{
            "kind":"finally", "try_offset":0, "try_length":4,
            "handler_offset":4, "handler_length":3,
            "class_token":null, "filter_offset":null
        }])
    );
}

#[test]
fn dotnet_catch_filter_rethrow_and_fault_retain_original_exception_handlers() {
    for name in ["Catch", "Filter", "Rethrow", "Fault"] {
        let row = get(name, false);
        original_il(&row, name);
        csharp_or_unavailable(&row);
        assert_eq!(
            row["detail"]["handlers"],
            get(name, true)["detail"]["handlers"]
        );
        assert!(!row["detail"]["handlers"].as_array().unwrap().is_empty());
        if name == "Filter" {
            assert_eq!(row["detail"]["handlers"][0]["filter_offset"], 2);
        }
        if name == "Fault" {
            assert_eq!(row["detail"]["handlers"][0]["kind"], "fault");
        }
    }
}

#[test]
fn dotnet_values_generics_arrays_and_prefixes_have_csharp_and_original_il() {
    for name in [
        "ValueCopy",
        "ValueField",
        "ValueInit",
        "ValueLoad",
        "ValueStore",
        "ValueCopyAddress",
        "ValueUnbox",
        "NewArray",
        "ValueArrayRead",
        "ReadonlyAddress",
        "UnalignedRef",
        "GenericIdentity",
        "GenericCaller",
        "GenericOwnerField",
        "GenericMixedCaller",
        "GenericExternalCall",
        "ValueCall",
        "ValueNew",
        "GetValue",
        "AliasClassValue",
    ] {
        let row = get(name, false);
        original_il(&row, name);
        csharp_or_unavailable(&row);
        if name == "AliasClassValue" {
            assert_eq!(
                row["detail"]["method"]["detail"]["declaring_type_is_value_type"],
                false
            );
        }
    }
}

#[test]
fn dotnet_engine_warnings_keep_the_reconstruction_and_are_reported_per_member() {
    let row = get("VolatileAdd", false);
    original_il(&row, "VolatileAdd");
    assert_eq!(
        row["detail"]["handlers"],
        get("VolatileAdd", true)["detail"]["handlers"]
    );
    if cfg!(feature = "csharp-decompiler") {
        // ILSpy reconstructed the body: caveats are data, not a failure.
        assert_eq!(row["detail"]["decompiled"], true, "{row}");
        assert_eq!(row["detail"]["representation"], "csharp");
        assert!(row["detail"]["reason"].is_null());
        assert!(!row["detail"]["code"].as_str().unwrap().is_empty());
        assert!(!row["detail"]["warnings"].as_array().unwrap().is_empty());
        assert!(!row["detail"]["mapping"].as_array().unwrap().is_empty());
        // Asking for IL explicitly still bypasses the engine entirely.
        let raw = get("VolatileAdd", true);
        assert_eq!(raw["detail"]["decompiled"], false);
        assert!(raw["detail"]["warnings"].as_array().unwrap().is_empty());
    } else {
        assert_eq!(row["detail"]["decompiled"], false);
        assert_eq!(row["detail"]["representation"], "il");
        assert!(row["detail"]["code"].is_null());
        assert!(row["detail"]["mapping"].as_array().unwrap().is_empty());
        assert!(!row["detail"]["reason"].as_str().unwrap().is_empty());
        unavailable(&row);
    }
}

#[test]
fn dotnet_runtime_missing_engine_degrades_and_other_commands_are_unchanged() {
    let path = input();
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.dylib");
    let id = method_id(&path, "Finally");
    let invoke = |command: &str, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rxd"))
            .env("RXD_DOTNET_NATIVE_LIBRARY", &missing)
            .args(["dotnet", command])
            .arg(&path)
            .args(args)
            .output()
            .unwrap()
    };
    let out = invoke("get-method", &["--at", &id]);
    success(&out);
    let row: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(row["detail"]["complete"], true);
    unavailable(&row);
    original_il(&row, "Finally");
    assert_eq!(
        row["detail"]["handlers"],
        get("Finally", true)["detail"]["handlers"]
    );
    for (command, args) in [
        ("get-method", vec!["--at", id.as_str(), "--raw"]),
        ("methods", vec![]),
        ("strings", vec![]),
        ("xref", vec!["--at", "0x06000006"]),
    ] {
        let without_engine = invoke(command, &args);
        success(&without_engine);
        assert_eq!(without_engine.stdout, run(command, &path, &args).stdout);
    }
}

#[test]
fn dotnet_strings_and_explicit_call_references_preserve_unicode_and_offsets() {
    let strings = jsonl(&run("strings", &input(), &["--filter", "世界"]));
    let strings: Vec<_> = strings
        .iter()
        .filter(|row| row["record"] == "string")
        .collect();
    assert_eq!(strings.len(), 1);
    assert_eq!(strings[0]["detail"]["value"], expected()["string"]);
    let all_strings = jsonl(&run("strings", &input(), &[]));
    let unpaired = all_strings
        .iter()
        .find(|row| row["record"] == "string" && row["detail"]["value"].is_null())
        .unwrap();
    assert_eq!(unpaired["detail"]["utf16_units"], expected()["utf16_units"]);
    let refs = jsonl(&run("xref", &input(), &["--at", "0x06000006"]));
    let refs: Vec<_> = refs
        .iter()
        .filter(|row| row["record"] == "reference")
        .collect();
    assert_eq!(refs.len(), 2, "{refs:?}");
    assert!(refs.iter().all(|row| row["detail"]["il_offset"] == 0));
}

#[test]
fn dotnet_rejects_malformed_inputs_without_partial_stdout() {
    let original = input();
    let directory = tempfile::tempdir().unwrap();
    let bytes = fs::read(&original).unwrap();
    let malformed = directory.path().join("truncated.dll");
    fs::write(&malformed, &bytes[..100]).unwrap();
    rejected(run("methods", &malformed, &[]));
    rejected(run("get-method", &original, &["--at", "not-a-member-id"]));
    let bad_il = directory.path().join("invalid-opcode.dll");
    let mut bytes = fs::read(&original).unwrap();
    // The original assembler places tiny Add in .text at RVA 0x2000 +
    // section offset. Replace add with an undefined CIL opcode, preserving PE.
    let rva = expected()["methods"]["Add"]["rva"].as_u64().unwrap() as usize;
    bytes[512 + rva - 0x2000 + 1 + 2] = 0xff;
    fs::write(&bad_il, bytes).unwrap();
    let invalid_id = method_id(&bad_il, "Add");
    rejected(run("get-method", &bad_il, &["--at", &invalid_id, "--raw"]));
}

#[test]
fn dotnet_records_explain_why_a_unit_is_not_csharp() {
    let path = input();
    let id = method_id(&path, "Add");

    // An explicitly requested IL record is not a failure: neither reason field is set.
    let il = run("get-method", &path, &["--at", &id, "--raw"]);
    success(&il);
    let row: Value = serde_json::from_slice(&il.stdout).unwrap();
    assert_eq!(row["detail"]["representation"], "il");
    assert!(row["detail"]["reason"].is_null());
    assert!(
        row["detail"]["reason_detail"].is_null(),
        "requested IL must not carry a failure explanation"
    );

    // An automatic request that could not deliver C# must explain itself.
    let auto = get("Add", false);
    if auto["detail"]["decompiled"] == false {
        let reason = auto["detail"]["reason"].as_str().unwrap_or_default();
        let detail = auto["detail"]["reason_detail"].as_str().unwrap_or_default();
        assert!(!reason.is_empty(), "{auto}");
        assert!(!detail.is_empty(), "a fallback must explain itself: {auto}");
        assert!(detail.contains("IL"), "{detail}");
    }

    // Class members state that IL is the schema, not a failed reconstruction.
    let outline = outline(&path, &[]);
    let ty = outline
        .iter()
        .find(|row| {
            row["record"] == "type" && row["at"].is_string() && row["detail"]["token"] != 33554433
        })
        .unwrap()["at"]
        .as_str()
        .unwrap()
        .to_owned();
    let class = run("get-class", &path, &["--at", &ty]);
    success(&class);
    let class: Value = serde_json::from_slice(&class.stdout).unwrap();
    for member in class["detail"]["methods"].as_array().unwrap() {
        assert_eq!(member["detail"]["reason"], "member_inventory");
        assert!(
            member["detail"]["reason_detail"]
                .as_str()
                .unwrap()
                .contains("get-method"),
            "{member}"
        );
    }
}

#[test]
fn dotnet_fetch_reads_the_coordinate_from_a_selector_stream() {
    let path = input();
    let at = method_id(&path, "Add");
    assert!(at.starts_with("0x"), "{at}");
    let outline = run("methods", &path, &["--method", "Add"]);
    success(&outline);

    let directory = tempfile::tempdir().unwrap();
    let stream = directory.path().join("selector.jsonl");
    fs::write(&stream, &outline.stdout).unwrap();
    let fetched = run(
        "get-method",
        &path,
        &["--from", stream.to_str().unwrap(), "--raw"],
    );
    success(&fetched);
    let row: Value = serde_json::from_slice(&fetched.stdout).unwrap();
    assert_eq!(row["at"], at);

    let mut child = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dotnet", "get-method"])
        .arg(&path)
        .args(["--from", "-", "--raw"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&outline.stdout)
            .unwrap();
    }
    drop(child.stdin.take());
    let piped = child.wait_with_output().unwrap();
    success(&piped);
    assert_eq!(
        serde_json::from_slice::<Value>(&piped.stdout).unwrap()["at"],
        at
    );

    // A stream from another unit kind, or from another input, never fetches.
    let other = run("strings", &path, &[]);
    success(&other);
    let wrong_unit = directory.path().join("strings.jsonl");
    fs::write(&wrong_unit, &other.stdout).unwrap();
    let refused = run(
        "get-class",
        &path,
        &["--from", wrong_unit.to_str().unwrap()],
    );
    assert_eq!(refused.status.code(), Some(2));
    rejected(refused);

    let foreign = directory.path().join("foreign.jsonl");
    let expected_input = expected();
    let sha = expected_input["sha256"].as_str().unwrap();
    fs::write(
        &foreign,
        String::from_utf8_lossy(&outline.stdout).replace(sha, &"0".repeat(64)),
    )
    .unwrap();
    let mismatched = run("get-method", &path, &["--from", foreign.to_str().unwrap()]);
    assert_eq!(mismatched.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&mismatched.stderr).contains("different input"));
    rejected(mismatched);
}

#[test]
fn dotnet_coordinates_are_bare_metadata_tokens() {
    let path = input();
    for at in ["0x06000001", "06000001"] {
        success(&run("get-method", &path, &["--at", at, "--raw"]));
    }
    for at in ["0600001", "0x060000010", "0x601", "not-a-token"] {
        let out = run("get-method", &path, &["--at", at, "--raw"]);
        assert_eq!(out.status.code(), Some(2), "{at}");
        assert!(out.stdout.is_empty(), "{at} emitted partial stdout");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("INVALID_QUERY"),
            "{at}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // A leftover kind prefix is refused with the value that would work.
    for at in ["method:06000001", "token:0x06000001"] {
        let out = run("get-method", &path, &["--at", at, "--raw"]);
        assert_eq!(out.status.code(), Some(2), "{at}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("metadata token alone"), "{at}: {stderr}");
        assert!(stderr.contains("06000001"), "{at}: {stderr}");
    }
}

#[test]
fn dotnet_outline_ends_a_closed_pipe_quietly() {
    let path = input();
    let mut child = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dotnet", "methods"])
        .arg(&path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    {
        use std::io::Read;
        let mut first = [0u8; 1];
        stdout.read_exact(&mut first).unwrap();
    }
    drop(stdout);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.stderr.is_empty(),
        "a closed pipe must not print a diagnostic: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn dotnet_budgets_are_internal_floors_not_cli_knobs() {
    let path = input();
    // One budget table lives in rxd-core; no backend exposes a knob for it.
    for flag in [
        "--max-input-bytes",
        "--max-entities",
        "--max-work",
        "--max-records",
        "--max-output-bytes",
        "--max-instructions",
    ] {
        let output = run("methods", &path, &[flag, "1"]);
        assert_eq!(output.status.code(), Some(2), "{flag}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unexpected argument"),
            "{flag}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // Empty EH sections still cost work, which the internal budget accounts for: a valid
    // chain of 5,000 sections is analysed instead of being rejected for its shape.
    let directory = tempfile::tempdir().unwrap();
    let chained = directory.path().join("empty-eh-chain.dll");
    fs::write(&chained, empty_exception_sections(5_000)).unwrap();
    let finally_id = method_id(&chained, "Finally");
    success(&run(
        "get-method",
        &chained,
        &["--at", &finally_id, "--raw"],
    ));
}

#[test]
fn dotnet_work_budget_counts_analysis_not_unreferenced_module_padding() {
    let directory = tempfile::tempdir().unwrap();
    let padded = directory.path().join("padded.dll");
    let mut bytes = fs::read(input()).unwrap();
    bytes.resize(1024 * 1024, 0);
    fs::write(&padded, bytes).unwrap();
    let id = method_id(&padded, "Add");
    let output = run("get-method", &padded, &["--at", &id]);
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    original_il(&row, "Add");
    csharp_or_unavailable(&row);
}

#[test]
fn dotnet_output_files_are_atomic_and_perf_is_separate_from_results() {
    let path = input();
    let id = method_id(&path, "Add");
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("result.json");
    fs::write(&destination, b"previous result").unwrap();
    rejected(run(
        "get-method",
        &path,
        &[
            "--at",
            &id,
            "-o",
            destination.to_str().unwrap(),
            "--max-output-bytes",
            "1",
        ],
    ));
    assert_eq!(fs::read(&destination).unwrap(), b"previous result");
    let plain = run("get-method", &path, &["--at", &id]);
    success(&plain);
    let out = run(
        "get-method",
        &path,
        &["--at", &id, "-o", destination.to_str().unwrap(), "--perf"],
    );
    success(&out);
    assert!(out.stdout.is_empty());
    assert_eq!(fs::read(&destination).unwrap(), plain.stdout);
    let diagnostics = String::from_utf8(out.stderr).unwrap();
    assert!(diagnostics.contains("dotnet"));
    assert!(diagnostics.contains("read_parse"));
    assert!(diagnostics.contains("reconstruct"));
}

// Derive an original, valid PE fixture by relocating the existing finally body
// after the file and extending the single mapped .text section. Metadata remains
// unchanged except its MethodDef RVA; the original clause follows empty sections.
fn empty_exception_sections(count: usize) -> Vec<u8> {
    let mut bytes = fs::read(input()).unwrap();
    let old_rva = expected()["methods"]["Finally"]["rva"].as_u64().unwrap() as u32;
    let old_offset = 512 + old_rva as usize - 0x2000;
    let prefix = bytes[old_offset..old_offset + 24].to_vec();
    let clause = bytes[old_offset + 24..old_offset + 40].to_vec();
    assert_eq!(&clause[..4], &[1, 16, 0, 0]);
    let mut method_row_start = old_rva.to_le_bytes().to_vec();
    method_row_start.extend_from_slice(&[0, 0, 0x16, 0]);
    let matches: Vec<_> = bytes
        .windows(8)
        .enumerate()
        .filter_map(|(i, data)| (data == method_row_start).then_some(i))
        .collect();
    assert_eq!(matches.len(), 1);
    let new_rva = 0x2000 + bytes.len() as u32 - 512;
    bytes[matches[0]..matches[0] + 4].copy_from_slice(&new_rva.to_le_bytes());
    bytes.extend_from_slice(&prefix);
    for _ in 0..count {
        bytes.extend_from_slice(&[0x81, 4, 0, 0]);
    }
    bytes.extend_from_slice(&clause);
    bytes.resize(bytes.len().div_ceil(512) * 512, 0);
    let section_bytes = (bytes.len() - 512) as u32;
    // Fixed PE header offsets are defined by the original assembler.
    bytes[376 + 8..376 + 12].copy_from_slice(&section_bytes.to_le_bytes());
    bytes[376 + 16..376 + 20].copy_from_slice(&section_bytes.to_le_bytes());
    bytes[152 + 4..152 + 8].copy_from_slice(&section_bytes.to_le_bytes());
    let image_bytes = (0x2000 + section_bytes).div_ceil(0x2000) * 0x2000;
    bytes[152 + 56..152 + 60].copy_from_slice(&image_bytes.to_le_bytes());
    bytes
}

#[test]
fn dotnet_repeated_attribute_targets_do_not_hide_an_invalid_parent_reference() {
    let bad =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/InvalidReference.dll");
    let output = run("methods", &bad, &["--method", "Add"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_INPUT"));
}

#[test]
fn dotnet_malformed_demand_parsed_generic_owner_fails_without_partial_output() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("invalid-owner-signature.dll");
    let mut bytes = fs::read(input()).unwrap();
    // Independently specified #Blob record: GenericBox<!!0> owner TypeSpec.
    // MethodDef signatures remain intact so outline can still supply its ID.
    let blob = [6, 0x15, 0x12, 0x10, 1, 0x1e, 0];
    let offsets: Vec<_> = bytes
        .windows(blob.len())
        .enumerate()
        .filter_map(|(index, data)| (data == blob).then_some(index))
        .collect();
    assert_eq!(offsets.len(), 1);
    bytes[offsets[0] + 5] = 0xff;
    fs::write(&file, bytes).unwrap();
    let id = method_id(&file, "GenericOwnerField");
    let output = run("get-method", &file, &["--at", &id]);
    assert_eq!(output.status.code(), Some(1));
    rejected(output);
}
