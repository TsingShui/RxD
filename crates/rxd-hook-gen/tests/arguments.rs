//! The actual root argument adapter, without building the root CLI.
#[path = "../../../src/cli/hook_gen.rs"]
mod hook_gen;

use clap::Parser;

#[derive(Parser)]
struct Driver {
    #[command(subcommand)]
    command: hook_gen::Command,
}

#[cfg(feature = "web")]
#[test]
fn web_arguments_generate_and_publish_without_native_resources() {
    let command = Driver::try_parse_from(["driver", "web", "--target", "fetch"])
        .unwrap()
        .command;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    hook_gen::run(
        command,
        || panic!("Web must not prepare native resources"),
        &mut stdout,
        &mut stderr,
    )
    .unwrap();
    assert!(String::from_utf8(stdout).unwrap().contains("globalThis"));
    assert!(stderr.is_empty());
}

#[cfg(feature = "native")]
#[test]
fn native_name_and_static_address_are_equivalent_without_engine_initialization() {
    let input = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/typed.elf");
    // Independently pinned fixture identity, not recomputed with the generator's hash implementation.
    let expected_hash = "2b81116e2a05aa270147a7a99b08ee4b9e92fdc009e4f8f99f4e4851f9395b36";
    let captured =
        rxd_hook_gen::CapturedInput::read(&input, rxd_hook_gen::Limits::default()).unwrap();
    assert_eq!(captured.source_id(), expected_hash);
    let owned = rxd_hook_gen::CapturedInput::from_bytes(
        "another-name",
        include_bytes!("native/typed.elf").to_vec(),
        rxd_hook_gen::Limits::default(),
    )
    .unwrap();
    assert_eq!(owned.source_id(), expected_hash);
    let mut outputs = Vec::new();
    for target in ["signed_scalar", "0x402000"] {
        let command =
            Driver::try_parse_from(["driver", "frida", "native", input.to_str().unwrap(), target])
                .unwrap()
                .command;
        let mut stdout = Vec::new();
        hook_gen::run(
            command,
            || panic!("known JS entry must stay engine-free"),
            &mut stdout,
            &mut Vec::new(),
        )
        .unwrap();
        outputs.push(stdout);
    }
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
fn argument_surface_rejects_cross_runtime_options_and_incomplete_selectors() {
    for args in [
        vec!["web", "--target", "fetch", "--lang", "ts"],
        vec!["web", "--target", "fetch", "--import"],
        vec!["web", "--target", "fetch", "--emit", "html"],
        vec!["frida", "native", "unused", "main", "--import"],
        vec!["frida", "native", "unused", "main", "--lang", "typescript"],
        vec![
            "frida",
            "native",
            "unused",
            "main",
            "--module",
            "lib.so",
            "--main-module",
        ],
        vec!["frida", "dex", "unused", "--class", "C"],
        vec!["frida", "dex", "unused", "--method-index", "1"],
        vec![
            "frida",
            "dex",
            "unused",
            "LC;->m()V",
            "--class",
            "C",
            "--method-index",
            "1",
        ],
    ] {
        let error = Driver::try_parse_from(std::iter::once("driver").chain(args))
            .err()
            .expect("invalid args");
        assert_eq!(error.exit_code(), 2);
    }
    for args in [
        vec!["web", "--target", "fetch", "--emit", "extension"],
        vec!["web", "--target", "fetch", "--out", "unused"],
        vec!["frida", "native", "unused", "0xFFFFFFFFFFFFFFFFF"],
        vec!["frida", "native", "unused", "0x"],
    ] {
        let command = Driver::try_parse_from(std::iter::once("driver").chain(args))
            .unwrap()
            .command;
        let mut stdout = Vec::new();
        let error = hook_gen::run(
            command,
            || panic!("argument error must precede resource loading"),
            &mut stdout,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.exit_code(), 2);
        assert!(stdout.is_empty());
    }
}

#[cfg(feature = "web")]
#[test]
fn extension_arguments_publish_complete_files_and_refuse_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let out = temp.path().join("extension");
    for attempt in 0..2 {
        let command = Driver::try_parse_from([
            "driver",
            "web",
            "--target",
            "fetch",
            "--emit",
            "extension",
            "--match",
            "https://example.com/*",
            "--out",
            out.to_str().unwrap(),
        ])
        .unwrap()
        .command;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = hook_gen::run(
            command,
            || panic!("Web is engine-free"),
            &mut stdout,
            &mut stderr,
        );
        assert!(stdout.is_empty());
        if attempt == 0 {
            result.unwrap();
            assert!(!stderr.is_empty());
            support::validate_extension(&out);
        } else {
            assert_eq!(result.unwrap_err().exit_code(), 1);
        }
    }
}

#[cfg(feature = "dex")]
#[test]
fn dex_argument_selectors_import_languages_and_error_codes_reach_real_provider() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("fixture.dex");
    let bytes = legacy_fixtures::const_string_dex(1);
    std::fs::write(&path, &bytes).unwrap();
    let path = path.to_str().unwrap();
    for lang in ["js", "ts"] {
        let mut equivalent = Vec::new();
        for selector in [
            vec!["LFixture0;->m0()V"],
            vec!["--class", "Fixture0", "--method-index", "0"],
        ] {
            let command = Driver::try_parse_from(
                ["driver", "frida", "dex", path]
                    .into_iter()
                    .chain(selector)
                    .chain(["--lang", lang, "--import"]),
            )
            .unwrap()
            .command;
            let mut stdout = Vec::new();
            hook_gen::run(
                command,
                || panic!("DEX is engine-free"),
                &mut stdout,
                &mut Vec::new(),
            )
            .unwrap();
            let source = String::from_utf8(stdout).unwrap();
            assert_eq!(source.matches("import Java from").count(), 1);
            if lang == "js" {
                support::validate_js(&source);
            } else {
                support::validate_ts(&source);
            }
            equivalent.push(source);
        }
        assert_eq!(equivalent[0], equivalent[1]);
    }
    let apk = temp.path().join("duplicate.apk");
    std::fs::write(
        &apk,
        legacy_fixtures::stored_zip(&[("classes.dex", &bytes), ("classes2.dex", &bytes)]),
    )
    .unwrap();
    for (input, method, code) in [
        (path, "LFixture0;->missing()V", 3),
        (apk.to_str().unwrap(), "LFixture0;->m0()V", 4),
        (path, "incomplete", 2),
    ] {
        let command = Driver::try_parse_from(["driver", "frida", "dex", input, method])
            .unwrap()
            .command;
        let mut stdout = Vec::new();
        let error = hook_gen::run(
            command,
            || panic!("DEX is engine-free"),
            &mut stdout,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.exit_code(), code);
        assert!(stdout.is_empty());
    }
}

#[cfg(feature = "kuna")]
#[test]
fn native_types_and_discovery_receive_explicit_prepared_resources() {
    let specs = std::env::var_os("RXD_HOOK_TEST_SPECS").expect("explicit prepared specs required");
    for (file, target, lang, expected) in [
        ("typed.elf", "signed_scalar", "ts", "debug-derived"),
        ("stripped.elf", "0x402000", "js", "Interceptor.attach"),
    ] {
        let input = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/native")
            .join(file);
        let command = Driver::try_parse_from([
            "driver",
            "frida",
            "native",
            input.to_str().unwrap(),
            target,
            "--lang",
            lang,
        ])
        .unwrap()
        .command;
        let mut stdout = Vec::new();
        hook_gen::run(
            command,
            || Ok(Some(specs.clone().into())),
            &mut stdout,
            &mut Vec::new(),
        )
        .unwrap();
        let source = String::from_utf8(stdout).unwrap();
        assert!(source.contains(expected), "{source}");
        if lang == "ts" {
            support::validate_ts(&source);
        } else {
            support::validate_js(&source);
        }
    }
}

#[cfg(feature = "kuna")]
#[test]
fn native_default_js_discovers_named_entries_and_returns_empty_errors() {
    let specs = std::env::var_os("RXD_HOOK_TEST_SPECS").expect("explicit prepared specs required");
    let input = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/stripped.elf");
    let mut outputs = Vec::new();
    for (target, error_code) in [
        ("sub_402000", None),
        ("0x402000", None),
        ("sub_402001", Some(3)),
        ("absent", Some(3)),
    ] {
        let command =
            Driver::try_parse_from(["driver", "frida", "native", input.to_str().unwrap(), target])
                .unwrap()
                .command;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = hook_gen::run(
            command,
            || Ok(Some(specs.clone().into())),
            &mut stdout,
            &mut stderr,
        );
        if let Some(code) = error_code {
            assert_eq!(result.unwrap_err().exit_code(), code);
            assert!(stdout.is_empty());
        } else {
            result.unwrap();
            outputs.push(stdout);
        }
        assert!(stderr.is_empty());
    }
    assert_eq!(outputs[0], outputs[1]);
    support::validate_js(std::str::from_utf8(&outputs[0]).unwrap());
}

#[cfg(feature = "dex")]
#[path = "../../../tests/dex/support.rs"]
mod legacy_fixtures;
#[cfg(any(feature = "dex", feature = "web", feature = "kuna"))]
mod support;
