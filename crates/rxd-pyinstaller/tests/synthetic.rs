mod common;

use std::{fs, path::PathBuf};

use common::{Archive, Entry, MAGIC, PYZ_MAGIC, PyzBuilder, pyc_header, write};
use rxd_core::{Limits, perf::Perf};
use rxd_pyinstaller::{Summary, detect, unpack};

fn perf() -> Perf {
    Perf::new(false, "analysis", "pyinstaller", "unpack")
}

fn destination(bytes: &[u8]) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let input = write(directory.path(), "app", bytes);
    let output = directory.path().join("out");
    fs::create_dir(&output).unwrap();
    (directory, input, output)
}

fn error_code(result: rxd_core::Result<Summary>) -> &'static str {
    match result {
        Ok(summary) => panic!("expected an error, got {summary:?}"),
        Err(error) => error.code,
    }
}

#[test]
fn writes_stored_and_compressed_entries() {
    let archive = Archive::new(vec![
        Entry::compressed("data/script.bin", b'b', b"compressed payload"),
        Entry::stored("raw.txt", b'x', b"stored payload"),
        Entry::stored("pyi-runtime-option", b'o', b""),
        Entry::stored("dependency", b'd', b""),
    ]);
    let (_directory, input, output) = destination(&archive.build());
    let summary = unpack(&input, &output, Limits::default(), 64, &mut perf()).unwrap();

    assert_eq!(summary.python.as_deref(), Some("3.9"));
    assert_eq!(summary.files, 2);
    assert_eq!(summary.bytes, 18 + 14);
    assert_eq!(summary.skipped, 2);
    assert_eq!(summary.pyz_files, 0);
    assert_eq!(summary.pyz_bytes, 0);
    assert_eq!(
        fs::read(output.join("data/script.bin")).unwrap(),
        b"compressed payload"
    );
    assert_eq!(fs::read(output.join("raw.txt")).unwrap(), b"stored payload");
    assert!(!output.join("pyi-runtime-option").exists());
    assert!(!output.join("dependency").exists());
}

#[test]
fn detects_a_cookie_followed_by_a_code_signature() {
    let archive =
        Archive::new(vec![Entry::stored("raw.txt", b'x', b"payload")]).trailing(&[0xaa; 8192]);
    let (directory, input, _output) = destination(&archive.build());
    assert!(detect(&input).unwrap());

    let plain = write(directory.path(), "plain", b"not a pyinstaller archive");
    assert!(!detect(&plain).unwrap());
}

#[test]
fn rejects_missing_and_truncated_cookies() {
    let (directory, _input, output) = destination(b"");

    let plain = write(directory.path(), "plain", b"not a pyinstaller archive");
    assert_eq!(
        error_code(unpack(&plain, &output, Limits::default(), 64, &mut perf())),
        "INVALID_INPUT"
    );

    let mut bytes = Archive::new(vec![Entry::stored("raw.txt", b'x', b"payload")]).build();
    bytes.truncate(bytes.len() - 8);
    let truncated = write(directory.path(), "truncated", &bytes);
    assert_eq!(
        error_code(unpack(
            &truncated,
            &output,
            Limits::default(),
            64,
            &mut perf()
        )),
        "INVALID_INPUT"
    );
}

#[test]
fn rejects_a_truncated_toc() {
    let mut bytes = Archive::new(vec![
        Entry::stored("a.txt", b'x', b"a"),
        Entry::stored("b.txt", b'x', b"b"),
    ])
    .build();
    let cookie = bytes
        .windows(MAGIC.len())
        .rposition(|window| window == MAGIC)
        .unwrap();
    let toc_length = u32::from_be_bytes(bytes[cookie + 16..cookie + 20].try_into().unwrap());
    bytes[cookie + 16..cookie + 20].copy_from_slice(&(toc_length - 3).to_be_bytes());

    let (_directory, input, output) = destination(&bytes);
    assert_eq!(
        error_code(unpack(&input, &output, Limits::default(), 64, &mut perf())),
        "INVALID_INPUT"
    );
}

#[test]
fn rejects_traversal_and_duplicate_names() {
    for name in [
        "../escape.txt",
        "a/../../escape.txt",
        "/absolute.txt",
        "a/./b.txt",
        "a//b.txt",
        "back\\slash.txt",
        "drive:name.txt",
    ] {
        let archive = Archive::new(vec![Entry::stored(name, b'x', b"payload")]);
        let (_directory, input, output) = destination(&archive.build());
        assert_eq!(
            error_code(unpack(&input, &output, Limits::default(), 64, &mut perf())),
            "INVALID_INPUT",
            "{name}"
        );
    }

    let archive = Archive::new(vec![
        Entry::stored("same.txt", b'x', b"first"),
        Entry::stored("same.txt", b'x', b"second"),
    ]);
    let (_directory, input, output) = destination(&archive.build());
    assert_eq!(
        error_code(unpack(&input, &output, Limits::default(), 64, &mut perf())),
        "INVALID_INPUT"
    );
}

#[test]
fn extracts_pyz_packages_and_modules() {
    let pyz = PyzBuilder::new(PYZ_MAGIC)
        .module("pkg", true, b"package bytecode")
        .module("pkg.mod", false, b"module bytecode")
        .empty_module("empty", false)
        .build();
    let archive = Archive::new(vec![Entry::compressed("PYZ", b'z', &pyz)]);
    let (_directory, input, output) = destination(&archive.build());
    let summary = unpack(&input, &output, Limits::default(), 64, &mut perf()).unwrap();

    let header = pyc_header(PYZ_MAGIC, 16);
    let mut package = header.clone();
    package.extend_from_slice(b"package bytecode");
    let mut module = header.clone();
    module.extend_from_slice(b"module bytecode");

    assert_eq!(
        fs::read(output.join("PYZ.pyz_extracted/pkg/__init__.pyc")).unwrap(),
        package
    );
    assert_eq!(
        fs::read(output.join("PYZ.pyz_extracted/pkg/mod.pyc")).unwrap(),
        module
    );
    assert_eq!(
        fs::read(output.join("PYZ.pyz_extracted/empty.pyc")).unwrap(),
        header
    );
    assert_eq!(fs::read(output.join("PYZ.pyz")).unwrap(), pyz);

    assert_eq!(summary.files, 4);
    assert_eq!(summary.pyz_files, 3);
    assert_eq!(summary.pyz_bytes, 32 + 31 + 16);
    assert_eq!(summary.bytes, pyz.len() as u64 + summary.pyz_bytes);
}

#[test]
fn extracts_a_dictionary_pyz_toc() {
    let pyz = PyzBuilder::new(PYZ_MAGIC)
        .module("plain", false, b"bytecode")
        .dictionary()
        .build();
    let archive = Archive::new(vec![Entry::compressed("PYZ", b'z', &pyz)]);
    let (_directory, input, output) = destination(&archive.build());
    let summary = unpack(&input, &output, Limits::default(), 64, &mut perf()).unwrap();

    assert_eq!(summary.pyz_files, 1);
    assert!(output.join("PYZ.pyz_extracted/plain.pyc").is_file());
}

#[test]
fn patches_bare_scripts_with_the_pyz_magic() {
    let magic = [0x55, 0x0d, 0x0d, 0x0a];
    let pyz = PyzBuilder::new(magic).module("m", false, b"x").build();
    let archive = Archive::new(vec![
        Entry::compressed("boot", b's', b"bare script"),
        Entry::compressed("PYZ", b'z', &pyz),
    ]);
    let (_directory, input, output) = destination(&archive.build());
    let summary = unpack(&input, &output, Limits::default(), 64, &mut perf()).unwrap();

    let mut expected = pyc_header(magic, 16);
    expected.extend_from_slice(b"bare script");
    assert_eq!(fs::read(output.join("boot.pyc")).unwrap(), expected);
    assert_eq!(summary.files, 3);
}

#[test]
fn keeps_existing_bytecode_headers() {
    let mut payload = pyc_header(PYZ_MAGIC, 16);
    payload.extend_from_slice(b"body");
    let archive = Archive::new(vec![Entry::compressed("mod", b'm', &payload)]);
    let (_directory, input, output) = destination(&archive.build());
    let summary = unpack(&input, &output, Limits::default(), 64, &mut perf()).unwrap();

    assert_eq!(fs::read(output.join("mod.pyc")).unwrap(), payload);
    assert_eq!(summary.files, 1);
    assert_eq!(summary.bytes, payload.len() as u64);
}

#[test]
fn rejects_bare_scripts_without_a_magic() {
    let archive = Archive::new(vec![Entry::compressed("boot", b's', b"bare")]);
    let (_directory, input, output) = destination(&archive.build());
    assert_eq!(
        error_code(unpack(&input, &output, Limits::default(), 64, &mut perf())),
        "UNSUPPORTED_INPUT"
    );
    assert!(output.join("boot.pyc").is_file());
}

#[test]
fn rejects_size_disagreement_and_zlib_failure() {
    let mut wrong = Entry::compressed("bad.bin", b'b', b"payload");
    wrong.uncompressed = Some(3);
    let archive = Archive::new(vec![wrong]);
    let (_directory, input, output) = destination(&archive.build());
    assert_eq!(
        error_code(unpack(&input, &output, Limits::default(), 64, &mut perf())),
        "INVALID_INPUT"
    );

    let archive = Archive::new(vec![Entry {
        name: "bad.bin".to_owned(),
        kind: b'b',
        flag: 1,
        data: b"not zlib".to_vec(),
        stored: None,
        uncompressed: Some(8),
        offset: None,
    }]);
    let (_directory, input, output) = destination(&archive.build());
    assert_eq!(
        error_code(unpack(&input, &output, Limits::default(), 64, &mut perf())),
        "UNSUPPORTED_INPUT"
    );

    let archive = Archive::new(vec![Entry {
        name: "bad.bin".to_owned(),
        kind: b'b',
        flag: 0,
        data: b"five!".to_vec(),
        stored: None,
        uncompressed: Some(3),
        offset: None,
    }]);
    let (_directory, input, output) = destination(&archive.build());
    assert_eq!(
        error_code(unpack(&input, &output, Limits::default(), 64, &mut perf())),
        "INVALID_INPUT"
    );
}

#[test]
fn enforces_the_output_cap() {
    let limits = Limits {
        output_bytes: Some(4),
        ..Limits::default()
    };

    let archive = Archive::new(vec![Entry::stored("big.bin", b'b', b"0123456789")]);
    let (_directory, input, output) = destination(&archive.build());
    assert_eq!(
        error_code(unpack(&input, &output, limits, 64, &mut perf())),
        "RESOURCE_LIMIT"
    );

    let archive = Archive::new(vec![Entry::compressed("big.bin", b'b', b"0123456789")]);
    let (_directory, input, output) = destination(&archive.build());
    assert_eq!(
        error_code(unpack(&input, &output, limits, 64, &mut perf())),
        "RESOURCE_LIMIT"
    );
}

#[test]
fn enforces_the_entry_limit() {
    let archive = Archive::new(vec![
        Entry::stored("a.txt", b'x', b"a"),
        Entry::stored("b.txt", b'x', b"b"),
        Entry::stored("skipped", b'd', b""),
    ]);
    let (_directory, input, output) = destination(&archive.build());
    assert_eq!(
        error_code(unpack(&input, &output, Limits::default(), 2, &mut perf())),
        "RESOURCE_LIMIT"
    );

    let pyz = PyzBuilder::new(PYZ_MAGIC)
        .module("a", false, b"a")
        .module("b", false, b"b")
        .build();
    let archive = Archive::new(vec![Entry::compressed("PYZ", b'z', &pyz)]);
    let (_directory, input, output) = destination(&archive.build());
    assert_eq!(
        error_code(unpack(&input, &output, Limits::default(), 1, &mut perf())),
        "RESOURCE_LIMIT"
    );
}

#[test]
fn rejects_invalid_pyz_payloads() {
    let archive = Archive::new(vec![Entry::stored("PYZ", b'z', b"junk")]);
    let (_directory, input, output) = destination(&archive.build());
    assert_eq!(
        error_code(unpack(&input, &output, Limits::default(), 64, &mut perf())),
        "INVALID_INPUT"
    );

    for name in ["pkg/evil", ".hidden", "a..b", "a:b"] {
        let pyz = PyzBuilder::new(PYZ_MAGIC).module(name, false, b"x").build();
        let archive = Archive::new(vec![Entry::compressed("PYZ", b'z', &pyz)]);
        let (_directory, input, output) = destination(&archive.build());
        assert_eq!(
            error_code(unpack(&input, &output, Limits::default(), 64, &mut perf())),
            "INVALID_INPUT",
            "{name}"
        );
    }

    let pyz = PyzBuilder::new(PYZ_MAGIC)
        .module("dup", false, b"a")
        .module("dup", false, b"b")
        .build();
    let archive = Archive::new(vec![Entry::compressed("PYZ", b'z', &pyz)]);
    let (_directory, input, output) = destination(&archive.build());
    assert_eq!(
        error_code(unpack(&input, &output, Limits::default(), 64, &mut perf())),
        "INVALID_INPUT"
    );
}

#[test]
fn rejects_invalid_pyz_module_compression() {
    let mut pyz = PyzBuilder::new(PYZ_MAGIC)
        .module("mod", false, b"bytecode")
        .build();
    let toc_position = u32::from_be_bytes(pyz[8..12].try_into().unwrap()) as usize;
    assert!(toc_position > 14);
    for byte in &mut pyz[12..toc_position] {
        *byte = 0xff;
    }

    let archive = Archive::new(vec![Entry::compressed("PYZ", b'z', &pyz)]);
    let (_directory, input, output) = destination(&archive.build());
    assert_eq!(
        error_code(unpack(&input, &output, Limits::default(), 64, &mut perf())),
        "UNSUPPORTED_INPUT"
    );
}
