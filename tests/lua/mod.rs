//! Real CLI checks for Lua bytecode navigation and reconstruction.
use super::*;

fn input(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/lua")
        .join(name)
}

fn run(command: &str, path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["unluac-rs", command])
        .arg(path)
        .args(args)
        .env_remove("RXD_CACHE_DIR")
        .output()
        .unwrap()
}

#[test]
fn lua_xref_distinguishes_absent_and_ambiguous_matches() {
    let absent = run(
        "xref",
        &input("sample-5.4.luac"),
        &["-m", "no-such-constant"],
    );
    assert!(!absent.status.success());
    assert!(
        String::from_utf8_lossy(&absent.stderr).contains("NOT_FOUND"),
        "{}",
        String::from_utf8_lossy(&absent.stderr)
    );
    let ambiguous = run("xref", &input("sample-5.4.luac"), &["-m", ""]);
    assert!(!ambiguous.status.success());
    assert!(
        String::from_utf8_lossy(&ambiguous.stderr).contains("AMBIGUOUS"),
        "{}",
        String::from_utf8_lossy(&ambiguous.stderr)
    );
}

#[test]
fn lua_rejects_input_that_is_not_a_chunk_without_partial_output() {
    let (_dir, path) = fixture(b"not a lua chunk", "fake.luac");
    let out = run("codes", &path, &[]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "partial stdout");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("INVALID_INPUT"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn lua_unknown_coordinate_is_a_query_error() {
    let out = run(
        "instructions",
        &input("sample-5.4.luac"),
        &["--at", "proto:99"],
    );
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "partial stdout");
}
