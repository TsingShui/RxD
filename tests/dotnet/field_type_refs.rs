//! Semantic field/type XREF uses the actual CLI navigation seam.
use super::*;

#[test]
fn dotnet_semantic_refs_reject_local_class_valuetype_contradictions_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("contradictory-owner.dll");
    let mut bytes = fs::read(proof_input()).unwrap();
    // Independent ECMA-335 blob: GENERICINST CLASS Holder<int>. Holder is a
    // TypeDef class; changing only CLASS to VALUETYPE contradicts that identity.
    let blob = [5, 0x15, 0x12, 0x0c, 1, 8];
    let positions: Vec<_> = bytes
        .windows(blob.len())
        .enumerate()
        .filter_map(|(index, item)| (item == blob).then_some(index))
        .collect();
    assert_eq!(positions.len(), 1);
    bytes[positions[0] + 2] = 0x11;
    fs::write(&path, bytes).unwrap();
    let destination = directory.path().join("result.jsonl");
    for token in ["0x1b000001", "0x04000006", "0x06000008"] {
        fs::write(&destination, b"preserved").unwrap();
        let output = run(
            "xref",
            &path,
            &["--at", token, "-o", destination.to_str().unwrap()],
        );
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_INPUT"));
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
    // Embedded CLASS Target in the independently assembled AliasType signature
    // must obey the same local kind proof as a declaring-owner TypeSpec.
    let mut bytes = fs::read(proof_input()).unwrap();
    let embedded = [6, 0, 1, 0x12, 8, 0x12, 8];
    let positions: Vec<_> = bytes
        .windows(embedded.len())
        .enumerate()
        .filter_map(|(index, item)| (item == embedded).then_some(index))
        .collect();
    assert!(!positions.is_empty());
    for position in positions {
        bytes[position + 3] = 0x11;
    }
    fs::write(&path, bytes).unwrap();
    let output = run("xref", &path, &["--at", "0x0600000a"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_INPUT"));
}
fn proof_input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/semantic/References.dll")
}

#[test]
fn dotnet_field_type_reference_queries_reject_recursive_types_and_malformed_field_signatures() {
    let path = proof_input();
    let directory = tempfile::tempdir().unwrap();
    for (needle, relative, value, name) in [
        (vec![5, 0x15, 0x12, 0x0c, 1, 8], 3, 6, "cycle.dll"),
        (vec![3, 6, 0x13, 0], 1, 7, "field-signature.dll"),
    ] {
        let mut bytes = fs::read(&path).unwrap();
        let offsets: Vec<_> = bytes
            .windows(needle.len())
            .enumerate()
            .filter_map(|(i, b)| (b == needle).then_some(i))
            .collect();
        assert_eq!(offsets.len(), 1);
        bytes[offsets[0] + relative] = value;
        let broken = directory.path().join(name);
        fs::write(&broken, bytes).unwrap();
        // The broken row itself is always in scope.
        let output = run("xref", &broken, &["--at", "0x04000006"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_INPUT"));
        // Only reached corruption fails: malformed field signatures are irrelevant to type queries.
        let output = run("xref", &broken, &["--at", "0x02000003"]);
        if name == "cycle.dll" {
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
        } else {
            success(&output);
        }
    }
}
