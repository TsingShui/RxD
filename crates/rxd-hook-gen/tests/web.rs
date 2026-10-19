#![cfg(feature = "web")]
mod support;

use rxd_hook_gen::{Artifact, ErrorKind, Limits, Request, generate, publish_extension, web};
use std::fs;

fn request(target: &str) -> web::Request {
    web::Request {
        target: target.into(),
        ..Default::default()
    }
}
fn snippet(target: &str) -> String {
    source(request(target))
}
fn source(request: web::Request) -> String {
    let artifact = generate(&Request::Web(request), Limits::default()).unwrap();
    let Artifact::Source(source) = artifact else {
        panic!("expected JS source")
    };
    assert_eq!(source.language, rxd_hook_gen::Language::JavaScript);
    source.source
}
fn extension() -> rxd_hook_gen::ExtensionArtifacts {
    let artifact = generate(
        &Request::Web(web::Request {
            emit: web::Emit::Extension,
            matches: vec!["https://*.example.com/*".into()],
            ..request("globalThis.fetch")
        }),
        Limits::default(),
    )
    .unwrap();
    let Artifact::Extension(artifact) = artifact else {
        panic!("expected extension")
    };
    artifact
}

#[test]
fn web_snippet_normalizes_page_paths_and_preserves_raw_call_contract_offline() {
    let source = snippet("WebSocket.prototype.send");
    assert_eq!(source, snippet("window.WebSocket.prototype.send"));
    assert_eq!(source, snippet("self.WebSocket.prototype.send"));
    assert_eq!(
        source,
        snippet("globalThis[\"WebSocket\"].prototype[\"send\"]")
    );
    support::validate_js(&format!("{source}\n{}", snippet("globalThis.fetch")));
    assert!(source.contains("new Proxy(original"));
    assert!(source.contains("__rxdApply(original, receiver, args)"));
    assert!(source.contains("return result;"));
    assert!(source.contains("throw thrown;"));
    for forbidden in [
        ".then(",
        "async ",
        "construct(",
        "JSON.stringify(",
        "eval(",
        "import ",
    ] {
        assert!(!source.contains(forbidden), "unexpected {forbidden}");
    }
    assert!(source.contains("Object.getOwnPropertyDescriptor"));
    assert!(
        source.contains("Object.defineProperty(receiver, key, { ...property, value: wrapped })")
    );
    assert!(source.contains("unsupported accessor property"));
    assert!(source.contains("unsupported inherited final property"));
    assert!(source.contains("!property.configurable && !property.writable"));
    assert!(source.contains("try { __rxdConsoleLog("));
    assert!(source.contains("__rxdLog(\"failure\", error);\n  throw error;"));
    let stages = ["web.target", "web.replaceability", "web.install"];
    let offsets: Vec<_> = stages
        .iter()
        .map(|stage| source.find(stage).unwrap())
        .collect();
    assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn web_intrinsic_targets_use_invocation_and_logging_captured_before_replacement() {
    for target in ["Reflect.apply", "console.log"] {
        let source = snippet(target);
        support::validate_js(&source);
        let replacement = source.find("Object.defineProperty(receiver, key").unwrap();
        assert!(source.find("const __rxdApply = Reflect.apply;").unwrap() < replacement);
        assert!(source.find("console.log.bind(console)").unwrap() < replacement);
        assert!(source.contains("const result = __rxdApply(original, receiver, args);"));
        assert!(!source.contains("Reflect.apply(original, receiver, args)"));
        assert!(!source.contains("try { console.log("));
        assert!(source.contains("try { __rxdConsoleLog("));
        assert!(source.contains("return result;") && source.contains("throw thrown;"));
    }
}

#[test]
fn web_paths_escape_unicode_reserved_names_and_reject_expression_or_prototype_attacks() {
    let unicode = snippet(r#"window.app["default"]["签名\"\\\n\u2028</script>"]"#);
    support::validate_js(&unicode);
    assert!(!unicode.contains("</script>"));
    assert!(unicode.contains("\\u003c"));
    assert_eq!(
        snippet(r#"app["\u0066etch"]"#),
        snippet("globalThis.app.fetch")
    );
    for target in [
        "",
        "globalThis",
        "window",
        "fetch()",
        "window.app?.sign",
        "app[sign]",
        "app[0]",
        "app['sign']",
        "window.签名",
        "window.#private",
        "window.app;alert(1)",
        "window.app[\"x\" + run()]",
        "window.__proto__.x",
        "app[\"__pr\\u006fto__\"].x",
        "app.constructor.prototype.x",
        "app..x",
        "window. app",
        " app.sign",
        "app.sign ",
    ] {
        let error = generate(&Request::Web(request(target)), Limits::default()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidArguments, "{target}");
        assert_eq!(error.exit_code(), 2);
    }
}

#[test]
fn web_extension_has_identical_payload_and_publishes_complete_pinned_mv3_offline() {
    let artifacts = extension();
    let payload = artifacts
        .files
        .iter()
        .find(|file| file.name == "hook.js")
        .unwrap();
    assert_eq!(payload.contents, snippet("globalThis.fetch").as_bytes());
    let temporary = tempfile::tempdir().unwrap();
    let destination = temporary.path().join("extension");
    let receipt = publish_extension(&artifacts, &destination, Limits::default()).unwrap();
    assert_eq!(receipt.directory, destination);
    assert_eq!(receipt.files.len(), 2);
    assert_eq!(
        receipt.bytes,
        artifacts
            .files
            .iter()
            .map(|file| file.contents.len())
            .sum::<usize>()
    );
    support::validate_extension(&destination);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(destination.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["minimum_chrome_version"], "111");
    assert_eq!(manifest["content_scripts"][0]["world"], "MAIN");
    assert_eq!(manifest["content_scripts"][0]["run_at"], "document_start");
    assert_eq!(manifest["content_scripts"][0]["all_frames"], false);
    assert_eq!(
        manifest["content_scripts"][0]["matches"],
        serde_json::json!(["https://*.example.com/*"])
    );
    assert!(manifest.get("permissions").is_none());
}

#[test]
fn web_wait_is_explicit_bounded_and_cleanup_keeps_setup_errors_distinct() {
    let source = source(web::Request {
        wait_ms: Some(250),
        ..request("window.app.sign")
    });
    support::validate_js(&source);
    assert!(source.contains("const waitMs = 250;"));
    assert!(source.contains("performance.now() + waitMs"));
    assert!(source.contains("clearTimeout(timer)"));
    assert!(source.contains("timer = null;"));
    assert!(source.contains("cleanup();\n    __rxdLog(\"failure\", error);\n    throw error;"));
    assert!(source.contains("__rxdLog(\"timeout\")"));
    let attempt = &source[source.find("function attempt()").unwrap()..];
    assert!(
        attempt.find("remaining <= 0").unwrap() < attempt.find("if (install())").unwrap(),
        "deadline must be checked before a late retry installs"
    );
    assert!(!snippet("app.sign").contains("setTimeout("));
    for wait in [0, u32::MAX] {
        assert_eq!(
            generate(
                &Request::Web(web::Request {
                    wait_ms: Some(wait),
                    ..request("app.sign")
                }),
                Limits::default()
            )
            .unwrap_err()
            .kind,
            ErrorKind::InvalidArguments
        );
    }
}

#[test]
fn web_options_and_all_artifact_budgets_fail_without_source_or_files() {
    for pattern in [
        "",
        "example.com/*",
        "https://",
        "https://example.com",
        "https://user@example.com/*",
        "https://example.com:443/*",
        "https://*example.com/*",
        "https://foo.*/*",
        "https://-bad.com/*",
        "file://example.com/*",
        "javascript://example.com/*",
        "https://example.com/\n*",
        "https://example.com/\\*",
    ] {
        assert_eq!(
            generate(
                &Request::Web(web::Request {
                    emit: web::Emit::Extension,
                    matches: vec![pattern.into()],
                    ..request("fetch")
                }),
                Limits::default()
            )
            .unwrap_err()
            .kind,
            ErrorKind::InvalidArguments,
            "{pattern}"
        );
    }
    for pattern in [
        "https://example.com/*",
        "*://*.example.com/path*",
        "file:///*",
        "http://localhost/*",
        "http://127.0.0.1/*",
        "<all_urls>",
    ] {
        generate(
            &Request::Web(web::Request {
                emit: web::Emit::Extension,
                matches: vec![pattern.into()],
                ..request("fetch")
            }),
            Limits::default(),
        )
        .unwrap();
    }
    assert_eq!(
        generate(
            &Request::Web(web::Request {
                emit: web::Emit::Extension,
                ..request("fetch")
            }),
            Limits::default()
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidArguments
    );
    assert_eq!(
        generate(
            &Request::Web(web::Request {
                matches: vec!["https://example.com/*".into()],
                ..request("fetch")
            }),
            Limits::default()
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidArguments
    );
    for limits in [
        Limits {
            target_bytes: 2,
            ..Limits::default()
        },
        Limits {
            source_bytes: 64,
            ..Limits::default()
        },
        Limits {
            artifact_bytes: 64,
            ..Limits::default()
        },
        Limits {
            expanded_bytes: 2,
            ..Limits::default()
        },
        Limits {
            analysis_steps: 2,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            generate(&Request::Web(request("globalThis.fetch")), limits)
                .unwrap_err()
                .kind,
            ErrorKind::ResourceLimit
        );
    }
    let artifacts = extension();
    let temporary = tempfile::tempdir().unwrap();
    for limits in [
        Limits {
            artifact_files: 1,
            ..Limits::default()
        },
        Limits {
            artifact_bytes: 64,
            ..Limits::default()
        },
        Limits {
            source_bytes: 64,
            ..Limits::default()
        },
    ] {
        let destination = temporary.path().join("not-published");
        assert_eq!(
            publish_extension(&artifacts, &destination, limits)
                .unwrap_err()
                .kind,
            ErrorKind::ResourceLimit
        );
        assert!(!destination.exists());
    }
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 0);
}

#[test]
fn web_publication_refuses_existing_content_and_revalidates_mutable_artifacts_atomically() {
    let artifacts = extension();
    let temporary = tempfile::tempdir().unwrap();
    let destination = temporary.path().join("existing");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep"), "unchanged").unwrap();
    assert!(publish_extension(&artifacts, &destination, Limits::default()).is_err());
    assert_eq!(
        fs::read_to_string(destination.join("keep")).unwrap(),
        "unchanged"
    );
    fs::remove_file(destination.join("keep")).unwrap();
    publish_extension(&artifacts, &destination, Limits::default()).unwrap();
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 2);
    let file = temporary.path().join("file");
    fs::write(&file, "keep file").unwrap();
    assert!(publish_extension(&artifacts, &file, Limits::default()).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), "keep file");
    assert!(
        publish_extension(
            &artifacts,
            &temporary.path().join("missing-parent/out"),
            Limits::default()
        )
        .is_err()
    );

    let original_manifest: serde_json::Value =
        serde_json::from_slice(&artifacts.files[0].contents).unwrap();
    let edits = [
        ("/permissions", serde_json::json!(["tabs"])),
        ("/minimum_chrome_version", serde_json::json!("110")),
        ("/content_scripts/0/world", serde_json::json!("ISOLATED")),
        (
            "/content_scripts/0/run_at",
            serde_json::json!("document_idle"),
        ),
        ("/content_scripts/0/all_frames", serde_json::json!(true)),
        (
            "/content_scripts/0/js",
            serde_json::json!(["https://example.com/hook.js"]),
        ),
        ("/content_scripts/0/js", serde_json::json!(["missing.js"])),
        ("/content_scripts/0/matches", serde_json::json!([])),
    ];
    for (pointer, value) in edits {
        let mut manifest = original_manifest.clone();
        if pointer == "/permissions" {
            manifest["permissions"] = value;
        } else {
            *manifest.pointer_mut(pointer).unwrap() = value;
        }
        let mut edited = artifacts.clone();
        edited.files[0].contents = serde_json::to_vec(&manifest).unwrap();
        let output = temporary.path().join("rejected");
        assert!(
            publish_extension(&edited, &output, Limits::default()).is_err(),
            "{pointer}"
        );
        assert!(!output.exists());
    }
    for name in [
        "../escape.js",
        "C:\\escape.js",
        "sub/payload.js",
        "..",
        "hook.js\0",
        "manifest.json",
    ] {
        let mut edited = artifacts.clone();
        edited.files[1].name = name.into();
        assert!(
            publish_extension(
                &edited,
                &temporary.path().join("rejected"),
                Limits::default()
            )
            .is_err()
        );
    }
    for payload in [vec![0xff], vec![]] {
        let mut edited = artifacts.clone();
        edited.files[1].contents = payload;
        assert!(
            publish_extension(
                &edited,
                &temporary.path().join("rejected"),
                Limits::default()
            )
            .is_err()
        );
    }
    let mut extra = artifacts.clone();
    extra.files.push(rxd_hook_gen::ArtifactFile {
        name: "extra.js".into(),
        contents: b"unreferenced".to_vec(),
    });
    assert!(
        publish_extension(
            &extra,
            &temporary.path().join("rejected"),
            Limits::default()
        )
        .is_err()
    );
    let mut malformed = artifacts.clone();
    malformed.files[0].contents = b"{".to_vec();
    assert!(
        publish_extension(
            &malformed,
            &temporary.path().join("rejected"),
            Limits::default()
        )
        .is_err()
    );
    assert_eq!(
        fs::read_dir(temporary.path()).unwrap().count(),
        2,
        "no staging leftovers or partial outputs"
    );
}

#[cfg(unix)]
#[test]
fn web_publication_refuses_final_symlinks_but_accepts_canonicalized_parent_aliases() {
    use std::os::unix::fs::symlink;
    let artifacts = extension();
    let temporary = tempfile::tempdir().unwrap();
    let real = temporary.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = temporary.path().join("link");
    symlink(&real, &link).unwrap();
    assert!(publish_extension(&artifacts, &link, Limits::default()).is_err());
    assert_eq!(fs::read_dir(&real).unwrap().count(), 0);
    publish_extension(&artifacts, &link.join("out"), Limits::default()).unwrap();
    assert!(real.join("out/manifest.json").is_file());
    let dangling = temporary.path().join("dangling");
    symlink(temporary.path().join("absent"), &dangling).unwrap();
    assert!(publish_extension(&artifacts, &dangling, Limits::default()).is_err());
    assert!(!temporary.path().join("absent").exists());
}

#[test]
fn web_publication_rejects_windows_device_artifact_names_before_writing() {
    let mut artifacts = extension();
    artifacts.files[1].name = "CON.js".into();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&artifacts.files[0].contents).unwrap();
    manifest["content_scripts"][0]["js"] = serde_json::json!(["CON.js"]);
    artifacts.files[0].contents = serde_json::to_vec(&manifest).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    assert!(
        publish_extension(&artifacts, &temporary.path().join("out"), Limits::default()).is_err()
    );
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 0);
}

#[test]
fn web_publication_cleans_staging_on_write_failure_and_racing_publishers_do_not_clobber() {
    let artifacts = extension();
    let temporary = tempfile::tempdir().unwrap();
    let empty = temporary.path().join("empty");
    fs::create_dir(&empty).unwrap();
    let mut unwritable = artifacts.clone();
    let long_name = format!("{}.js", "x".repeat(300));
    unwritable.files[1].name = long_name.clone();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&unwritable.files[0].contents).unwrap();
    manifest["content_scripts"][0]["js"] = serde_json::json!([long_name]);
    unwritable.files[0].contents = serde_json::to_vec(&manifest).unwrap();
    assert!(publish_extension(&unwritable, &empty, Limits::default()).is_err());
    assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
    let destination = temporary.path().join("racing");
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    publish_extension(&artifacts, &destination, Limits::default())
                })
            })
            .collect();
        assert_eq!(
            tasks
                .into_iter()
                .map(|task| task.join().unwrap())
                .filter(Result::is_ok)
                .count(),
            1
        );
    });
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 2);
    for file in artifacts.files {
        assert_eq!(
            fs::read(destination.join(file.name)).unwrap(),
            file.contents
        );
    }
}

#[test]
fn web_emit_only_accepts_js_snippet_or_extension_and_publication_requires_destination() {
    assert_eq!("snippet".parse::<web::Emit>().unwrap(), web::Emit::Snippet);
    assert_eq!(
        "extension".parse::<web::Emit>().unwrap(),
        web::Emit::Extension
    );
    for emit in ["", "html", "ts", "puppeteer", "Extension"] {
        assert_eq!(
            emit.parse::<web::Emit>().unwrap_err().kind,
            ErrorKind::InvalidArguments
        );
    }
    assert!(publish_extension(&extension(), std::path::Path::new(""), Limits::default()).is_err());
}

#[test]
fn web_extension_and_snippet_share_escaped_target_and_wait_payload_exactly() {
    let request = web::Request {
        wait_ms: Some(500),
        ..request(r#"window.app["default"]["签名\"\n"]"#)
    };
    let expected = source(request.clone());
    let Artifact::Extension(artifacts) = generate(
        &Request::Web(web::Request {
            emit: web::Emit::Extension,
            matches: vec!["https://example.com/app*".into()],
            ..request
        }),
        Limits::default(),
    )
    .unwrap() else {
        panic!("expected extension")
    };
    assert_eq!(
        artifacts
            .files
            .iter()
            .find(|file| file.name == "hook.js")
            .unwrap()
            .contents,
        expected.as_bytes()
    );
    let directory = tempfile::tempdir().unwrap();
    publish_extension(&artifacts, &directory.path().join("out"), Limits::default()).unwrap();
    support::validate_extension(&directory.path().join("out"));
}
