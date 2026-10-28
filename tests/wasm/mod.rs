//! Static WebAssembly CLI contracts using a deterministic hand-written fixture.
use super::*;

fn input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm/fixture.wasm")
}

fn run(command: &str, path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["wasm", command])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn wasm_rejects_non_wasm_input_without_partial_stdout() {
    let (_dir, path) = fixture(b"not a wasm module".as_slice(), "bad.wasm");
    let out = run("types", &path, &[]);
    failure(out, 1, "INVALID_INPUT");
}

#[test]
fn wasm_output_file_matches_stdout_bytes() {
    let path = input();
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("out.jsonl");
    let stdout = run("functions", &path, &[]);
    success(&stdout);
    let to_file = run("functions", &path, &["-o", out_path.to_str().unwrap()]);
    success(&to_file);
    assert!(to_file.stdout.is_empty(), "-o must not also write stdout");
    let file_bytes = fs::read(&out_path).unwrap();
    assert_eq!(file_bytes, stdout.stdout);
}
