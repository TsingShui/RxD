#![cfg(feature = "native")]
#[path = "native/fixtures.rs"]
mod fixtures;
mod support;
use rxd_hook_gen::{Artifact, CapturedInput, Language, Limits, Request, generate, native};

fn request(bytes: Vec<u8>, target: native::Target) -> Request {
    Request::Native(native::Request {
        input: CapturedInput::from_bytes("/untrusted/path/libsample.so", bytes, Limits::default())
            .unwrap(),
        target,
        options: native::Options::default(),
    })
}
fn source(request: &Request) -> String {
    match generate(request, Limits::default()).unwrap() {
        Artifact::Source(source) => source.source,
        _ => panic!("expected source"),
    }
}

#[test]
fn elf_symbol_and_static_vma_use_image_mapping_not_file_offset() {
    let named = request(fixtures::elf(false), native::Target::Name("observe".into()));
    let addressed = request(
        fixtures::elf(false),
        native::Target::StaticAddress(0x702000),
    );
    let js = source(&named);
    assert_eq!(js, source(&addressed));
    assert!(js.contains(".base.add(\"0x2000\")"), "{js}");
    assert!(js.contains("Process.getModuleByName(\"libsample.so\")"));
    assert!(js.contains("__rxdArch !== \"x64\""));
    assert!(!js.contains("Java"));
    let stages = [
        "__rxdStage = \"native.module\";",
        "__rxdStage = \"native.target\";",
        "__rxdStage = \"native.attach\";",
    ];
    assert!(js.find(stages[0]).unwrap() < js.find(stages[1]).unwrap());
    assert!(js.find(stages[1]).unwrap() < js.find(stages[2]).unwrap());
    assert_eq!(js.matches("__rxdLog(\"begin\")").count(), 3);
    assert_eq!(js.matches("__rxdLog(\"success\")").count(), 3);
    assert!(js.contains("throw __rxdError;") && js.contains("__rxdError instanceof Error"));
    support::validate_js(&format!("{js}\n{js}"));
    let mut ts = named;
    let Request::Native(ref mut req) = ts else {
        unreachable!()
    };
    req.options.language = Language::TypeScript;
    let ts = source(&ts);
    assert!(ts.contains("this: InvocationContext, args: InvocationArguments"));
    assert!(ts.contains("retval: InvocationReturnValue"));
    assert!(ts.contains("signature: unknown"));
    support::validate_ts(&format!("{ts}\n{ts}"));
}

#[test]
fn stripped_entry_is_hookable_but_unknown_targets_require_discovery() {
    let mut bytes = fixtures::elf(true);
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[24..32].copy_from_slice(&0x702000u64.to_le_bytes());
    let entry = request(bytes.clone(), native::Target::StaticAddress(0x702000));
    let js = source(&entry);
    assert!(js.contains("Process.mainModule"));
    support::validate_js(&js);
    let interior = request(bytes.clone(), native::Target::StaticAddress(0x702001));
    assert_eq!(
        generate(&interior, Limits::default()).unwrap_err().kind,
        rxd_hook_gen::ErrorKind::EngineUnavailable
    );
    let missing = request(bytes, native::Target::Name("absent".into()));
    assert_eq!(
        generate(&missing, Limits::default()).unwrap_err().kind,
        rxd_hook_gen::ErrorKind::EngineUnavailable
    );
}

#[cfg(feature = "kuna")]
fn typed_request(name: &str) -> Request {
    let mut req = request(
        include_bytes!("native/typed.elf").to_vec(),
        native::Target::Name(name.into()),
    );
    let Request::Native(ref mut native) = req else {
        unreachable!()
    };
    native.options.language = Language::TypeScript;
    native.options.specs_dir = Some(
        std::env::var_os("RXD_HOOK_TEST_SPECS")
            .expect("explicit prepared Kuna specs required for engine E2E")
            .into(),
    );
    req
}

#[cfg(feature = "kuna")]
#[test]
fn selected_binary_debug_signature_reaches_typed_locals_and_real_conversions() {
    let ts = source(&typed_request("signed_scalar"));
    assert!(ts.contains("debug-derived"), "{ts}");
    assert!(ts.contains("\"count\""), "{ts}");
    assert!(ts.contains("args[0].toInt32()"), "{ts}");
    assert!(ts.contains("retval.toInt32()"), "{ts}");
    assert!(!ts.contains("unrelated"));
    support::validate_ts(&ts);
}

#[cfg(feature = "kuna")]
#[test]
fn native_scalar_width_signedness_and_unsupported_abi_are_not_casts() {
    let unsigned = source(&typed_request("wide_scalar"));
    assert!(
        unsigned.contains(": UInt64 = uint64(args[0].toString())"),
        "{unsigned}"
    );
    assert!(!unsigned.contains("toNumber"));
    let signed = source(&typed_request("signed_wide"));
    assert!(
        signed.contains(": Int64 = int64(args[0].toString())"),
        "{signed}"
    );
    let pointer = source(&typed_request("pointer_scalar"));
    assert!(pointer.contains(": NativePointer = args[0]"), "{pointer}");
    assert!(!pointer.contains("readPointer") && !pointer.contains("readUtf"));
    let mixed = source(&typed_request("mixed_scalar"));
    assert!(mixed.contains("args[1].toUInt32() & 0xff"), "{mixed}");
    assert!(mixed.contains("(args[2].toInt32() << 16) >> 16"), "{mixed}");
    let float = source(&typed_request("unsupported_float"));
    assert!(float.contains("decoding unavailable"), "{float}");
    assert!(!float.contains(".toInt32()"), "{float}");
    let variadic = source(&typed_request("unsupported_variadic"));
    assert!(variadic.contains("\"variadic\":true"), "{variadic}");
    assert!(variadic.contains("decoding unavailable"), "{variadic}");
    support::validate_ts(&[unsigned, signed, pointer, mixed, float, variadic].join("\n"));
}

#[test]
fn pe_macho_arm_and_thumb_keep_their_native_coordinates() {
    let cases: &[(&[u8], &str, u64, &str, &str)] = &[
        (
            include_bytes!("native/mapping.dll"),
            "observe",
            0x180001000,
            "0x1000",
            "x64",
        ),
        (
            include_bytes!("native/mapping.dylib"),
            "_observe",
            0x2a8,
            "0x2a8",
            "arm64",
        ),
        (
            include_bytes!("native/thumb.so"),
            "observe",
            0x2001,
            "0x2000",
            "arm",
        ),
        (
            include_bytes!("native/arm64.so"),
            "observe",
            0x2000,
            "0x2000",
            "arm64",
        ),
    ];
    let mut combined = String::new();
    for (bytes, name, address, offset, arch) in cases {
        let named = request(bytes.to_vec(), native::Target::Name((*name).into()));
        let addressed = request(bytes.to_vec(), native::Target::StaticAddress(*address));
        let js = source(&named);
        assert_eq!(js, source(&addressed));
        assert!(js.contains(&format!(".base.add(\"{offset}\")")), "{js}");
        assert!(js.contains(&format!("__rxdArch !== \"{arch}\"")), "{js}");
        assert_eq!(js.contains(".or(1)"), *arch == "arm");
        combined.push_str(&js);
    }
    support::validate_js(&combined);
}

#[cfg(feature = "kuna")]
#[test]
fn stripped_discovery_requires_exact_entry_and_inference_is_not_debug_information() {
    let mut stripped = typed_request("signed_scalar");
    let Request::Native(ref mut req) = stripped else {
        unreachable!()
    };
    req.input = CapturedInput::from_bytes(
        "stripped",
        include_bytes!("native/stripped.elf").to_vec(),
        Limits::default(),
    )
    .unwrap();
    req.target = native::Target::StaticAddress(0x402000);
    let ts = source(&stripped);
    assert!(ts.contains("inferred by Kuna"), "{ts}");
    assert!(!ts.contains("debug-derived"));
    support::validate_ts(&ts);
    let Request::Native(ref mut req) = stripped else {
        unreachable!()
    };
    req.target = native::Target::StaticAddress(0x402001);
    let err = generate(&stripped, Limits::default()).unwrap_err();
    assert_eq!(err.kind, rxd_hook_gen::ErrorKind::NotFound, "{err}");
    let mut inferred = typed_request("signed_scalar");
    let Request::Native(ref mut req) = inferred else {
        unreachable!()
    };
    req.input = CapturedInput::from_bytes(
        "inferred",
        include_bytes!("native/inferred.elf").to_vec(),
        Limits::default(),
    )
    .unwrap();
    let ts = source(&inferred);
    assert!(ts.contains("inferred by Kuna"));
    support::validate_ts(&ts);
}

#[cfg(feature = "kuna")]
#[test]
fn placeholder_names_require_discovered_executable_function_entries() {
    for language in [Language::JavaScript, Language::TypeScript] {
        let mut req = typed_request("sub_402001");
        let Request::Native(ref mut native) = req else {
            unreachable!()
        };
        native.input = CapturedInput::from_bytes(
            "stripped",
            include_bytes!("native/stripped.elf").to_vec(),
            Limits::default(),
        )
        .unwrap();
        native.options.language = language;
        // Independent fixture layout: 0x402000 is a function start; +1 is interior.
        let error = generate(&req, Limits::default()).unwrap_err();
        assert_eq!(error.kind, rxd_hook_gen::ErrorKind::NotFound, "{error}");
        let Request::Native(ref mut native) = req else {
            unreachable!()
        };
        native.target = native::Target::Name("sub_402000".into());
        let named = source(&req);
        let Request::Native(ref mut native) = req else {
            unreachable!()
        };
        native.target = native::Target::StaticAddress(0x402000);
        assert_eq!(named, source(&req));
        match language {
            Language::JavaScript => support::validate_js(&named),
            Language::TypeScript => support::validate_ts(&named),
        }
    }
}

#[test]
fn public_errors_budgets_ambiguity_and_escaping_are_atomic() {
    use rxd_hook_gen::ErrorKind;
    let mut duplicated = fixtures::elf(false);
    duplicated[0x4a0..0x4a8].copy_from_slice(&72u64.to_le_bytes());
    let symbol = duplicated[0x318..0x330].to_vec();
    duplicated[0x330..0x348].copy_from_slice(&symbol);
    duplicated[0x338..0x340].copy_from_slice(&0x702002u64.to_le_bytes());
    let ambiguous = request(duplicated, native::Target::Name("observe".into()));
    assert_eq!(
        generate(&ambiguous, Limits::default())
            .unwrap_err()
            .exit_code(),
        4
    );
    let req = request(fixtures::elf(false), native::Target::StaticAddress(0x1000));
    assert_eq!(
        generate(&req, Limits::default()).unwrap_err().kind,
        ErrorKind::NotFound
    );
    let req = request(vec![0; 64], native::Target::Name("observe".into()));
    assert_eq!(
        generate(&req, Limits::default()).unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    let req = request(fixtures::elf(false), native::Target::Name("observe".into()));
    for limits in [
        Limits {
            input_bytes: 8,
            ..Limits::default()
        },
        Limits {
            source_bytes: 8,
            ..Limits::default()
        },
        Limits {
            analysis_steps: 1,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            generate(&req, limits).unwrap_err().kind,
            ErrorKind::ResourceLimit
        );
    }
    let mut escaped = req;
    let Request::Native(ref mut req) = escaped else {
        unreachable!()
    };
    req.options.module = native::RuntimeModule::Named("bad\"\\\u{2028}</script>".into());
    support::validate_js(&source(&escaped));
}

#[test]
fn offsets_above_js_safe_integer_range_stay_hexadecimal() {
    let mut bytes = fixtures::elf(false);
    // Independent ELF layout: text at 0x20000000702000, symbol +1, header at 0x700000.
    bytes[136..144].copy_from_slice(&0x20000000702000u64.to_le_bytes());
    bytes[144..152].copy_from_slice(&0x20000000702000u64.to_le_bytes());
    bytes[0x450..0x458].copy_from_slice(&0x20000000702000u64.to_le_bytes());
    bytes[0x320..0x328].copy_from_slice(&0x20000000702001u64.to_le_bytes());
    let req = request(bytes, native::Target::Name("observe".into()));
    let js = source(&req);
    assert!(js.contains(".base.add(\"0x20000000002001\")"), "{js}");
    support::validate_js(&js);
}

#[cfg(feature = "kuna")]
#[test]
fn selected_analysis_only_spends_the_selected_function_instruction_budget() {
    let mut selected = typed_request("signed_scalar");
    // Independent ELF readobj layout: 7,816 input bytes, 2,810 section bytes,
    // 980 load-segment bytes. A 11,620-step ceiling leaves fourteen steps
    // for the entry inventory plus the selected function's instruction ceiling.
    let limits = Limits {
        analysis_steps: 11_620,
        ..Limits::default()
    };
    let artifact = generate(&selected, limits).unwrap();
    let Artifact::Source(ts) = artifact else {
        unreachable!()
    };
    assert!(ts.source.contains("args[0].toInt32()"));
    let Request::Native(ref mut req) = selected else {
        unreachable!()
    };
    req.target = native::Target::Name("unrelated".into());
    assert_eq!(
        generate(&selected, limits).unwrap_err().kind,
        rxd_hook_gen::ErrorKind::ResourceLimit,
        "larger selected function must exhaust the remaining instruction ceiling"
    );
}

#[test]
fn optional_analysis_is_explicit_and_known_raw_generation_stays_lazy() {
    let mut req = request(fixtures::elf(false), native::Target::Name("observe".into()));
    let Request::Native(ref mut native) = req else {
        unreachable!()
    };
    native.options.specs_dir = Some("/nonexistent-rxd-hook-specs".into());
    assert!(source(&req).contains("Interceptor.attach"));
    let Request::Native(ref mut native) = req else {
        unreachable!()
    };
    native.options.language = Language::TypeScript;
    assert_eq!(
        generate(&req, Limits::default()).unwrap_err().kind,
        rxd_hook_gen::ErrorKind::EngineUnavailable
    );
}

#[test]
fn unsupported_architectures_mappings_and_malformed_selectors_fail_explicitly() {
    use rxd_hook_gen::ErrorKind;
    let mut relocatable = fixtures::elf(false);
    relocatable[16..18].copy_from_slice(&1u16.to_le_bytes());
    assert_eq!(
        generate(
            &request(relocatable, native::Target::Name("observe".into())),
            Limits::default()
        )
        .unwrap_err()
        .kind,
        ErrorKind::Unsupported
    );
    let mut unknown_arch = fixtures::elf(false);
    unknown_arch[18..20].copy_from_slice(&243u16.to_le_bytes()); // RISC-V, not a supported Frida Process.arch.
    assert_eq!(
        generate(
            &request(unknown_arch, native::Target::Name("observe".into())),
            Limits::default()
        )
        .unwrap_err()
        .kind,
        ErrorKind::Unsupported
    );
    let unaligned = request(
        include_bytes!("native/arm64.so").to_vec(),
        native::Target::StaticAddress(0x2001),
    );
    assert_eq!(
        generate(&unaligned, Limits::default()).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    let empty = request(fixtures::elf(false), native::Target::Name(String::new()));
    assert_eq!(
        generate(&empty, Limits::default()).unwrap_err().exit_code(),
        2
    );
}

#[cfg(feature = "kuna")]
#[test]
fn declared_debug_expansion_is_rejected_before_engine_decompression() {
    let mut req = typed_request("signed_scalar");
    let Request::Native(ref mut native) = req else {
        unreachable!()
    };
    let mut bytes = include_bytes!("native/typed.elf").to_vec();
    // Independent ELF layout from llvm-readobj: section 7 .debug_info,
    // file offset 0x1411. Replace its payload with an ELF64 compression header
    // claiming one TiB; no enormous allocation or actual compressed data needed.
    let shoff = u64::from_le_bytes(bytes[40..48].try_into().unwrap()) as usize;
    bytes[shoff + 7 * 64 + 8..shoff + 7 * 64 + 16].copy_from_slice(&0x800u64.to_le_bytes());
    bytes[0x1411..0x1429].fill(0);
    bytes[0x1411..0x1415].copy_from_slice(&1u32.to_le_bytes());
    bytes[0x1419..0x1421].copy_from_slice(&(1u64 << 40).to_le_bytes());
    native.input = CapturedInput::from_bytes("oversized-debug", bytes, Limits::default()).unwrap();
    let error = generate(&req, Limits::default()).unwrap_err();
    assert_eq!(
        error.kind,
        rxd_hook_gen::ErrorKind::ResourceLimit,
        "{error}"
    );
}

#[test]
fn stripped_macho_lc_main_entryoff_is_translated_from_file_mapping() {
    let req = request(
        include_bytes!("native/mapping-executable").to_vec(),
        native::Target::StaticAddress(0x1000002f0),
    );
    let js = source(&req);
    assert!(js.contains("Process.mainModule"));
    assert!(js.contains(".base.add(\"0x2f0\")"), "{js}");
    support::validate_js(&js);
    let header = request(
        include_bytes!("native/mapping-executable").to_vec(),
        native::Target::Name("__mh_execute_header".into()),
    );
    assert!(generate(&header, Limits::default()).is_err());
}

#[cfg(feature = "kuna")]
#[test]
fn macho_selected_analysis_does_not_budget_unmapped_pagezero_as_code() {
    let mut req = typed_request("unused");
    let Request::Native(ref mut native) = req else {
        unreachable!()
    };
    native.input = CapturedInput::from_bytes(
        "mapping-executable",
        include_bytes!("native/mapping-executable").to_vec(),
        Limits::default(),
    )
    .unwrap();
    native.target = native::Target::StaticAddress(0x1000002f0);
    let ts = source(&req);
    assert!(ts.contains(".base.add(\"0x2f0\")"));
    support::validate_ts(&ts);
    let Request::Native(ref mut native) = req else {
        unreachable!()
    };
    native.target = native::Target::Name("__mh_execute_header".into());
    assert_eq!(
        generate(&req, Limits::default()).unwrap_err().kind,
        rxd_hook_gen::ErrorKind::NotFound
    );
}

#[test]
fn different_architecture_ts_snippets_do_not_narrow_global_process_types() {
    let mut linux = request(fixtures::elf(false), native::Target::Name("observe".into()));
    let mut darwin = request(
        include_bytes!("native/mapping.dylib").to_vec(),
        native::Target::Name("_observe".into()),
    );
    for req in [&mut linux, &mut darwin] {
        let Request::Native(native) = req else {
            unreachable!()
        };
        native.options.language = Language::TypeScript;
    }
    support::validate_ts(&format!("{}\n{}", source(&linux), source(&darwin)));
}

#[test]
fn pe_thumb_machine_mode_is_preserved_even_when_entry_rva_is_even() {
    // Independently authored pinned upstream fixture/generator armv4t_thumb_pe.s:
    // preferred base 0x400000, executable entry RVA 0x1001, Thumb machine 0x1c2.
    let original = include_bytes!(
        "../../../third_party/kuna/decompiler/crates/kuna-analysis/tests/fixtures/armv4t_thumb_pe.exe"
    );
    let pe = u32::from_le_bytes(original[0x3c..0x40].try_into().unwrap()) as usize;
    for machine in [0x1c2u16, 0x1c4u16] {
        // THUMB and ARMNT both establish Thumb mode.
        let mut bytes = original.to_vec();
        bytes[pe + 4..pe + 6].copy_from_slice(&machine.to_le_bytes());
        bytes[pe + 24 + 16..pe + 24 + 20].copy_from_slice(&0x1000u32.to_le_bytes());
        let js = source(&request(
            bytes.clone(),
            native::Target::StaticAddress(0x401000),
        ));
        assert!(js.contains("__rxdArch !== \"arm\""));
        assert!(js.contains(".base.add(\"0x1000\")") && js.contains(".or(1)"));
        support::validate_js(&js);
        #[cfg(feature = "kuna")]
        {
            let mut typed = typed_request("unused");
            let Request::Native(ref mut native) = typed else {
                unreachable!()
            };
            native.input =
                CapturedInput::from_bytes("thumb.exe", bytes, Limits::default()).unwrap();
            native.target = native::Target::StaticAddress(0x401000);
            // Debug ARM spec loading is substantially slower than release;
            // semantic test budget is separate from fresh-process latency gates.
            let Artifact::Source(ts) = generate(
                &typed,
                Limits {
                    analysis_timeout_ms: 10_000,
                    ..Limits::default()
                },
            )
            .unwrap() else {
                unreachable!()
            };
            let ts = ts.source;
            assert!(ts.contains(".or(1)"), "{ts}");
            support::validate_ts(&ts);
        }
    }
}
