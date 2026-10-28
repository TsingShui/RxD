use super::{failure, success};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;
use zip::{CompressionMethod, ZipWriter, write::FileOptions};

const ABC: &[u8] = include_bytes!("../corpus/abc/minimal.abc");

fn expected() -> Value {
    serde_json::from_str(include_str!("../corpus/abc/minimal.json")).unwrap()
}

fn query(input: &Path, command: &str, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["ark", "list", command])
        .arg(input)
        .args(flags)
        .output()
        .unwrap()
}

fn fetch(input: &Path, at: &str, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["ark", "decompile"])
        .arg(input)
        .args(["--at", at, "--raw"])
        .args(flags)
        .output()
        .unwrap()
}

fn source(input: &Path, at: &str, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["ark", "decompile"])
        .arg(input)
        .args(["--at", at])
        .args(flags)
        .output()
        .unwrap()
}

fn fixture_with_body(vregs: u8, args: u8, instructions: &[u8]) -> (Vec<u8>, String) {
    assert!(instructions.len() < 128 && vregs < 128 && args < 128);
    let expected = expected();
    let method = &expected["record"]["methods"][1];
    let offset = method["offset"].as_u64().unwrap() as usize;
    let mut bytes = ABC.to_vec();
    let code_offset = bytes.len() as u32;
    bytes[offset + 10..offset + 14].copy_from_slice(&code_offset.to_le_bytes());
    bytes.extend_from_slice(&[vregs, args, instructions.len() as u8, 0]);
    bytes.extend_from_slice(instructions);
    let length = (bytes.len() as u32).to_le_bytes();
    bytes[16..20].copy_from_slice(&length);
    bytes[72..76].copy_from_slice(&length);
    checksum(&mut bytes);
    (bytes, method["at"].as_str().unwrap().to_owned())
}

fn fixture_with_nested(parent: &[u8], child: &[u8]) -> (Vec<u8>, String, String) {
    let (mut bytes, parent_at) = fixture_with_body(1, 3, parent);
    let expected = expected();
    let first = expected["record"]["methods"][0]["offset"].as_u64().unwrap() as usize;
    let second = expected["record"]["methods"][1]["offset"].as_u64().unwrap() as u32;
    let child_at = format!("method:0x{first:x}");
    let code = bytes.len() as u32;
    bytes[first + 10..first + 14].copy_from_slice(&code.to_le_bytes());
    bytes[124..128].copy_from_slice(&(first as u32).to_le_bytes());
    bytes[128..132].copy_from_slice(&second.to_le_bytes());
    bytes.extend_from_slice(&[0, 3, child.len() as u8, 0]);
    bytes.extend_from_slice(child);
    let length = (bytes.len() as u32).to_le_bytes();
    bytes[16..20].copy_from_slice(&length);
    bytes[72..76].copy_from_slice(&length);
    checksum(&mut bytes);
    (bytes, parent_at, child_at)
}

fn fixture_with_kind(
    flags: u32,
    vregs: u8,
    args: u8,
    instructions: &[u8],
    tries: &[u8],
) -> (Vec<u8>, String, u32) {
    assert!(instructions.len() < 128 && vregs < 128 && args < 128);
    let expected = expected();
    let old_method = expected["record"]["methods"][1]["offset"].as_u64().unwrap() as usize;
    let mut bytes = ABC.to_vec();
    let method_name = bytes[old_method + 4..old_method + 8].to_vec();
    let record = bytes.len() as u32;
    bytes.extend_from_slice(b"\x0bLnew;\0");
    bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 1, 0]);
    let method = bytes.len() as u32;
    bytes.extend_from_slice(&[2, 0, 0xff, 0xff]);
    bytes.extend_from_slice(&method_name);
    let mut value = flags;
    while value >= 128 {
        bytes.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    bytes.push(value as u8);
    bytes.push(1);
    let code_address = bytes.len();
    bytes.extend_from_slice(&[0, 0, 0, 0, 0]);
    let code = bytes.len() as u32;
    bytes[code_address..code_address + 4].copy_from_slice(&code.to_le_bytes());
    bytes.extend_from_slice(&[
        vregs,
        args,
        instructions.len() as u8,
        u8::from(!tries.is_empty()),
    ]);
    bytes.extend_from_slice(instructions);
    bytes.extend_from_slice(tries);
    let length = (bytes.len() as u32).to_le_bytes();
    bytes[60..64].copy_from_slice(&record.to_le_bytes());
    bytes[116..120].copy_from_slice(&record.to_le_bytes());
    bytes[16..20].copy_from_slice(&length);
    bytes[72..76].copy_from_slice(&length);
    checksum(&mut bytes);
    (bytes, format!("method:0x{method:x}"), record)
}

fn fixture_with_call_type(call_type: u32, args: u8, body: &[u8]) -> (Vec<u8>, String) {
    let (mut bytes, old_at, _) = fixture_with_kind(0x108, 1, args, body, &[]);
    let old_method = usize::from_str_radix(old_at.trim_start_matches("method:0x"), 16).unwrap();
    let name = bytes[old_method + 4..old_method + 8].to_vec();
    let code = bytes[old_method + 11..old_method + 15].to_vec();
    let other = u32::from_le_bytes(bytes[120..124].try_into().unwrap());
    let record = bytes.len() as u32;
    bytes.extend_from_slice(b"\x0dLcall;\0");
    bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 1, 0]);
    let method = bytes.len() as u32;
    bytes.extend_from_slice(&[2, 0, 0xff, 0xff]);
    bytes.extend_from_slice(&name);
    bytes.extend_from_slice(&[0x88, 2, 1]);
    bytes.extend_from_slice(&code);
    bytes.push(6);
    let annotation_patch = bytes.len();
    bytes.extend_from_slice(&[0, 0, 0, 0, 0]);
    let class = bytes.len() as u32;
    let class_name = b"L_ESCallTypeAnnotation;";
    bytes.push((class_name.len() * 2 + 1) as u8);
    bytes.extend_from_slice(class_name);
    bytes.push(0);
    let element = bytes.len() as u32;
    bytes.extend_from_slice(b"\x11callType\0");
    let annotation = bytes.len() as u32;
    bytes.extend_from_slice(&[4, 0, 1, 0]);
    bytes.extend_from_slice(&element.to_le_bytes());
    bytes.extend_from_slice(&call_type.to_le_bytes());
    bytes.push(b'7');
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    let classes = bytes.len() as u32;
    for word in [2, 6, record, other, class] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    let length = (bytes.len() as u32).to_le_bytes();
    bytes[annotation_patch..annotation_patch + 4].copy_from_slice(&annotation.to_le_bytes());
    bytes[60..64].copy_from_slice(&record.to_le_bytes());
    bytes[76..80].copy_from_slice(&5u32.to_le_bytes());
    bytes[80..84].copy_from_slice(&classes.to_le_bytes());
    bytes[16..20].copy_from_slice(&length);
    bytes[72..76].copy_from_slice(&length);
    checksum(&mut bytes);
    (bytes, format!("method:0x{method:x}"))
}

fn append_entity(bytes: &mut Vec<u8>, index: usize, entity: &[u8]) {
    let offset = bytes.len() as u32;
    bytes[124 + 4 * index..128 + 4 * index].copy_from_slice(&offset.to_le_bytes());
    bytes.extend_from_slice(entity);
    let length = (bytes.len() as u32).to_le_bytes();
    bytes[16..20].copy_from_slice(&length);
    bytes[72..76].copy_from_slice(&length);
    checksum(bytes);
}

fn rows(output: &Output) -> Vec<Vec<String>> {
    success(output);
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| line.split('\t').map(str::to_owned).collect())
        .collect()
}

fn checksum(bytes: &mut [u8]) {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &bytes[12..] {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    bytes[8..12].copy_from_slice(&((b << 16) | a).to_le_bytes());
}

fn hap(path: &Path, entries: &[(&str, &[u8])]) {
    let mut zip = ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in entries {
        zip.start_file(
            *name,
            FileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn ark_navigates_records_and_methods_with_unicode_and_precise_code_sizes() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("minimal.abc");
    fs::write(&input, ABC).unwrap();
    let expected = expected();
    let record_at = expected["record"]["at"].as_str().unwrap();
    let records = rows(&query(&input, "record", &[]));
    assert_eq!(records.len(), 2);
    assert_eq!(records[0][0], record_at);
    assert_eq!(
        serde_json::from_str::<Value>(&records[0][1]).unwrap(),
        expected["record"]["name"]
    );
    assert_eq!(records[0][2], "2");
    let methods = rows(&query(&input, "method", &["--at", record_at]));
    for (actual, expected) in methods
        .iter()
        .zip(expected["record"]["methods"].as_array().unwrap())
    {
        assert_eq!(actual[0], expected["at"]);
        assert_eq!(
            serde_json::from_str::<Value>(&actual[1]).unwrap(),
            expected["name"]
        );
        assert_eq!(actual[2], expected["code_size"].to_string());
        assert_eq!(actual[3], "3");
        assert_eq!(actual[4], "0");
    }
    assert_eq!(methods.len(), 2);
    assert_eq!(
        rows(&query(&input, "method", &["--at", record_at, "-f", "加法"])).len(),
        1
    );
    assert!(
        rows(&query(
            &input,
            "method",
            &["--at", record_at, "-f", "absent"]
        ))
        .is_empty()
    );
    assert_eq!(rows(&query(&input, "record", &["-f", "main"])).len(), 1);
    assert!(
        rows(&query(
            &input,
            "record",
            &["-f", "main", "--case", "sensitive"]
        ))
        .is_empty()
    );
    assert!(
        rows(&query(
            &input,
            "method",
            &["--at", expected["empty_record"]["at"].as_str().unwrap()]
        ))
        .is_empty()
    );
    failure(
        query(&input, "method", &["--at", "record:0x1", "-f", "absent"]),
        2,
        "NOT_FOUND",
    );
    failure(
        query(&input, "method", &["--at", "method:0x88"]),
        2,
        "INVALID_QUERY",
    );
}

#[test]
fn ark_uses_declared_function_kinds_and_rejects_unproven_arrow_frames() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("kinds.abc");
    let (bytes, at, _) =
        fixture_with_kind(0x208, 3, 3, &[0x45, 0, 5, 0x45, 1, 4, 0x60, 0, 0x64], &[]);
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(
        text.contains("function _ark_capture_") && text.contains(" = () => {"),
        "{text}"
    );
    assert!(
        text.contains("return __ark_context[\"receiver\"];"),
        "{text}"
    );
    assert!(text.ends_with("}\n"), "{text}");

    let (bytes, at, _) = fixture_with_kind(0x208, 0, 3, &[0x3c, 0, 0x64], &[]);
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(
        text.contains("function _ark_capture_") && text.contains(" = () => {"),
        "{text}"
    );
    assert!(
        text.contains("return __ark_env[\"slots\"][0]") && text.contains("_ark_capture_"),
        "{text}"
    );

    let (bytes, at, _) = fixture_with_kind(0x208, 3, 3, &[0x45, 0, 3, 0x60, 0, 0x64], &[]);
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("return __ark_f_"),
        "function values use their emitted name"
    );
    let (bytes, at, _) = fixture_with_kind(0x308, 0, 3, &[0x65], &[]);
    fs::write(&input, bytes).unwrap();
    failure(source(&input, &at, &[]), 1, "DECOMPILE_FAILED");

    let (bytes, at, _) = fixture_with_kind(0x208, 3, 3, &[0x45, 0, 3, 0x64], &[]);
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);

    let (bytes, at, _) = fixture_with_kind(0x8, 0, 3, &[0x65], &[]);
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.starts_with("function _ark_capture_"), "{text}");
    assert!(text.ends_with("}\n"), "{text}");
}

#[test]
fn ark_uses_the_method_call_type_annotation_for_its_argument_frame() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("call-type.abc");
    for (call_type, args, body, expected) in [
        (0, 1, vec![0x60, 1, 0x64], "return p0;"),
        (1, 2, vec![0x45, 0, 1, 0x60, 0, 0x64], "return this;"),
        (2, 2, vec![0x45, 0, 1, 0x60, 0, 0x64], "return new.target;"),
        (9, 3, vec![0x45, 0, 2, 0x60, 0, 0x64], "return this;"),
        (10, 3, vec![0x45, 0, 2, 0x60, 0, 0x64], "return new.target;"),
    ] {
        let (bytes, at) = fixture_with_call_type(call_type, args, &body);
        fs::write(&input, bytes).unwrap();
        let result = source(&input, &at, &[]);
        success(&result);
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(text.contains(expected), "{text}");
        assert!(text.contains("(p0)"), "{text}");
    }
    let (bytes, at) = fixture_with_call_type(8, 2, &[0x45, 0, 1, 0x60, 0, 0x64]);
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("return __ark_f_"),
        "function values use their emitted name"
    );
    let (bytes, at) = fixture_with_call_type(11, 2, &[0x65]);
    fs::write(&input, bytes).unwrap();
    failure(source(&input, &at, &[]), 1, "INVALID_INPUT");
}

#[test]
fn ark_resolves_method_operands_with_the_encoded_index_header() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("regions.abc");
    for (flags, expected_text) in [(8, "func_main_0"), (0x10008, "你好")] {
        let (mut bytes, at, record) = fixture_with_kind(flags, 0, 3, &[0x3e, 0, 0, 0x64], &[]);
        let region_start = u32::from_le_bytes(bytes[68..72].try_into().unwrap());
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
        let table = bytes.len() as u32;
        let classes = table + 80;
        let entities = classes + 12;
        let length = entities + 4;
        for word in [
            region_start,
            record,
            4,
            108,
            3,
            124,
            u32::MAX,
            u32::MAX,
            u32::MAX,
            u32::MAX,
            record,
            length,
            3,
            classes,
            1,
            entities,
            u32::MAX,
            u32::MAX,
            u32::MAX,
            u32::MAX,
            2,
            6,
            record,
            expected()["text_offset"].as_u64().unwrap() as u32,
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes[52..56].copy_from_slice(&2u32.to_le_bytes());
        bytes[56..60].copy_from_slice(&table.to_le_bytes());
        bytes[16..20].copy_from_slice(&length.to_le_bytes());
        checksum(&mut bytes);
        fs::write(&input, bytes).unwrap();
        let result = source(&input, &at, &[]);
        success(&result);
        assert!(
            String::from_utf8(result.stdout)
                .unwrap()
                .contains(expected_text)
        );
    }
    let (bytes, at, _) = fixture_with_kind(0x20008, 0, 3, &[0x65], &[]);
    fs::write(&input, bytes).unwrap();
    failure(source(&input, &at, &[]), 1, "INVALID_INPUT");
}

#[test]
fn ark_hap_selection_matches_bare_abc_and_rejects_ambiguous_or_unsafe_entries() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("minimal.abc");
    let single = directory.path().join("single.hap");
    let multiple = directory.path().join("multiple.hap");
    fs::write(&input, ABC).unwrap();
    hap(&single, &[("module.json", b"{}"), ("ets/modules.abc", ABC)]);
    let expected = expected();
    let record_at = expected["record"]["at"].as_str().unwrap();
    for (command, flags) in [("record", vec![]), ("method", vec!["--at", record_at])] {
        let bare = query(&input, command, &flags);
        success(&bare);
        let packaged = query(&single, command, &flags);
        success(&packaged);
        assert_eq!(bare.stdout, packaged.stdout);
    }
    let mut alternate = ABC.to_vec();
    let at = alternate
        .windows("法".len())
        .position(|bytes| bytes == "法".as_bytes())
        .unwrap();
    alternate[at..at + 3].copy_from_slice("减".as_bytes());
    checksum(&mut alternate);
    hap(
        &multiple,
        &[("ets/modules.abc", ABC), ("ets/alternate.abc", &alternate)],
    );
    failure(query(&multiple, "record", &[]), 2, "AMBIGUOUS");
    let selected = query(
        &multiple,
        "method",
        &["--at", record_at, "--entry", "ets/alternate.abc"],
    );
    success(&selected);
    assert!(String::from_utf8(selected.stdout).unwrap().contains("加减"));
    failure(
        query(&multiple, "record", &["--entry", "missing.abc"]),
        2,
        "NOT_FOUND",
    );
    failure(
        query(&input, "record", &["--entry", "ets/modules.abc"]),
        2,
        "INVALID_QUERY",
    );
    let duplicate = directory.path().join("duplicate.hap");
    hap(
        &duplicate,
        &[("ets/modules.abc", ABC), ("ets/modules.abc", ABC)],
    );
    failure(
        query(&duplicate, "record", &["--entry", "ets/modules.abc"]),
        2,
        "AMBIGUOUS",
    );
    let unsafe_entry = directory.path().join("unsafe.hap");
    hap(&unsafe_entry, &[("../escape.abc", ABC)]);
    failure(query(&unsafe_entry, "record", &[]), 1, "INVALID_INPUT");
    assert!(!directory.path().join("escape.abc").exists());
    let mut damaged = fs::read(&single).unwrap();
    let at = damaged
        .windows(4)
        .position(|bytes| bytes == b"PK\x01\x02")
        .unwrap();
    // The first entry is metadata; damage the selected ABC central-directory CRC.
    let next = damaged[at + 4..]
        .windows(4)
        .position(|bytes| bytes == b"PK\x01\x02")
        .unwrap()
        + at
        + 4;
    damaged[next + 16] ^= 1;
    let corrupt = directory.path().join("crc.hap");
    fs::write(&corrupt, damaged).unwrap();
    failure(query(&corrupt, "record", &[]), 1, "INVALID_INPUT");
}

#[test]
fn ark_raw_fetch_validates_all_instruction_bytes_references_and_exception_boundaries() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("input.abc");
    let packaged = directory.path().join("input.hap");
    fs::write(&input, ABC).unwrap();
    hap(&packaged, &[("ets/modules.abc", ABC)]);
    let expected = expected();
    let record_at = expected["record"]["at"].as_str().unwrap();
    let method = &expected["record"]["methods"][1];
    let method_at = method["at"].as_str().unwrap();
    for at in [record_at, method_at] {
        let bare = fetch(&input, at, &[]);
        let hap = fetch(&packaged, at, &[]);
        success(&bare);
        success(&hap);
        assert_eq!(bare.stdout, hap.stdout);
        assert!(
            String::from_utf8(bare.stdout)
                .unwrap()
                .contains("0x0\tldai 7\n0x5\treturn\n")
        );
    }
    let code = method["code_offset"].as_u64().unwrap() as usize;
    let start = code + 4;
    let mut string = ABC.to_vec();
    string[start..start + 6].copy_from_slice(&[0x3e, 2, 0, 0x65, 0x65, 0x64]);
    checksum(&mut string);
    fs::write(&input, string).unwrap();
    let result = fetch(&input, method_at, &[]);
    success(&result);
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("你好\\u0000😀")
    );
    let output = directory.path().join("out");
    fs::write(&output, b"sentinel").unwrap();
    for body in [
        [0xff, 0, 0, 0, 0, 0x64],
        [0x63, 0, 0, 0, 0, 0x64],
        [0x60, 255, 0, 0, 0, 0x64],
        [0x4d, 1, 0, 0, 0, 0x64],
        [0x3e, 3, 0, 0x65, 0x65, 0x64],
        [0x65, 0x65, 0x65, 0x65, 0x65, 0xfe],
    ] {
        let mut damaged = ABC.to_vec();
        damaged[start..start + 6].copy_from_slice(&body);
        checksum(&mut damaged);
        fs::write(&input, damaged).unwrap();
        failure(
            fetch(&input, method_at, &["-o", output.to_str().unwrap()]),
            1,
            "INVALID_INPUT",
        );
        assert_eq!(fs::read(&output).unwrap(), b"sentinel");
    }
    let mut damaged = ABC.to_vec();
    damaged[code + 3] = 1;
    damaged.extend_from_slice(&[0, 5, 1, 0, 1, 0]);
    let length = (damaged.len() as u32).to_le_bytes();
    damaged[16..20].copy_from_slice(&length);
    damaged[72..76].copy_from_slice(&length);
    checksum(&mut damaged);
    fs::write(&input, damaged).unwrap();
    failure(
        fetch(&input, method_at, &["-o", output.to_str().unwrap()]),
        1,
        "INVALID_INPUT",
    );
    assert_eq!(fs::read(&output).unwrap(), b"sentinel");
}

#[test]
fn ark_reconstructs_expressions_if_else_and_loops_without_executing_bytecode() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("input.abc");
    let hap_path = directory.path().join("input.hap");
    let (bytes, at) = fixture_with_body(
        0,
        4,
        &[
            0x60, 3, 0x4f, 9, 0x62, 1, 0, 0, 0, 0x4d, 7, 0x62, 2, 0, 0, 0, 0x64,
        ],
    );
    fs::write(&input, &bytes).unwrap();
    hap(&hap_path, &[("ets/modules.abc", &bytes)]);
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout.clone()).unwrap();
    assert!(text.contains("\"use strict\";"), "{text}");
    assert!(text.contains("p0 === false || p0 === 0"), "{text}");
    assert!(text.contains("? 2 : 1;"), "{text}");
    assert!(text.contains("return acc;"), "{text}");
    let packed = source(&hap_path, &at, &[]);
    success(&packed);
    assert_eq!(result.stdout, packed.stdout);

    let (bytes, at) = fixture_with_body(
        1,
        4,
        &[
            0x60, 4, 0x61, 0, 0x60, 0, 0x4f, 10, 0x60, 0, 0x22, 0, 0x61, 0, 0x4d, 246, 0x60, 0,
            0x64,
        ],
    );
    fs::write(&input, &bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(
        text.contains("while (true) {") && text.contains("break __ark_loop_"),
        "{text}"
    );
    assert!(text.contains("return --__ark_u;"), "{text}");

    let (bytes, at) = fixture_with_body(0, 5, &[0x60, 3, 0x42, 0, 2, 0, 0x2e, 0, 3, 4, 0x64]);
    fs::write(&input, &bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("你好\\0😀"), "{text}");
    assert!(
        text.contains("return Reflect[\"apply\"](t1, p0, [p1]);"),
        "{text}"
    );

    let (bytes, at) = fixture_with_body(0, 3, &[0xae, 0x64]);
    fs::write(&input, &bytes).unwrap();
    let output = directory.path().join("source.js");
    fs::write(&output, b"sentinel").unwrap();
    failure(
        source(&input, &at, &["-o", output.to_str().unwrap()]),
        1,
        "DECOMPILE_FAILED",
    );
    assert_eq!(fs::read(output).unwrap(), b"sentinel");
}

#[test]
fn ark_reconstructs_property_definition_index_access_and_rest_arguments() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("input.abc");
    let (bytes, at) = fixture_with_body(
        0,
        4,
        &[
            0x62, 7, 0, 0, 0, 0xdc, 0, 0, 0, 3, 0x60, 3, 0x3a, 0, 42, 0, 0x64,
        ],
    );
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(
        text.contains("Object[\"defineProperty\"](p0, \"func_main_0\",")
            && text.contains("[\"value\"]: 7"),
        "{text}"
    );
    assert!(text.contains("p0[42]"), "{text}");

    let (bytes, at) = fixture_with_body(0, 5, &[0xfb, 1, 0, 4, 3, 0xcf, 1, 0x64]);
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(
        text.contains("Object[\"defineProperty\"](p0, p1,")
            && text.contains("[\"enumerable\"]: true"),
        "{text}"
    );
    assert!(
        text.contains("return Array[\"prototype\"][\"slice\"][\"call\"](arguments, 1);"),
        "{text}"
    );
}

#[test]
fn ark_structures_crossing_forward_branches_without_losing_paths() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("branches.abc");
    let (bytes, at) = fixture_with_body(
        0,
        5,
        &[
            0x60, 3, 0x4f, 8, 0x60, 4, 0x51, 10, 0x60, 3, 0x62, 0, 0, 0, 0, 0x64, 0x62, 1, 0, 0, 0,
            0x64,
        ],
    );
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("__ark_pc_"), "{text}");
    assert!(text.contains("continue __ark_flow_"), "{text}");
    assert!(
        text.contains("return 0;") && text.contains("return 1;"),
        "{text}"
    );
}

#[test]
fn ark_distinguishes_own_literal_properties_from_unknown_receivers() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("own.abc");
    let (bytes, at) = fixture_with_body(
        1,
        3,
        &[
            0x05, 0, 0x61, 0, 0x62, 7, 0, 0, 0, 0x79, 0, 0, 2, 0, 0x60, 0, 0x64,
        ],
    );
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("= [];"), "{text}");
    assert!(
        text.contains("Object[\"defineProperty\"](") && text.contains("[\"enumerable\"]: true"),
        "{text}"
    );

    let (bytes, at) = fixture_with_body(0, 4, &[0x62, 7, 0, 0, 0, 0x79, 0, 3, 2, 0, 0x64]);
    fs::write(&input, bytes).unwrap();
    failure(source(&input, &at, &[]), 1, "DECOMPILE_FAILED");

    let branch = [
        0x60, 4, 0x4f, 8, 0x05, 0, 0x61, 0, 0x4d, 6, 0x60, 4, 0x61, 0, 0x62, 7, 0, 0, 0, 0x79, 0,
        0, 2, 0, 0x64,
    ];
    let (bytes, at) = fixture_with_body(1, 4, &branch);
    fs::write(&input, bytes).unwrap();
    failure(source(&input, &at, &[]), 1, "DECOMPILE_FAILED");
    let mut both_fresh = branch;
    both_fresh[10] = 0x05;
    both_fresh[11] = 0;
    let (bytes, at) = fixture_with_body(1, 4, &both_fresh);
    fs::write(&input, bytes).unwrap();
    success(&source(&input, &at, &[]));
}

#[test]
fn ark_reconstructs_nested_definitions_with_shared_lexical_cells() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("nested.abc");
    let package = directory.path().join("nested.hap");
    let (bytes, parent, child) = fixture_with_nested(
        &[0x09, 1, 0x62, 7, 0, 0, 0, 0x3d, 0, 0x33, 0, 0, 0, 1, 0x64],
        &[0x3c, 0, 0x64],
    );
    fs::write(&input, &bytes).unwrap();
    hap(&package, &[("ets/modules.abc", &bytes)]);
    let result = source(&input, &parent, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout.clone()).unwrap();
    let target = child.trim_start_matches("method:0x");
    assert!(text.contains("__ark_env[\"slots\"][0] = 7;"), "{text}");
    assert!(
        text.contains(&format!("_ark_capture_{target}({{")),
        "{text}"
    );
    assert!(
        text.contains("Object[\"defineProperty\"](")
            && text.contains("\"length\", { [\"value\"]: 1 }"),
        "{text}"
    );
    assert!(
        text.contains(&format!("function _ark_capture_{target}(__ark_context)")),
        "{text}"
    );
    assert!(text.contains("return __ark_env[\"slots\"][0];"), "{text}");
    let packed = source(&package, &parent, &[]);
    success(&packed);
    assert_eq!(result.stdout, packed.stdout);
    let direct = source(&input, &child, &[]);
    success(&direct);
    let direct = String::from_utf8(direct.stdout).unwrap();
    assert!(
        !direct.contains(parent.trim_start_matches("method:0x")),
        "{direct}"
    );

    let (bytes, parent, _) = fixture_with_nested(&[0x33, 0, 0, 0, 0, 0x64], &[0xad, 0x64]);
    fs::write(&input, bytes).unwrap();
    failure(source(&input, &parent, &[]), 1, "DECOMPILE_FAILED");

    let (bytes, parent, child) =
        fixture_with_nested(&[0x33, 0, 0, 0, 0, 0x64], &[0x33, 0, 1, 0, 0, 0x64]);
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &parent, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    for method in [parent, child] {
        assert_eq!(
            text.matches(&format!(
                "function _ark_capture_{}(",
                method.trim_start_matches("method:0x")
            ))
            .count(),
            1,
            "{text}"
        );
    }
}

#[test]
fn ark_method_reconstruction_preserves_lexical_cells_and_hole_checks() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("lexical.abc");
    let (bytes, at) = fixture_with_body(
        0,
        3,
        &[
            0x09, 1, 0x62, 7, 0, 0, 0, 0x3d, 0, 0x3c, 0, 0xfe, 9, 0, 0, 0x69, 0x64,
        ],
    );
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(
        text.contains("function _ark_capture_") && text.contains("(__ark_context)"),
        "{text}"
    );
    assert!(
        text.contains("__ark_env = __ark_context[\"env\"]"),
        "{text}"
    );
    assert!(
        text.contains("[\"slots\"]: Array(1)[\"fill\"](__ark_context[\"hole\"])"),
        "{text}"
    );
    assert!(text.contains("__ark_env[\"slots\"][0] = 7;"), "{text}");
    assert!(
        text.contains("=== __ark_context[\"hole\"]")
            && text.contains("throw new ReferenceError(\"func_main_0\" + \" is not initialized\")"),
        "{text}"
    );
    assert!(
        text.contains("__ark_env = __ark_env[\"parent\"];"),
        "{text}"
    );

    let (bytes, at) = fixture_with_body(0, 3, &[0x3c, 0x21, 0x64]);
    fs::write(&input, bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(
        text.contains("__ark_env[\"parent\"][\"slots\"][2]"),
        "{text}"
    );
}

#[test]
fn ark_method_reconstruction_resolves_its_module_import_and_local_binding() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("module.abc");
    let hap_path = directory.path().join("module.hap");
    let fixture = include_bytes!("../corpus/abc/module.abc");
    let expected: Value = serde_json::from_str(include_str!("../corpus/abc/module.json")).unwrap();
    let method = expected["method"].as_str().unwrap();
    fs::write(&input, fixture).unwrap();
    hap(&hap_path, &[("ets/modules.abc", fixture)]);
    let output = source(&input, method, &[]);
    success(&output);
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(
        text.starts_with("import __ark_i_0 from \"@example/logger\";\n"),
        "{text}"
    );

    assert!(
        text.contains("__ark_context[\"locals\"][\"counter\"] = t1;"),
        "{text}"
    );
    assert!(!text.contains("throw.undefinedifholewithname"), "{text}");
    let packed = source(&hap_path, method, &[]);
    success(&packed);
    assert_eq!(output.stdout, packed.stdout);

    let mut damaged = fixture.to_vec();
    let start = damaged
        .windows(6)
        .position(|part| part == [0x7e, 0, 0xfe, 9, 0, 0])
        .unwrap();
    damaged[start + 1] = 1;
    checksum(&mut damaged);
    fs::write(&input, damaged).unwrap();
    failure(source(&input, method, &[]), 1, "INVALID_INPUT");
}

#[test]
fn ark_reconstructs_a_proven_async_resolve_and_reject_wrapper() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("async.abc");
    let body = [
        0xae, 0x61, 0, 0x62, 7, 0, 0, 0, 0xcd, 0, 0x64, 0x61, 1, 0xce, 0, 0x64,
    ];
    let (bytes, at, _) = fixture_with_kind(0x408, 2, 3, &body, &[3, 8, 1, 0, 11, 5]);
    fs::write(&input, &bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("= async function __ark_f_"), "{text}");
    assert!(text.contains("return 7;"), "{text}");
    assert!(!text.contains("asyncfunction"), "{text}");

    let (bytes, at, _) = fixture_with_kind(
        0x408,
        2,
        3,
        &[
            0xae, 0x61, 0, 0x4d, 9, 0x62, 7, 0, 0, 0, 0xcd, 0, 0x64, 0x61, 1, 0xce, 0, 0x64,
        ],
        &[3, 10, 1, 0, 13, 5],
    );
    fs::write(&input, bytes).unwrap();
    failure(source(&input, &at, &[]), 1, "DECOMPILE_FAILED");

    // The promise cell must not escape through an implicit argument register range.
    let (bytes, at, _) = fixture_with_kind(
        0x408,
        2,
        3,
        &[
            0xae, 0x61, 1, 0x65, 0x73, 0, 2, 0, 0xcd, 1, 0x64, 0x61, 0, 0xce, 1, 0x64,
        ],
        &[3, 8, 1, 0, 11, 5],
    );
    fs::write(&input, bytes).unwrap();
    failure(source(&input, &at, &[]), 1, "DECOMPILE_FAILED");
}

#[test]
fn ark_reconstructs_a_catch_all_without_executing_the_input() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("input.abc");
    let (mut bytes, at) =
        fixture_with_body(1, 4, &[0x60, 4, 0x29, 0, 0x4d, 5, 0x61, 0, 0x64, 0x64]);
    let method = expected()["record"]["methods"][1]["offset"]
        .as_u64()
        .unwrap() as usize;
    let code = u32::from_le_bytes(bytes[method + 10..method + 14].try_into().unwrap()) as usize;
    bytes[code + 3] = 1;
    bytes.extend_from_slice(&[0, 4, 1, 0, 6, 3]);
    let length = (bytes.len() as u32).to_le_bytes();
    bytes[16..20].copy_from_slice(&length);
    bytes[72..76].copy_from_slice(&length);
    checksum(&mut bytes);
    fs::write(&input, &bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(
        text.contains("try {") && text.contains("return acc;"),
        "{text}"
    );
    assert!(text.contains("(0, p0)()"), "{text}");
    assert!(text.contains("} catch (__ark_exception_0) {"), "{text}");
    assert!(text.contains("acc = __ark_exception_0;"), "{text}");

    // A typed catch must fail rather than become an unconditional catch.
    let type_byte = bytes.len() - 3;
    bytes[type_byte] = 1;
    checksum(&mut bytes);
    fs::write(&input, &bytes).unwrap();
    failure(source(&input, &at, &[]), 1, "DECOMPILE_FAILED");

    let (mut bytes, at) = fixture_with_body(
        1,
        4,
        &[
            0x60, 4, 0x29, 0, 0x4d, 5, 0x61, 0, 0x64, 0x60, 4, 0x29, 0, 0x4d, 5, 0x61, 0, 0x64,
            0x64,
        ],
    );
    let code = u32::from_le_bytes(bytes[method + 10..method + 14].try_into().unwrap()) as usize;
    bytes[code + 3] = 2;
    bytes.extend_from_slice(&[0, 4, 1, 0, 6, 3, 9, 4, 1, 0, 15, 3]);
    let length = (bytes.len() as u32).to_le_bytes();
    bytes[16..20].copy_from_slice(&length);
    bytes[72..76].copy_from_slice(&length);
    checksum(&mut bytes);
    fs::write(&input, &bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    assert_eq!(
        String::from_utf8(result.stdout)
            .unwrap()
            .matches("try {")
            .count(),
        2
    );
}

#[test]
fn ark_reconstructs_exact_global_bindings_and_scalar_literal_buffers() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("input.abc");
    let (bytes, at) = fixture_with_body(0, 3, &[0x3f, 0, 0, 0, 0x64]);
    fs::write(&input, &bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("return func_main_0;")
    );

    let (mut bytes, at) = fixture_with_body(0, 3, &[0x3f, 0, 0, 0, 0x64]);
    append_entity(&mut bytes, 0, b"\x07acc\0");
    fs::write(&input, &bytes).unwrap();
    failure(source(&input, &at, &[]), 1, "DECOMPILE_FAILED");

    let (mut bytes, at) = fixture_with_body(0, 3, &[0x07, 0, 0, 0, 0x64]);
    let expected = expected();
    let method = expected["record"]["methods"][1]["offset"].as_u64().unwrap() as usize;
    let name = u32::from_le_bytes(bytes[method + 4..method + 8].try_into().unwrap());
    let mut object = 4u32.to_le_bytes().to_vec();
    object.push(5);
    object.extend_from_slice(&name.to_le_bytes());
    object.push(2);
    object.extend_from_slice(&(-7i32).to_le_bytes());
    append_entity(&mut bytes, 0, &object);
    fs::write(&input, &bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("[\"加法😀\"]: -7")
    );

    let (mut bytes, at) = fixture_with_body(0, 3, &[0x06, 0, 0, 0, 0x64]);
    let mut array = 6u32.to_le_bytes().to_vec();
    array.extend_from_slice(&[1, 1, 4]);
    array.extend_from_slice(&(-0.0f64).to_bits().to_le_bytes());
    array.extend_from_slice(&[0xff, 0]);
    append_entity(&mut bytes, 0, &array);
    fs::write(&input, &bytes).unwrap();
    let result = source(&input, &at, &[]);
    success(&result);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(
        text.contains("true") && text.contains("-0") && text.contains("null"),
        "{text}"
    );

    let (mut bytes, at) = fixture_with_body(0, 3, &[0x06, 0, 0, 0, 0x64]);
    let mut unsupported = 2u32.to_le_bytes().to_vec();
    unsupported.extend_from_slice(&[8, 0]);
    append_entity(&mut bytes, 0, &unsupported);
    fs::write(&input, &bytes).unwrap();
    failure(source(&input, &at, &[]), 1, "DECOMPILE_FAILED");
}

#[test]
fn ark_refuses_corrupt_metadata_atomically_and_preserves_utf16_units() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("input.abc");
    let output = directory.path().join("output.tsv");
    fs::write(&output, b"sentinel").unwrap();
    let expected = expected();
    let record_at = expected["record"]["at"].as_str().unwrap();
    let mut bad_checksum = ABC.to_vec();
    *bad_checksum.last_mut().unwrap() ^= 1;
    let mut bad_version = ABC.to_vec();
    bad_version[12] = 13;
    checksum(&mut bad_version);
    let mut bad_offset = ABC.to_vec();
    bad_offset[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
    checksum(&mut bad_offset);
    for (bytes, code) in [
        (ABC[..59].to_vec(), "INVALID_INPUT"),
        (bad_checksum, "INVALID_INPUT"),
        (bad_version, "UNSUPPORTED_INPUT"),
        (bad_offset, "INVALID_INPUT"),
    ] {
        fs::write(&input, bytes).unwrap();
        failure(
            query(&input, "record", &["-o", output.to_str().unwrap()]),
            1,
            code,
        );
        assert_eq!(fs::read(&output).unwrap(), b"sentinel");
    }
    let mut surrogate = ABC.to_vec();
    let at = surrogate
        .windows(3)
        .position(|bytes| bytes == b"\xed\xb8\x80")
        .unwrap();
    surrogate[at..at + 3].copy_from_slice(b"\xed\xa0\xbd");
    checksum(&mut surrogate);
    fs::write(&input, surrogate).unwrap();
    let result = query(&input, "method", &["--at", record_at]);
    success(&result);
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("\"\\u52a0\\u6cd5\\ud83d\\ud83d\"")
    );
    failure(
        query(&input, "record", &["-o", input.to_str().unwrap()]),
        2,
        "INVALID_QUERY",
    );
}
