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
fn dotnet_metadata_budget_shared_blobs_do_not_bypass_unselected_signature_validation() {
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
    // The invalid method is unrelated to the requested earlier member. Its blob
    // was already valid under the FIELD grammar; it must still fail as a METHOD.
    let output = run(
        "methods",
        &changed,
        &[
            "--class",
            "Newtonsoft.Json.JsonReader",
            "--name",
            "ReadAsInt32",
            "-o",
            destination.to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("INVALID_INPUT"), "{diagnostic}");
    assert!(diagnostic.contains("calling convention"), "{diagnostic}");
    assert_eq!(fs::read(destination).unwrap(), b"keep");
}

#[test]
fn dotnet_metadata_budget_deferred_call_projection_keeps_generic_contexts_independent() {
    // resolve_call is a public library operation, not used by the current CLI's
    // ILSpy path. Exercise that seam directly rather than expose a test-only flag.
    let parsed = rxd_dotnet::Input::read(&input(), rxd_dotnet::Limits::default()).unwrap();
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
