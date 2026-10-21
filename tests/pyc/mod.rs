//! Python bytecode decompilation through the actual rxd CLI.
use super::*;

mod boundaries;
mod codes;
mod constant_refs;
mod contracts;
mod get_code;
mod refs;
mod strings;
mod versions;

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

fn shared_static_limits() -> rxd_pyc::static_analysis::Limits {
    let budget = rxd_core::Budget::default();
    rxd_pyc::static_analysis::Limits {
        input_bytes: budget.input_bytes,
        entities: usize::MAX,
        work: budget.work,
    }
}

fn codes_library(
    path: &Path,
    version: Option<(u8, u8)>,
    filter: Option<&str>,
    name: Option<&str>,
    index: Option<usize>,
    limits: rxd_pyc::static_analysis::Limits,
) -> (rxd_core::perf::Perf, rxd_core::Result<Vec<usize>>) {
    let mut perf = pyc_perf("codes");
    let result = rxd_pyc::static_analysis::Analysis::load(path, version, limits, &mut perf)
        .and_then(|mut analysis| {
            let filter = filter.map(|needle| rxd_core::filter::Contains::new(needle, false));
            let selected = analysis.select_codes(filter.as_ref(), name, index);
            perf.counters.work = Some(analysis.evidence.work);
            perf.counters.records = selected.as_ref().ok().map(Vec::len);
            selected
        });
    (perf, result)
}

fn strings_library(
    path: &Path,
    version: Option<(u8, u8)>,
    filter: Option<&str>,
    filter_hex: Option<&str>,
    limits: rxd_pyc::static_analysis::Limits,
) -> (
    rxd_core::perf::Perf,
    rxd_core::Result<Vec<rxd_pyc::string_constants::Literal>>,
) {
    let mut perf = pyc_perf("strings");
    let result = rxd_pyc::static_analysis::Analysis::load(path, version, limits, &mut perf)
        .and_then(|mut analysis| {
            let hex = filter_hex.map(|value| {
                value
                    .as_bytes()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| {
                        let nibble = |byte: u8| match byte {
                            b'0'..=b'9' => byte - b'0',
                            b'a'..=b'f' => byte - b'a' + 10,
                            _ => byte - b'A' + 10,
                        };
                        nibble(pair[0]) * 16 + nibble(pair[1])
                    })
                    .collect::<Vec<_>>()
            });
            let query = if let Some(text) = filter {
                rxd_pyc::string_constants::Query::Text {
                    text,
                    case_sensitive: false,
                }
            } else if let Some(bytes) = &hex {
                rxd_pyc::string_constants::Query::Hex(bytes)
            } else {
                rxd_pyc::string_constants::Query::All
            };
            let literals = rxd_pyc::string_constants::enumerate(&mut analysis, &query);
            perf.counters.work = Some(analysis.evidence.work);
            perf.counters.records = literals.as_ref().ok().map(Vec::len);
            literals
        });
    (perf, result)
}

fn xref_library(
    path: &Path,
    version: Option<(u8, u8)>,
    selector: &str,
    within: Option<&str>,
    limits: rxd_pyc::static_analysis::Limits,
    instructions: usize,
) -> (
    rxd_core::perf::Perf,
    rxd_core::Result<rxd_pyc::references::Scan>,
) {
    let mut perf = pyc_perf("xref");
    let result = rxd_pyc::static_analysis::Analysis::load(path, version, limits, &mut perf)
        .and_then(|mut analysis| {
            let selected = within.map(|at| analysis.parse_code_id(at)).transpose()?;
            let constant = rxd_pyc::string_constants::resolve_id(&mut analysis, selector)?;
            let mut scan = rxd_pyc::references::Scan::default();
            let result = rxd_pyc::references::scan(
                &mut analysis,
                &rxd_pyc::references::Query::Constant {
                    id: selector,
                    selected: &constant,
                },
                selected,
                instructions,
                &mut scan,
            );
            perf.counters.work = Some(analysis.evidence.work);
            perf.counters.instructions = Some(scan.instructions);
            perf.counters.records = Some(scan.hits.len());
            result.map(|()| scan)
        });
    (perf, result)
}

fn get_code_library(
    path: &Path,
    version: Option<(u8, u8)>,
    at: &str,
    limits: rxd_pyc::static_analysis::Limits,
    instructions: usize,
) -> (
    rxd_core::perf::Perf,
    rxd_core::Result<rxd_pyc::code_view::Evidence>,
) {
    let mut perf = pyc_perf("get-code");
    let result = rxd_pyc::static_analysis::Analysis::load(path, version, limits, &mut perf)
        .and_then(|mut analysis| {
            let selected = analysis.parse_code_id(at)?;
            let evidence = rxd_pyc::code_view::build(&mut analysis, selected, instructions);
            perf.counters.work = Some(analysis.evidence.work);
            perf.counters.instructions = evidence.as_ref().ok().map(|e| e.instructions.len());
            evidence
        });
    (perf, result)
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
    }
}

fn xref_name_library(
    path: &Path,
    version: Option<(u8, u8)>,
    name: &str,
    within: Option<&str>,
    limits: rxd_pyc::static_analysis::Limits,
    instructions: usize,
) -> (
    rxd_core::perf::Perf,
    rxd_core::Result<rxd_pyc::references::Scan>,
) {
    let mut perf = pyc_perf("xref");
    let result = rxd_pyc::static_analysis::Analysis::load(path, version, limits, &mut perf)
        .and_then(|mut analysis| {
            let selected = within.map(|at| analysis.parse_code_id(at)).transpose()?;
            let mut scan = rxd_pyc::references::Scan::default();
            let result = rxd_pyc::references::scan(
                &mut analysis,
                &rxd_pyc::references::Query::Name(name),
                selected,
                instructions,
                &mut scan,
            );
            perf.counters.work = Some(analysis.evidence.work);
            perf.counters.instructions = Some(scan.instructions);
            perf.counters.records = Some(scan.hits.len());
            result.map(|()| scan)
        });
    (perf, result)
}

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

fn assert_envelope(row: &Value) {
    assert_js_keys(
        row,
        &[
            "record",
            "backend",
            "command",
            "schema_version",
            "at",
            "source_id",
            "parent_at",
            "context_at",
            "start",
            "end",
            "detail",
        ],
    );
    assert_eq!(row["schema_version"], 2);
    assert_eq!(row["backend"], "pyc");
    assert_eq!(row["command"], "decompile");
    assert_eq!(row["record"], "source");
    for key in ["at", "parent_at", "context_at", "start", "end"] {
        assert!(row[key].is_null());
    }
    assert_js_keys(
        &row["detail"],
        &[
            "input_bytes",
            "python_version",
            "representation",
            "complete",
            "source",
        ],
    );
    assert_eq!(row["detail"]["representation"], "decompiled_python");
}

#[test]
fn pyc_decompile_reconstructs_old_and_new_python_without_an_interpreter() {
    let modern = input("hello.3.12.pyc");
    let output = run(&modern, &["--json"]);
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_envelope(&row);
    assert_eq!(row["detail"]["complete"], true);
    assert_eq!(row["detail"]["python_version"], "3.12");
    assert_eq!(
        row["source_id"],
        format!("{:x}", Sha256::digest(fs::read(&modern).unwrap()))
    );
    let source = row["detail"]["source"].as_str().unwrap();
    assert!(source.contains("class Greeter:"));
    assert!(source.contains("def fib(n):"));
    assert_eq!(run(&modern, &[]).stdout, source.as_bytes());

    let old = run(&input("simple_const.2.7.pyc"), &["--json"]);
    success(&old);
    let row: Value = serde_json::from_slice(&old.stdout).unwrap();
    assert_eq!(row["detail"]["python_version"], "2.7");
    assert_eq!(row["detail"]["complete"], true);
    assert!(row["detail"]["source"].as_str().unwrap().contains("a = 42"));
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

#[test]
fn pyc_outputs_reject_resolved_input_aliases_for_every_command() {
    let dir = tempfile::tempdir().unwrap();
    let original = fs::read(input("hello.3.12.pyc")).unwrap();
    let source = dir.path().join("input.pyc");
    fs::write(&source, &original).unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    let codes = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "codes"])
        .arg(&source)
        .output()
        .unwrap();
    success(&codes);
    let id = String::from_utf8(codes.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str::<Value>(s).unwrap())
        .find(|row| row["record"] == "code")
        .unwrap()["at"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut aliases = vec![
        PathBuf::from("input.pyc"),
        PathBuf::from("./input.pyc"),
        PathBuf::from("nested/../input.pyc"),
        source.clone(),
    ];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path(), dir.path().join("linked")).unwrap();
        std::os::unix::fs::symlink(&source, dir.path().join("linked-input.pyc")).unwrap();
        aliases.push(PathBuf::from("linked/input.pyc"));
        aliases.push(PathBuf::from("linked-input.pyc"));
        fs::hard_link(&source, dir.path().join("hard-input.pyc")).unwrap();
        aliases.push(PathBuf::from("hard-input.pyc"));
        // Case-insensitive filesystems may preserve spelling in canonical paths.
        if dir.path().join("INPUT.PYC").exists() {
            aliases.push(PathBuf::from("INPUT.PYC"));
        }
    }
    for (command, selection) in [
        ("codes", vec![]),
        ("strings", vec![]),
        ("xref", vec!["--kind", "name", "--at", "fib"]),
        ("get-code", vec!["--at", id.as_str()]),
        ("decompile", vec!["--json"]),
    ] {
        for alias in &aliases {
            let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
                .current_dir(dir.path())
                .args(["pyc", command])
                .arg(&source)
                .args(&selection)
                .arg("-o")
                .arg(alias)
                .output()
                .unwrap();
            assert!(
                fs::read(&source).unwrap() == original,
                "{command}: {alias:?} changed input"
            );
            failure(output, 2, "INVALID_QUERY");
        }
        #[cfg(unix)]
        {
            let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
                .current_dir(dir.path())
                .args(["pyc", command])
                .arg("linked-input.pyc")
                .args(&selection)
                .arg("-o")
                .arg(&source)
                .output()
                .unwrap();
            failure(output, 2, "INVALID_QUERY");
            assert!(fs::read(&source).unwrap() == original);
        }
        let mut destinations = vec!["new.json", "existing.json", "nested/result.json"];
        #[cfg(unix)]
        destinations.push("linked/result.json");
        for destination in destinations {
            fs::write(dir.path().join("existing.json"), b"keep").unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
                .current_dir(dir.path())
                .args(["pyc", command])
                .arg(&source)
                .args(&selection)
                .arg("-o")
                .arg(destination)
                .output()
                .unwrap();
            success(&output);
            assert!(output.stdout.is_empty());
            assert!(!fs::read(dir.path().join(destination)).unwrap().is_empty());
            assert_eq!(fs::read(&source).unwrap(), original);
        }
    }
}
