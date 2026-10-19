//! Python bytecode decompilation through the actual rxd CLI.
use super::*;

mod boundaries;
mod constant_refs;
mod get_code;
mod outline;
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

fn assert_envelope(row: &Value) {
    assert_js_keys(
        row,
        &[
            "record",
            "backend",
            "command",
            "schema_version",
            "id",
            "source_id",
            "parent_id",
            "context_id",
            "start",
            "end",
            "detail",
        ],
    );
    assert_eq!(row["schema_version"], 2);
    assert_eq!(row["backend"], "pyc");
    assert_eq!(row["command"], "decompile");
    assert_eq!(row["record"], "source");
    for key in ["id", "parent_id", "context_id", "start", "end"] {
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

    failure(
        run(&input("hello.3.12.pyc"), &["--max-input-bytes", "16"]),
        1,
        "RESOURCE_LIMIT",
    );
    failure(
        run(
            &input("hello.3.12.pyc"),
            &["--json", "--max-output-bytes", "32"],
        ),
        1,
        "RESOURCE_LIMIT",
    );
}

#[test]
fn pyc_outputs_reject_resolved_input_aliases_for_every_command() {
    let dir = tempfile::tempdir().unwrap();
    let original = fs::read(input("hello.3.12.pyc")).unwrap();
    let source = dir.path().join("input.pyc");
    fs::write(&source, &original).unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    let outline = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "outline"])
        .arg(&source)
        .output()
        .unwrap();
    success(&outline);
    let id = String::from_utf8(outline.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str::<Value>(s).unwrap())
        .find(|row| row["record"] == "code")
        .unwrap()["id"]
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
        ("outline", vec![]),
        ("strings", vec![]),
        ("xref", vec!["--name", "fib"]),
        ("get-code", vec![id.as_str()]),
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
