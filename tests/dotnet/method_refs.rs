//! Method XREF closes outline → refs → caller method/class navigation.
use super::*;

#[test]
fn dotnet_semantic_signature_expansion_has_no_work_quota_and_preserves_atomic_output() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/dotnet/semantic/expanded/References.dll");
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("refs.jsonl");
    fs::write(&destination, b"preserved").unwrap();
    // The independent assembler substitutes a 1,000-parameter function pointer into
    // 101 !0 positions. Repeated analysis work is not retained memory or a quota.
    let output = run("xref", &path, &["--at", "0x0a000007"]);
    success(&output);
    let written = run(
        "xref",
        &path,
        &["--at", "0x0a000007", "-o", destination.to_str().unwrap()],
    );
    success(&written);
    assert!(written.stdout.is_empty());
    assert_eq!(fs::read(destination).unwrap(), output.stdout);
}
fn semantic_input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/semantic/References.dll")
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
    assert_eq!(instantiation.len(), 2);
    assert_eq!(instantiation[0]["detail"]["target_token"], 0x2b000001u32);
    assert_eq!(instantiation[1]["detail"]["target_token"], 0x2b000003u32);
    let other = jsonl(&run("xref", &path, &["--at", "0x2b000002"]));
    assert_eq!(other.len(), 1);
    let generic = jsonl(&run("xref", &path, &["--at", "0x06000004"]));
    assert_eq!(generic.len(), 3);
    let owners = jsonl(&run("xref", &path, &["--at", "0x06000008"]));
    assert_eq!(owners.len(), 2);
    let owner_int = jsonl(&run("xref", &path, &["--at", "0x0a000007"]));
    assert_eq!(owner_int.len(), 1);
    assert_eq!(
        owner_int[0]["detail"]["owner_arguments"],
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
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["detail"]["resolution_status"], status);
        // `target` is the coordinate: the canonical token when the operand resolves to one,
        // otherwise the operand's own token. `target_token` stays the operand's token.
        let operand = rows[0]["detail"]["target_token"].as_u64().unwrap() as u32;
        assert_eq!(
            rows[0]["detail"]["target"],
            format!("0x{:08x}", canonical.unwrap_or(operand))
        );
    }
    let ambiguous_definition = jsonl(&run("xref", &path, &["--at", "0x06000005"]));
    assert_eq!(ambiguous_definition.len(), 0);
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
    assert_eq!(rows.len(), 4);
}

#[test]
fn dotnet_method_refs_compare_scoped_signature_types_instead_of_display_names() {
    let rows = jsonl(&run("xref", &semantic_input(), &["--at", "0x0600000a"]));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["detail"]["target_token"], 0x0a00000cu32);
    assert_eq!(rows[1]["detail"]["target_token"], 0x0a00000du32);
    let external = jsonl(&run("xref", &semantic_input(), &["--at", "0x0a00000e"]));
    assert_eq!(external.len(), 1);
    assert_eq!(external[0]["detail"]["resolution_status"], "unresolved");
    assert_eq!(
        external[0]["detail"]["target"],
        format!(
            "0x{:08x}",
            external[0]["detail"]["target_token"].as_u64().unwrap()
        )
    );
}

#[test]
fn dotnet_method_refs_preserve_constructed_types_embedded_in_signatures() {
    let rows = jsonl(&run("xref", &semantic_input(), &["--at", "0x0600000b"]));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["detail"]["target_token"], 0x0a00000fu32);
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
