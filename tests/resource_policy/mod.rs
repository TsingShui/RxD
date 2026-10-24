//! Ordinary CLI scaling is distinct from explicit limits and MCP service admission.
use super::*;

#[cfg(feature = "backend-dotnet")]
#[test]
fn input_scaled_dotnet_xref_finishes_a_real_module_above_the_old_work_quota() {
    let input = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/dotnet/newtonsoft-json-13.0.3/Newtonsoft.Json.dll");
    let directory = TempDir::new().unwrap();
    let destination = directory.path().join("xref.jsonl");
    fs::write(&destination, b"previous result").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["dotnet", "xref"])
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
    let rows: Vec<Value> = fs::read_to_string(&destination)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0]["source_id"],
        "8caa89ad89b5efb9405edd796110b18c63e9545e3dfd9c2719ba6de1be484786"
    );
    assert_eq!(rows[1]["records"], 0);
    assert_eq!(rows[1]["complete"], true);
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
    assert!(budget.work > 1_000_000);
    assert_eq!(analysis["resource_limits"]["work"], budget.work);
    assert_eq!(
        analysis["resource_limits"]["metadata_bytes"],
        budget.metadata_bytes
    );
    assert_eq!(
        analysis["resource_limits"]["output_bytes"],
        budget.output_bytes
    );
}

#[cfg(feature = "backend-pyc")]
#[test]
fn input_scaled_pyc_output_policy_covers_static_and_source_json() {
    let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pyc/hello.3.12.pyc");
    let original = fs::read(&input).unwrap();
    let budget = rxd_core::Budget::for_input_bytes(original.len()).unwrap();
    let directory = TempDir::new().unwrap();
    let destination = directory.path().join("result.jsonl");
    let commands: &[(&str, &[&str])] = &[
        ("codes", &[]),
        ("strings", &[]),
        ("xref", &["--kind", "name", "-m", "print"]),
        ("get-code", &["--at", "code:0"]),
        ("decompile", &["--json"]),
    ];
    for (command, args) in commands {
        fs::write(&destination, b"previous result").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["pyc", command])
            .arg(&input)
            .args(*args)
            .args(["--perf", "-o"])
            .arg(&destination)
            .output()
            .unwrap();
        success(&output);
        assert!(output.stdout.is_empty(), "{command}");
        let bytes = fs::read(&destination).unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'), "{command}");
        let rows: Vec<Value> = std::str::from_utf8(&bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let last = rows.last().unwrap();
        if last["record"] == "summary" {
            assert_eq!(last["complete"], true, "{command}");
            assert_eq!(last["records"], rows.len() - 2, "{command}");
        } else {
            assert_eq!(rows.len(), 1, "{command}");
            assert_eq!(last["detail"]["complete"], true, "{command}");
        }
        let reports: Vec<Value> = String::from_utf8(output.stderr)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let analysis = reports
            .iter()
            .find(|row| row["scope"] == "analysis")
            .unwrap();
        assert_eq!(analysis["status"], "ok", "{command}");
        assert_eq!(
            analysis["resource_limits"]["output_bytes"], budget.output_bytes,
            "{command}"
        );
        assert_eq!(
            analysis["counters"]["output_bytes"],
            bytes.len(),
            "{command}"
        );
    }
    assert_eq!(fs::read(&input).unwrap(), original);
}

#[cfg(feature = "backend-jvm")]
#[test]
fn input_scaled_jvm_keeps_format_valid_metadata_above_four_kib() {
    // A complete Java 8 class, no methods/code. CONSTANT_Utf8 has a u16 length;
    // the former 4096-unit metadata policy is not part of that format.
    let name = "A".repeat(5000);
    let mut bytes = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52, 0, 5];
    for (text, index) in [(name.as_str(), 1u16), ("java/lang/Object", 3u16)] {
        bytes.push(1);
        bytes.extend_from_slice(&(text.len() as u16).to_be_bytes());
        bytes.extend_from_slice(text.as_bytes());
        bytes.push(7);
        bytes.extend_from_slice(&index.to_be_bytes());
    }
    for value in [0x21u16, 2, 4, 0, 0, 0, 0] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("LongName.class");
    fs::write(&input, &bytes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["jar", "classes"])
        .arg(&input)
        .output()
        .unwrap();
    success(&output);
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["source_id"], rxd_core::identity::input_id(&bytes));
    assert_eq!(rows[1]["at"], format!("L{name};"));
    assert_eq!(rows[2]["complete"], true);
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
