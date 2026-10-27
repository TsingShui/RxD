//! Real CLI checks for Lua bytecode navigation and reconstruction.
use super::*;

fn input(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/lua")
        .join(name)
}

fn run(command: &str, path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["unluac-rs", command])
        .arg(path)
        .args(args)
        .env_remove("RXD_CACHE_DIR")
        .output()
        .unwrap()
}

/// Rows are TAB-separated: the first cell selects, the rest is the payload.
fn rows(output: &Output) -> Vec<Vec<String>> {
    success(output);
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.split('\t').map(str::to_owned).collect())
        .collect()
}

#[test]
fn lua_codes_lists_the_main_and_nested_prototype() {
    let rows = rows(&run("codes", &input("sample-5.4.luac"), &[]));
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0][0], "proto:0");
    assert_eq!(rows[1][0], "proto:1");
    // `luac -l` reports 0 params for main and 1 for the nested function.
    assert!(rows[0][1].contains("params=0 vararg=true"), "{rows:?}");
    assert!(rows[1][1].contains("params=1 vararg=false"), "{rows:?}");
    assert!(rows[1][1].contains("lines=2-4"), "{rows:?}");
}

#[test]
fn lua_strings_reports_pool_slots_with_the_owning_prototype() {
    let listed = rows(&run("strings", &input("sample-5.4.luac"), &[]));
    assert_eq!(
        listed,
        vec![
            vec![
                "proto:0:const:0".to_owned(),
                "\"authorization-token\"".to_owned()
            ],
            vec!["proto:1:const:0".to_owned(), "\":\"".to_owned()],
        ]
    );
    let filtered = rows(&run(
        "strings",
        &input("sample-5.4.luac"),
        &["-f", "author"],
    ));
    assert_eq!(filtered.len(), 1, "{filtered:?}");
}

#[test]
fn lua_instructions_preserve_compiler_order_and_byte_offsets() {
    let out = run(
        "instructions",
        &input("sample-5.4.luac"),
        &["--at", "proto:1"],
    );
    let instructions = rows(&out);
    assert_eq!(instructions.len(), 6, "{instructions:?}");
    // `luac -l` prints GETUPVAL, LOADK, MOVE, CONCAT, RETURN1, RETURN0.
    let opcodes: Vec<&str> = instructions
        .iter()
        .map(|row| row[1].split(' ').next().unwrap())
        .collect();
    assert_eq!(
        opcodes,
        ["GETUPVAL", "LOADK", "MOVE", "CONCAT", "RETURN1", "RETURN0"]
    );
    let offsets: Vec<usize> = instructions
        .iter()
        .map(|row| row[0].parse().unwrap())
        .collect();
    assert!(
        offsets.windows(2).all(|pair| pair[0] < pair[1]),
        "{offsets:?}"
    );
}

#[test]
fn lua_xref_points_at_the_loading_instruction() {
    let traced = rows(&run(
        "xref",
        &input("sample-5.4.luac"),
        &["--at", "proto:1:const:0"],
    ));
    assert_eq!(
        traced,
        vec![vec![
            "proto:1:ins:1".to_owned(),
            "proto:1".to_owned(),
            "LOADK 2 0".to_owned()
        ]]
    );
    // `-m` resolves a unique constant text, then traces it like `--at`.
    let matched = rows(&run("xref", &input("sample-5.4.luac"), &["-m", "token"]));
    assert_eq!(matched.len(), 1, "{matched:?}");
    assert_eq!(matched[0][1], "proto:0");
}

#[test]
fn lua_xref_distinguishes_absent_and_ambiguous_matches() {
    let absent = run(
        "xref",
        &input("sample-5.4.luac"),
        &["-m", "no-such-constant"],
    );
    assert!(!absent.status.success());
    assert!(
        String::from_utf8_lossy(&absent.stderr).contains("NOT_FOUND"),
        "{}",
        String::from_utf8_lossy(&absent.stderr)
    );
    let ambiguous = run("xref", &input("sample-5.4.luac"), &["-m", ""]);
    assert!(!ambiguous.status.success());
    assert!(
        String::from_utf8_lossy(&ambiguous.stderr).contains("AMBIGUOUS"),
        "{}",
        String::from_utf8_lossy(&ambiguous.stderr)
    );
}

#[test]
fn lua_decompile_reconstructs_the_original_statements() {
    let out = run("decompile", &input("sample-5.4.luac"), &[]);
    success(&out);
    let record: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(record["backend"], "lua");
    assert_eq!(record["dialect"], "lua5.4");
    let source = record["source"].as_str().unwrap();
    assert!(
        source.contains("local secret = \"authorization-token\""),
        "{source}"
    );
    assert!(source.contains("return greet"), "{source}");
}

#[test]
fn lua_rejects_input_that_is_not_a_chunk_without_partial_output() {
    let (_dir, path) = fixture(b"not a lua chunk", "fake.luac");
    let out = run("codes", &path, &[]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "partial stdout");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("INVALID_INPUT"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn lua_unknown_coordinate_is_a_query_error() {
    let out = run(
        "instructions",
        &input("sample-5.4.luac"),
        &["--at", "proto:99"],
    );
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "partial stdout");
}
