//! Static .NET CLI contracts using independently assembled original fixtures.
use super::*;

mod class;
#[cfg(all(feature = "csharp-decompiler", unix))]
mod class_csharp;
mod closure;
mod field_type_refs;
mod metadata_budget;
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
        ("asar", cfg!(feature = "backend-asar")),
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
fn dotnet_outline_rows_have_a_fixed_schema_and_keep_fetch_ids_navigable() {
    let path = input();
    let rows = outline(&path, &["--name", "Add"]);
    for row in &rows[..] {
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
        let rows = outline(&path, &["--name", name]);
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
fn dotnet_search_and_reference_streams_route_exact_token_hits_to_original_methods() {
    let path = input();
    let strings = jsonl(&run("strings", &path, &["--filter", "世界"]));
    let literal = &strings[0];
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
    let hit = &rows[0];
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
    assert!(zero.is_empty());
}

fn input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/Fixture.dll")
}

fn expected() -> Value {
    serde_json::from_str(include_str!("../fixtures/dotnet/expected.json")).unwrap()
}

fn outline(path: &Path, args: &[&str]) -> Vec<Value> {
    let mut head: Vec<&str> = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if *arg == "--name" {
            skip = true;
            continue;
        }
        head.push(arg);
    }
    let mut rows = Vec::new();
    for sub in ["classes", "fields", "methods"] {
        let sub_args: &[&str] = if sub == "methods" { args } else { &head };
        rows.extend(jsonl(&run(sub, path, sub_args)));
    }
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
    outline(path, &["--name", name])
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
    let filtered = outline(&path, &["--class", "Rxd.测试.Probe", "--name", "Add"]);
    assert_eq!(
        filtered.iter().filter(|v| v["record"] == "method").count(),
        1
    );
    // The method filter only reaches the methods listing, so it shows up there.
    let methods_only = jsonl(&run(
        "methods",
        &path,
        &["--class", "Rxd.测试.Probe", "--name", "Add"],
    ));
    assert_eq!(methods_only.len(), 1);
    assert_eq!(methods_only[0]["detail"]["name"], "Add");
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
    // Five thousand empty EH sections are valid and fully traversed.
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

// Relocate valid IL/EH by extending .text and updating its MethodDef RVA.
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
fn dotnet_unrelated_invalid_parent_reference_stays_out_of_local_listings() {
    let bad =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/InvalidReference.dll");
    // A name-only query must ignore unrelated custom-attribute corruption.
    let output = run("methods", &bad, &["--name", "Add"]);
    success(&output);
    let rows = jsonl(&output);
    assert_eq!(rows.len(), 1);

    // The explicit whole-structure check still reports the corruption.
    let parsed = rxd_dotnet::Input::read(&bad, rxd_dotnet::Limits::default()).unwrap();
    let error = parsed.verify_module().unwrap_err().to_string();
    assert!(error.contains("INVALID_INPUT"), "{error}");
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
