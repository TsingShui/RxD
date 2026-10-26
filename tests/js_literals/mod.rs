use super::*;

#[test]
fn intrinsic_composition_rejects_global_roots_and_runtime_dependencies() {
    let base = "function t(){const a=['47'];return a;}function d(i){return t()[i];}";
    let tail = "const x=String.fromCharCode(+d(0));const y=d(0)+'!';";
    for effect in [
        "global.String.fromCharCode=()=> 'WRONG';",
        "global['String'].fromCharCode=()=> 'WRONG';",
        "this.String.fromCharCode=()=> 'WRONG';",
        "this['String'].fromCharCode=()=> 'WRONG';",
        "require('./mutate.cjs');",
        "const load=require;load('./mutate.cjs');",
        "(0,require)('./mutate.cjs');",
        "module.require('./mutate.cjs');",
        "import('./mutate.mjs');",
        "import './mutate.mjs';",
        "export * from './mutate.mjs';",
    ] {
        let (_dir, path) = fixture(format!("{base}{effect}{tail}"), "intrinsics.mjs");
        let mut value_args = vec!["--name", "x", "--resolve-static"];
        let mut find_args = vec!["-f", "47!", "--resolve-static"];
        if effect.starts_with("this") {
            value_args.extend(["--source-type", "script"]);
            find_args.extend(["--source-type", "script"]);
        }
        let rows = jsonl(&invoke("values", &path, &value_args));
        assert_eq!(
            rows[0]["detail"]["value"]["resolution"], "unresolved",
            "{effect}"
        );
        let rows = jsonl(&invoke("find", &path, &find_args));
        assert_eq!(rows.len(), 1, "{effect}");
        assert_eq!(rows[0]["detail"]["resolution"], "decoder");
    }
    let (_dir, path) = fixture(
        format!("import type {{Unused}} from './types.mjs';{base}{tail}"),
        "types.mts",
    );
    let rows = jsonl(&invoke(
        "values",
        &path,
        &["--name", "x", "--resolve-static"],
    ));
    assert_eq!(rows[0]["detail"]["value"]["value"], "/");
}

#[test]
fn exact_decoder_facts_keep_their_admission_above_composition_limits() {
    for units in [32768, 32769] {
        let literal = "a".repeat(units);
        let source = format!(
            "function t(){{const a=['{literal}','needle'];return a;}}function d(i){{return t()[i];}}const x=d(0);const composed=d(0)+'!';obj[d(0)];const y=d(1);"
        );
        let (_dir, path) = fixture(source, "large-fact.js");
        let rows = jsonl(&invoke(
            "values",
            &path,
            &["--name", "x", "--resolve-static"],
        ));
        assert_eq!(rows[0]["detail"]["value"]["resolution"], "decoder");
        assert_eq!(
            rows[0]["detail"]["value"]["value"].as_str().unwrap().len(),
            units
        );
        let rows = jsonl(&invoke(
            "find",
            &path,
            &["-f", "needle", "--resolve-static"],
        ));
        assert_eq!(
            rows.iter()
                .filter(|r| r["detail"]["resolution"] == "decoder")
                .count(),
            1
        );
        let rows = jsonl(&invoke(
            "values",
            &path,
            &["--name", "composed", "--resolve-static"],
        ));
        assert_eq!(rows[0]["detail"]["value"]["resolution"], "unresolved");
    }
}

#[test]
fn visible_prototype_capabilities_disable_intrinsic_dependent_composition() {
    for mutation in [
        "Object.getPrototypeOf('').slice=()=> 'WRONG';",
        "Object['get'+'PrototypeOf']('').slice=()=> 'WRONG';",
        "const {getPrototypeOf:p}=Object;p('').slice=()=> 'WRONG';",
        "''.__proto__.slice=()=> 'WRONG';",
        "const getter=''.__lookupGetter__('__proto__');getter.call('').slice=()=> 'WRONG';",
        "Object[d(1)+d(2)]('').slice=()=> 'WRONG';",
        "const proto=''.__proto__;Object.defineProperty(proto,'slice',{value:()=> 'WRONG'});",
    ] {
        let source = format!(
            "function t(){{const a=['abc','get','PrototypeOf'];return a;}}function d(i){{return t()[i];}}{mutation}const x=d(0).slice(0,1);const y=d(0)+'!';"
        );
        let (_dir, path) = fixture(source, "prototype.js");
        let rows = jsonl(&invoke(
            "values",
            &path,
            &["--name", "x", "--resolve-static"],
        ));
        assert_eq!(
            rows[0]["detail"]["value"]["resolution"], "unresolved",
            "{mutation}"
        );
        let rows = jsonl(&invoke("find", &path, &["-f", "abc!", "--resolve-static"]));
        assert_eq!(rows.len(), 1, "{mutation}");
        assert_eq!(rows[0]["detail"]["resolution"], "decoder");
    }
}

#[test]
fn strings_production_occurrences_match_independent_acorn_oracle() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let expected: Value =
        serde_json::from_slice(&fs::read(root.join("strings-expectations.json")).unwrap()).unwrap();
    for case in expected["cases"].as_array().unwrap() {
        let path = root.join(case["file"].as_str().unwrap());
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(&path).unwrap())),
            case["sha256"]
        );
        let rows = jsonl(&invoke("strings", &path, &[]));
        let tuples: Vec<_> = rows
            .iter()
            .map(|r| serde_json::json!([r["start"], r["end"], r["detail"]["value"]]))
            .collect();
        assert_eq!(rows.len(), case["occurrences"].as_u64().unwrap() as usize);
        assert_eq!(
            format!("{:x}", Sha256::digest(serde_json::to_vec(&tuples).unwrap())),
            case["occurrences_sha256"]
        );
        if path.file_name().unwrap() == "application-rc4.js" {
            assert!(
                jsonl(&invoke(
                    "strings",
                    &path,
                    &["-f", "/api/login", "--resolve-static"]
                ))
                .is_empty()
            );
        }
    }
}

#[test]
fn strings_directory_failures_are_atomic_and_keep_file_coordinates() {
    let dir = tempfile::tempdir().unwrap();
    let sources = dir.path().join("src");
    fs::create_dir(&sources).unwrap();
    fs::write(sources.join("a.js"), "const a='same';").unwrap();
    fs::write(sources.join("b.ts"), "const b:string='SAME';").unwrap();
    let rows = jsonl(&invoke("strings", &sources, &["-f", "same"]));
    assert_eq!(rows.iter().filter(|r| r["record"] == "file").count(), 2);
    assert_eq!(rows.iter().filter(|r| r["record"] == "match").count(), 2);
    let output = dir.path().join("output.jsonl");
    fs::write(&output, "sentinel").unwrap();
    fs::write(sources.join("z.js"), "invalid {{ {").unwrap();
    let failure = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["js", "strings"])
        .arg(&sources)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(!failure.status.success());
    assert!(failure.stdout.is_empty());
    assert_eq!(fs::read_to_string(&output).unwrap(), "sentinel");
    let empty = dir.path().join("empty");
    fs::create_dir(&empty).unwrap();
    let failure = invoke("strings", &empty, &["-f", ""]);
    assert!(!failure.status.success());
    assert!(failure.stdout.is_empty());
}

#[test]
fn composed_utf16_and_intrinsic_guards_preserve_exact_values() {
    let source = r#"function t(){const a=['\uD83D','\uDE00','\uD800'];return a;}function d(i){return t()[i];}const pair=d(0)+d(1);const lone=d(2)+'suffix';"#;
    let (_dir, path) = fixture(source, "utf16.js");
    let rows = jsonl(&invoke("find", &path, &["-f", "😀", "--resolve-static"]));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["detail"]["resolution"], "decoder");
    assert_eq!(rows[0]["detail"]["raw"], "d(0)+d(1)");
    let output = invoke("strings", &path, &["-f", "suffix", "--resolve-static"]);
    success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.lines()
            .any(|l| l.contains("\\ud800") && l.contains("\"resolution\":\"decoder\""))
    );
    let (_dir, path) = fixture(
        "function t(){const a=['47'];return a;}function d(i){return t()[i]}function String(){}String.fromCharCode(d(0));unknown(d(0))+'!';",
        "shadow.js",
    );
    assert!(jsonl(&invoke("find", &path, &["-f", "/", "--resolve-static"])).is_empty());
    assert!(jsonl(&invoke("find", &path, &["-f", "47!", "--resolve-static"])).is_empty());
}

#[test]
fn strings_enumerate_occurrences_without_guessing_a_needle() {
    let source = "const a=['same','same','', 'Ä'];const x=`pre${runtime}post`;";
    let (_dir, path) = fixture(source, "literals.js");
    let all = jsonl(&invoke("strings", &path, &[]));
    assert_eq!(all.len(), 6, "{all:?}");
    assert_eq!(
        all.iter()
            .filter(|r| r["detail"]["value"] == "same")
            .count(),
        2
    );
    assert!(all.iter().any(|r| r["detail"]["value"] == ""));
    assert_eq!(
        all.iter()
            .filter(|r| r["detail"]["dynamic"] == true)
            .count(),
        2
    );
    assert_eq!(all, jsonl(&invoke("strings", &path, &[])));
    for needle in ["same", "ä", "pre", "absent"] {
        let listed = jsonl(&invoke("strings", &path, &["-f", needle]));
        assert_eq!(listed, jsonl(&invoke("find", &path, &["-f", needle])));
    }
    assert!(
        jsonl(&invoke(
            "strings",
            &path,
            &["-f", "ä", "--case", "sensitive"]
        ))
        .is_empty()
    );
    for row in &all {
        let context = row["context_at"].as_str().unwrap();
        let bytes = invoke("slice", &path, &["--at", context]);
        success(&bytes);
        assert!(!bytes.stdout.is_empty());
    }
    let missing = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["js", "find"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!missing.status.success());
}

#[test]
fn decoder_composition_is_shared_but_opt_in_and_source_backed() {
    let source = "function t(){const a=['/api','/login','lo','g','47'];return a;}function d(i){return t()[i];}const path=d(0)+d(1);const o={[d(2)+d(3)](){}};o[d(2)+d(3)]();fetch(d(0)+d(1));const c=String.fromCharCode(+d(4));const text=`X${d(0)}${d(1)}`;";
    let (_dir, path) = fixture(source, "composed.js");
    assert!(jsonl(&invoke("find", &path, &["-f", "/api/login"])).is_empty());
    let rows = jsonl(&invoke(
        "find",
        &path,
        &["-f", "/api/login", "--resolve-static"],
    ));
    assert!(rows.len() >= 3, "{rows:?}");
    assert!(rows.iter().all(|r| r["detail"]["resolution"] == "decoder"));
    let listed = jsonl(&invoke(
        "strings",
        &path,
        &["-f", "/api/login", "--resolve-static"],
    ));
    assert_eq!(rows, listed);
    let values = jsonl(&invoke(
        "values",
        &path,
        &[
            "--kind",
            "initializer",
            "--name",
            "path",
            "--resolve-static",
        ],
    ));
    assert_eq!(values[0]["detail"]["value"]["value"], "/api/login");
    assert_eq!(values[0]["detail"]["value"]["resolution"], "decoder");
    let resources = jsonl(&invoke("resources", &path, &["--resolve-static"]));
    assert_eq!(resources[0]["detail"]["name"], "/api/login");
    assert_eq!(resources[0]["detail"]["resolution"], "decoder");
    assert!(
        !jsonl(&invoke(
            "calls",
            &path,
            &["--name", "log", "--resolve-static"]
        ))
        .is_empty()
    );
    assert!(
        !jsonl(&invoke(
            "members",
            &path,
            &["--name", "log", "--resolve-static"]
        ))
        .is_empty()
    );
    let methods = jsonl(&invoke(
        "methods",
        &path,
        &["-f", "log", "--resolve-static"],
    ));
    assert_eq!(methods.len(), 1);
    assert_eq!(methods[0]["detail"]["label_origin"], "decoder");
    assert!(methods[0]["detail"]["name"].is_null());
    let chars = jsonl(&invoke(
        "values",
        &path,
        &["--name", "c", "--resolve-static"],
    ));
    assert_eq!(chars[0]["detail"]["value"]["value"], "/");
    for row in rows {
        let start = row["start"].as_u64().unwrap() as usize;
        let end = row["end"].as_u64().unwrap() as usize;
        assert_eq!(row["detail"]["raw"], &source[start..end]);
    }
}

#[test]
fn composition_does_not_bypass_privacy_mutation_or_add_factory_read_inference() {
    for tail in [
        "[][d(0)+d(1)];d(2)+'!';",
        "const key=d(0);obj[key];d(2)+'!';",
        "t().reverse();d(2)+'!';",
        "d=other;d(2)+'!';",
    ] {
        let source = format!(
            "function t(){{const a=['con','structor','needle'];return a;}}function d(i){{return t()[i];}}{tail}"
        );
        let (_dir, path) = fixture(source, "unsafe.js");
        assert!(
            jsonl(&invoke(
                "find",
                &path,
                &["-f", "needle!", "--resolve-static"]
            ))
            .is_empty()
        );
    }
    let (_dir, path) = fixture(
        "function t(){const a=['/api','/login'];return a;}t()[0]+t()[1];",
        "factory.js",
    );
    assert!(
        jsonl(&invoke(
            "find",
            &path,
            &["-f", "/api/login", "--resolve-static"]
        ))
        .is_empty()
    );
}
