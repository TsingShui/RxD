use super::support::Client;
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path, process::Command};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

#[test]
fn mcp_ark_nested_navigation_matches_cli_and_preserves_hap_entry_selection() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("tests/corpus/abc/minimal.abc");
    let expected: Value = serde_json::from_str(include_str!("../corpus/abc/minimal.json")).unwrap();
    let record_at = expected["record"]["at"].as_str().unwrap();
    let mut client = Client::start(&[]);
    client.initialize();
    for (leaf, tool, flags) in [
        ("record", "abc_list_record", vec![]),
        ("method", "abc_list_method", vec!["--at", record_at]),
    ] {
        let cli = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["ark", "list", leaf])
            .arg(&input)
            .args(flags)
            .output()
            .unwrap();
        assert!(cli.status.success(), "{cli:?}");
        let mut arguments = json!({"input":{"path":input}});
        if leaf == "method" {
            arguments["at"] = json!(record_at);
        }
        let result = client.tool(tool, arguments);
        assert_ne!(result["isError"], true, "{result}");
        assert_eq!(
            result["structuredContent"]["text"],
            String::from_utf8(cli.stdout).unwrap()
        );
    }
    let method_at = expected["record"]["methods"][1]["at"].as_str().unwrap();
    let source = client.tool(
        "abc_decompile",
        json!({"input":{"path":input},"at":method_at}),
    );
    assert_ne!(source["isError"], true, "{source}");
    let cli_source = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["ark", "decompile"])
        .arg(&input)
        .args(["--at", method_at])
        .output()
        .unwrap();
    assert!(cli_source.status.success(), "{cli_source:?}");
    assert_eq!(
        source["structuredContent"]["text"],
        String::from_utf8(cli_source.stdout).unwrap()
    );
    let raw = client.tool(
        "abc_decompile",
        json!({"input":{"path":input},"at":method_at,"raw":true}),
    );
    assert_ne!(raw["isError"], true, "{raw}");
    assert!(
        raw["structuredContent"]["text"]
            .as_str()
            .unwrap()
            .contains("0x0\tldai 7\n0x5\treturn\n")
    );
    let directory = TempDir::new().unwrap();
    let hap = directory.path().join("multiple.hap");
    let mut archive = ZipWriter::new(fs::File::create(&hap).unwrap());
    for entry in ["ets/modules.abc", "ets/other.abc"] {
        archive.start_file(entry, FileOptions::default()).unwrap();
        archive
            .write_all(include_bytes!("../corpus/abc/minimal.abc"))
            .unwrap();
    }
    archive.finish().unwrap();
    let ambiguous = client.tool("abc_list_record", json!({"input":{"path":hap}}));
    assert_eq!(
        ambiguous["structuredContent"]["error"]["code"], "AMBIGUOUS",
        "{ambiguous}"
    );
    let selected = client.tool(
        "abc_list_method",
        json!({"input":{"path":hap},
        "entry":"ets/other.abc", "at":record_at}),
    );
    assert_ne!(selected["isError"], true, "{selected}");
    assert!(
        selected["structuredContent"]["text"]
            .as_str()
            .unwrap()
            .contains("加法")
    );
    client.disconnect();
    assert!(client.wait_exit().success());
}
