//! Ordinary CLI scaling is distinct from explicit limits and MCP service admission.
use super::*;

#[cfg(feature = "backend-pyc")]
#[test]
fn input_scaled_pyc_still_refuses_recursive_expansion_atomically() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("deep.marshal");
    let destination = directory.path().join("codes.jsonl");
    // 129 nested one-element small tuples, then None: structurally finite bytes,
    // but beyond the independent marshal recursion guard.
    let mut bytes = b")\x01".repeat(129);
    bytes.push(b'N');
    fs::write(&input, &bytes).unwrap();
    fs::write(&destination, b"keep me").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "codes"])
        .arg(&input)
        .args(["--version", "3.11", "-o"])
        .arg(&destination)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("RESOURCE_LIMIT"));
    assert_eq!(fs::read(&destination).unwrap(), b"keep me");
    assert_eq!(fs::read(&input).unwrap(), bytes);
}
