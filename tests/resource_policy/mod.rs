//! Ordinary CLI scaling is distinct from explicit limits and MCP service admission.
use super::*;

#[test]
fn cli_has_no_work_instruction_or_output_quota_and_keeps_atomic_file_parity() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = TempDir::new().unwrap();
    let destination = directory.path().join("output.jsonl");
    let cases: &[(&[&str], &str, &[&str])] = &[
        #[cfg(feature = "backend-dotnet")]
        (
            &["ilspy", "get-method"],
            "tests/corpus/dotnet/newtonsoft-json-13.0.3/Newtonsoft.Json.dll",
            &["--at", "0x060000ee", "--raw"],
        ),
        #[cfg(feature = "backend-wasm")]
        (&["wasm", "functions"], "tests/corpus/wasm/basic.wasm", &[]),
    ];
    for (command, input, args) in cases {
        let input = root.join(input);
        let stdout = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(*command)
            .arg(&input)
            .args(*args)
            .args(["--perf"])
            .output()
            .unwrap();
        success(&stdout);
        let reports: Vec<Value> = String::from_utf8(stdout.stderr)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let perf = reports
            .iter()
            .find(|row| row["scope"] == "analysis")
            .unwrap();
        assert!(perf["limits"]["output_bytes"].is_null(), "{command:?}");
        assert!(perf["limits"]["work"].is_null(), "{command:?}");
        assert!(perf["resource_limits"]["work"].is_null(), "{command:?}");
        assert!(
            perf["resource_limits"]["instructions"].is_null(),
            "{command:?}"
        );
        assert!(
            perf["resource_limits"]["output_bytes"].is_null(),
            "{command:?}"
        );
        assert_eq!(
            perf["counters"]["output_bytes"],
            stdout.stdout.len(),
            "{command:?}"
        );
        fs::write(&destination, b"previous output").unwrap();
        let file = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(*command)
            .arg(&input)
            .args(*args)
            .arg("-o")
            .arg(&destination)
            .output()
            .unwrap();
        success(&file);
        assert!(file.stdout.is_empty(), "{command:?}");
        assert_eq!(
            fs::read(&destination).unwrap(),
            stdout.stdout,
            "{command:?}"
        );
    }
}

#[cfg(feature = "backend-dotnet")]
#[test]
fn input_scaled_dotnet_xref_finishes_a_real_module_above_the_old_work_quota() {
    let input = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/dotnet/newtonsoft-json-13.0.3/Newtonsoft.Json.dll");
    let directory = TempDir::new().unwrap();
    let destination = directory.path().join("xref.jsonl");
    fs::write(&destination, b"previous result").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["ilspy", "xref"])
        .arg(&input)
        .args([
            "--kind",
            "string",
            "-m",
            "__rxd_nonexistent_budget_probe__",
            "--perf",
            "-o",
        ])
        .arg(&destination)
        .output()
        .unwrap();
    success(&output);
    assert!(output.stdout.is_empty());
    assert!(fs::read_to_string(&destination).unwrap().is_empty());
    let profiles: Vec<Value> = String::from_utf8(output.stderr)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let analysis = profiles
        .iter()
        .find(|row| row["scope"] == "analysis")
        .unwrap();
    assert_eq!(analysis["status"], "ok");
    let budget =
        rxd_core::Budget::for_input_bytes(fs::metadata(&input).unwrap().len() as usize).unwrap();
    assert_eq!(budget.work, None);
    assert!(analysis["resource_limits"].get("work").is_none());
    assert_eq!(
        analysis["resource_limits"]["metadata_bytes"],
        budget.metadata_bytes
    );
    assert_eq!(budget.output_bytes, None);
    assert!(analysis["resource_limits"].get("output_bytes").is_none());
}
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
