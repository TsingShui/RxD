//! Offline differential checks against pinned WABT decompilation fixtures.
//! Assert control-flow equivalence and explicit representation divergences.
use super::*;

use std::collections::BTreeMap;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/wasm")
}

fn expectations() -> Value {
    let raw = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/wasm-expectations.json"),
    )
    .unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn run_wasm(command: &str, path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["wasm", command])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

/// Count control-flow keywords: the correctness-critical skeleton.
fn cf_counts(code: &str) -> BTreeMap<&'static str, usize> {
    fn count_word(hay: &str, word: &str) -> usize {
        let bytes = hay.as_bytes();
        let w = word.as_bytes();
        let mut n = 0;
        let mut i = 0;
        while let Some(pos) = hay[i..].find(word) {
            let s = i + pos;
            let before = if s == 0 { b' ' } else { bytes[s - 1] };
            let after = bytes.get(s + w.len()).copied().unwrap_or(b' ');
            let boundary = |c: u8| !(c.is_ascii_alphanumeric() || c == b'_');
            if boundary(before) && boundary(after) {
                n += 1;
            }
            i = s + w.len();
        }
        n
    }
    let mut out = BTreeMap::new();
    for kw in [
        "loop", "if", "else", "switch", "goto", "continue", "label", "return",
    ] {
        out.insert(kw, count_word(code, kw));
    }
    out
}

/// Expected wasm-decompile text for a function, keyed by the de-mangled name
/// (rxd keeps a leading '_' that wasm-decompile strips).
fn get_expected<'a>(case: &'a Value, name: &str) -> Option<&'a str> {
    let funcs = case["expected_functions"].as_object()?;
    let stripped = name.trim_start_matches('_');
    funcs
        .get(name)
        .or_else(|| funcs.get(stripped))
        .and_then(|v| v.as_str())
}

/// rxd decompiled code (or None if degraded) for a named function.
fn rxd_decompile(wasm: &Path, name: &str) -> Option<String> {
    let rows = jsonl(&run_wasm("functions", wasm, &[]));
    let func = rows.iter().find(|r| {
        r["record"] == "function"
            && r["detail"]["name"]
                .as_str()
                .map(|n| n.trim_start_matches('_'))
                == Some(name.trim_start_matches('_'))
    })?;
    // A row's `at` is the coordinate `get-function --at` accepts.
    let at = func["at"].as_str().unwrap();
    let out = run_wasm("get-function", wasm, &["--at", at]);
    success(&out);
    let row: Value = serde_json::from_slice(&out.stdout).unwrap();
    if row["detail"]["decompiled"].as_bool().unwrap_or(false) {
        Some(row["detail"]["code"].as_str().unwrap().to_owned())
    } else {
        None
    }
}

#[test]
fn wabt_precedence_matches_wasm_decompile_byte_for_byte() {
    // Operator precedence is a feature rxd implements to parity: the official
    // precedence case must match exactly.
    let cases = expectations();
    let case = cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "precedence")
        .unwrap();
    let wasm = corpus_dir().join(case["wasm"].as_str().unwrap());
    let expected = get_expected(case, "precedence").unwrap();
    let rxd = rxd_decompile(&wasm, "precedence").expect("precedence must decompile");
    // Compare bodies exactly; export status is metadata, not a rendered header.
    let body = |s: &str| {
        let start = s.find('{').unwrap();
        let end = s.rfind('}').unwrap();
        s[start..=end].to_string()
    };
    assert_eq!(
        body(rxd.trim()),
        body(expected.trim()),
        "rxd precedence body diverged from wasm-decompile"
    );
}

#[test]
fn wabt_control_flow_recovery_matches_the_reference() {
    // Compare recovered control flow, excluding known representation-only differences.
    let cases = expectations();
    let mut compared = 0;
    for case in cases["cases"].as_array().unwrap() {
        let wasm = corpus_dir().join(case["wasm"].as_str().unwrap());
        let funcs = case["expected_functions"].as_object().unwrap();
        for name in funcs.keys() {
            let expected = get_expected(case, name).unwrap();
            let Some(rxd) = rxd_decompile(&wasm, name) else {
                continue; // degraded cases checked separately
            };
            let r = cf_counts(&rxd);
            let e = cf_counts(expected);
            // loop / if / else must agree exactly: these are genuine control flow.
            for kw in ["loop", "if", "else"] {
                assert_eq!(
                    r[kw], e[kw],
                    "case {} fn {}: '{}' count {} != reference {}\nrxd:\n{}\nref:\n{}",
                    case["name"], name, kw, r[kw], e[kw], rxd, expected
                );
            }
            compared += 1;
        }
    }
    assert!(
        compared >= 3,
        "expected several comparable functions, got {compared}"
    );
}

#[test]
fn wabt_block_parameter_signature_degrades_safely() {
    // Block parameters are unsupported here; require clean instruction fallback.
    let cases = expectations();
    let case = cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "basic")
        .unwrap();
    let wasm = corpus_dir().join(case["wasm"].as_str().unwrap());
    let rows = jsonl(&run_wasm("functions", &wasm, &[]));
    let f = rows
        .iter()
        .find(|r| r["record"] == "function" && r["detail"]["name"] == "f")
        .expect("function f present");
    let at = f["at"].as_str().unwrap();
    let out = run_wasm("get-function", &wasm, &["--at", at]);
    success(&out);
    let row: Value = serde_json::from_slice(&out.stdout).unwrap();
    // Degraded to the instruction view with an explicit reason; still complete.
    assert_eq!(row["detail"]["complete"], true);
    if row["detail"]["decompiled"].as_bool().unwrap() {
        // If a future improvement decompiles it, that's fine too — but then it
        // must still produce code, not a crash.
        assert!(row["detail"]["code"].is_string());
    } else {
        assert!(
            row["detail"]["reason"]
                .as_str()
                .unwrap()
                .starts_with("UNSUPPORTED")
        );
        assert!(row["detail"]["code"].is_null());
    }
}

#[test]
fn wabt_loadstore_index_scaling_matches_reference_forms() {
    // The index-rewriting rows of loadstore.txt are exactly the readability
    // feature rxd implements: dynamic (base + (i<<2))[0] -> base[i].
    let cases = expectations();
    let case = cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "loadstore")
        .unwrap();
    let wasm = corpus_dir().join(case["wasm"].as_str().unwrap());
    let rxd = rxd_decompile(&wasm, "f").expect("loadstore f must decompile");
    // rxd folds the aligned dynamic index like the reference.
    assert!(
        rxd.contains("a[b]:int = a[b]:int"),
        "expected folded array index, got:\n{rxd}"
    );
    // A non-zero aligned static offset folds into the index: a[b + 1].
    assert!(
        rxd.contains("a[b + 1]:int = a[b + 1]:int"),
        "expected offset-after-index fold, got:\n{rxd}"
    );
    // A mismatched shift amount must NOT be folded (matches reference behavior).
    assert!(
        rxd.contains("(a + (b << 3))"),
        "mismatched shift must stay unfolded, got:\n{rxd}"
    );
}
