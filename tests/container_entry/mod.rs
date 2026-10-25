use super::{failure, jsonl, pack_asar, success, write_asar};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(backend: &str, command: &str, input: &Path, entry: Option<&str>, args: &[&str]) -> Output {
    let mut query = Command::new(env!("CARGO_BIN_EXE_rxd"));
    query.args([backend, command]).arg(input);
    if let Some(entry) = entry {
        query.args(["--entry", entry]);
    }
    query.args(args).output().unwrap()
}

#[cfg(feature = "backend-html")]
#[test]
fn html_asar_entry_queries_match_original_documents_and_preserve_nested_origin() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("app.asar");
    let entry = "pages/主页.html";
    let original = dir.path().join(entry);
    fs::create_dir(original.parent().unwrap()).unwrap();
    let html = "\u{feff}<!doctype html>\r\n<main id='内容'><script src='missing.js'></script><script>const text='你好';\r\n</script><a href='/login'>入口</a></main>";
    fs::write(&original, html).unwrap();
    pack_asar(
        &archive,
        &[(entry, html.as_bytes()), ("pages/missing.js", b"\xff")],
    );
    for (command, args) in [
        ("entries", vec![]),
        ("entries", vec!["--tag", "script"]),
        ("entries", vec!["--attr", "id=内容", "--depth", "0"]),
        ("resources", vec![]),
        ("resources", vec!["--kind", "script", "--include-content"]),
        (
            "resources",
            vec![
                "--base-url",
                "https://example.org/app/index.html",
                "--location",
                "external",
            ],
        ),
    ] {
        let direct = run("html", command, &archive, Some(entry), &args);
        let ordinary = run("html", command, &original, None, &args);
        success(&direct);
        success(&ordinary);
        assert_eq!(direct.stdout, ordinary.stdout, "{command} {args:?}");
    }
    let catalog = jsonl(&run("html", "entries", &archive, Some(entry), &[]));
    let script = catalog
        .iter()
        .find(|row| {
            row["detail"]["tag"] == "script"
                && row["detail"]["content_end"].as_u64() > row["detail"]["content_start"].as_u64()
        })
        .unwrap();
    let at = script["at"].as_str().unwrap();
    let destination = dir.path().join("script.js");
    for extra in [vec![], vec!["--content"]] {
        let mut args = vec!["--at", at, "-o", destination.to_str().unwrap()];
        args.extend(extra);
        let direct = run("html", "extract", &archive, Some(entry), &args);
        success(&direct);
        let artifact = fs::read(&destination).unwrap();
        let mut receipt: Value = serde_json::from_slice(&direct.stdout).unwrap();
        assert_eq!(receipt["detail"]["origin"]["container"], entry);
        assert_eq!(
            receipt["detail"]["container_origin"],
            json!({
                "container":archive, "format":"asar", "entry":entry
            })
        );
        let ordinary = run("html", "extract", &original, None, &args);
        success(&ordinary);
        let expected: Value = serde_json::from_slice(&ordinary.stdout).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), artifact);
        receipt["detail"]
            .as_object_mut()
            .unwrap()
            .remove("container_origin");
        receipt["detail"]["origin"]["container"] = json!(original);
        assert_eq!(receipt, expected);
    }
    // A logical entry name is not a filesystem input: it may also name the output.
    let replacement = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["html", "extract"])
        .arg(&archive)
        .args(["--entry", entry, "--at", at, "--content", "-o", entry])
        .current_dir(dir.path())
        .output()
        .unwrap();
    success(&replacement);
    assert_eq!(
        fs::read(&original).unwrap(),
        "const text='你好';\r\n".as_bytes()
    );
}

#[cfg(feature = "backend-wasm")]
#[test]
fn wasm_asar_entry_queries_preserve_all_module_views_and_function_offsets() {
    let dir = tempfile::tempdir().unwrap();
    let original = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm/fixture.wasm");
    let bytes = fs::read(&original).unwrap();
    let archive = dir.path().join("app.asar");
    let entry = "assets/引擎.bin";
    pack_asar(&archive, &[(entry, &bytes)]);
    for command in [
        "types",
        "imports",
        "exports",
        "functions",
        "tables",
        "memories",
        "globals",
        "data",
        "elements",
        "strings",
    ] {
        let direct = run("wasm", command, &archive, Some(entry), &[]);
        let ordinary = run("wasm", command, &original, None, &[]);
        success(&direct);
        success(&ordinary);
        assert_eq!(direct.stdout, ordinary.stdout, "{command}");
    }
    let functions = jsonl(&run("wasm", "functions", &archive, Some(entry), &[]));
    let at = functions[0]["at"].as_str().unwrap();
    let direct = run("wasm", "xref", &archive, Some(entry), &["--at", at]);
    let ordinary = run("wasm", "xref", &original, None, &["--at", at]);
    success(&direct);
    success(&ordinary);
    assert_eq!(direct.stdout, ordinary.stdout);
    for raw in [false, true] {
        let mut args = vec!["--at", at];
        if raw {
            args.push("--raw");
        }
        let direct = run("wasm", "get-function", &archive, Some(entry), &args);
        let ordinary = run("wasm", "get-function", &original, None, &args);
        success(&direct);
        success(&ordinary);
        let mut view: Value = serde_json::from_slice(&direct.stdout).unwrap();
        assert_eq!(
            view["detail"]
                .as_object_mut()
                .unwrap()
                .remove("origin")
                .unwrap(),
            json!({
                "container":archive, "format":"asar", "entry":entry
            })
        );
        assert_eq!(
            view,
            serde_json::from_slice::<Value>(&ordinary.stdout).unwrap()
        );
        if raw {
            let start = view["start"].as_u64().unwrap() as usize;
            let end = view["end"].as_u64().unwrap() as usize;
            let hex = bytes[start..end]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(view["detail"]["bytecode_hex"], hex);
        }
    }
    let perf = run("wasm", "functions", &archive, Some(entry), &["--perf"]);
    success(&perf);
    let record = String::from_utf8(perf.stderr)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|row| row["scope"] == "analysis")
        .unwrap();
    assert_eq!(record["counters"]["input_bytes"], bytes.len());
    assert_eq!(record["resource_limits"]["input_bytes"], bytes.len());
}

#[test]
fn container_asar_entry_failures_and_unpacked_reads_preserve_original_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("app.asar");
    let unpacked = dir.path().join("app.asar.unpacked");
    fs::create_dir(&unpacked).unwrap();
    let cases = [
        #[cfg(feature = "backend-html")]
        (
            "html",
            "entries",
            "view.html",
            "<main>你好</main>".repeat(100).into_bytes(),
            vec![0xff],
        ),
        #[cfg(feature = "backend-wasm")]
        (
            "wasm",
            "functions",
            "module.wasm",
            fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm/fixture.wasm"),
            )
            .unwrap(),
            b"not wasm".to_vec(),
        ),
    ];
    let output = dir.path().join("keep");
    for (backend, command, entry, bytes, invalid) in cases {
        write_asar(
            &archive,
            &json!({"files":{
                (entry):{"size":bytes.len(),"offset":"0"},
                "decoy":{"size":u64::MAX,"offset":u64::MAX.to_string()},
                "link":{"link":entry}
            }}),
            &bytes,
        );
        success(&run(backend, command, &archive, Some(entry), &[]));
        fs::write(&output, "keep").unwrap();
        for name in ["missing", "decoy", "link", "../escape", "/absolute", ""] {
            failure(
                run(
                    backend,
                    command,
                    &archive,
                    Some(name),
                    &["-o", output.to_str().unwrap()],
                ),
                1,
                "INVALID_INPUT",
            );
            assert_eq!(fs::read(&output).unwrap(), b"keep");
        }
        let carrier = fs::read(&archive).unwrap();
        failure(
            run(
                backend,
                command,
                &archive,
                Some(entry),
                &["-o", archive.to_str().unwrap()],
            ),
            2,
            "output must not replace",
        );
        assert_eq!(fs::read(&archive).unwrap(), carrier);
        pack_asar(&archive, &[(entry, &invalid)]);
        failure(
            run(
                backend,
                command,
                &archive,
                Some(entry),
                &["-o", output.to_str().unwrap()],
            ),
            1,
            "INVALID_INPUT",
        );
        assert_eq!(fs::read(&output).unwrap(), b"keep");
        write_asar(
            &archive,
            &json!({"files":{(entry):{"size":bytes.len(),"unpacked":true}}}),
            b"",
        );
        let external = unpacked.join(entry);
        fs::write(&external, &bytes).unwrap();
        let direct = run(backend, command, &archive, Some(entry), &[]);
        let ordinary = run(backend, command, &external, None, &[]);
        success(&direct);
        success(&ordinary);
        assert_eq!(direct.stdout, ordinary.stdout);
        failure(
            run(
                backend,
                command,
                &archive,
                Some(entry),
                &["-o", external.to_str().unwrap()],
            ),
            2,
            "output must not replace",
        );
        assert_eq!(fs::read(&external).unwrap(), bytes);
        fs::write(&external, "short").unwrap();
        failure(
            run(
                backend,
                command,
                &archive,
                Some(entry),
                &["-o", output.to_str().unwrap()],
            ),
            1,
            "size differs",
        );
        fs::remove_file(&external).unwrap();
        failure(
            run(backend, command, &archive, Some(entry), &[]),
            1,
            "IO_ERROR",
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&output, &external).unwrap();
            failure(
                run(backend, command, &archive, Some(entry), &[]),
                1,
                "IO_ERROR",
            );
            fs::remove_file(&external).unwrap();
        }
    }
}
