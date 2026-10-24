use super::*;

#[test]
fn pyc_reconstructs_dataclasses_guarded_capture_cases_across_final_versions() {
    for version in ["3.11.11", "3.12.14", "3.13.2"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/pyc/matrix")
            .join(version)
            .join("dataclasses.timestamp.pyc");
        let original = fs::read(&path).unwrap();
        let output = run(&path, &["--json"]);
        success(&output);
        let row: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_envelope(&row);
        assert_eq!(row["detail"]["complete"], true, "{version}");
        let source = row["detail"]["source"].as_str().unwrap();
        let slots = source
            .split("def _get_slots(cls):")
            .nth(1)
            .unwrap()
            .split("\ndef _add_slots(")
            .next()
            .unwrap();
        assert!(
            slots.contains("case iterable if not hasattr(iterable, '__next__'):"),
            "{version}: {slots}"
        );
        assert!(slots.contains("yield from iterable"), "{version}: {slots}");
        assert!(!slots.contains("iterable = None"), "{version}: {slots}");
        assert!(!source.contains("WARNING: Decompyle incomplete"));
        let raw = run(&path, &[]);
        success(&raw);
        assert_eq!(raw.stdout, source.as_bytes());
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("dataclasses.py");
        fs::write(&out, b"sentinel").unwrap();
        let saved = run(&path, &["-o", out.to_str().unwrap()]);
        success(&saved);
        assert!(saved.stdout.is_empty());
        assert_eq!(fs::read(out).unwrap(), raw.stdout);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

// Handwritten CPython 3.12 marshal layout; never executes or compiles input code.
fn raw_module(code: &[u8], constant: &[u8]) -> Vec<u8> {
    fn bytes(out: &mut Vec<u8>, value: &[u8]) {
        out.push(b's');
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(value);
    }
    let mut out = vec![b'c'];
    for value in [0_u32, 0, 0, 2, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    bytes(&mut out, code);
    out.extend_from_slice(b")\x02N");
    out.extend_from_slice(constant);
    out.extend_from_slice(b")\x01z\x01x)\x00");
    bytes(&mut out, b"");
    for text in [b"fixture.py".as_slice(), b"<module>", b"<module>"] {
        out.extend_from_slice(&[b'z', text.len() as u8]);
        out.extend_from_slice(text);
    }
    out.extend_from_slice(&1_u32.to_le_bytes());
    bytes(&mut out, b"");
    bytes(&mut out, b"");
    out
}

#[test]
fn pyc_incomplete_reconstruction_is_explicit_and_preserves_output() {
    for opcode in [17, 255] {
        let bytes = raw_module(&[opcode, 0, 121, 0], b"N");
        let (dir, path) = fixture(&bytes, "unsupported.marshal");
        let dest = dir.path().join("keep.py");
        fs::write(&dest, b"keep").unwrap();
        failure(
            run(&path, &["--version", "3.12", "-o", dest.to_str().unwrap()]),
            1,
            "INCOMPLETE_DECOMPILATION",
        );
        assert_eq!(fs::read(&dest).unwrap(), b"keep");
        let output = run(&path, &["--version", "3.12", "--json"]);
        success(&output);
        let row: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_envelope(&row);
        assert_eq!(row["detail"]["complete"], false);
        assert!(
            row["detail"]["source"]
                .as_str()
                .unwrap()
                .contains("UNIMPLEMENTED")
        );
        assert_eq!(row["source_id"], rxd_core::identity::input_id(&bytes));
    }
}

#[test]
fn pyc_propagates_incomplete_nested_lambda_reconstruction() {
    let mut nested = raw_module(&[255, 0, 100, 0, 83, 0], b"N");
    let marker = b"z\x08<module>";
    let replacement = b"z\x08<lambda>";
    for index in 0..nested.len().saturating_sub(marker.len()) {
        if &nested[index..index + marker.len()] == marker {
            nested[index..index + marker.len()].copy_from_slice(replacement);
        }
    }
    let (_dir, path) = fixture(
        raw_module(&[151, 0, 100, 1, 132, 0, 90, 0, 121, 0], &nested),
        "lambda.marshal",
    );
    let output = run(&path, &["--version", "3.12", "--json"]);
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["detail"]["complete"], false);
    failure(
        run(&path, &["--version", "3.12"]),
        1,
        "INCOMPLETE_DECOMPILATION",
    );
}

#[test]
fn pyc_enforces_exact_raw_and_json_output_budgets_with_perf_stages() {
    let mut constant = vec![b'u'];
    let text = "世界\u{1}".repeat(50);
    constant.extend_from_slice(&(text.len() as u32).to_le_bytes());
    constant.extend_from_slice(text.as_bytes());
    let (_dir, path) = fixture(
        raw_module(&[151, 0, 100, 1, 90, 0, 121, 0], &constant),
        "unicode.marshal",
    );
    let raw = run(&path, &["--version", "3.12"]);
    success(&raw);
    assert!(String::from_utf8_lossy(&raw.stdout).contains("世界\\x01"));
    let json = run(&path, &["--version", "3.12", "--json", "--perf"]);
    success(&json);
    let row: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_envelope(&row);
    assert_eq!(
        row["detail"]["source"].as_str().unwrap().as_bytes(),
        raw.stdout
    );
    // Output volume is not a ceiling: the library always answers completely.
    let (_, result) = decompile_library(&path, Some((3, 12)), shared_decompile_limits());
    assert_eq!(result.unwrap().source.as_bytes(), raw.stdout);
    assert_no_budget_flags(&path, "decompile");
    let records: Vec<Value> = String::from_utf8(json.stderr)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    let analysis = records.iter().find(|r| r["scope"] == "analysis").unwrap();
    for stage in [
        "read",
        "hash",
        "parse",
        "decompile",
        "generate",
        "serialize",
        "write",
    ] {
        assert!(analysis["stages_ns"][stage].is_u64(), "missing {stage}");
    }
    assert_eq!(analysis["source_id"], row["source_id"]);
}

#[test]
fn pyc_bounds_marshal_depth_reference_expansion_and_integer_conversion() {
    let mut depth = b")\x01".repeat(129);
    depth.push(b'N');
    let mut expanded = vec![b')', 20, b')' | 0x80, 0];
    for index in 0_u32..19 {
        expanded.extend_from_slice(&[b')' | 0x80, 2]);
        for _ in 0..2 {
            expanded.push(b'r');
            expanded.extend_from_slice(&index.to_le_bytes());
        }
    }
    let mut long = vec![b'l'];
    long.extend_from_slice(&5000_i32.to_le_bytes());
    long.extend_from_slice(&[0xff, 0x7f].repeat(5000));
    for (name, bytes) in [("depth", depth), ("shared", expanded), ("long", long)] {
        let (_dir, path) = fixture(bytes, name);
        failure(run(&path, &["--version", "3.12"]), 1, "RESOURCE_LIMIT");
    }
    let (_dir, path) = fixture([b')' | 0x80, 1, b'r', 0, 0, 0, 0], "cycle");
    failure(run(&path, &["--version", "3.12"]), 1, "INVALID_INPUT");
    failure(
        run(&input("hello.3.12.pyc"), &["--version", "9.9"]),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn pyc_rejects_non_regular_input() {
    let dir = tempfile::tempdir().unwrap();
    failure(run(dir.path(), &[]), 1, "INVALID_INPUT");
    #[cfg(unix)]
    {
        let path = dir.path().join("fifo");
        let path_c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path_c.as_ptr(), 0o600) }, 0);
        failure(run(&path, &[]), 1, "INVALID_INPUT");
    }
}

#[test]
fn pyc_rejects_trailing_marshal_bytes_without_replacing_output() {
    let raw = raw_module(&[151, 0, 121, 0], b"N");
    let pyc = fs::read(input("hello.3.12.pyc")).unwrap();
    for (name, original, args) in [
        ("raw", raw, vec!["--version", "3.12"]),
        ("pyc", pyc, vec![]),
    ] {
        for suffix in [b"\0".as_slice(), b"garbage", b"N"] {
            let mut bytes = original.clone();
            bytes.extend_from_slice(suffix);
            let (dir, path) = fixture(&bytes, name);
            let dest = dir.path().join("keep.py");
            fs::write(&dest, b"keep").unwrap();
            for json in [false, true] {
                let mut query = args.clone();
                if json {
                    query.push("--json");
                }
                query.extend(["-o", dest.to_str().unwrap()]);
                failure(run(&path, &query), 1, "INVALID_INPUT");
                assert_eq!(fs::read(&dest).unwrap(), b"keep");
                assert_eq!(fs::read(&path).unwrap(), bytes);
            }
        }
    }
}

#[test]
fn pyc_rejects_truncated_wordcode_without_partial_source() {
    let bytes = raw_module(&[100], b"N");
    let (dir, path) = fixture(&bytes, "truncated.marshal");
    let dest = dir.path().join("keep.py");
    fs::write(&dest, b"keep").unwrap();
    for json in [false, true] {
        let mut args = vec!["--version", "3.12", "-o", dest.to_str().unwrap()];
        if json {
            args.push("--json");
        }
        failure(run(&path, &args), 1, "INVALID_INPUT");
        assert_eq!(fs::read(&dest).unwrap(), b"keep");
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

fn raw_legacy_module(code: &[u8]) -> Vec<u8> {
    let pyc = fs::read(input("simple_const.2.7.pyc")).unwrap();
    let mut raw = pyc[8..].to_vec();
    // CPython 2.7: tag, four 32-bit fields, byte-string tag/length, co_code.
    assert_eq!(raw[0], b'c');
    assert_eq!(raw[17], b's');
    let old_size = u32::from_le_bytes(raw[18..22].try_into().unwrap()) as usize;
    raw.splice(22..22 + old_size, code.iter().copied());
    raw[18..22].copy_from_slice(&(code.len() as u32).to_le_bytes());
    raw
}

#[test]
fn pyc_rejects_unterminated_and_overflowing_extended_args_and_short_caches() {
    let cases: &[(&str, &str, &[u8])] = &[
        ("word-prefix", "3.12", &[144, 1]),
        (
            "word-overflow",
            "3.12",
            &[144, 255, 144, 255, 144, 255, 144, 255, 100, 0],
        ),
        (
            "word-too-many",
            "3.12",
            &[144, 0, 144, 0, 144, 0, 144, 0, 100, 0],
        ),
        ("word-no-arg", "3.12", &[144, 1, 9, 0]),
        ("cache-empty", "3.12", &[116, 0]),
        ("cache-short", "3.12", &[116, 0, 0, 0, 0, 0, 0, 0]),
        ("legacy-low-byte", "2.7", &[100, 0]),
        ("legacy-prefix", "2.7", &[145, 1, 0]),
        (
            "legacy-overflow",
            "2.7",
            &[145, 255, 255, 145, 1, 0, 100, 0, 0],
        ),
    ];
    for &(name, version, code) in cases {
        let bytes = if version == "2.7" {
            raw_legacy_module(code)
        } else {
            raw_module(code, b"N")
        };
        let (dir, path) = fixture(&bytes, name);
        let dest = dir.path().join("keep.py");
        fs::write(&dest, b"keep").unwrap();
        for json in [false, true] {
            let mut args = vec!["--version", version, "-o", dest.to_str().unwrap()];
            if json {
                args.push("--json");
            }
            failure(run(&path, &args), 1, "INVALID_INPUT");
            assert_eq!(fs::read(&dest).unwrap(), b"keep");
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }
}

#[test]
fn pyc_accepts_adjacent_operand_cache_and_shared_reference_boundaries() {
    let mut shared = vec![b')', 3, b')' | 0x80, 0];
    for _ in 0..2 {
        shared.extend_from_slice(&[b'r', 0, 0, 0, 0]);
    }
    let mut word = vec![151, 0];
    word.extend_from_slice(&[144, 0].repeat(3));
    word.extend_from_slice(&[100, 1, 90, 0, 121, 0]);
    let cases = [
        (raw_module(&word, &shared), "3.12", "x = ((), (), ())"),
        (
            raw_module(
                &[151, 0, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 121, 0],
                b"N",
            ),
            "3.12",
            "x",
        ),
        (
            raw_legacy_module(&[145, 0, 0, 100, 0, 0, 90, 0, 0, 100, 7, 0, 83]),
            "2.7",
            "a = 42",
        ),
    ];
    for (bytes, version, expected) in cases {
        let (_dir, path) = fixture(bytes, "boundary.marshal");
        let output = run(&path, &["--version", version, "--json"]);
        success(&output);
        let row: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_envelope(&row);
        assert_eq!(row["detail"]["complete"], true);
        assert!(row["detail"]["source"].as_str().unwrap().contains(expected));
    }
}

fn named_module(code: &[u8], name: &str) -> Vec<u8> {
    let mut raw = raw_module(code, b"N");
    let marker = b"z\x08<module>";
    let mut replacement = vec![b'z', name.len() as u8];
    replacement.extend_from_slice(name.as_bytes());
    let mut start = 0;
    while let Some(index) = raw[start..].windows(marker.len()).position(|w| w == marker) {
        let index = start + index;
        raw.splice(index..index + marker.len(), replacement.iter().copied());
        start = index + replacement.len();
    }
    raw
}

#[test]
fn pyc_propagates_malformed_nested_functions_lambdas_classes_and_wrappers() {
    for name in [
        "f",
        "<lambda>",
        "C",
        "<generic parameters of X>",
        "<genexpr>",
    ] {
        let parent_code: &[u8] = match name {
            "C" => &[
                151, 0, 2, 0, 71, 0, 100, 1, 132, 0, 100, 2, 171, 2, 0, 0, 0, 0, 0, 0, 90, 0, 121,
                0,
            ],
            "<generic parameters of X>" => &[
                151, 0, 2, 0, 100, 1, 132, 0, 171, 0, 0, 0, 0, 0, 0, 0, 90, 0, 121, 0,
            ],
            "<genexpr>" => &[
                151, 0, 2, 0, 100, 1, 132, 0, 100, 0, 171, 1, 0, 0, 0, 0, 0, 0, 90, 0, 121, 0,
            ],
            _ => &[151, 0, 100, 1, 132, 0, 90, 0, 121, 0],
        };
        for code in [b"\x64".as_slice(), b"\x90\0", b"\x74\0"] {
            let nested = named_module(code, name);
            let mut bytes = raw_module(parent_code, &nested);
            if name == "C" {
                let consts = 26 + parent_code.len();
                bytes[consts + 1] = 3;
                bytes.splice(
                    consts + 3 + nested.len()..consts + 3 + nested.len(),
                    *b"z\x01C",
                );
            }
            let (dir, path) = fixture(&bytes, "nested.marshal");
            let dest = dir.path().join("keep.py");
            fs::write(&dest, b"keep").unwrap();
            for json in [false, true] {
                let mut args = vec!["--version", "3.12", "-o", dest.to_str().unwrap()];
                if json {
                    args.push("--json");
                }
                failure(run(&path, &args), 1, "INVALID_INPUT");
                assert_eq!(fs::read(&dest).unwrap(), b"keep");
                assert_eq!(fs::read(&path).unwrap(), bytes);
            }
        }
    }
}

#[test]
fn pyc_requires_exactly_one_complete_code_root_in_header_and_raw_modes() {
    let valid = raw_module(&[151, 0, 121, 0], b"N");
    let header = fs::read(input("hello.3.12.pyc")).unwrap()[..16].to_vec();
    for payload in [b"N".as_slice(), b"c", &valid[..valid.len() - 1]] {
        for raw in [false, true] {
            let mut bytes = if raw { vec![] } else { header.clone() };
            bytes.extend_from_slice(payload);
            let (_dir, path) = fixture(bytes, "root.pyc");
            let args = if raw {
                vec!["--version", "3.12", "--json"]
            } else {
                vec!["--json"]
            };
            failure(run(&path, &args), 1, "INVALID_INPUT");
        }
    }
    failure(
        run(&input("hello.3.12.pyc"), &["--version", "3.12"]),
        1,
        "INVALID_INPUT",
    );
}

#[test]
fn pyc_decompile_work_budget_is_exact_shared_and_reported() {
    let bytes = raw_module(&[151, 0, 100, 1, 90, 0, 121, 0], b"i\x2a\0\0\0");
    let (dir, path) = fixture(&bytes, "work.marshal");
    let budget = rxd_core::Budget::default();
    let base = shared_decompile_limits();
    let (perf, result) = decompile_library(&path, Some((3, 12)), base);
    let result = result.unwrap();
    assert!(result.complete);
    let perf = perf_value(&perf);
    let work = perf["counters"]["work"].as_u64().unwrap();
    assert!(work > 1);
    assert_eq!(perf["resource_limits"]["work"], Value::from(budget.work));
    assert_eq!(
        perf["resource_limits"]["ast_depth"],
        Value::from(rxd_core::Limits::default().ast_depth)
    );
    assert_no_budget_flags(&path, "decompile");
    let mut exact = base;
    exact.work = work as usize;
    let (_, result) = decompile_library(&path, Some((3, 12)), exact);
    assert!(result.is_ok());
    let mut less = exact;
    less.work = work as usize - 1;
    let (perf, result) = decompile_library(&path, Some((3, 12)), less);
    assert_eq!(result.err().unwrap().code, "RESOURCE_LIMIT");
    let perf = perf_value(&perf);
    assert!(perf["counters"]["work"].as_u64().unwrap() < work);
    assert!(perf["stages_ns"]["generate"].as_u64().unwrap() > 0);
    // A rejected input never replaces an existing output file.
    let dest = dir.path().join("keep.py");
    fs::write(&dest, b"keep").unwrap();
    let (_bad_dir, bad) = fixture(b"not a pyc", "bad.pyc");
    failure(
        run(&bad, &["-o", dest.to_str().unwrap()]),
        1,
        "INVALID_INPUT",
    );
    assert_eq!(fs::read(&dest).unwrap(), b"keep");
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn pyc_bounds_instruction_built_depth_with_shallow_marshal() {
    let mut code = vec![151, 0, 100, 1];
    code.extend_from_slice(&[11, 0].repeat(7));
    code.extend_from_slice(&[90, 0, 121, 0]);
    let bytes = raw_module(&code, b"i\x2a\0\0\0");
    let (_dir, path) = fixture(&bytes, "depth.marshal");
    assert_no_budget_flags(&path, "decompile");
    let mut exact = shared_decompile_limits();
    exact.ast_depth = 9;
    assert!(decompile_library(&path, Some((3, 12)), exact).1.is_ok());
    let mut shallow = exact;
    shallow.ast_depth = 8;
    let (perf, result) = decompile_library(&path, Some((3, 12)), shallow);
    assert_eq!(result.err().unwrap().code, "RESOURCE_LIMIT");
    let perf = perf_value(&perf);
    assert_eq!(perf["resource_limits"]["ast_depth"], 8);
    assert_eq!(perf["counters"]["ast_depth"], 8);
    // Deep instruction-built nesting is still bounded even though marshal stays shallow.
    let mut deep = vec![151, 0, 100, 1];
    deep.extend_from_slice(&[11, 0].repeat(160));
    deep.extend_from_slice(&[90, 0, 121, 0]);
    fs::write(&path, raw_module(&deep, b"N")).unwrap();
    let mut bounded = shared_decompile_limits();
    bounded.ast_depth = 128;
    assert_eq!(
        decompile_library(&path, Some((3, 12)), bounded)
            .1
            .err()
            .unwrap()
            .code,
        "RESOURCE_LIMIT"
    );
}

#[test]
fn pyc_shares_work_across_repeated_nested_code_references() {
    let mut nested = named_module(&[151, 0, 100, 1, 90, 0, 121, 0], "f");
    nested[0] |= 0x80;
    let once = &[151, 0, 100, 1, 132, 0, 90, 0, 121, 0];
    let twice = &[151, 0, 100, 1, 132, 0, 90, 0, 100, 2, 132, 0, 90, 0, 121, 0];
    let shared = |code: &[u8]| {
        let mut raw = raw_module(code, &nested);
        let consts = 26 + code.len();
        raw[consts + 1] = 3;
        raw.splice(
            consts + 3 + nested.len()..consts + 3 + nested.len(),
            [b'r', 0, 0, 0, 0],
        );
        raw
    };
    let (_dir, path) = fixture(shared(once), "shared.marshal");
    let output = run(&path, &["--version", "3.12", "--json", "--perf"]);
    success(&output);
    let once_work = perf_reports(&output)
        .into_iter()
        .find(|r| r["scope"] == "analysis")
        .unwrap()["counters"]["work"]
        .as_u64()
        .unwrap() as usize;
    fs::write(&path, shared(twice)).unwrap();
    let output = run(&path, &["--version", "3.12", "--json", "--perf"]);
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["detail"]["complete"], true);
    assert_eq!(
        row["detail"]["source"]
            .as_str()
            .unwrap()
            .matches("def x(")
            .count(),
        2
    );
    assert_no_budget_flags(&path, "decompile");
    let mut limits = shared_decompile_limits();
    limits.work = once_work;
    assert_eq!(
        decompile_library(&path, Some((3, 12)), limits)
            .1
            .err()
            .unwrap()
            .code,
        "RESOURCE_LIMIT"
    );
}

#[test]
fn pyc_rejects_operand_driven_expansion_before_allocation() {
    // Three prefixes form a legal u32 operand. BUILD_TUPLE must not reserve
    // billions of stack placeholders before consulting the work budget.
    let bytes = raw_module(
        &[
            151, 0, 144, 255, 144, 255, 144, 255, 102, 255, 90, 0, 121, 0,
        ],
        b"N",
    );
    let (_dir, path) = fixture(&bytes, "expand.marshal");
    assert_no_budget_flags(&path, "decompile");
    let mut limits = shared_decompile_limits();
    limits.work = 256;
    let (perf, result) = decompile_library(&path, Some((3, 12)), limits);
    assert_eq!(result.err().unwrap().code, "RESOURCE_LIMIT");
    let perf = perf_value(&perf);
    assert!(perf["counters"]["work"].as_u64().unwrap() <= 256);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let small = raw_module(&[151, 0, 100, 1, 102, 1, 90, 0, 121, 0], b"N");
    fs::write(&path, small).unwrap();
    let mut ok = shared_decompile_limits();
    ok.work = 4096;
    assert!(decompile_library(&path, Some((3, 12)), ok).1.is_ok());
}

#[test]
fn pyc_charges_borrowed_name_copies_before_generation() {
    let mut bytes = raw_module(&[151, 0, 137, 0, 1, 0, 121, 0], b"N");
    let marker = b")\x01z\x01x)\0s\0\0\0\0";
    let start = bytes
        .windows(marker.len())
        .position(|w| w == marker)
        .unwrap();
    let mut names = b")\x01z\x01x)\x01u".to_vec();
    names.extend_from_slice(&16_384_u32.to_le_bytes());
    names.extend_from_slice(&[b'a'; 16_384]);
    names.extend_from_slice(b"s\x01\0\0\0\x80");
    bytes.splice(start..start + marker.len(), names);
    let (_dir, path) = fixture(&bytes, "name-copy.marshal");
    assert_no_budget_flags(&path, "decompile");
    let mut limits = shared_decompile_limits();
    limits.work = 8_192;
    let (perf, result) = decompile_library(&path, Some((3, 12)), limits);
    assert_eq!(result.err().unwrap().code, "RESOURCE_LIMIT");
    assert_eq!(
        perf_value(&perf)["stages_ns"]["generate"]
            .as_u64()
            .unwrap_or(0),
        0
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let mut ok = shared_decompile_limits();
    ok.work = 200_000;
    assert!(decompile_library(&path, Some((3, 12)), ok).1.is_ok());
}

#[test]
fn pyc_charges_ast_signature_copies_before_generation() {
    let mut nested = named_module(&[151, 0, 121, 0], "f");
    nested[1..5].copy_from_slice(&1_u32.to_le_bytes());
    let marker = b")\x01z\x01x)\0s\0\0\0\0";
    let start = nested
        .windows(marker.len())
        .position(|w| w == marker)
        .unwrap();
    let mut names = b")\x01z\x01x)\x01u".to_vec();
    names.extend_from_slice(&32_768_u32.to_le_bytes());
    names.extend_from_slice(&[b'a'; 32_768]);
    names.extend_from_slice(b"s\x01\0\0\0\x20");
    nested.splice(start..start + marker.len(), names);
    let bytes = raw_module(&[151, 0, 100, 1, 132, 0, 90, 0, 121, 0], &nested);
    let (_dir, path) = fixture(&bytes, "signature-copy.marshal");
    assert_no_budget_flags(&path, "decompile");
    let mut limits = shared_decompile_limits();
    limits.work = 50_000;
    let (perf, result) = decompile_library(&path, Some((3, 12)), limits);
    assert_eq!(result.err().unwrap().code, "RESOURCE_LIMIT");
    assert_eq!(
        perf_value(&perf)["stages_ns"]["generate"]
            .as_u64()
            .unwrap_or(0),
        0
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let mut ok = shared_decompile_limits();
    ok.work = 1_000_000;
    assert!(decompile_library(&path, Some((3, 12)), ok).1.is_ok());
}

#[test]
fn pyc_charges_reused_format_spec_copies_before_recovery_allocation() {
    for (tag, payload) in [(b'u', b'a'), (b's', 0xff)] {
        let mut constant = vec![tag];
        constant.extend_from_slice(&1_048_576_u32.to_le_bytes());
        constant.resize(1_048_581, payload);
        let mut code = vec![151, 0];
        for _ in 0..16 {
            code.extend_from_slice(&[100, 0, 100, 1, 155, 4]);
        }
        code.extend_from_slice(&[102, 16, 90, 0, 121, 0]);
        let bytes = raw_module(&code, &constant);
        let (_dir, path) = fixture(&bytes, "reused-format-spec.marshal");
        assert_no_budget_flags(&path, "decompile");
        let mut limits = shared_decompile_limits();
        limits.work = 100_000;
        let (perf, result) = decompile_library(&path, Some((3, 12)), limits);
        assert_eq!(result.err().unwrap().code, "RESOURCE_LIMIT");
        let perf = perf_value(&perf);
        assert!(perf["stages_ns"]["decompile"].as_u64().unwrap() > 0);
        assert_eq!(
            perf["stages_ns"]["generate"].as_u64().unwrap_or(0),
            0,
            "format-spec bytes must be charged before recovery copies them"
        );
        assert!(perf["counters"]["work"].as_u64().unwrap() < 100_000);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    let mut constant = b"u".to_vec();
    constant.extend_from_slice(&4_u32.to_le_bytes());
    constant.extend_from_slice(b"04.2");
    let (_dir, path) = fixture(
        raw_module(&[151, 0, 100, 0, 100, 1, 155, 4, 90, 0, 121, 0], &constant),
        "small-format-spec.marshal",
    );
    let mut limits = shared_decompile_limits();
    limits.work = 8_192;
    let result = decompile_library(&path, Some((3, 12)), limits).1.unwrap();
    assert!(result.complete);
    assert!(result.source.contains("{None:04.2}"));
}
