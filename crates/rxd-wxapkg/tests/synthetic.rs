use std::{fs, path::Path};

use rxd_core::{Limits, perf::Perf};
use rxd_wxapkg::unpack;

fn package(files: &[(&str, &[u8])]) -> Vec<u8> {
    let index_len = files.iter().map(|(name, _)| 12 + name.len()).sum::<usize>();
    let data_start = 18 + index_len;
    let body_len = files.iter().map(|(_, data)| data.len()).sum::<usize>();
    let mut index = Vec::new();
    let mut body = Vec::new();
    for (name, data) in files {
        index.extend_from_slice(&(name.len() as u32).to_be_bytes());
        index.extend_from_slice(name.as_bytes());
        index.extend_from_slice(&((data_start + body.len()) as u32).to_be_bytes());
        index.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
    }
    let mut bytes = Vec::new();
    bytes.push(0xbe);
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&(index_len as u32).to_be_bytes());
    bytes.extend_from_slice(&(body_len as u32).to_be_bytes());
    bytes.push(0xed);
    bytes.extend_from_slice(&(files.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&index);
    bytes.extend_from_slice(&body);
    bytes
}

fn run(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("app.wxapkg");
    let output = directory.path().join("out");
    fs::write(&input, bytes).unwrap();
    fs::create_dir(&output).unwrap();
    (directory, input, output)
}

fn invoke(input: &Path, output: &Path) -> rxd_core::Result<rxd_wxapkg::Summary> {
    unpack(
        input,
        output,
        Limits::default(),
        64,
        None,
        &mut Perf::new(false, "analysis", "wxapkg", "unpack"),
    )
}

#[test]
fn extracts_indexed_files_and_ignores_leading_slashes() {
    let bytes = package(&[
        ("/app-config.json", b"{}"),
        ("/pages/index/index.wxml", b"<view />"),
        ("/assets/logo.png", b"\x89PNG"),
    ]);
    let (_directory, input, output) = run(&bytes);
    let summary = invoke(&input, &output).unwrap();
    assert_eq!(summary.files, 3);
    assert_eq!(summary.bytes, 2 + 8 + 4);
    assert!(!summary.encrypted);
    assert_eq!(fs::read(output.join("app-config.json")).unwrap(), b"{}");
    assert_eq!(
        fs::read(output.join("pages/index/index.wxml")).unwrap(),
        b"<view />"
    );
    assert_eq!(
        fs::read(output.join("assets/logo.png")).unwrap(),
        b"\x89PNG"
    );
}

#[test]
fn rejects_traversal_duplicates_and_short_files() {
    let bytes = package(&[("/../escape", b"x")]);
    let (_directory, input, output) = run(&bytes);
    assert_eq!(invoke(&input, &output).unwrap_err().code, "INVALID_INPUT");

    let bytes = package(&[("/same", b"a"), ("/same", b"b")]);
    let (_directory, input, output) = run(&bytes);
    assert_eq!(invoke(&input, &output).unwrap_err().code, "INVALID_INPUT");

    let (_directory, input, output) = run(b"\xbe\0");
    assert_eq!(
        invoke(&input, &output).unwrap_err().code,
        "UNSUPPORTED_INPUT"
    );
}

#[test]
fn rejects_an_entry_outside_the_body() {
    let mut bytes = package(&[("/a", b"abc")]);
    bytes[24..28].copy_from_slice(&99u32.to_be_bytes());
    let (_directory, input, output) = run(&bytes);
    assert_eq!(invoke(&input, &output).unwrap_err().code, "INVALID_INPUT");
}

#[test]
fn tolerates_a_declared_body_length_past_the_file() {
    let mut bytes = package(&[("/file", b"payload")]);
    let body = u32::from_be_bytes(bytes[9..13].try_into().unwrap()) + 4;
    bytes[9..13].copy_from_slice(&body.to_be_bytes());
    let (_directory, input, output) = run(&bytes);
    let summary = invoke(&input, &output).unwrap();
    assert_eq!(summary.files, 1);
    assert_eq!(fs::read(output.join("file")).unwrap(), b"payload");
}
