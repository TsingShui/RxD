//! Real CLI checks for the Dart AOT backend that need no production snapshot.
//!
//! A 14 MB `libapp.so` is too large to pin in this repository, so the snapshot
//! dependent paths are exercised against the sample described in the commit that
//! added them; these cases pin the admission contract instead.
use super::*;

fn run(command: &str, path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dae", command])
        .arg(path)
        .env_remove("RXD_CACHE_DIR")
        .output()
        .unwrap()
}

#[test]
fn dae_rejects_input_that_is_not_an_elf_arm64_snapshot() {
    let (_dir, path) = fixture(b"not a snapshot", "fake.so");
    for command in ["info", "functions", "strings", "decompile"] {
        let out = run(command, &path);
        assert!(!out.status.success(), "{command} accepted a non-ELF input");
        assert!(out.stdout.is_empty(), "{command} wrote partial stdout");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("UNSUPPORTED_PLATFORM"),
            "{command}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn dae_xref_requires_exactly_one_selector() {
    let (_dir, path) = fixture(b"x", "fake.so");
    let neither = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dae", "xref"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!neither.status.success(), "xref accepted no selector");
    let both = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dae", "xref"])
        .arg(&path)
        .args(["--at", "str:0x10", "-m", "text"])
        .output()
        .unwrap();
    assert!(!both.status.success(), "xref accepted two selectors");
}

#[test]
fn dae_rejects_a_malformed_string_coordinate() {
    // Admission runs before coordinate parsing, so this pins the failure path
    // without a production snapshot; a valid snapshot is needed to see the
    // coordinate diagnostic itself.
    let (_dir, path) = fixture(b"x", "fake.so");
    let out = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dae", "xref"])
        .arg(&path)
        .args(["--at", "str:not-hex"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "partial stdout");
}
