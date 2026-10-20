//! Method XREF closes outline → refs → caller method/class navigation.
use super::*;

#[test]
fn dotnet_semantic_signature_expansion_consumes_work_before_cloning() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/dotnet/semantic/expanded/References.dll");
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("refs.jsonl");
    fs::write(&destination, b"preserved").unwrap();
    // The independent assembler substitutes a 1,000-parameter function pointer into
    // 101 !0 positions. The work budget that admits those copies is an internal floor,
    // so the query completes and the output is complete rather than a partial rewrite.
    let output = run("xref", &path, &["--at", "0x0a000007"]);
    failure(output, 1, "RESOURCE_LIMIT");
    assert_eq!(fs::read(destination).unwrap(), b"preserved");
}

#[test]
fn dotnet_semantic_method_refs_find_generic_instantiations_and_preserve_raw_tokens() {
    let path = input();
    let id = method_id(&path, "GenericIdentity");
    let rows = jsonl(&run("xref", &path, &["--at", &id]));
    assert_eq!(rows[0]["schema_version"], 2);
    assert_eq!(rows.last().unwrap()["records"], 1);
    let hit = &rows[1];
    assert_eq!(hit["detail"]["name"], "GenericCaller");
    assert_eq!(hit["detail"]["il_offset"], 1);
    assert_eq!(hit["detail"]["target_token"], 0x2b000001u32);
    assert_eq!(hit["detail"]["resolution_status"], "resolved");
    assert_eq!(hit["detail"]["canonical_target_id"], id);
    assert_eq!(
        hit["detail"]["method_arguments"],
        serde_json::json!([{"kind":"method_parameter","index":1}])
    );
    success(&run(
        "get-method",
        &path,
        &["--at", hit["context_at"].as_str().unwrap(), "--raw"],
    ));
    success(&run(
        "get-class",
        &path,
        &[
            "--at",
            hit["detail"]["declaring_type_id"].as_str().unwrap(),
            "--raw",
        ],
    ));
    let by_token = jsonl(&run("xref", &path, &["--at", "0x06000021"]));
    assert_eq!(by_token[1], rows[1]);
}

fn semantic_input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/semantic/References.dll")
}

#[test]
fn dotnet_semantic_method_refs_prove_local_aliases_without_matching_overloads_or_external_scopes() {
    let path = semantic_input();
    let outline = outline(&path, &[]);
    let id = outline
        .iter()
        .find(|row| row["record"] == "method" && row["detail"]["token"] == 0x06000001u32)
        .unwrap()["at"]
        .as_str()
        .unwrap();
    let rows = jsonl(&run("xref", &path, &["--at", id]));
    let sites: Vec<_> = rows[1..rows.len() - 1]
        .iter()
        .map(|row| {
            (
                row["detail"]["il_offset"].as_u64().unwrap(),
                row["detail"]["opcode"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        sites,
        [(0, "call"), (5, "ldftn"), (11, "ldtoken"), (16, "jmp")]
    );
    assert!(
        rows[1..rows.len() - 1]
            .iter()
            .all(|row| row["detail"]["canonical_target_id"] == id
                && row["detail"]["resolution_status"] == "resolved")
    );
}

#[test]
fn dotnet_reference_scans_validate_the_complete_heap_even_after_one_thousand_unused_entries() {
    let path = semantic_input();
    let expected: Value =
        serde_json::from_slice(&fs::read(path.with_file_name("expected.json")).unwrap()).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    let heap_start = expected["stream_offsets"]["#US"].as_u64().unwrap() as usize;
    bytes[heap_start + 1 + 1001 * 4 + 3] = 2;
    let directory = tempfile::tempdir().unwrap();
    let malformed = directory.path().join("invalid-unused-user-string.dll");
    fs::write(&malformed, bytes).unwrap();
    let destination = directory.path().join("refs.jsonl");
    fs::write(&destination, b"preserved").unwrap();
    let output = run(
        "xref",
        &malformed,
        &["--at", "0x0a000001", "-o", destination.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_INPUT"));
    assert_eq!(fs::read(destination).unwrap(), b"preserved");
}

#[test]
fn dotnet_semantic_method_refs_keep_instantiations_nested_members_and_ambiguous_targets_distinct() {
    let path = semantic_input();
    let instantiation = jsonl(&run("xref", &path, &["--at", "0x2b000001"]));
    assert_eq!(instantiation.last().unwrap()["records"], 2);
    assert_eq!(instantiation[1]["detail"]["target_token"], 0x2b000001u32);
    assert_eq!(instantiation[2]["detail"]["target_token"], 0x2b000003u32);
    let other = jsonl(&run("xref", &path, &["--at", "0x2b000002"]));
    assert_eq!(other.last().unwrap()["records"], 1);
    let generic = jsonl(&run("xref", &path, &["--at", "0x06000004"]));
    assert_eq!(generic.last().unwrap()["records"], 3);
    let owners = jsonl(&run("xref", &path, &["--at", "0x06000008"]));
    assert_eq!(owners.last().unwrap()["records"], 2);
    let owner_int = jsonl(&run("xref", &path, &["--at", "0x0a000007"]));
    assert_eq!(owner_int.last().unwrap()["records"], 1);
    assert_eq!(
        owner_int[1]["detail"]["owner_arguments"],
        serde_json::json!([{"kind":"primitive","element":8}])
    );
    for (token, status, canonical) in [
        ("0x0a000003", "resolved", Some(0x06000003u32)),
        ("0x0a000006", "resolved", Some(0x06000009u32)),
        ("0x0a000009", "ambiguous", None),
        ("0x0a000004", "external", None),
        ("0x0a000005", "external", None),
        ("0x0a00000a", "external", None),
        ("0x0a000011", "unresolved", None),
    ] {
        let rows = jsonl(&run("xref", &path, &["--at", token]));
        assert_eq!(rows.last().unwrap()["records"], 1);
        assert_eq!(rows[1]["detail"]["resolution_status"], status);
        assert_eq!(
            rows[1]["detail"]["canonical_target_token"],
            serde_json::json!(canonical)
        );
        assert_eq!(rows[0]["detail"]["target"]["resolution_status"], status);
    }
    let ambiguous_definition = jsonl(&run("xref", &path, &["--at", "0x06000005"]));
    assert_eq!(ambiguous_definition.last().unwrap()["records"], 0);
}

#[test]
fn dotnet_method_reference_failures_reject_bad_selectors_and_preserve_atomic_outputs() {
    let path = input();
    let id = method_id(&path, "GenericIdentity");
    for args in [
        vec![],
        vec!["--at", &id, "--at", "0x06000021"],
        vec!["--at", "rxd1:dotnet:foreign:method:06000021"],
        vec!["--at", "0x0600ffff"],
        vec!["--at", "0x11000001"],
    ] {
        let output = run("xref", &path, &args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
    }
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("refs.jsonl");
    fs::write(&destination, b"preserved").unwrap();
    let output = run(
        "xref",
        &path,
        &["--at", "0x06000006", "-o", destination.to_str().unwrap()],
    );
    success(&output);
    assert!(!fs::read(&destination).unwrap().is_empty());
    let plain = run("xref", &path, &["--at", &id]);
    success(&plain);
    let repeat = run("xref", &path, &["--at", &id, "--perf"]);
    success(&repeat);
    assert_eq!(plain.stdout, repeat.stdout);
    assert_eq!(String::from_utf8(repeat.stderr).unwrap().lines().count(), 2);
    let file = run(
        "xref",
        &path,
        &["--at", &id, "-o", destination.to_str().unwrap()],
    );
    success(&file);
    assert!(file.stdout.is_empty());
    assert_eq!(fs::read(destination).unwrap(), plain.stdout);
    let semantic_rows = jsonl(&run("xref", &path, &["--at", &id]));
    let keys = semantic_rows[1]["detail"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for args in [
        vec!["--at", "0x06000006"],
        vec!["--at", "0x70000001"],
        vec!["-f", "世界\nλ", "--kind", "string"],
    ] {
        let rows = jsonl(&run("xref", &path, &args));
        for row in &rows[1..rows.len() - 1] {
            assert_eq!(
                row["detail"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
                keys
            );
            assert_eq!(row["parent_at"], row["detail"]["declaring_type_id"]);
        }
    }
}

#[test]
fn dotnet_semantic_method_scans_reject_resolution_cycles_and_invalid_unselected_il() {
    let path = semantic_input();
    let directory = tempfile::tempdir().unwrap();
    let mut bytes = fs::read(&path).unwrap();
    let signature = [5, 0x15, 0x12, 0x0c, 1, 8];
    let offsets: Vec<_> = bytes
        .windows(signature.len())
        .enumerate()
        .filter_map(|(i, bytes)| (bytes == signature).then_some(i))
        .collect();
    assert_eq!(offsets.len(), 1);
    bytes[offsets[0] + 3] = 6; // TypeSpec row 1 recursively names itself.
    let cyclic = directory.path().join("cyclic-typespec.dll");
    fs::write(&cyclic, bytes).unwrap();
    let output = run("xref", &cyclic, &["--at", "0x06000001"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_INPUT"));
    let path = input();
    let mut bytes = fs::read(&path).unwrap();
    let offset =
        512 + expected()["methods"]["Merge"]["rva"].as_u64().unwrap() as usize - 0x2000 + 1;
    bytes[offset + 2] = 2; // branch now enters the following branch's operand.
    let malformed = directory.path().join("bad-unselected-branch.dll");
    fs::write(&malformed, bytes).unwrap();
    let output = run("xref", &malformed, &["--at", "0x06000021"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_INPUT"));
    let rows = jsonl(&run("xref", &semantic_input(), &["--at", "0x06000001"]));
    assert_eq!(rows.last().unwrap()["records"], 4);
}

#[test]
fn dotnet_method_refs_compare_scoped_signature_types_instead_of_display_names() {
    let rows = jsonl(&run("xref", &semantic_input(), &["--at", "0x0600000a"]));
    assert_eq!(rows.last().unwrap()["records"], 2);
    assert_eq!(rows[1]["detail"]["target_token"], 0x0a00000cu32);
    assert_eq!(rows[2]["detail"]["target_token"], 0x0a00000du32);
    let external = jsonl(&run("xref", &semantic_input(), &["--at", "0x0a00000e"]));
    assert_eq!(external.last().unwrap()["records"], 1);
    assert_eq!(external[1]["detail"]["resolution_status"], "unresolved");
    assert!(external[1]["detail"]["canonical_target_id"].is_null());
}

#[test]
fn dotnet_method_refs_preserve_constructed_types_embedded_in_signatures() {
    let rows = jsonl(&run("xref", &semantic_input(), &["--at", "0x0600000b"]));
    assert_eq!(rows.last().unwrap()["records"], 1);
    assert_eq!(rows[1]["detail"]["target_token"], 0x0a00000fu32);
}

#[test]
fn dotnet_method_refs_reject_methodspec_arity_even_when_no_parameter_requires_the_extra_argument() {
    let path = semantic_input();
    let mut bytes = fs::read(&path).unwrap();
    let sig = [7, 0x10, 1, 1, 0x1e, 0, 0x1e];
    let offsets: Vec<_> = bytes
        .windows(sig.len())
        .enumerate()
        .filter_map(|(i, data)| (data == sig).then_some(i))
        .collect();
    assert!(!offsets.is_empty());
    // Both definition and local alias now declare two generic parameters, while
    // the explicit MethodSpecs still contain one. Existing !!0 remains in range.
    for offset in offsets {
        bytes[offset + 2] = 2;
    }
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("wrong-methodspec-arity.dll");
    fs::write(&file, bytes).unwrap();
    let out = run("xref", &file, &["--at", "0x06000001"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("INVALID_INPUT"));
}
