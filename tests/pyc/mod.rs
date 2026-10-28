//! Python bytecode decompilation through the actual rxd CLI.
use super::*;

mod boundaries;
mod codes;
mod contracts;
mod refs;
mod strings;

fn input(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pyc")
        .join(name)
}

fn run(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "decompile"])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

/// Library-level budget harness: the CLI exposes no ceiling flags, so tests that assert
/// exact accounting drive `rxd_pyc` directly with the same shared projections.
fn pyc_perf(command: &'static str) -> rxd_core::perf::Perf {
    rxd_core::perf::Perf::new(true, "analysis", "pyc", command)
}

fn perf_value(perf: &rxd_core::perf::Perf) -> Value {
    serde_json::to_value(perf).unwrap()
}

fn decompile_library(
    path: &Path,
    version: Option<(u8, u8)>,
    limits: rxd_pyc::Limits,
) -> (
    rxd_core::perf::Perf,
    rxd_core::Result<rxd_pyc::Decompilation>,
) {
    let mut perf = pyc_perf("decompile");
    let result = rxd_pyc::run(path, version, limits, &mut perf);
    (perf, result)
}

fn shared_decompile_limits() -> rxd_pyc::Limits {
    let budget = rxd_core::Budget::default();
    rxd_pyc::Limits {
        input_bytes: budget.input_bytes,
        work: budget.work,
        ast_depth: rxd_core::Limits::default().ast_depth,
        output_bytes: budget.output_bytes,
        marshal: rxd_pyc::marshal_limits(budget),
    }
}

/// Exercise the complete CLI JSON/JSONL through the public bounded writer. There is
/// no CLI output-budget flag and no independent record-count limit to dial down.
/// Assert every former `--max-*` spelling is refused as an unknown argument.
fn assert_no_budget_flags(path: &Path, command: &str) {
    for flag in [
        "--max-input-bytes",
        "--max-output-bytes",
        "--max-work",
        "--max-ast-depth",
        "--max-entities",
        "--max-records",
        "--max-instructions",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["pyc", command])
            .arg(path)
            .args([flag, "1"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{command} {flag}");
        assert!(output.stdout.is_empty(), "{command} {flag}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unexpected argument"),
            "{command} {flag}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn pyc_raw_marshal_requires_an_explicit_version_and_preserves_atomic_output() {
    let bytes = fs::read(input("hello.3.12.pyc")).unwrap();
    let (dir, raw) = fixture(&bytes[16..], "raw.marshal");
    let out = dir.path().join("source.py");
    let rendered = run(&raw, &["--version", "3.12", "-o", out.to_str().unwrap()]);
    success(&rendered);
    assert!(rendered.stdout.is_empty());
    assert_eq!(
        fs::read(&out).unwrap(),
        run(&input("hello.3.12.pyc"), &[]).stdout
    );

    fs::write(&out, b"keep").unwrap();
    failure(
        run(&raw, &["-o", out.to_str().unwrap()]),
        1,
        "INVALID_INPUT",
    );
    assert_eq!(fs::read(&out).unwrap(), b"keep");
    failure(
        run(&raw, &["--version", "3.12", "-o", raw.to_str().unwrap()]),
        2,
        "INVALID_QUERY",
    );
    assert_eq!(fs::read(&raw).unwrap(), bytes[16..]);
}

#[test]
fn pyc_rejects_malformed_and_budget_exceeding_inputs_without_output() {
    let (dir, invalid) = fixture(b"not a pyc", "invalid.pyc");
    failure(run(&invalid, &[]), 1, "INVALID_INPUT");

    let mut huge_tuple = vec![b'('];
    huge_tuple.extend_from_slice(&u32::MAX.to_le_bytes());
    let bomb = dir.path().join("bomb.marshal");
    fs::write(&bomb, huge_tuple).unwrap();
    failure(run(&bomb, &["--version", "3.12"]), 1, "INVALID_INPUT");

    // Ceilings live in the shared budget table; no command exposes a `--max-*` flag.
    assert_no_budget_flags(&input("hello.3.12.pyc"), "decompile");
}
