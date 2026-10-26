use super::{failure, jsonl, success};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Cursor, Write},
    path::Path,
    process::{Command, Output},
};
use zip::{CompressionMethod, ZipWriter, write::FileOptions};

pub(crate) fn pack(path: &Path, entries: &[(&str, &[u8])]) {
    fs::write(path, packed(entries, CompressionMethod::Deflated, false)).unwrap();
}

fn packed(entries: &[(&str, &[u8])], method: CompressionMethod, wide: bool) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default()
        .compression_method(method)
        .large_file(wide);
    for (name, bytes) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn run(backend: &str, command: &str, input: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args([backend, command])
        .arg(input)
        .args(args)
        .output()
        .unwrap()
}
fn central(bytes: &[u8], index: usize) -> usize {
    let end = bytes.len() - 22;
    let mut at = u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap()) as usize;
    for _ in 0..index {
        let n = |i| u16::from_le_bytes(bytes[at + i..at + i + 2].try_into().unwrap()) as usize;
        at += 46 + n(28) + n(30) + n(32);
    }
    assert_eq!(&bytes[at..at + 4], b"PK\x01\x02");
    at
}
fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn zip_families_preserve_duplicate_coordinates_unicode_and_atomic_extraction() {
    let dir = tempfile::tempdir().unwrap();
    for extension in ["zip", "aar", "vsix", "xpi", "whl", "nupkg", "bin"] {
        let input = dir.path().join(format!("container.{extension}"));
        let bytes = packed(
            &[
                ("目录/MAIN.js", b"first\r\n"),
                ("目录/MAIN.js", b"second"),
                ("empty", b""),
            ],
            CompressionMethod::Stored,
            false,
        );
        fs::write(&input, &bytes).unwrap();
        let listing = run("zip", "entries", &input, &[]);
        success(&listing);
        let rows = jsonl(&listing);
        assert_eq!(rows.len(), 3);
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(row["at"], format!("zip:{index}"));
            assert_eq!(row["detail"]["readable"], true);
            assert_eq!(row["detail"]["compression"], "stored");
        }
        assert_eq!(
            jsonl(&run("zip", "entries", &input, &["-f", "main"])).len(),
            2
        );
        assert!(
            jsonl(&run(
                "zip",
                "entries",
                &input,
                &["-f", "main", "--case", "sensitive"]
            ))
            .is_empty()
        );
        let output = dir.path().join("original");
        fs::write(&output, b"previous").unwrap();
        failure(
            run(
                "zip",
                "extract",
                &input,
                &["--at", "目录/MAIN.js", "-o", output.to_str().unwrap()],
            ),
            2,
            "AMBIGUOUS",
        );
        assert_eq!(fs::read(&output).unwrap(), b"previous");
        for (at, expected) in [
            ("zip:0", &b"first\r\n"[..]),
            ("zip:1", b"second"),
            ("empty", b""),
        ] {
            let result = run(
                "zip",
                "extract",
                &input,
                &["--at", at, "-o", output.to_str().unwrap()],
            );
            success(&result);
            assert_eq!(fs::read(&output).unwrap(), expected);
            let receipt: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(receipt["detail"]["complete"], true);
            assert_eq!(receipt["detail"]["file_bytes"], expected.len());
            assert_eq!(receipt["detail"]["origin"]["format"], "zip");
            assert_eq!(receipt["detail"]["origin"]["container"], json!(input));
        }
        for at in ["zip:99", "absent"] {
            failure(
                run(
                    "zip",
                    "extract",
                    &input,
                    &["--at", at, "-o", output.to_str().unwrap()],
                ),
                2,
                "NOT_FOUND",
            );
        }
        failure(
            run(
                "zip",
                "extract",
                &input,
                &["--at", "zip:00", "-o", output.to_str().unwrap()],
            ),
            2,
            "INVALID_QUERY",
        );
        failure(
            run(
                "zip",
                "extract",
                &input,
                &["--at", "zip:0", "-o", input.to_str().unwrap()],
            ),
            2,
            "INVALID_QUERY",
        );
        assert_eq!(fs::read(&input).unwrap(), bytes);
    }
}

#[test]
fn zip_metadata_retains_unsafe_paths_links_directories_and_unsupported_entries() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("unsafe.zip");
    let output = dir.path().join("output");
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(CompressionMethod::Stored);
    writer.add_directory("folder/", options).unwrap();
    writer.add_symlink("link", "outside", options).unwrap();
    for name in [
        "../escape",
        "/absolute",
        "a/../b",
        "a//b",
        "a\\b",
        "C:drive",
        "nul\0name",
        "encrypted",
        "codec",
        "safe",
    ] {
        writer.start_file(name, options).unwrap();
        writer.write_all(b"bytes").unwrap();
    }
    let mut bytes = writer.finish().unwrap().into_inner();
    let encrypted = central(&bytes, 9);
    put16(&mut bytes, encrypted + 8, 1);
    let codec = central(&bytes, 10);
    put16(&mut bytes, codec + 10, 12);
    fs::write(&input, bytes).unwrap();
    let listing = run("zip", "entries", &input, &[]);
    success(&listing);
    let rows = jsonl(&listing);
    assert_eq!(rows.len(), 12);
    assert_eq!(rows[0]["detail"]["kind"], "directory");
    assert_eq!(rows[1]["detail"]["kind"], "link");
    assert_eq!(rows[9]["detail"]["encrypted"], true);
    assert_eq!(rows[10]["detail"]["compression"], "unsupported");
    for (index, row) in rows.iter().enumerate().take(11) {
        assert_eq!(row["detail"]["readable"], false);
        fs::write(&output, b"previous").unwrap();
        let expected = if (2..9).contains(&index) {
            "INVALID_INPUT"
        } else {
            "UNSUPPORTED_INPUT"
        };
        failure(
            run(
                "zip",
                "extract",
                &input,
                &[
                    "--at",
                    row["at"].as_str().unwrap(),
                    "-o",
                    output.to_str().unwrap(),
                ],
            ),
            1,
            expected,
        );
        assert_eq!(fs::read(&output).unwrap(), b"previous");
    }
    success(&run(
        "zip",
        "extract",
        &input,
        &["--at", "safe", "-o", output.to_str().unwrap()],
    ));
    assert_eq!(fs::read(&output).unwrap(), b"bytes");
    assert!(!dir.path().join("escape").exists());
}

#[test]
fn zip_corruption_size_forgery_and_directory_counts_fail_without_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("bad.zip");
    let output = dir.path().join("result");
    let source = packed(&[("file", b"payload")], CompressionMethod::Stored, false);
    let cd = central(&source, 0);
    let mut crc = source.clone();
    crc[34] ^= 1;
    let mut range = source.clone();
    put32(&mut range, cd + 20, u32::MAX);
    let mut short = source.clone();
    put32(&mut short, cd + 24, 1);
    let mut count = source.clone();
    let footer = count.len() - 22;
    put16(&mut count, footer + 8, 65534);
    put16(&mut count, footer + 10, 65534);
    let mut truncated = source.clone();
    truncated.truncate(source.len() - 5);
    let mut bomb = packed(
        &[("file", &vec![b'x'; 1 << 20])],
        CompressionMethod::Deflated,
        false,
    );
    let bomb_cd = central(&bomb, 0);
    put32(&mut bomb, bomb_cd + 24, 1);
    for bytes in [crc, range, short, count, truncated, bomb] {
        fs::write(&input, bytes).unwrap();
        for exists in [false, true] {
            if exists {
                fs::write(&output, b"previous").unwrap();
            } else {
                let _ = fs::remove_file(&output);
            }
            failure(
                run(
                    "zip",
                    "extract",
                    &input,
                    &["--at", "zip:0", "-o", output.to_str().unwrap()],
                ),
                1,
                "INVALID_INPUT",
            );
            if exists {
                assert_eq!(fs::read(&output).unwrap(), b"previous");
            } else {
                assert!(!output.exists());
            }
        }
    }
    let mut oversized = packed(&[("file", b"x")], CompressionMethod::Stored, false);
    let cd = central(&oversized, 0);
    let extra = [
        1u16.to_le_bytes().as_slice(),
        8u16.to_le_bytes().as_slice(),
        (u64::from(u32::MAX) + 1).to_le_bytes().as_slice(),
    ]
    .concat();
    oversized.splice(cd + 50..cd + 50, extra);
    put16(&mut oversized, cd + 30, 12);
    put32(&mut oversized, cd + 24, u32::MAX);
    let footer = oversized.len() - 22;
    let size = u32::from_le_bytes(oversized[footer + 12..footer + 16].try_into().unwrap());
    put32(&mut oversized, footer + 12, size + 12);
    fs::write(&input, oversized).unwrap();
    let rows = jsonl(&run("zip", "entries", &input, &[]));
    assert_eq!(rows[0]["detail"]["bytes"], 1u64 << 32);
    assert_eq!(rows[0]["detail"]["readable"], false);
    failure(
        run(
            "zip",
            "extract",
            &input,
            &["--at", "zip:0", "-o", output.to_str().unwrap()],
        ),
        1,
        "RESOURCE_LIMIT",
    );
    assert_eq!(fs::read(&output).unwrap(), b"previous");
    fs::write(&input, packed(&[], CompressionMethod::Stored, false)).unwrap();
    assert!(jsonl(&run("zip", "entries", &input, &[])).is_empty());
}

#[test]
fn zip_comment_signatures_do_not_hide_the_real_central_directory() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("comment.zip");
    let output = dir.path().join("file");
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("file", FileOptions::default()).unwrap();
    writer.write_all(b"original").unwrap();
    let comment = format!("{}PK\u{5}\u{6}{}", "x".repeat(30000), "y".repeat(35531));
    assert_eq!(comment.len(), 65535);
    writer.set_comment(comment);
    fs::write(&input, writer.finish().unwrap().into_inner()).unwrap();
    let rows = jsonl(&run("zip", "entries", &input, &[]));
    assert_eq!(rows.len(), 1);
    success(&run(
        "zip",
        "extract",
        &input,
        &["--at", "zip:0", "-o", output.to_str().unwrap()],
    ));
    assert_eq!(fs::read(output).unwrap(), b"original");
}

#[test]
fn zip64_entry_fields_global_directory_and_data_descriptors_are_supported() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("wide.zip");
    let output = dir.path().join("file");
    let base = packed(
        &[("file", b"exact bytes")],
        CompressionMethod::Deflated,
        true,
    );
    fs::write(&input, &base).unwrap();
    success(&run(
        "zip",
        "extract",
        &input,
        &["--at", "file", "-o", output.to_str().unwrap()],
    ));
    assert_eq!(fs::read(&output).unwrap(), b"exact bytes");
    let footer = base.len() - 22;
    let cd = central(&base, 0);
    let mut wide = base[..footer].to_vec();
    wide.extend_from_slice(b"PK\x06\x06");
    wide.extend_from_slice(&44u64.to_le_bytes());
    wide.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    for n in [1u64, 1, (footer - cd) as u64, cd as u64] {
        wide.extend_from_slice(&n.to_le_bytes());
    }
    wide.extend_from_slice(b"PK\x06\x07");
    wide.extend_from_slice(&0u32.to_le_bytes());
    wide.extend_from_slice(&(footer as u64).to_le_bytes());
    wide.extend_from_slice(&1u32.to_le_bytes());
    let mut end = base[footer..].to_vec();
    put16(&mut end, 8, u16::MAX);
    put16(&mut end, 10, u16::MAX);
    put32(&mut end, 12, u32::MAX);
    put32(&mut end, 16, u32::MAX);
    wide.extend_from_slice(&end);
    fs::write(&input, wide).unwrap();
    success(&run(
        "zip",
        "extract",
        &input,
        &["--at", "zip:0", "-o", output.to_str().unwrap()],
    ));
    assert_eq!(fs::read(&output).unwrap(), b"exact bytes");
    let mut descriptor = packed(&[("file", b"descriptor")], CompressionMethod::Stored, false);
    let cd = central(&descriptor, 0);
    let fields = descriptor[14..26].to_vec();
    put16(&mut descriptor, 6, 8);
    descriptor[14..26].fill(0);
    put16(&mut descriptor, cd + 8, 8);
    descriptor.splice(
        cd..cd,
        [b"PK\x07\x08".as_slice(), fields.as_slice()].concat(),
    );
    let footer = descriptor.len() - 22;
    put32(&mut descriptor, footer + 16, (cd + 16) as u32);
    fs::write(&input, descriptor).unwrap();
    success(&run(
        "zip",
        "extract",
        &input,
        &["--at", "file", "-o", output.to_str().unwrap()],
    ));
    assert_eq!(fs::read(&output).unwrap(), b"descriptor");
}

#[test]
fn zip_published_jar_metadata_and_selected_manifest_match_independent_expectations() {
    let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/jvm/gson-2.11.0.jar");
    let rows = jsonl(&run("zip", "entries", &input, &[]));
    assert_eq!(rows.len(), 244);
    assert_eq!(rows[1]["detail"]["path"], "META-INF/MANIFEST.MF");
    assert_eq!(rows[1]["detail"]["bytes"], 1241);
    assert_eq!(rows[1]["detail"]["crc32"], "fc1fe544");
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("manifest");
    success(&run(
        "zip",
        "extract",
        &input,
        &["--at", "zip:1", "-o", output.to_str().unwrap()],
    ));
    let text = fs::read_to_string(output).unwrap();
    assert!(text.contains("Bundle-Version: 2.11.0"));
}

#[test]
fn zip_production_aar_and_vsix_match_pinned_independent_metadata_and_payload_hashes() {
    use sha2::{Digest, Sha256};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/zip");
    let expectations: Vec<Value> =
        serde_json::from_slice(include_bytes!("../corpus/zip/expectations.json")).unwrap();
    let directory = tempfile::tempdir().unwrap();
    for expected in expectations {
        let input = root.join(expected["file"].as_str().unwrap());
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(&input).unwrap())),
            expected["sha256"]
        );
        let listing = run("zip", "entries", &input, &[]);
        success(&listing);
        let rows = jsonl(&listing);
        assert_eq!(rows.len() as u64, expected["entries"].as_u64().unwrap());
        assert_eq!(
            rows.iter()
                .map(|row| row["detail"]["bytes"].as_u64().unwrap())
                .sum::<u64>(),
            expected["file_bytes"]
        );
        for selected in expected["selected"].as_array().unwrap() {
            let row = rows.iter().find(|row| row["at"] == selected["at"]).unwrap();
            for field in ["path", "bytes", "compressed_bytes", "crc32"] {
                assert_eq!(row["detail"][field], selected[field], "{input:?} {field}");
            }
            let output = directory.path().join("original");
            let extracted = run(
                "zip",
                "extract",
                &input,
                &[
                    "--at",
                    selected["at"].as_str().unwrap(),
                    "-o",
                    output.to_str().unwrap(),
                ],
            );
            success(&extracted);
            assert_eq!(
                format!("{:x}", Sha256::digest(fs::read(output).unwrap())),
                selected["sha256"]
            );
        }
    }
}

#[cfg(feature = "backend-jvm")]
#[test]
fn zip_aar_classes_jar_can_be_explicitly_extracted_then_analyzed() {
    let dir = tempfile::tempdir().unwrap();
    let original =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jvm/flow-deflated.jar");
    let input = dir.path().join("sdk.aar");
    let output = dir.path().join("classes.jar");
    pack(
        &input,
        &[
            ("classes.jar", &fs::read(&original).unwrap()),
            ("AndroidManifest.xml", b"<manifest/>"),
        ],
    );
    success(&run(
        "zip",
        "extract",
        &input,
        &["--at", "classes.jar", "-o", output.to_str().unwrap()],
    ));
    let direct = run("jar", "classes", &output, &[]);
    let expected = run("jar", "classes", &original, &[]);
    success(&direct);
    success(&expected);
    assert_eq!(direct.stdout, expected.stdout);
}

#[cfg(all(feature = "backend-html", feature = "backend-wasm"))]
#[test]
fn zip_html_and_wasm_entry_queries_match_original_bytes_and_keep_container_origins() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("site.zip");
    let html = "<!doctype html><script>const text='你好';</script><a href='never-fetched.js'>x</a>";
    let wasm =
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm/fixture.wasm"))
            .unwrap();
    pack(
        &input,
        &[("page.html", html.as_bytes()), ("core.wasm", &wasm)],
    );
    for (backend, path, source, commands) in [
        (
            "html",
            "page.html",
            html.as_bytes(),
            vec!["entries", "resources"],
        ),
        (
            "wasm",
            "core.wasm",
            wasm.as_slice(),
            vec![
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
            ],
        ),
    ] {
        let original = dir.path().join(path);
        fs::write(&original, source).unwrap();
        for command in commands {
            let direct = run(backend, command, &input, &["--entry", path]);
            let expected = run(backend, command, &original, &[]);
            success(&direct);
            success(&expected);
            assert_eq!(direct.stdout, expected.stdout, "{backend} {command}");
        }
    }
    let rows = jsonl(&run(
        "html",
        "entries",
        &input,
        &["--entry", "page.html", "--tag", "script"],
    ));
    let at = rows[0]["at"].as_str().unwrap();
    let output = dir.path().join("script.js");
    let extracted = run(
        "html",
        "extract",
        &input,
        &[
            "--entry",
            "zip:0",
            "--at",
            at,
            "--content",
            "-o",
            output.to_str().unwrap(),
        ],
    );
    success(&extracted);
    assert_eq!(fs::read(&output).unwrap(), "const text='你好';".as_bytes());
    let receipt: Value = serde_json::from_slice(&extracted.stdout).unwrap();
    assert_eq!(receipt["detail"]["origin"]["container"], "page.html");
    assert_eq!(
        receipt["detail"]["container_origin"],
        json!({"container":input,"format":"zip","entry":"zip:0"})
    );
    for raw in [false, true] {
        let mut args = vec!["--entry", "zip:1", "--at", "func:1"];
        if raw {
            args.push("--raw");
        }
        let view = run("wasm", "get-function", &input, &args);
        success(&view);
        let mut receipt: Value = serde_json::from_slice(&view.stdout).unwrap();
        assert_eq!(
            receipt["detail"]
                .as_object_mut()
                .unwrap()
                .remove("origin")
                .unwrap(),
            json!({"container":input,"format":"zip","entry":"zip:1"})
        );
        let args = if raw {
            vec!["--at", "func:1", "--raw"]
        } else {
            vec!["--at", "func:1"]
        };
        let expected = run("wasm", "get-function", &dir.path().join("core.wasm"), &args);
        success(&expected);
        assert_eq!(
            receipt,
            serde_json::from_slice::<Value>(&expected.stdout).unwrap()
        );
    }
}

#[cfg(unix)]
#[test]
fn zip_input_symlinks_and_fifo_are_refused_and_output_alias_does_not_replace_input() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.zip");
    pack(&input, &[("file", b"original")]);
    let before = fs::read(&input).unwrap();
    let link = dir.path().join("link");
    symlink(&input, &link).unwrap();
    failure(run("zip", "entries", &link, &[]), 1, "INVALID_INPUT");
    failure(
        run(
            "zip",
            "extract",
            &input,
            &["--at", "file", "-o", link.to_str().unwrap()],
        ),
        2,
        "INVALID_QUERY",
    );
    let fifo = dir.path().join("fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    failure(run("zip", "entries", &fifo, &[]), 1, "INVALID_INPUT");
    assert_eq!(fs::read(input).unwrap(), before);
}
