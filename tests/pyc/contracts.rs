//! Coordinate/content selectors through the real CLI, over pinned CPython output.
use super::*;

fn path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/pyc/3.12.14/graph.timestamp.pyc")
}
fn query(command: &str, path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", command])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}
fn rows(output: &Output) -> Vec<Value> {
    let result = jsonl(output);
    assert_eq!(result.last().unwrap()["complete"], true);
    result
}
fn hits(rows: &[Value]) -> Vec<Value> {
    rows.iter()
        .filter(|r| r["record"] == "reference")
        .cloned()
        .collect()
}

#[test]
fn pyc_contract_name_content_search_keeps_payloads_order_case_and_scope() {
    let path = path();
    let filtered = rows(&query("xref", &path, &["--kind", "name", "-f", "VAL"]));
    assert_eq!(filtered[0]["query"]["case"], "insensitive");
    let found = hits(&filtered);
    assert!(!found.is_empty());
    let names: std::collections::BTreeSet<_> = found
        .iter()
        .map(|r| r["detail"]["target_name"].as_str().unwrap())
        .collect();
    for name in names {
        assert!(name.to_lowercase().contains("val"));
        let exact = hits(&rows(&query(
            "xref",
            &path,
            &["--kind", "name", "--at", name],
        )));
        let subset: Vec<_> = found
            .iter()
            .filter(|r| r["detail"]["target_name"] == name)
            .cloned()
            .collect();
        assert_eq!(subset, exact);
    }
    let strict = rows(&query(
        "xref",
        &path,
        &["--kind", "name", "-f", "VAL", "--case", "sensitive"],
    ));
    assert!(hits(&strict).is_empty());
    let owner = found[0]["context_at"].as_str().unwrap();
    let scoped = hits(&rows(&query(
        "xref",
        &path,
        &["--kind", "name", "-f", "VAL", "--within", owner],
    )));
    assert_eq!(
        scoped,
        found
            .iter()
            .filter(|r| r["context_at"] == owner)
            .cloned()
            .collect::<Vec<_>>()
    );
    failure(query("xref", &path, &["-f", "value"]), 2, "--kind");
    failure(
        query("xref", &path, &["--kind", "constant", "-f", "value"]),
        2,
        "INVALID_QUERY",
    );
    failure(
        query("xref", &path, &["--at", "const:9999:0"]),
        2,
        "NOT_FOUND",
    );
    failure(
        query("xref", &path, &["--at", "const:9999:bad"]),
        2,
        "INVALID_QUERY",
    );
}

#[test]
fn pyc_contract_listing_and_constant_streams_bind_to_the_same_input() {
    let path = path();
    let dir = tempfile::tempdir().unwrap();
    let code_stream = dir.path().join("codes.jsonl");
    let selected = query("codes", &path, &["--name", "same"]);
    let code_rows = rows(&selected);
    let at = code_rows[1]["at"].as_str().unwrap();
    fs::write(&code_stream, &selected.stdout).unwrap();
    let from = query("codes", &path, &["--from", code_stream.to_str().unwrap()]);
    assert_eq!(from.stdout, query("codes", &path, &["--at", at]).stdout);
    let intersected = rows(&query(
        "codes",
        &path,
        &["--at", at, "--name", "same", "-f", "SAM"],
    ));
    assert_eq!(intersected[1], code_rows[1]);
    assert_eq!(intersected.len(), 3);
    failure(
        query("codes", &path, &["--at", "code:9999", "--name", "missing"]),
        2,
        "NOT_FOUND",
    );
    failure(
        query("codes", &path, &["--index", "0"]),
        2,
        "unexpected argument",
    );

    let constant_stream = dir.path().join("strings.jsonl");
    let constants = query("strings", &path, &["-f", "shared-container-marker"]);
    let literal_rows = rows(&constants);
    let literal = literal_rows[1]["at"].as_str().unwrap();
    fs::write(&constant_stream, &constants.stdout).unwrap();
    let from = query(
        "xref",
        &path,
        &["--from", constant_stream.to_str().unwrap()],
    );
    let references = rows(&from);
    assert_eq!(references[0]["query"]["kind"], "constant");
    assert_eq!(from.stdout, query("xref", &path, &["--at", literal]).stdout);
    assert!(
        hits(&references)
            .iter()
            .all(|r| r["detail"]["target_name"].is_null())
    );
    failure(
        query(
            "xref",
            &path,
            &[
                "--kind",
                "name",
                "--from",
                constant_stream.to_str().unwrap(),
            ],
        ),
        2,
        "constant",
    );

    // Timestamp-only mutation is still valid, same-sized input with a different hash.
    let mut changed = fs::read(&path).unwrap();
    changed[8] ^= 1;
    let changed_path = dir.path().join("changed.pyc");
    fs::write(&changed_path, changed).unwrap();
    success(&query("codes", &changed_path, &[]));
    let dest = dir.path().join("keep.jsonl");
    for (command, stream) in [
        ("codes", &code_stream),
        ("get-code", &code_stream),
        ("xref", &constant_stream),
    ] {
        fs::write(&dest, b"keep").unwrap();
        failure(
            query(
                command,
                &changed_path,
                &[
                    "--from",
                    stream.to_str().unwrap(),
                    "-o",
                    dest.to_str().unwrap(),
                ],
            ),
            1,
            "SOURCE_MISMATCH",
        );
        assert_eq!(fs::read(&dest).unwrap(), b"keep");
    }
}
