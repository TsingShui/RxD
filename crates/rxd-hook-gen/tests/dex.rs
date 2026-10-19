#![cfg(feature = "dex")]
#[path = "dex/fixtures.rs"]
mod fixtures;
#[path = "../../../tests/dex/support.rs"]
mod legacy_fixtures;
mod support;

use rxd_hook_gen::{Artifact, CapturedInput, Language, Limits, Request, dex, generate};

fn request(bytes: Vec<u8>, selector: dex::Selector) -> Request {
    Request::Dex(dex::Request {
        input: CapturedInput::from_bytes("not-a-path.dex", bytes, Limits::default()).unwrap(),
        selector,
        options: dex::Options::default(),
    })
}
fn source(request: &Request) -> String {
    let Artifact::Source(artifact) = generate(request, Limits::default()).unwrap() else {
        panic!("DEX must return complete source");
    };
    artifact.source
}

#[test]
fn archive_work_budgets_report_resource_errors_without_an_artifact() {
    let dex = legacy_fixtures::const_string_dex(1);
    let apk = legacy_fixtures::stored_zip(&[("classes.dex", &dex), ("classes2.dex", &dex)]);
    let req = request(apk, dex::Selector::Signature("LFixture0;->m0()V".into()));
    let error = generate(
        &req,
        Limits {
            analysis_steps: 1,
            ..Limits::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, rxd_hook_gen::ErrorKind::ResourceLimit);
    assert_eq!(error.exit_code(), 1);
}

#[test]
fn original_dollar_names_reserved_class_and_return_only_overloads_are_not_misdirected() {
    let methods = vec![
        fixtures::method("$compiler", &[], "V", 1),
        fixtures::method("class", &[], "V", 1),
        fixtures::method("same", &["I"], "I", 1),
        fixtures::method("same", &["I"], "Ljava/lang/String;", 1),
    ];
    for (name, proto, bridge) in [
        ("$compiler", "()V", "[\"_$compiler\"]"),
        ("class", "()V", "$find(\"class\")"),
        ("same", "(I)Ljava/lang/String;", "[\"same\"]"),
    ] {
        let mut req = request(
            fixtures::dex(&methods),
            dex::Selector::Signature(format!("{}->{name}{proto}", fixtures::CLASS)),
        );
        let js = source(&req);
        assert!(js.contains(bridge), "{js}");
        assert!(js.contains("returnType.className"));
        assert!(js.contains("argumentTypes"));
        support::validate_js(&js);
        let Request::Dex(ref mut dex) = req else {
            unreachable!()
        };
        dex.options.language = Language::TypeScript;
        support::validate_ts(&source(&req));
    }
    let collisions = fixtures::dex(&[
        fixtures::method("$x", &[], "V", 1),
        fixtures::method("_$x", &[], "V", 1),
    ]);
    let error = generate(
        &request(
            collisions,
            dex::Selector::Signature(format!("{}->$x()V", fixtures::CLASS)),
        ),
        Limits::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, rxd_hook_gen::ErrorKind::Ambiguous);
}

#[test]
fn real_dex_selectors_resolve_the_same_declared_method_and_validate_all_variants() {
    let bytes = legacy_fixtures::const_string_dex(1);
    let mut signature = request(
        bytes.clone(),
        dex::Selector::Signature("LFixture0;->m0()V".into()),
    );
    let indexed = request(
        bytes,
        dex::Selector::MethodIndex {
            class: "LFixture0;".into(),
            method_index: 0,
        },
    );
    let js = source(&signature);
    assert_eq!(js, source(&indexed));
    assert!(js.contains("Java.use(\"Fixture0\")"));
    assert!(js.contains("__rxdClass[\"m0\"]"));
    assert!(js.contains("const __rxdExpectedArgs = []"));
    assert!(js.contains("__rxdOriginal.call(this)"));
    support::validate_js(&format!("{js}\n{js}"));
    for language in [Language::JavaScript, Language::TypeScript] {
        for import in [false, true] {
            let Request::Dex(ref mut dex) = signature else {
                unreachable!()
            };
            dex.options.language = language;
            dex.options.import_java_bridge = import;
            let rendered = source(&signature);
            assert_eq!(
                rendered
                    .matches("import Java from \"frida-java-bridge\";")
                    .count(),
                usize::from(import)
            );
            if language == Language::JavaScript {
                support::validate_js(&rendered);
            } else {
                support::validate_ts(&rendered);
                if !import {
                    support::validate_ts(&format!("{rendered}\n{rendered}"));
                    support::validate_ts(&format!(
                        "import Java from \"frida-java-bridge\";\n{rendered}\n{rendered}"
                    ));
                }
            }
        }
    }
}

#[test]
fn declared_flags_exact_types_receivers_and_invocation_fidelity_survive_real_apk_selection() {
    let bytes = fixtures::dex(&fixtures::methods());
    let apk = legacy_fixtures::deflated_zip(&[("classes.dex", &bytes)]);
    let scenarios = [
        (
            "<init>(I)V",
            "__rxdClass[\"$init\"]",
            "__rxdOriginal.call(this, arg0)",
            "arg0: number",
        ),
        (
            "nativeCall(J)J",
            "\"long\"",
            "__rxdOriginal.call(this, arg0)",
            "arg0: Int64",
        ),
        (
            "overload(I)I",
            "\"int\"",
            "__rxdOriginal.call(this, arg0)",
            "const __rxdResult: number",
        ),
        (
            "overload(Ljava/lang/String;)Ljava/lang/String;",
            "\"java.lang.String\"",
            "return __rxdResult;",
            "arg0: string | null",
        ),
        (
            "staticVoid([I[[Ljava/lang/String;CLjava/lang/Object;)V",
            "\"[[Ljava.lang.String;\"",
            "__rxdOriginal.call(__rxdClass, arg0, arg1, arg2, arg3)",
            "arg0: __rxdPrimitiveArray<number> | null, arg1: Array<Array<string | null> | null> | null, arg2: string, arg3: __rxdWrapper | null",
        ),
        (
            "unicode$λ(ZBSFD[J[Lexample/原$Inner;)[I",
            "\"[Lexample.原$Inner;\"",
            "return __rxdResult;",
            "arg5: __rxdPrimitiveArray<Int64> | null, arg6: Array<__rxdWrapper | null> | null",
        ),
    ];
    let mut combined_js = String::new();
    let mut combined_ts = String::new();
    for (signature, selector, call, typed) in scenarios {
        let mut req = request(
            apk.clone(),
            dex::Selector::Signature(format!("{}->{signature}", fixtures::CLASS)),
        );
        let js = source(&req);
        assert!(js.contains("Java.use(\"example.原$Inner\")"));
        assert!(js.contains(selector), "{js}");
        assert!(js.contains(call), "{js}");
        assert!(!js.contains("$new"));
        assert!(js.contains("throw __rxdThrown;"));
        assert!(js.contains("try { __rxdConsoleLog("));
        assert!(!js.contains("JSON.stringify"));
        let stages = [
            "\"dex.schedule\"",
            "__rxdLog(\"waiting\")",
            "\"dex.java-use\"",
            "Java.use(",
            "\"dex.overload\"",
            "returnType.className",
            "\"dex.implementation\"",
            "__rxdOriginal.implementation = function",
            "__rxdLog(\"success\")",
        ];
        let mut tail = js.as_str();
        for stage in stages {
            let found = tail
                .find(stage)
                .unwrap_or_else(|| panic!("missing/out of order {stage}: {js}"));
            tail = &tail[found + stage.len()..];
        }
        assert!(js.contains("__rxdError instanceof Error"));
        assert!(js.contains("throw __rxdError;"));
        combined_js.push_str(&js);
        let Request::Dex(ref mut dex) = req else {
            unreachable!()
        };
        dex.options.language = Language::TypeScript;
        let ts = source(&req);
        assert!(ts.contains(typed), "{ts}");
        assert!(ts.contains("this: __rxdWrapper"));
        assert!(!ts.contains("any"));
        if signature.ends_with('V') {
            assert!(ts.contains("): void"));
            assert!(!ts.contains("const __rxdResult"));
        }
        combined_ts.push_str(&ts);
    }
    support::validate_js(&combined_js);
    support::validate_ts(&combined_ts);
    support::validate_ts(&format!(
        "import Java from \"frida-java-bridge\";\n{combined_ts}"
    ));
    // method_ids index 0 is an external reference, not the class's first declaration.
    let missing = request(
        bytes.clone(),
        dex::Selector::MethodIndex {
            class: fixtures::CLASS.into(),
            method_index: 0,
        },
    );
    assert_eq!(
        generate(&missing, Limits::default())
            .unwrap_err()
            .exit_code(),
        3
    );
    // Index 4 is nativeCall, even though it is not slot 4 in the virtual-method block.
    let indexed = request(
        bytes.clone(),
        dex::Selector::MethodIndex {
            class: "example.原$Inner".into(),
            method_index: 4,
        },
    );
    let signature = request(
        bytes.clone(),
        dex::Selector::Signature(format!("{}->nativeCall(J)J", fixtures::CLASS)),
    );
    assert_eq!(source(&indexed), source(&signature));
    for member in ["abstracted()V", "<clinit>()V"] {
        let req = request(
            bytes.clone(),
            dex::Selector::Signature(format!("{}->{member}", fixtures::CLASS)),
        );
        assert_eq!(
            generate(&req, Limits::default()).unwrap_err().kind,
            rxd_hook_gen::ErrorKind::Unsupported
        );
    }
}

#[test]
fn multidex_and_duplicate_physical_and_logical_definitions_require_exact_entries() {
    let dex = fixtures::dex(&fixtures::methods());
    let selector = dex::Selector::Signature(format!("{}->nativeCall(J)J", fixtures::CLASS));
    for (bytes, entry) in [
        (
            legacy_fixtures::stored_zip(&[("classes.dex", &dex), ("classes2.dex", &dex)]),
            "classes2.dex",
        ),
        (
            fixtures::container(&fixtures::methods()),
            "classes.dex!classes2.dex",
        ),
        (
            legacy_fixtures::deflated_zip(&[(
                "classes.dex",
                &fixtures::container(&fixtures::methods()),
            )]),
            "classes.dex!classes2.dex",
        ),
    ] {
        let mut req = request(bytes, selector.clone());
        let error = generate(&req, Limits::default()).unwrap_err();
        assert_eq!(error.kind, rxd_hook_gen::ErrorKind::Ambiguous);
        assert_eq!(error.exit_code(), 4);
        let Request::Dex(ref mut dex_req) = req else {
            unreachable!()
        };
        dex_req.options.entry = Some(entry.into());
        let generated = source(&req);
        assert!(generated.contains("nativeCall"));
        support::validate_js(&generated);
        let Request::Dex(ref mut dex_req) = req else {
            unreachable!()
        };
        dex_req.options.entry = Some("absent.dex".into());
        assert_eq!(
            generate(&req, Limits::default()).unwrap_err().exit_code(),
            3
        );
    }
    let duplicates = legacy_fixtures::stored_zip(&[("classes.dex", &dex), ("classes.dex", &dex)]);
    let mut req = request(duplicates, selector);
    let Request::Dex(ref mut dex_req) = req else {
        unreachable!()
    };
    dex_req.options.entry = Some("classes.dex".into());
    assert_eq!(
        generate(&req, Limits::default()).unwrap_err().exit_code(),
        4
    );
    // Two definitions inside one physical DEX must not collapse to find_class's first hit.
    let mut duplicate_class = dex;
    let class_offset = u32::from_le_bytes(duplicate_class[0x64..0x68].try_into().unwrap()) as usize;
    let row = duplicate_class[class_offset..class_offset + 32].to_vec();
    let offset = duplicate_class.len();
    duplicate_class.extend_from_slice(&row);
    duplicate_class.extend_from_slice(&row);
    fixtures::put32(&mut duplicate_class, 0x60, 2);
    fixtures::put32(&mut duplicate_class, 0x64, offset as u32);
    let size = duplicate_class.len() as u32;
    fixtures::put32(&mut duplicate_class, 0x20, size);
    let req = request(
        duplicate_class,
        dex::Selector::Signature(format!("{}->nativeCall(J)J", fixtures::CLASS)),
    );
    assert_eq!(
        generate(&req, Limits::default()).unwrap_err().exit_code(),
        4
    );
}

#[test]
fn immutable_capture_identity_escaping_and_public_failures_are_atomic() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("capture.dex");
    let name = "switch\"\\${name}\u{2028}λ😀";
    let bytes = fixtures::dex(&[fixtures::method(name, &[], "V", 1)]);
    std::fs::write(&path, &bytes).unwrap();
    let input = CapturedInput::read(&path, Limits::default()).unwrap();
    let id = input.source_id().to_owned();
    std::fs::write(&path, b"changed after capture").unwrap();
    let req = Request::Dex(dex::Request {
        input,
        selector: dex::Selector::MethodIndex {
            class: fixtures::CLASS.into(),
            method_index: 0,
        },
        options: dex::Options::default(),
    });
    let rendered = source(&req);
    assert!(rendered.contains(&id));
    assert!(rendered.contains("\\u2028"));
    assert!(rendered.contains("\\\""));
    support::validate_js(&rendered);
    let Request::Dex(mut typed) = req.clone() else {
        unreachable!()
    };
    typed.options.language = Language::TypeScript;
    support::validate_ts(&source(&Request::Dex(typed)));
    let renamed = request(
        bytes.clone(),
        dex::Selector::MethodIndex {
            class: fixtures::CLASS.into(),
            method_index: 0,
        },
    );
    assert_eq!(rendered, source(&renamed));
    let changed =
        CapturedInput::from_bytes("same", b"changed".to_vec(), Limits::default()).unwrap();
    assert_ne!(id, changed.source_id());
    for limits in [
        Limits {
            input_bytes: bytes.len() - 1,
            ..Limits::default()
        },
        Limits {
            expanded_bytes: bytes.len() - 1,
            ..Limits::default()
        },
        Limits {
            analysis_steps: 1,
            ..Limits::default()
        },
        Limits {
            target_bytes: 4,
            ..Limits::default()
        },
        Limits {
            source_bytes: 16,
            ..Limits::default()
        },
        Limits {
            artifact_bytes: 16,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            generate(&req, limits).unwrap_err().kind,
            rxd_hook_gen::ErrorKind::ResourceLimit
        );
    }
    for signature in [
        "",
        "Fixture->m()V",
        "LFixture0;->m",
        "LFixture0;->m()",
        "LFixture0;->m(V)V",
        "LFixture0;->m([V)V",
        "LFixture0;->m(I)Vx",
    ] {
        assert_eq!(
            generate(
                &request(bytes.clone(), dex::Selector::Signature(signature.into())),
                Limits::default()
            )
            .unwrap_err()
            .exit_code(),
            2
        );
    }
    let absent = request(
        bytes.clone(),
        dex::Selector::Signature(format!("{}->missing()V", fixtures::CLASS)),
    );
    assert_eq!(
        generate(&absent, Limits::default())
            .unwrap_err()
            .exit_code(),
        3
    );
    for malformed in [
        b"not a DEX".to_vec(),
        bytes[..0x60].to_vec(),
        {
            let mut bad = bytes.clone();
            fixtures::put32(&mut bad, 0x58, u32::MAX);
            bad
        },
        {
            let mut bad = bytes.clone();
            let class = u32::from_le_bytes(bad[0x64..0x68].try_into().unwrap()) as usize;
            let offset = bad.len() as u32;
            bad.extend_from_slice(&[0, 0, 0xff, 0xff, 0xff, 0xff, 0x0f, 0]);
            fixtures::put32(&mut bad, class + 24, offset);
            let size = bad.len() as u32;
            fixtures::put32(&mut bad, 0x20, size);
            bad
        },
    ] {
        let malformed = request(
            malformed,
            dex::Selector::MethodIndex {
                class: fixtures::CLASS.into(),
                method_index: 0,
            },
        );
        assert_eq!(
            generate(&malformed, Limits::default()).unwrap_err().kind,
            rxd_hook_gen::ErrorKind::InvalidInput
        );
    }
}

#[test]
fn pinned_radguard_dex_retains_original_coroutine_names_and_nonlocal_method_index() {
    // Existing pinned third-party fixture, not a newly downloaded/runtime input.
    // Independent DEX table read: method_ids[386], not class-local member 386.
    let bytes = include_bytes!("../../../third_party/radguard/tests/fixtures/sample.dex").to_vec();
    let class = "Lkotlinx/coroutines/flow/AbstractFlow$collect$1;";
    let mut selected = request(
        bytes.clone(),
        dex::Selector::Signature(format!(
            "{class}->invokeSuspend(Ljava/lang/Object;)Ljava/lang/Object;"
        )),
    );
    let indexed = request(
        bytes,
        dex::Selector::MethodIndex {
            class: class.into(),
            method_index: 386,
        },
    );
    let js = source(&selected);
    assert_eq!(js, source(&indexed));
    assert!(js.contains("Java.use(\"kotlinx.coroutines.flow.AbstractFlow$collect$1\")"));
    assert!(js.contains("__rxdClass[\"invokeSuspend\"]"));
    support::validate_js(&js);
    let Request::Dex(ref mut dex) = selected else {
        unreachable!()
    };
    dex.options.language = Language::TypeScript;
    support::validate_ts(&source(&selected));
}

#[test]
fn mismatched_declaring_class_and_out_of_table_class_data_are_not_method_matches() {
    let original = fixtures::dex(&fixtures::methods());
    let methods = u32::from_le_bytes(original[0x5c..0x60].try_into().unwrap()) as usize;
    let other_owner = original[methods..methods + 2].to_vec();
    let mut wrong_owner = original.clone();
    // NativeCall is independently known to be method_ids[4] in this fixture.
    wrong_owner[methods + 4 * 8..methods + 4 * 8 + 2].copy_from_slice(&other_owner);
    let class = u32::from_le_bytes(original[0x64..0x68].try_into().unwrap()) as usize;
    let mut wrong_index = original.clone();
    let offset = wrong_index.len() as u32;
    // One virtual method with index 127, beyond the 9-row method_ids table.
    wrong_index.extend_from_slice(&[0, 0, 0, 1, 127, 1, 0]);
    fixtures::put32(&mut wrong_index, class + 24, offset);
    let size = wrong_index.len() as u32;
    fixtures::put32(&mut wrong_index, 0x20, size);
    for bytes in [wrong_owner, wrong_index] {
        let req = request(
            bytes,
            dex::Selector::MethodIndex {
                class: fixtures::CLASS.into(),
                method_index: 4,
            },
        );
        assert_eq!(
            generate(&req, Limits::default()).unwrap_err().kind,
            rxd_hook_gen::ErrorKind::InvalidInput
        );
    }
}

#[test]
fn malformed_mutf8_identities_are_not_silently_repaired_or_relengthened() {
    let bytes = fixtures::dex(&[fixtures::method("uniqueName", &[], "V", 1)]);
    let name = bytes
        .windows(11)
        .position(|b| b == b"uniqueName\0")
        .unwrap();
    let mut wrong_length = bytes.clone();
    wrong_length[name - 1] = 9; // Original declared UTF-16 length is 10.
    let mut invalid_utf8 = bytes;
    invalid_utf8[name] = 0xff;
    for malformed in [wrong_length, invalid_utf8] {
        let req = request(
            malformed,
            dex::Selector::MethodIndex {
                class: fixtures::CLASS.into(),
                method_index: 0,
            },
        );
        assert_eq!(
            generate(&req, Limits::default()).unwrap_err().kind,
            rxd_hook_gen::ErrorKind::InvalidInput
        );
    }
}
