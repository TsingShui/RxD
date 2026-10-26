use super::{failure, success};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;
use zip::{CompressionMethod, ZipWriter, write::FileOptions};

fn run(archive: &Path, output: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["unpack"])
        .arg(archive)
        .args(["-o"])
        .arg(output)
        .output()
        .unwrap()
}

fn receipt(output: &Output) -> Value {
    success(output);
    serde_json::from_slice(&output.stdout).unwrap()
}

fn write_asar(path: &Path, header: &Value, data: &[u8]) {
    let json = serde_json::to_vec(header).unwrap();
    let payload = (json.len() + 4).next_multiple_of(4);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&4_u32.to_le_bytes());
    bytes.extend_from_slice(&((payload + 4) as u32).to_le_bytes());
    bytes.extend_from_slice(&(payload as u32).to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&json);
    bytes.resize(payload + 12, 0);
    bytes.extend_from_slice(data);
    fs::write(path, bytes).unwrap();
}

fn asar_fixture(directory: &Path) -> PathBuf {
    let archive = directory.join("app.asar");
    let header = json!({"files":{
        "dir":{"files":{"a.txt":{"size":5,"offset":"0"}}},
        "empty":{"size":0,"offset":"5"},
        "link.txt":{"link":"dir/a.txt"},
        "side.txt":{"size":4,"unpacked":true}
    }});
    write_asar(&archive, &header, b"alpha");
    let sidecar = directory.join("app.asar.unpacked");
    fs::create_dir(&sidecar).unwrap();
    fs::write(sidecar.join("side.txt"), b"side").unwrap();
    archive
}

fn zip_fixture(path: &Path, entries: &[(&str, &[u8], CompressionMethod)]) {
    let file = fs::File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    for (name, bytes, method) in entries {
        let options = FileOptions::default().compression_method(*method);
        zip.start_file(*name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn unpack_extracts_complete_asar_trees_and_resolves_links_and_unpacked_files() {
    let directory = TempDir::new().unwrap();
    let archive = asar_fixture(directory.path());
    let output = directory.path().join("out");
    let receipt = receipt(&run(&archive, &output));
    assert_eq!(receipt["detail"]["format"], "asar");
    assert_eq!(receipt["detail"]["files"], 4);
    assert_eq!(receipt["detail"]["bytes"], 14);
    assert_eq!(fs::read(output.join("dir/a.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(output.join("empty")).unwrap(), b"");
    assert_eq!(fs::read(output.join("link.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(output.join("side.txt")).unwrap(), b"side");
}

#[test]
fn unpack_extracts_zip_trees_and_detects_format_from_content() {
    let directory = TempDir::new().unwrap();
    let archive = directory.path().join("archive.bin");
    zip_fixture(
        &archive,
        &[
            ("dir/a.txt", b"alpha", CompressionMethod::Deflated),
            ("empty", b"", CompressionMethod::Stored),
            (
                "unicode/你好.txt",
                "你好".as_bytes(),
                CompressionMethod::Deflated,
            ),
        ],
    );
    let output = directory.path().join("out");
    let receipt = receipt(&run(&archive, &output));
    assert_eq!(receipt["detail"]["format"], "zip");
    assert_eq!(receipt["detail"]["files"], 3);
    assert_eq!(fs::read(output.join("dir/a.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(output.join("empty")).unwrap(), b"");
    assert_eq!(
        fs::read(output.join("unicode/你好.txt")).unwrap(),
        "你好".as_bytes()
    );
}

#[test]
fn unpack_rejects_unsafe_duplicate_link_and_existing_destinations_without_partial_trees() {
    let directory = TempDir::new().unwrap();
    let output = directory.path().join("out");

    let traversal = directory.path().join("traversal.zip");
    zip_fixture(
        &traversal,
        &[("../escape", b"x", CompressionMethod::Stored)],
    );
    failure(run(&traversal, &output), 1, "INVALID_INPUT");
    assert!(!output.exists());

    let duplicate = directory.path().join("duplicate.zip");
    zip_fixture(
        &duplicate,
        &[
            ("same.txt", b"one", CompressionMethod::Stored),
            ("same.txt", b"two", CompressionMethod::Stored),
        ],
    );
    failure(run(&duplicate, &output), 1, "INVALID_INPUT");
    assert!(!output.exists());

    let link = directory.path().join("link.zip");
    {
        let file = fs::File::create(&link).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = FileOptions::default().compression_method(CompressionMethod::Stored);
        zip.add_symlink("link", "target", options).unwrap();
        zip.finish().unwrap();
    }
    failure(run(&link, &output), 1, "UNSUPPORTED_INPUT");
    assert!(!output.exists());

    let archive = zip_fixture_ok(directory.path());
    fs::create_dir(&output).unwrap();
    failure(run(&archive, &output), 2, "INVALID_QUERY");
    assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
}

fn zip_fixture_ok(directory: &Path) -> PathBuf {
    let archive = directory.join("ok.zip");
    zip_fixture(&archive, &[("file.txt", b"ok", CompressionMethod::Stored)]);
    archive
}

#[test]
fn unpack_rejects_an_unsafe_asar_member_and_cleans_the_destination() {
    let directory = TempDir::new().unwrap();
    let archive = directory.path().join("bad.asar");
    write_asar(
        &archive,
        &json!({"files":{"../escape":{"size":1,"offset":"0"}}}),
        b"x",
    );
    let output = directory.path().join("out");
    failure(run(&archive, &output), 1, "INVALID_INPUT");
    assert!(!output.exists());
}

#[test]
fn unpack_extracts_pinned_production_archives_with_independent_payload_hashes() {
    use sha2::{Digest, Sha256};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../corpus/zip/expectations.json")).unwrap();
    for (index, case) in cases.iter().enumerate() {
        let archive = root
            .join("tests/corpus/zip")
            .join(case["file"].as_str().unwrap());
        let directory = TempDir::new().unwrap();
        let output = directory.path().join(format!("out{index}"));
        let receipt = receipt(&run(&archive, &output));
        assert_eq!(receipt["detail"]["complete"], true);
        assert_eq!(receipt["detail"]["format"], "zip");
        for selected in case["selected"].as_array().unwrap() {
            let bytes = fs::read(output.join(selected["path"].as_str().unwrap())).unwrap();
            assert_eq!(bytes.len() as u64, selected["bytes"].as_u64().unwrap());
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                selected["sha256"].as_str().unwrap()
            );
        }
    }
}

#[test]
fn unpack_extracts_a_pinned_pyinstaller_archive_with_pyz_modules_and_pyc_headers() {
    use sha2::{Digest, Sha256};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let expectations: Value =
        serde_json::from_str(include_str!("../corpus/pyinstaller/expectations.json")).unwrap();
    let archive = root
        .join("tests/corpus/pyinstaller")
        .join(expectations["file"].as_str().unwrap());
    let directory = TempDir::new().unwrap();
    let output = directory.path().join("out");
    let receipt = receipt(&run(&archive, &output));
    let detail = &receipt["detail"];
    for key in [
        "format",
        "python",
        "files",
        "bytes",
        "pyz_files",
        "pyz_bytes",
        "skipped",
    ] {
        assert_eq!(detail[key], expectations["receipt"][key], "{key}");
    }
    for key in ["pyc", "pyz_module"] {
        let bytes = fs::read(output.join(expectations[key]["path"].as_str().unwrap())).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            expectations[key]["sha256"].as_str().unwrap(),
            "{key}"
        );
    }
    let pyc = fs::read(output.join("hello.pyc")).unwrap();
    assert_eq!(&pyc[..4], b"\x61\r\r\n");
    #[cfg(feature = "backend-pyc")]
    {
        let decompiled = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["pyc", "decompile"])
            .arg(output.join("hello.pyc"))
            .output()
            .unwrap();
        success(&decompiled);
        let source = String::from_utf8(decompiled.stdout).unwrap();
        assert!(source.contains("json.dumps"), "{source}");
    }
}

#[test]
fn unpack_finds_the_launcher_of_a_pyinstaller_one_dir_tree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = root.join("tests/corpus/pyinstaller/hello");
    let directory = TempDir::new().unwrap();
    let application = directory.path().join("application");
    fs::create_dir(&application).unwrap();
    fs::copy(&fixture, application.join("hello")).unwrap();
    fs::create_dir(application.join("_internal")).unwrap();
    fs::write(
        application.join("_internal/base_library.zip"),
        b"placeholder",
    )
    .unwrap();
    let output = directory.path().join("out");
    let receipt = receipt(&run(&application, &output));
    assert_eq!(receipt["detail"]["format"], "pyinstaller");
    assert_eq!(receipt["detail"]["layout"], "onedir");
    assert_eq!(
        receipt["detail"]["launcher"],
        application.join("hello").to_string_lossy().as_ref()
    );
    assert!(output.join("hello.pyc").is_file());
    assert!(output.join("PYZ.pyz_extracted/json/__init__.pyc").is_file());
    assert!(!output.join("_internal").exists());
}

#[test]
fn unpack_extracts_normal_and_v1mmwx_wxapkg_packages() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = TempDir::new().unwrap();

    let normal = directory.path().join("normal.wxapkg");
    fs::copy(root.join("tests/fixtures/wxapkg/sample.wxapkg"), &normal).unwrap();
    let output = directory.path().join("normal-out");
    let normal_receipt = receipt(&run(&normal, &output));
    assert_eq!(normal_receipt["detail"]["format"], "wxapkg");
    assert_eq!(normal_receipt["detail"]["files"], 3);
    assert_eq!(normal_receipt["detail"]["encrypted"], false);
    assert_eq!(fs::read(output.join("app-service.js")).unwrap(), b"App({})");

    let encrypted = directory.path().join("encrypted.wxapkg");
    fs::copy(
        root.join("tests/fixtures/wxapkg/sample.enc.wxapkg"),
        &encrypted,
    )
    .unwrap();
    let output = directory.path().join("encrypted-out");
    failure(run(&encrypted, &output), 1, "UNSUPPORTED_INPUT");
    assert!(!output.exists());

    let output = directory.path().join("encrypted-out");
    let request = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["unpack"])
        .arg(&encrypted)
        .args(["-o"])
        .arg(&output)
        .args(["--appid", "wx1234567890abcdef"])
        .output()
        .unwrap();
    let decrypted = receipt(&request);
    assert_eq!(decrypted["detail"]["format"], "wxapkg");
    assert_eq!(decrypted["detail"]["encrypted"], true);
    assert_eq!(decrypted["detail"]["appid"], "wx1234567890abcdef");
    assert_eq!(
        fs::read(output.join("pages/index/index.wxml")).unwrap(),
        b"<view>hello</view>"
    );
}
