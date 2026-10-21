//! Complete Agent navigation uses returned IDs across real CLI processes.
use super::*;

fn fixture(directory: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/dotnet")
        .join(directory)
        .join(if directory == "sugar" {
            "Sugar.dll"
        } else {
            "Closure.dll"
        })
}

#[test]
fn dotnet_navigation_rejects_late_fetch_failures_without_partial_output() {
    let directory = tempfile::tempdir().unwrap();
    let path = fixture("closure");
    let copied = directory.path().join("same-content.dll");
    fs::copy(&path, &copied).unwrap();
    let strings = jsonl(&run("strings", &path, &["--filter", "deep λ"]));
    let hit = &strings[1];
    let method_id = hit["context_at"].as_str().unwrap();
    let type_id = hit["parent_at"].as_str().unwrap();
    success(&run("get-method", &copied, &["--at", method_id, "--raw"]));
    let destination = directory.path().join("fetch.json");
    for (command, _id) in [("get-method", method_id), ("get-class", type_id)] {
        fs::write(&destination, b"previous complete result").unwrap();
        // An unusable coordinate is refused, and a refused fetch leaves the previous
        // result in place; budgets are internal floors, not CLI knobs.
        let output = run(
            command,
            &copied,
            &["--at", "0x0ffffff0", "-o", destination.to_str().unwrap()],
        );
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(fs::read(&destination).unwrap(), b"previous complete result");
    }
}
