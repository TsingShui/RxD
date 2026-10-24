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
    let filtered = rows(&query("xref", &path, &["--kind", "name", "-m", "VAL"]));
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
        &["--kind", "name", "-m", "VAL", "--case", "sensitive"],
    ));
    assert!(hits(&strict).is_empty());
    let owner = found[0]["context_at"].as_str().unwrap();
    let scoped = hits(&rows(&query(
        "xref",
        &path,
        &["--kind", "name", "-m", "VAL", "--within", owner],
    )));
    assert_eq!(
        scoped,
        found
            .iter()
            .filter(|r| r["context_at"] == owner)
            .cloned()
            .collect::<Vec<_>>()
    );
    failure(query("xref", &path, &["-m", "value"]), 2, "--kind");
    failure(
        query("xref", &path, &["--kind", "constant", "-m", "value"]),
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
fn pyc_contract_listing_and_constant_coordinates() {
    let path = path();
    let selected = query("codes", &path, &["--name", "same"]);
    let code_rows = rows(&selected);
    let at = code_rows[1]["at"].as_str().unwrap();
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

    let constants = query("strings", &path, &["-f", "shared-container-marker"]);
    let literal_rows = rows(&constants);
    let literal = literal_rows[1]["at"].as_str().unwrap();
    let references = rows(&query("xref", &path, &["--at", literal]));
    assert_eq!(references[0]["query"]["kind"], "constant");
    assert!(
        hits(&references)
            .iter()
            .all(|r| r["detail"]["target_name"].is_null())
    );
}
