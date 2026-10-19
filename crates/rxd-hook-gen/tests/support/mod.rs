//! Shared offline checks for provider module E2E. The checked source is never executed.
#![allow(dead_code)]
use std::{fs, path::Path, process::Command};

fn validator(mode: &str, path: &Path) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devtools/hook-gen/validate.mjs");
    let output = Command::new("node")
        .arg(&script)
        .arg(mode)
        .arg(path)
        .output()
        .expect("Node is required for offline validation; see devtools/hook-gen/README.md");
    assert!(
        output.status.success(),
        "offline {mode} validation failed; run explicit npm setup from devtools/hook-gen/README.md first\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Pass concatenated snippets as one string to check composition in a single scope.
pub fn validate_js(source: &str) {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("artifact.js");
    fs::write(&path, source).unwrap();
    validator("js", &path);
}

/// Strict, noEmit, actual pinned Gum/Java bridge declarations; never skipLibCheck.
pub fn validate_ts(source: &str) {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("artifact.ts");
    fs::write(&path, source).unwrap();
    validator("ts", &path);
}

/// Validate an actual published extension, including local JS files and pinned MV3 types.
pub fn validate_extension(directory: &Path) {
    validator("extension", directory);
}
