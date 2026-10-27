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
    jsonl(output)
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
