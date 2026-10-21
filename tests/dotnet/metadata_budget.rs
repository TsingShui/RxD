//! A published assembly must support ordinary metadata navigation under the shared budget.
use super::*;
use sha2::{Digest, Sha256};

fn production() -> (PathBuf, Value) {
    let directory =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/dotnet/newtonsoft-json-13.0.3");
    let oracle: Value =
        serde_json::from_slice(&fs::read(directory.join("metadata.json")).unwrap()).unwrap();
    let path = directory.join("Newtonsoft.Json.dll");
    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        bytes.len() as u64,
        oracle["provenance"]["dll_bytes"].as_u64().unwrap()
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(bytes)),
        oracle["provenance"]["dll_sha256"]
    );
    (path, oracle)
}

#[test]
fn dotnet_metadata_budget_real_listings_and_method_stream_preserve_complete_original_data() {
    let (path, oracle) = production();
    assert_eq!(
        rxd_dotnet::Limits::default().work,
        rxd_core::Budget::default().work
    );
    assert_eq!(rxd_dotnet::Limits::default().work, 1_000_000);
    for (command, record, table) in [
        ("classes", "class", 2u32),
        ("fields", "field", 4),
        ("methods", "method", 6),
    ] {
        let output = run(command, &path, &[]);
        success(&output);
        let rows = jsonl(&output);
        let count = oracle["counts"][command].as_u64().unwrap() as usize;
        assert_eq!(rows[0]["source_id"], oracle["provenance"]["dll_sha256"]);
        assert_eq!(rows.last().unwrap()["complete"], true);
        assert_eq!(rows.last().unwrap()["records"], count);
        assert_eq!(rows.len(), count + 2);
        for (index, row) in rows[1..rows.len() - 1].iter().enumerate() {
            assert_eq!(row["record"], record);
            assert_eq!(
                row["at"],
                format!("0x{:08x}", table << 24 | (index as u32 + 1))
            );
        }
    }
    let matches = jsonl(&run(
        "methods",
        &path,
        &["--class", "Newtonsoft.Json.JsonReader", "-f", "ReadAsInt32"],
    ));
    assert_eq!(matches.last().unwrap()["records"], 2);
    let listed = run(
        "methods",
        &path,
        &[
            "--class",
            "Newtonsoft.Json.JsonReader",
            "--name",
            "ReadAsInt32",
        ],
    );
    success(&listed);
    let rows = jsonl(&listed);
    assert_eq!(rows.last().unwrap()["records"], 1);
    assert_eq!(rows[1]["at"], "0x060000fb");
    let directory = tempfile::tempdir().unwrap();
    let stream = directory.path().join("methods.jsonl");
    fs::write(&stream, &listed.stdout).unwrap();
    let output = run(
        "get-method",
        &path,
        &["--from", stream.to_str().unwrap(), "--raw"],
    );
    success(&output);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["at"], rows[1]["at"]);
    assert_eq!(body["detail"]["method"], rows[1]);
    assert_eq!(body["detail"]["representation"], "il");
    assert_eq!(body["detail"]["complete"], true);
    let original = fs::read(&path).unwrap();
    let start = body["start"].as_u64().unwrap() as usize;
    let end = body["end"].as_u64().unwrap() as usize;
    let il = &original[start..end];
    assert_eq!(
        body["detail"]["il_hex"],
        il.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let independent: Value =
        serde_json::from_str(include_str!("../corpus/dotnet/class-expectations.json")).unwrap();
    let expected = &independent["workloads"][0]["methods"]["0x060000fb"];
    assert_eq!(format!("{:x}", Sha256::digest(il)), expected["il_sha256"]);
    assert_eq!(body["detail"]["handlers"], expected["handlers"]);
}

#[test]
fn dotnet_class_stream_routes_fetch_and_type_xref_with_content_identity() {
    let (path, _) = production();
    let original = fs::read(&path).unwrap();
    let name = "Newtonsoft.Json.JsonConverter";
    let listed = run("classes", &path, &["--class", name]);
    let rows = jsonl(&listed);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1]["record"], "class");
    assert_eq!(rows[1]["detail"]["full_name"], name);
    assert_eq!(rows[2]["records"], 1);
    assert_eq!(rows[2]["complete"], true);
    let at = rows[1]["at"].as_str().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let stream = directory.path().join("classes.jsonl");
    fs::write(&stream, &listed.stdout).unwrap();
    let commands: &[(&str, &[&str])] = &[("get-class", &["--raw"]), ("xref", &[])];
    for &(command, extra) in commands {
        let mut args = vec!["--at", at];
        args.extend_from_slice(extra);
        let direct = run(command, &path, &args);
        success(&direct);
        let mut args = vec!["--from", stream.to_str().unwrap()];
        args.extend_from_slice(extra);
        let from_file = run(command, &path, &args);
        success(&from_file);
        let mut child = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["dotnet", command])
            .arg(&path)
            .args(["--from", "-"])
            .args(extra)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&listed.stdout)
                .unwrap();
        }
        let from_stdin = child.wait_with_output().unwrap();
        success(&from_stdin);
        for fetched in [&from_file, &from_stdin] {
            if command == "get-class" {
                assert_eq!(fetched.stdout, direct.stdout);
                let body: Value = serde_json::from_slice(&fetched.stdout).unwrap();
                assert_eq!(body["at"], at);
                assert_eq!(body["detail"]["type"], rows[1]);
                assert_eq!(body["detail"]["representation"], "il");
                assert_eq!(body["detail"]["complete"], true);
            } else {
                let direct_rows = jsonl(&direct);
                let fetched_rows = jsonl(fetched);
                // Query echoes differ by --at/--from; the target and evidence do not.
                assert_eq!(fetched_rows[0]["detail"], direct_rows[0]["detail"]);
                assert_eq!(fetched_rows[0]["query"]["kind"], "type");
                assert_eq!(fetched_rows[0]["detail"]["target"]["target_kind"], "type");
                assert_eq!(fetched_rows[0]["detail"]["target"]["name"], name);
                assert_eq!(fetched_rows[1..], direct_rows[1..]);
                assert_eq!(fetched_rows.last().unwrap()["complete"], true);
                assert!(fetched_rows.last().unwrap()["records"].as_u64().unwrap() > 0);
            }
        }
    }
    // A class record still denotes a type token, not a new --kind class spelling.
    failure(
        run(
            "xref",
            &path,
            &["--from", stream.to_str().unwrap(), "--kind", "method"],
        ),
        2,
        "is a type token, but --kind method",
    );
    // A same-size, valid DOS-header mutation keeps the token but changes content identity.
    let mut changed = original.clone();
    changed[0x20] ^= 1;
    let foreign = directory.path().join("changed.dll");
    fs::write(&foreign, &changed).unwrap();
    success(&run("classes", &foreign, &["--class", name]));
    let destination = directory.path().join("keep.jsonl");
    for &(command, extra) in commands {
        fs::write(&destination, b"keep").unwrap();
        let mut args = vec![
            "--from",
            stream.to_str().unwrap(),
            "-o",
            destination.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        failure(run(command, &foreign, &args), 1, "SOURCE_MISMATCH");
        assert_eq!(fs::read(&destination).unwrap(), b"keep");
    }
    assert_eq!(fs::read(&foreign).unwrap(), changed);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn dotnet_working_set_and_class_projection_bounds_are_real_resource_failures() {
    // The per-body and whole-class projections are bounded by the selected output
    // envelope. `Input::read` ignores that dimension, so the guards can be exercised
    // without a giant fixture: the accounting must refuse before allocating/serializing.
    let fixture = input();
    let tight = rxd_dotnet::Limits {
        output_bytes: 1,
        ..rxd_dotnet::Limits::default()
    };
    let mut parsed = rxd_dotnet::Input::read(&fixture, tight).unwrap();
    let token = parsed
        .method_by_id(&method_id(&fixture, "Add"))
        .unwrap()
        .token;
    let error = parsed.body_token(token).err().unwrap().to_string();
    assert!(error.contains("RESOURCE_LIMIT"), "{error}");
    assert!(error.contains("IL working set"), "{error}");

    // An interface has metadata and declarations but no method bodies, so the class
    // projection reaches its own serialized-output bound first.
    let closure =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/closure/Closure.dll");
    let mut parsed = rxd_dotnet::Input::read(&closure, tight).unwrap();
    let interface = parsed
        .types
        .iter()
        .find(|ty| ty.full_name.as_str() == "Rxd.Closure.IContract`1")
        .unwrap();
    let interface_id = interface.id.clone();
    let root = parsed.prepare_class(&interface_id).unwrap();
    let error = parsed.class_body(root).err().unwrap().to_string();
    assert!(error.contains("RESOURCE_LIMIT"), "{error}");
    assert!(error.contains("class output"), "{error}");

    // The same operations succeed under the default projection allowance.
    let mut parsed = rxd_dotnet::Input::read(&fixture, rxd_dotnet::Limits::default()).unwrap();
    let token = parsed
        .method_by_id(&method_id(&fixture, "Add"))
        .unwrap()
        .token;
    parsed.body_token(token).unwrap();
    let mut parsed = rxd_dotnet::Input::read(&closure, rxd_dotnet::Limits::default()).unwrap();
    let interface = parsed
        .types
        .iter()
        .find(|ty| ty.full_name.as_str() == "Rxd.Closure.IContract`1")
        .unwrap();
    let interface_id = interface.id.clone();
    let root = parsed.prepare_class(&interface_id).unwrap();
    parsed.class_body(root).unwrap();
}

#[test]
fn dotnet_unreadable_index_rows_are_reported_without_failing_local_queries() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/closure");
    let path = directory.join("Closure.dll");
    let oracle: Value =
        serde_json::from_slice(&fs::read(directory.join("expected.json")).unwrap()).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    let nested = oracle["table_file_offsets"]["nested_class"]
        .as_u64()
        .unwrap() as usize;
    // Second NestedClass row: nested_class at +4, enclosing_class at +6 (u16 each).
    // Point the enclosing index past the TypeDef table so that row cannot be indexed.
    bytes[nested + 6..nested + 8].copy_from_slice(&0xffffu16.to_le_bytes());
    let temp = tempfile::tempdir().unwrap();
    let broken = temp.path().join("unreadable-nested.dll");
    fs::write(&broken, &bytes).unwrap();

    // Another class still lists; the stream says it is not complete and names the row.
    let listed = run(
        "methods",
        &broken,
        &["--class", "Rxd.Closure.Base", "--name", "Inherited"],
    );
    // `success()` requires a complete summary; this stream is deliberately incomplete.
    assert_eq!(listed.status.code(), Some(0));
    assert!(listed.stderr.is_empty());
    // `jsonl()` insists on a complete summary; parse this deliberately incomplete one.
    let rows: Vec<Value> = String::from_utf8(listed.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.last().unwrap()["records"], 1);
    assert_eq!(rows.last().unwrap()["complete"], true);
    assert_eq!(rows[1]["at"], "0x06000004");

    // A FETCH that does not need the skipped row still answers.
    success(&run(
        "get-method",
        &broken,
        &["--at", "0x06000004", "--raw"],
    ));

    // The sibling nested class is still returned; a produced class body is the
    // success criterion, not proving that an unrelated row was readable.
    let sibling = run("get-class", &broken, &["--at", "0x02000008", "--raw"]);
    assert_eq!(sibling.status.code(), Some(0));
    let sibling: Value = serde_json::from_slice(&sibling.stdout).unwrap();
    assert_eq!(sibling["detail"]["complete"], true);

    // The skipped type itself is reported, atomically.
    let destination = temp.path().join("keep.json");
    fs::write(&destination, b"keep").unwrap();
    failure(
        run(
            "get-class",
            &broken,
            &[
                "--at",
                "0x02000009",
                "--raw",
                "-o",
                destination.to_str().unwrap(),
            ],
        ),
        1,
        "INVALID_INPUT",
    );
    assert_eq!(fs::read(&destination).unwrap(), b"keep");
    assert_eq!(fs::read(&broken).unwrap(), bytes);
}

#[test]
fn dotnet_lazy_signatures_keep_unrelated_corruption_local() {
    let (path, oracle) = production();
    let mut bytes = fs::read(&path).unwrap();
    let offset = oracle["mutation"]["last_method_signature_index_offset"]
        .as_u64()
        .unwrap() as usize;
    let width = oracle["mutation"]["blob_index_width"].as_u64().unwrap() as usize;
    let field = oracle["mutation"]["first_field_signature_index"]
        .as_u64()
        .unwrap() as u32;
    bytes[offset..offset + width].copy_from_slice(&field.to_le_bytes()[..width]);
    let directory = tempfile::tempdir().unwrap();
    let changed = directory.path().join("wrong-signature.dll");
    fs::write(&changed, bytes).unwrap();
    let destination = directory.path().join("keep.jsonl");
    fs::write(&destination, b"keep").unwrap();

    // The invalid signature belongs to the last method, which this filtered listing
    // never emits or resolves: the query still answers, completely, from the rows and
    // names it did read.
    let listed = run(
        "methods",
        &changed,
        &[
            "--class",
            "Newtonsoft.Json.JsonReader",
            "--name",
            "ReadAsInt32",
        ],
    );
    success(&listed);
    let rows = jsonl(&listed);
    assert_eq!(rows.last().unwrap()["records"], 1);
    assert_eq!(rows.last().unwrap()["complete"], true);
    // Resolved on demand for the emitted row, not left as the lazy placeholder.
    assert_eq!(rows[1]["detail"]["parameters"], serde_json::json!([]));
    assert!(
        rows[1]["detail"]["signature"]
            .as_str()
            .unwrap()
            .contains("int32")
    );

    // Fetching the broken row itself is in scope: it fails atomically.
    let last = format!(
        "0x{:08x}",
        0x0600_0000 | oracle["counts"]["methods"].as_u64().unwrap()
    );
    fs::write(&destination, b"keep").unwrap();
    failure(
        run(
            "get-method",
            &changed,
            &["--at", &last, "--raw", "-o", destination.to_str().unwrap()],
        ),
        1,
        "calling convention",
    );
    assert_eq!(fs::read(&destination).unwrap(), b"keep");
}

#[test]
fn dotnet_metadata_budget_deferred_call_projection_keeps_generic_contexts_independent() {
    // resolve_call is a public library operation, not used by the current CLI's
    // ILSpy path. Exercise that seam directly rather than expose a test-only flag.
    let mut parsed = rxd_dotnet::Input::read(&input(), rxd_dotnet::Limits::default()).unwrap();
    parsed.resolve_method_token(0x06000021).unwrap();
    parsed.resolve_method_token(0x06000022).unwrap();
    parsed.resolve_method_token(0x06000024).unwrap();
    let generic = parsed.method_by_id("0x06000022").unwrap();
    let mixed = parsed.method_by_id("0x06000024").unwrap();
    for (context, token, returns) in [
        (generic, 0x2b000001, "!!1"),
        (mixed, 0x2b000002, "!!0"),
        (generic, 0x2b000001, "!!1"),
    ] {
        let call = parsed.resolve_call(token, context).unwrap();
        assert_eq!(call.token, token);
        assert_eq!(call.parameters.as_slice(), ["!!1"]);
        assert_eq!(call.return_type.as_str(), returns);
    }
    let add = parsed.resolve_call(0x06000001, generic).unwrap();
    assert_eq!(add.parameters.as_slice(), ["int32", "int32"]);
    assert_eq!(add.return_type.as_str(), "int32");
    assert_eq!(
        parsed
            .method_by_id("0x06000021")
            .unwrap()
            .return_type
            .as_str(),
        "!!0"
    );
    assert!(parsed.resolve_call(0x0a000002, generic).is_err());

    // A deferred projection cannot reset the work already consumed by metadata.
    // Find the public read boundary rather than bake in platform-dependent costs.
    let (mut low, mut high) = (0, rxd_dotnet::Limits::default().work);
    while high - low > 1 {
        let work = low + (high - low) / 2;
        let limits = rxd_dotnet::Limits {
            work,
            ..rxd_dotnet::Limits::default()
        };
        match rxd_dotnet::Input::read(&input(), limits) {
            Ok(_) => high = work,
            Err(error) => {
                assert!(error.to_string().contains("RESOURCE_LIMIT"), "{error:#}");
                low = work;
            }
        }
    }
    let limits = rxd_dotnet::Limits {
        work: high,
        ..rxd_dotnet::Limits::default()
    };
    let exhausted = rxd_dotnet::Input::read(&input(), limits).unwrap();
    let context = exhausted.method_by_id("0x06000022").unwrap();
    let error = exhausted.resolve_call(0x06000001, context).unwrap_err();
    assert!(error.to_string().contains("RESOURCE_LIMIT"), "{error:#}");
}
