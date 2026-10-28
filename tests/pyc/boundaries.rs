use super::*;

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
fn pyc_enforces_exact_source_output_and_perf_payload_parity() {
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
    // Source is the payload: no record wraps it, and stdout matches the library.
    let (_, result) = decompile_library(&path, Some((3, 12)), shared_decompile_limits());
    assert_eq!(result.unwrap().source.as_bytes(), raw.stdout);
    assert_no_budget_flags(&path, "decompile");
    let perf = run(&path, &["--version", "3.12", "--perf"]);
    success(&perf);
    assert_eq!(perf.stdout, raw.stdout, "perf must not change the payload");
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
    exact.work = Some(work as usize);
    let (_, result) = decompile_library(&path, Some((3, 12)), exact);
    assert!(result.is_ok());
    let mut less = exact;
    less.work = Some(work as usize - 1);
    let (perf, result) = decompile_library(&path, Some((3, 12)), less);
    assert_eq!(result.err().unwrap().code, "RESOURCE_LIMIT");
    let perf = perf_value(&perf);
    assert!(perf["counters"]["work"].as_u64().unwrap() < work);
    assert!(perf["stages_ns"]["generate"].as_u64().unwrap() > 0);

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
    limits.work = Some(256);
    let (perf, result) = decompile_library(&path, Some((3, 12)), limits);
    assert_eq!(result.err().unwrap().code, "RESOURCE_LIMIT");
    let perf = perf_value(&perf);
    assert!(perf["counters"]["work"].as_u64().unwrap() <= 256);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let small = raw_module(&[151, 0, 100, 1, 102, 1, 90, 0, 121, 0], b"N");
    fs::write(&path, small).unwrap();
    let mut ok = shared_decompile_limits();
    ok.work = Some(4096);
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
    limits.work = Some(8_192);
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
    ok.work = Some(200_000);
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
    limits.work = Some(50_000);
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
    ok.work = Some(1_000_000);
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
        limits.work = Some(100_000);
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
    limits.work = Some(8_192);
    let result = decompile_library(&path, Some((3, 12)), limits).1.unwrap();
    assert!(result.complete);
    assert!(result.source.contains("{None:04.2}"));
}
