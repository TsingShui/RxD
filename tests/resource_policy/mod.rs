//! Ordinary CLI scaling is distinct from explicit limits and MCP service admission.
use super::*;

#[cfg(feature = "backend-js")]
#[test]
fn review_js_module_metadata_charges_every_repeated_owned_field() {
    // Fixed metadata admission is a public backend API contract, not a CLI quota knob.
    let ids = (0..32)
        .map(|i| format!("'{}{i}'", "c".repeat(256)))
        .collect::<Vec<_>>()
        .join(",");
    let factories = (0..32)
        .map(|i| format!("{i}:(m,e,r)=>{{}}"))
        .collect::<Vec<_>>()
        .join(",");
    let names = (0..32)
        .map(|i| format!("v{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let source = "S".repeat(4096);
    let cases = [
        (
            format!(
                "(self.webpackChunk_app=self.webpackChunk_app||[]).push([[{ids}],{{{factories}}}]);"
            ),
            rxd_js::ModuleListing::Modules,
            "modules",
            33,
        ),
        (
            format!("import {{{names}}} from '{source}';"),
            rxd_js::ModuleListing::Imports,
            "imports",
            32,
        ),
        (
            format!("export {{{names}}} from '{source}';"),
            rxd_js::ModuleListing::Exports,
            "exports",
            32,
        ),
    ];
    for (source, listing, command, count) in cases {
        let (_dir, path) = fixture(&source, "metadata.mjs");
        let mut limits =
            rxd_core::Limits::from(rxd_core::Budget::for_input_bytes(source.len()).unwrap());
        limits.metadata_bytes = 128 * 1024;
        let query = rxd_js::Query::Modules(rxd_js::ModuleQuery {
            listing,
            names: rxd_js::NameQuery::default(),
            within: None,
        });
        let mut perf = rxd_core::perf::Perf::new(false, "analysis", "js", command);
        match rxd_js::run(
            &path,
            rxd_js::SourceMode::Auto,
            rxd_js::Syntax::Auto,
            query,
            limits,
            &mut perf,
        ) {
            Err(error) => assert_eq!(error.code, "RESOURCE_LIMIT"),
            Ok(bytes) => panic!(
                "metadata admission bypass for {command}: {} bytes",
                bytes.len()
            ),
        }
        assert_eq!(jsonl(&invoke(command, &path, &[])).len(), count);
    }
}

#[test]
fn cli_has_no_work_instruction_or_output_quota_and_keeps_atomic_file_parity() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = TempDir::new().unwrap();
    let destination = directory.path().join("output.jsonl");
    let cases: &[(&[&str], &str, &[&str])] = &[
        #[cfg(feature = "backend-js")]
        (&["js", "values"], "tests/fixtures/example.js", &[]),
        #[cfg(feature = "backend-dotnet")]
        (
            &["dotnet", "get-method"],
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
fn pyc_static_and_source_json_have_no_input_scaled_output_quota() {
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
        if bytes.is_empty() {
            continue;
        }
        assert_eq!(bytes.last(), Some(&b'\n'), "{command}");
        let rows: Vec<Value> = std::str::from_utf8(&bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        if rows.len() == 1 && rows[0].get("backend").is_some() {
            assert_eq!(rows[0]["detail"]["complete"], true, "{command}");
        } else {
            assert!(
                rows.iter().all(|row| row["record"] != "summary"),
                "{command}"
            );
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
        assert_eq!(budget.output_bytes, None);
        assert!(
            analysis["resource_limits"].get("output_bytes").is_none(),
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
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["at"], format!("L{name};"));
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
