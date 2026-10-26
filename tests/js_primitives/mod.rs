use super::*;

#[test]
fn strings_filters_provenance_and_validated_source_regions() {
    let source = "function t(){const a=['/api','/login'];return a;}function d(i){return t()[i];}function target(){const x=d(0)+d(1);return 'raw';}function empty(){return 1;}target();const s='pre'+'fix';";
    let (_dir, path) = fixture(source, "filter.js");
    let all = jsonl(&invoke("strings", &path, &["--resolve-static"]));
    for resolution in ["literal", "static", "decoder"] {
        let rows = jsonl(&invoke(
            "strings",
            &path,
            &["--resolve-static", "--resolution", resolution],
        ));
        let expected: Vec<_> = all
            .iter()
            .filter(|r| {
                if resolution == "literal" {
                    r["detail"]["resolution"].is_null()
                } else {
                    r["detail"]["resolution"] == resolution
                }
            })
            .cloned()
            .collect();
        assert_eq!(rows, expected);
    }
    assert!(jsonl(&invoke("strings", &path, &["--resolution", "decoder"])).is_empty());
    let target = jsonl(&invoke("functions", &path, &["--name", "target"]));
    let id = target[0]["at"].as_str().unwrap();
    let inside = jsonl(&invoke(
        "strings",
        &path,
        &[
            "--within",
            id,
            "--resolve-static",
            "--resolution",
            "decoder",
        ],
    ));
    assert_eq!(inside.len(), 3);
    assert!(
        inside
            .iter()
            .all(|r| r["start"].as_u64() >= target[0]["start"].as_u64()
                && r["end"].as_u64() <= target[0]["end"].as_u64())
    );
    let fragment = inside[0]["context_at"].as_str().unwrap();
    assert_eq!(
        inside,
        jsonl(&invoke(
            "strings",
            &path,
            &[
                "--within",
                fragment,
                "--resolve-static",
                "--resolution",
                "decoder"
            ]
        ))
    );
    let symbols = jsonl(&invoke("symbols", &path, &["--name", "x"]));
    assert_eq!(
        inside,
        jsonl(&invoke(
            "strings",
            &path,
            &[
                "--within",
                symbols[0]["at"].as_str().unwrap(),
                "--resolve-static",
                "--resolution",
                "decoder"
            ]
        ))
    );
    let empty = jsonl(&invoke("functions", &path, &["--name", "empty"]));
    assert!(
        jsonl(&invoke(
            "strings",
            &path,
            &["--within", empty[0]["at"].as_str().unwrap()]
        ))
        .is_empty()
    );
    let bad = invoke("strings", &path, &["--within", "function_declaration:1:2"]);
    assert!(!bad.status.success());
    assert!(bad.stdout.is_empty());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("NOT_FOUND"));
    let dir = path.parent().unwrap();
    let bad = invoke("strings", dir, &["--within", id]);
    assert!(!bad.status.success());
    assert!(bad.stdout.is_empty());
    let (_dir, path) = fixture(
        "function work(){const x=['a','b'].join('');}Array.prototype.join=()=> 'WRONG';work();",
        "scope.js",
    );
    let work = jsonl(&invoke("functions", &path, &["--name", "work"]));
    assert!(
        jsonl(&invoke(
            "strings",
            &path,
            &[
                "--within",
                work[0]["at"].as_str().unwrap(),
                "--resolution",
                "static",
                "--resolve-static"
            ]
        ))
        .is_empty()
    );
}

#[test]
fn character_code_folds_preserve_utf16_positions_and_out_of_range_semantics() {
    let source = r#"const xor=String.fromCharCode('a'.charCodeAt(0)^2);const point=String.fromCodePoint('😀'.codePointAt(0));const nan=`N:${'a'.charCodeAt(9)}`;const absent=`U:${'a'.codePointAt(9)}`;const unit=String.fromCharCode('😀'.charCodeAt(0))+'suffix';const low=String.fromCharCode('😀'.charCodeAt(1))+'suffix';const defaulted=String.fromCharCode('ab'.charCodeAt(-0.5));"#;
    let (_dir, path) = fixture(source, "codes.js");
    for (name, wanted) in [
        ("xor", "c"),
        ("point", "😀"),
        ("nan", "N:NaN"),
        ("absent", "U:undefined"),
        ("defaulted", "a"),
    ] {
        let rows = jsonl(&invoke(
            "values",
            &path,
            &["--name", name, "--resolve-static"],
        ));
        assert_eq!(rows[0]["detail"]["value"]["value"], wanted, "{name}");
        assert_eq!(rows[0]["detail"]["value"]["resolution"], "static");
    }
    let raw = invoke("find", &path, &["-f", "suffix", "--resolve-static"]);
    success(&raw);
    let text = String::from_utf8(raw.stdout).unwrap();
    assert!(text.contains("\\ud83d"));
    assert!(text.contains("\\ude00"));
}

#[test]
fn closed_array_join_composes_primitive_and_decoder_values() {
    let source = "function t(){const a=['/api','/login'];return a;}function d(i){return t()[i];}const path=[d(0),d(1)].join('');fetch([d(0),d(1)].join(''));const primitive=[1,true,null,,''.codePointAt(9)].join('|');const comma=['a','b'].join();const empty=[].join('-');";
    let (_dir, path) = fixture(source, "join.js");
    let rows = jsonl(&invoke(
        "find",
        &path,
        &["-f", "/api/login", "--resolve-static"],
    ));
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r["detail"]["resolution"] == "decoder"));
    let resources = jsonl(&invoke("resources", &path, &["--resolve-static"]));
    assert_eq!(resources[0]["detail"]["name"], "/api/login");
    for (name, wanted) in [("primitive", "1|true|||"), ("comma", "a,b"), ("empty", "")] {
        let rows = jsonl(&invoke(
            "values",
            &path,
            &["--name", name, "--resolve-static"],
        ));
        assert_eq!(rows[0]["detail"]["value"]["value"], wanted, "{name}");
    }
    assert!(jsonl(&invoke("find", &path, &["-f", "/api/login"])).is_empty());
}

#[test]
fn primitive_folding_rejects_mutated_intrinsics_and_nonclosed_arrays() {
    for source in [
        "Object.getPrototypeOf('').charCodeAt=()=>1;const x=String.fromCharCode('a'.charCodeAt(0));",
        "Array.prototype.join=()=> 'WRONG';const x=['a','b'].join('');",
        "global.String.fromCodePoint=()=> 'WRONG';const x=String.fromCodePoint('a'.codePointAt(0));",
        "const arr=['a','b'];arr.reverse();const x=arr.join('');",
        "const x=[['a','b']].join('');",
        "const x=[...unknown].join('');",
        "const x=['a'].join(unknown);",
        "const x='abc'.split('').reverse().join('');",
        "const x=(3).charCodeAt(0);",
        "const x=[{toString(){return 'WRONG'}}].join('');",
    ] {
        let (_dir, path) = fixture(source, "unsafe.js");
        let rows = jsonl(&invoke(
            "values",
            &path,
            &["--name", "x", "--resolve-static"],
        ));
        assert_eq!(
            rows[0]["detail"]["value"]["resolution"], "unresolved",
            "{source}"
        );
    }
    let (_dir, path) = fixture(
        format!("const x=['a','b'].join('{}');", "s".repeat(32768)),
        "expanded.js",
    );
    let rows = jsonl(&invoke(
        "values",
        &path,
        &["--name", "x", "--resolve-static"],
    ));
    assert_eq!(rows[0]["detail"]["value"]["resolution"], "unresolved");
}
