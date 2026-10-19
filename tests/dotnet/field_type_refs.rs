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
            "refs",
            &path,
            &[
                "--token",
                token,
                "--semantic",
                "-o",
                destination.to_str().unwrap(),
            ],
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
    let output = run("refs", &path, &["--token", "0x0600000a", "--semantic"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_INPUT"));
}

#[test]
fn dotnet_semantic_field_refs_find_constructed_owner_and_route_to_caller_fetches() {
    let path = input();
    let outline = jsonl(&run("outline", &path, &[]));
    let id = outline
        .iter()
        .find(|row| row["record"] == "field" && row["detail"]["token"] == 0x04000003u32)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let raw = jsonl(&run("refs", &path, &["--token", "0x04000003"]));
    assert_eq!(raw.last().unwrap()["records"], 0);
    let rows = jsonl(&run("refs", &path, &["--id", id]));
    assert_eq!(rows.last().unwrap()["records"], 1);
    let hit = &rows[1];
    assert_eq!(hit["detail"]["target_kind"], "field");
    assert_eq!(hit["detail"]["canonical_target_id"], id);
    assert_eq!(hit["detail"]["target_token"], 0x0a000002u32);
    assert_eq!(hit["detail"]["il_offset"], 1);
    assert_eq!(hit["detail"]["name"], "GenericOwnerField");
    assert_eq!(hit["detail"]["resolution_status"], "resolved");
    success(&run(
        "get-method",
        &path,
        &[hit["context_id"].as_str().unwrap(), "--il"],
    ));
    success(&run(
        "get-class",
        &path,
        &[hit["parent_id"].as_str().unwrap(), "--il"],
    ));
    let by_token = jsonl(&run(
        "refs",
        &path,
        &["--token", "0x04000003", "--semantic"],
    ));
    assert_eq!(by_token[1], rows[1]);
}

fn proof_input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/semantic/References.dll")
}

fn proof_sites(path: &Path, tokens: &[u32]) -> Vec<Value> {
    let expected: Value =
        serde_json::from_slice(&fs::read(path.with_file_name("expected.json")).unwrap()).unwrap();
    expected["field_type_sites"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|site| tokens.contains(&(site["target_token"].as_u64().unwrap() as u32)))
        .cloned()
        .collect()
}
fn assert_proof_sites(rows: &[Value], expected: &[Value]) {
    let actual: Vec<_> = rows[1..rows.len()-1].iter().map(|row| serde_json::json!({"method_token":row["detail"]["method_token"],"il_offset":row["detail"]["il_offset"],"opcode":row["detail"]["opcode"],"target_token":row["detail"]["target_token"]})).collect();
    assert_eq!(actual, expected);
    assert_eq!(rows[0]["schema_version"], 2);
    assert_eq!(rows.last().unwrap()["records"], expected.len());
    assert_eq!(rows.last().unwrap()["complete"], true);
}

#[test]
fn dotnet_semantic_type_refs_include_only_explicit_named_and_constructed_operands() {
    let path = proof_input();
    let outline = jsonl(&run("outline", &path, &[]));
    for (token, operands) in [
        (0x02000002u32, vec![0x02000002, 0x01000001]),
        (0x02000003, vec![0x1b000001, 0x1b000002, 0x1b000006]),
        (0x02000004, vec![0x01000005]),
    ] {
        let id = outline
            .iter()
            .find(|row| row["record"] == "type" && row["detail"]["token"] == token)
            .unwrap()["id"]
            .as_str()
            .unwrap();
        let rows = jsonl(&run("refs", &path, &["--id", id]));
        assert_proof_sites(&rows, &proof_sites(&path, &operands));
        for hit in &rows[1..rows.len() - 1] {
            assert_eq!(hit["detail"]["target_kind"], "type");
            assert_eq!(hit["detail"]["canonical_target_id"], id);
            success(&run(
                "get-method",
                &path,
                &[hit["context_id"].as_str().unwrap(), "--il"],
            ));
            success(&run(
                "get-class",
                &path,
                &[hit["parent_id"].as_str().unwrap(), "--il"],
            ));
        }
    }
}

#[test]
fn dotnet_field_refs_keep_signature_owner_instantiations_and_external_scopes_distinct() {
    let path = proof_input();
    for (token, operands, status) in [
        ("0x04000001", vec![0x04000001, 0x0a000012], "resolved"),
        ("0x04000002", vec![0x0a000013], "resolved"),
        ("0x04000006", vec![0x0a000014, 0x0a000015], "resolved"),
        ("0x0a000014", vec![0x0a000014], "resolved"),
        ("0x0a000015", vec![0x0a000015], "resolved"),
        ("0x04000007", vec![0x0a000017], "resolved"),
        ("0x0a000016", vec![0x0a000016], "external"),
        ("0x0a000019", vec![0x0a000019], "external"),
        ("0x0a000018", vec![0x0a000018], "ambiguous"),
        ("0x0a00001a", vec![0x0a00001a], "unresolved"),
        ("0x0a00001b", vec![0x0a00001b], "unresolved"),
        ("0x04000003", vec![], "resolved"),
        ("0x04000005", vec![], "resolved"),
    ] {
        let rows = jsonl(&run("refs", &path, &["--token", token, "--semantic"]));
        assert_proof_sites(&rows, &proof_sites(&path, &operands));
        assert_eq!(rows[0]["detail"]["target"]["resolution_status"], status);
        for row in &rows[1..rows.len() - 1] {
            assert_eq!(row["detail"]["resolution_status"], status);
            assert!(row["detail"]["signature"].is_null());
            assert!(!row["detail"]["type_signature"].is_null());
            if status != "resolved" {
                assert!(row["detail"]["canonical_target_id"].is_null());
            }
        }
    }
}

#[test]
fn dotnet_type_refs_preserve_concrete_shapes_and_keep_unresolved_targets_honest() {
    let path = proof_input();
    for (token, operands, status, canonical) in [
        (
            "0x1b000001",
            vec![0x1b000001, 0x1b000006],
            "resolved",
            Some(0x02000003u32),
        ),
        ("0x1b000002", vec![0x1b000002], "resolved", Some(0x02000003)),
        ("0x1b000003", vec![0x1b000003], "unresolved", None),
        ("0x1b000004", vec![0x1b000004], "unresolved", None),
        ("0x1b000005", vec![0x1b000005], "unresolved", None),
        ("0x01000002", vec![0x01000002], "external", None),
        ("0x01000003", vec![0x01000003], "external", None),
        ("0x01000006", vec![0x01000006], "external", None),
        ("0x01000007", vec![0x01000007], "unresolved", None),
    ] {
        let rows = jsonl(&run("refs", &path, &["--token", token, "--semantic"]));
        assert_proof_sites(&rows, &proof_sites(&path, &operands));
        let target = &rows[0]["detail"]["target"];
        assert_eq!(target["resolution_status"], status);
        assert_eq!(target["canonical_token"], serde_json::json!(canonical));
    }
    for token in [0x1b000001u32, 0x04000001, 0x02000002] {
        let encoded = format!("0x{token:08x}");
        let rows = jsonl(&run("refs", &path, &["--token", &encoded]));
        assert_proof_sites(&rows, &proof_sites(&path, &[token]));
        assert!(rows[0]["detail"]["target"].is_null());
        assert!(
            rows[1..rows.len() - 1]
                .iter()
                .all(|row| row["detail"]["resolution_status"] == "exact")
        );
    }
}

#[test]
fn dotnet_field_type_reference_queries_keep_id_budget_and_atomic_failure_contracts() {
    let path = proof_input();
    let outline = jsonl(&run("outline", &path, &[]));
    let field = outline
        .iter()
        .find(|row| row["record"] == "field" && row["detail"]["token"] == 0x04000001u32)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let ty = outline
        .iter()
        .find(|row| row["record"] == "type" && row["detail"]["token"] == 0x02000003u32)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    for id in [
        field.replace(":field:", ":type:"),
        ty.replace(":type:", ":field:"),
        field.replacen(
            outline[0]["source_id"].as_str().unwrap(),
            &"0".repeat(64),
            1,
        ),
    ] {
        let output = run("refs", &path, &["--id", &id]);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
    success(&run("outline", &path, &["--max-work", "7000"]));
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("refs.jsonl");
    for id in [field, ty] {
        for (limit, value) in [
            ("--max-work", "7000"),
            ("--max-instructions", "2"),
            ("--max-records", "1"),
            ("--max-output-bytes", "128"),
        ] {
            fs::write(&destination, b"preserved").unwrap();
            let output = run(
                "refs",
                &path,
                &[
                    "--id",
                    id,
                    limit,
                    value,
                    "-o",
                    destination.to_str().unwrap(),
                ],
            );
            assert_eq!(
                output.status.code(),
                Some(1),
                "{limit}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains("RESOURCE_LIMIT"));
            assert_eq!(fs::read(&destination).unwrap(), b"preserved");
        }
        let plain = run("refs", &path, &["--id", id]);
        success(&plain);
        let perf = run("refs", &path, &["--id", id, "--perf"]);
        success(&perf);
        assert_eq!(plain.stdout, perf.stdout);
        assert_eq!(String::from_utf8(perf.stderr).unwrap().lines().count(), 2);
        let file = run(
            "refs",
            &path,
            &["--id", id, "-o", destination.to_str().unwrap()],
        );
        success(&file);
        assert!(file.stdout.is_empty());
        assert_eq!(fs::read(&destination).unwrap(), plain.stdout);
    }
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
        for token in ["0x02000003", "0x04000006"] {
            let output = run("refs", &broken, &["--token", token, "--semantic"]);
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_INPUT"));
        }
    }
}
