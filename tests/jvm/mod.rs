use std::{
    fs,
    io::{Cursor, Write},
    process::{Command, Output},
};

use zip::{CompressionMethod, ZipWriter, write::FileOptions};

mod interfaces;
mod multi_release;

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        zip.start_file(
            *name,
            FileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn run(input: &std::path::Path, operation: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["jar", operation])
        .arg(input)
        .args(args)
        .output()
        .unwrap()
}

fn text(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn aar_navigates_nested_classes_and_disambiguates_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("library.aar");
    let jar = fs::read("tests/corpus/jvm/gson-2.11.0.jar").unwrap();
    let nested = archive(&[("dependency.jar", &jar)]);
    fs::write(
        &input,
        archive(&[("classes.jar", &jar), ("libs/bundle.zip", &nested)]),
    )
    .unwrap();
    let classes = text(run(
        &input,
        "classes",
        &["--class", "com.google.gson.GsonBuilder"],
    ));
    assert_eq!(classes.lines().count(), 3, "{classes}");
    assert!(classes.contains("classes.jar!Lcom/google/gson/GsonBuilder;"));
    assert!(classes.contains("libs/bundle.zip!dependency.jar!Lcom/google/gson/GsonBuilder;"));
    assert_eq!(
        classes,
        text(run(
            &input,
            "classes",
            &["--class", "com.google.gson.GsonBuilder"]
        ))
    );
    let entries = text(run(&input, "entries", &["-f", "GsonBuilder.class"]));
    assert_eq!(entries.lines().count(), 3, "{entries}");
    let at = "libs/bundle.zip!dependency.jar!Lcom/google/gson/GsonBuilder;";
    let methods = text(run(
        &input,
        "methods",
        &["--class", at, "--name", "serializeNulls"],
    ));
    assert_eq!(methods.lines().count(), 2, "{methods}");
    let method = methods.lines().nth(1).unwrap();
    assert!(method.starts_with(at));
    let raw = text(run(&input, "get-method", &["--at", method, "--raw"]));
    assert!(raw.contains("areturn"));
    let java = text(run(&input, "get-class", &["--at", at]));
    assert!(java.contains("class GsonBuilder"));
    let ambiguous = run(
        &input,
        "get-class",
        &["--at", "Lcom/google/gson/GsonBuilder;"],
    );
    assert!(!ambiguous.status.success());
    assert!(ambiguous.stdout.is_empty());
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("defined more than once"));
    let refs = text(run(&input, "xref", &["--at", method]));
    assert!(refs.starts_with("# entry\tcontext\tmatch\n"));
}

#[test]
fn malformed_or_excessively_nested_archives_fail_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("bad.aar");
    fs::write(&input, archive(&[("classes.jar", b"not a ZIP")])).unwrap();
    for operation in ["classes", "entries"] {
        let result = run(&input, operation, &[]);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
    }
    let mut data = fs::read("tests/corpus/jvm/gson-2.11.0.jar").unwrap();
    for _ in 0..18 {
        data = archive(&[("nested.jar", &data)]);
    }
    fs::write(&input, data).unwrap();
    let result = run(&input, "classes", &[]);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("depth"));
}
