//! Final returned-ID workflows and fixed EVM stream schemas through the actual CLI.
use super::*;

pub(super) const HEADER_DETAIL_KEYS: &[&str] = &[
    "carrier",
    "bytecode",
    "offset_unit",
    "basic_block_end",
    "boundary_status",
    "target_kind",
    "target_decoded_offset",
    "target_semantics",
    "coverage",
    "literal_scope",
    "excluded_unindexed_pushes",
    "discovery_complete",
    "disassembly_complete",
    "undecoded_tail",
    "source_call_graph_complete",
    "abi_verified",
    "data_scope",
    "metadata_range",
    "copy_witnesses",
    "unknown_copy_witnesses",
    "unvisited_copy_sites",
    "data_reference_coverage",
    "unknown_copy_evidence",
    "metadata",
];

fn assert_stream_schema(rows: &[Value]) {
    assert_js_keys(&rows[0]["detail"], HEADER_DETAIL_KEYS);
}

#[test]
fn evm_navigation_streams_share_one_fixed_nullable_analysis_record() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/evm");
    for encoding in ["raw", "hex"] {
        let suffix = if encoding == "raw" { "bin" } else { "hex" };
        let path = root.join(format!("authored-navigation-legacy.runtime.{suffix}"));
        let outline = jsonl(&run("interface", &path, encoding, &[]));
        assert_stream_schema(&outline);
        assert_eq!(outline[0]["detail"]["target_kind"], Value::Null);
        for args in [
            vec!["--kind", "number", "--query", "7"],
            vec!["--kind", "text", "--query", "RXD_DATA_NAVIGATION"],
            vec!["--kind", "text", "--query", "absent-final-acceptance"],
        ] {
            let rows = jsonl(&run("strings", &path, encoding, &args));
            assert_stream_schema(&rows);
            for literal in rows
                .iter()
                .filter(|r| r["record"] == "literal" || r["record"] == "data_literal")
            {
                let refs = jsonl(&run(
                    "xref",
                    &path,
                    encoding,
                    &["--at", literal["at"].as_str().unwrap()],
                ));
                assert_stream_schema(&refs);
            }
        }
        for unit in outline
            .iter()
            .filter(|r| r["record"] == "function" || r["record"] == "basic_block")
            .take(10)
        {
            assert_stream_schema(&jsonl(&run(
                "xref",
                &path,
                encoding,
                &["--at", unit["at"].as_str().unwrap()],
            )));
        }
        let (_empty_dir, empty) = fixture(
            if encoding == "raw" {
                b"".as_slice()
            } else {
                b"0x\n".as_slice()
            },
            "empty",
        );
        assert_stream_schema(&jsonl(&run("interface", &empty, encoding, &[])));
        assert_stream_schema(&jsonl(&run(
            "strings",
            &empty,
            encoding,
            &["--kind", "text"],
        )));
    }
}

#[test]
fn evm_function_fetch_has_a_distinct_fixed_record_for_raw_supported_and_fallback_targets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/evm");
    let detail_keys = [
        "unit_kind",
        "selector",
        "dispatch",
        "decoded_entry",
        "entry_evidence",
        "entry_state_status",
        "entry_states",
        "included_block_ids",
        "segments",
        "blocks",
        "instruction_count",
        "context_count",
        "boundary_status",
        "boundary_complete",
        "ownership_status",
        "cross_entries",
        "unknown_successors",
        "discovery_complete",
        "carrier",
        "bytecode",
        "instructions",
        "control_flow",
        "unresolved_evidence",
        "reconstruction",
        "complete",
    ];
    for encoding in ["raw", "hex"] {
        let path = root.join(format!(
            "authored-navigation.runtime.{}",
            if encoding == "raw" { "bin" } else { "hex" }
        ));
        let rows = jsonl(&run("interface", &path, encoding, &[]));
        let production = root.join(format!(
            "openzeppelin-erc20.runtime.{}",
            if encoding == "raw" { "bin" } else { "hex" }
        ));
        for (selected_path, selector, expected) in [
            (&path, "1ba64846", "success"),
            (&path, "0bb0b13a", "success"),
            (&path, "188b85b4", "success"),
            (&path, "e48c8755", "success"),
            (&path, "e21f37ce", "success"),
            (&production, "095ea7b3", "raw_fallback"),
        ] {
            let rows = if selected_path == &path {
                rows.clone()
            } else {
                jsonl(&run("interface", selected_path, encoding, &[]))
            };
            let candidate = rows
                .iter()
                .find(|r| r["record"] == "function" && r["detail"]["selector"] == selector)
                .unwrap();
            let id = candidate["at"].as_str().unwrap();
            for raw in [false, true] {
                let args = if raw {
                    vec!["--at", id, "--raw"]
                } else {
                    vec!["--at", id]
                };
                let output = run("get-function", selected_path, encoding, &args);
                success(&output);
                let row: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(row["record"], "function_code");
                assert_eq!(row["at"], candidate["at"]);
                assert_js_keys(&row["detail"], &detail_keys);
                assert_eq!(
                    row["detail"]["reconstruction"]["status"],
                    if raw { "not_requested" } else { expected }
                );
            }
        }
    }
}

#[test]
fn evm_embedded_and_enumerated_basic_blocks_share_a_fixed_nullable_schema() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/evm/authored-navigation.runtime.bin");
    let rows = jsonl(&run("interface", &path, "raw", &[]));
    let keys = [
        "unit_kind",
        "decoded_start",
        "decoded_end",
        "last_opcode_start",
        "instruction_count",
        "upstream_block_id",
        "boundary_status",
        "owner_ids",
        "shared",
    ];
    for row in rows.iter().filter(|r| r["record"] == "basic_block") {
        assert_js_keys(&row["detail"], &keys);
        assert_eq!(row["detail"]["owner_ids"], Value::Null);
        assert_eq!(row["detail"]["shared"], Value::Null);
    }
    for row in rows.iter().filter(|r| r["record"] == "function") {
        for block in row["detail"]["blocks"].as_array().unwrap() {
            assert_js_keys(&block["detail"], &keys);
            assert!(block["detail"]["owner_ids"].is_array());
            assert!(block["detail"]["shared"].is_boolean());
        }
    }
}

fn fetch_context(path: &Path, encoding: &str, at: &str, raw: bool) -> Value {
    // A short coordinate names its kind after `<encoding>:`; a whole content-bound ID
    // names it after the carrier hash.
    let kind = at.split(':').nth(1);
    let command = if kind == Some("function") {
        "get-function"
    } else {
        "get-block"
    };
    let mut args = vec!["--at", at];
    if raw && command == "get-function" {
        args.push("--raw");
    }
    let output = run(command, path, encoding, &args);
    success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_original_segments(row: &Value, carrier: &[u8], decoded: &[u8], encoding: &str) {
    for segment in row["detail"]["segments"].as_array().unwrap() {
        let start = segment["decoded_start"].as_u64().unwrap() as usize;
        let end = segment["decoded_end"].as_u64().unwrap() as usize;
        let expected = decoded[start..end]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(segment["raw_hex"], expected);
        let cs = segment["carrier_start"].as_u64().unwrap() as usize;
        let ce = segment["carrier_end"].as_u64().unwrap() as usize;
        if encoding == "raw" {
            assert_eq!(&carrier[cs..ce], &decoded[start..end]);
        } else {
            assert_eq!(&carrier[cs..ce], expected.as_bytes());
        }
    }
}

#[test]
fn evm_final_returned_id_workflows_reconstruct_known_behavior_and_preserve_original_evidence() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/evm");
    let independent: Value =
        serde_json::from_slice(&fs::read(root.join("acceptance-expectations.json")).unwrap())
            .unwrap();
    for encoding in ["raw", "hex"] {
        let suffix = if encoding == "raw" { "bin" } else { "hex" };
        let path = root.join(format!("authored-navigation.runtime.{suffix}"));
        let carrier = fs::read(&path).unwrap();
        let decoded = fs::read(root.join("authored-navigation.runtime.bin")).unwrap();
        let outline = jsonl(&run("interface", &path, encoding, &[]));
        let add = outline
            .iter()
            .find(|r| r["record"] == "function" && r["detail"]["selector"] == "1ba64846")
            .unwrap();
        let add_id = add["at"].as_str().unwrap();
        let row = fetch_context(&path, encoding, add_id, false);
        assert_eq!(row["record"], "function_code");
        assert_eq!(
            row["detail"]["reconstruction"]["control_flow_status"],
            "structured_acyclic"
        );
        assert_original_segments(&row, &carrier, &decoded, encoding);
        let expected = &independent["samples"][0]["functions"][0];
        assert_eq!(row["detail"]["decoded_entry"], expected["entry_offset"]);
        for fact in expected["instruction_facts"].as_array().unwrap() {
            let instruction = row["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["decoded_offset"] == fact["offset"])
                .unwrap();
            assert_eq!(instruction["decoded_end"], fact["end"]);
            assert_eq!(instruction["opcode"], fact["opcode"]);
            if let Some(immediate) = fact.get("immediate") {
                assert_eq!(instruction["operand_hex"], *immediate);
            }
        }
        for args in [
            vec!["--kind", "number", "--query", "7"],
            vec!["--kind", "bytes", "--query", "07"],
            vec!["--kind", "text", "--query", "RXD_DATA_NAVIGATION"],
        ] {
            let search = jsonl(&run("strings", &path, encoding, &args));
            let literal = search
                .iter()
                .find(|r| {
                    r["record"] == "literal"
                        && r["detail"]["instruction_offset"]
                            == if args[1] == "text" { 213 } else { 537 }
                })
                .unwrap();
            let refs = jsonl(&run(
                "xref",
                &path,
                encoding,
                &["--at", literal["at"].as_str().unwrap()],
            ));
            let reference = refs
                .iter()
                .find(|r| r["record"] == "literal_reference")
                .unwrap();
            let context = reference["context_at"].as_str().unwrap();
            let raw = fetch_context(&path, encoding, context, true);
            let reconstructed = fetch_context(&path, encoding, context, false);
            assert_eq!(
                reconstructed["detail"]["reconstruction"]["status"],
                "success"
            );
            assert_eq!(
                raw["detail"]["instructions"],
                reconstructed["detail"]["instructions"]
            );
            assert_original_segments(&reconstructed, &carrier, &decoded, encoding);
            if args[1] != "text" {
                assert_eq!(context, add_id);
            }
        }
        let blocks = jsonl(&run("blocks", &path, encoding, &[]));
        let block = blocks
            .iter()
            .find(|r| r["record"] == "basic_block" && r["detail"]["decoded_start"] == 152)
            .unwrap();
        for target in [block["at"].as_str().unwrap(), add_id] {
            let refs = jsonl(&run("xref", &path, encoding, &["--at", target]));
            let reference = refs
                .iter()
                .find(|r| {
                    r["record"] == "code_reference"
                        && (target == add_id || r["context_at"] == add_id)
                })
                .unwrap();
            let caller = fetch_context(
                &path,
                encoding,
                reference["context_at"].as_str().unwrap(),
                false,
            );
            let pc = reference["detail"]["instruction_offset"].as_u64().unwrap() as usize;
            assert_eq!(reference["detail"]["opcode"], decoded[pc]);
            assert!(
                caller["detail"]["instructions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|i| i["decoded_offset"] == pc)
            );
            if target != add_id {
                assert_eq!(caller["detail"]["reconstruction"]["status"], "success");
            }
        }
        let legacy = root.join(format!("authored-navigation-legacy.runtime.{suffix}"));
        let search = jsonl(&run(
            "strings",
            &legacy,
            encoding,
            &["--kind", "text", "--query", "RXD_DATA_NAVIGATION"],
        ));
        let data = search
            .iter()
            .find(|r| r["record"] == "data_literal" && r["detail"]["decoded_start"] == 492)
            .unwrap();
        let refs = jsonl(&run(
            "xref",
            &legacy,
            encoding,
            &["--at", data["at"].as_str().unwrap()],
        ));
        let reference = refs
            .iter()
            .find(|r| r["record"] == "data_reference")
            .unwrap();
        assert_eq!(reference["detail"]["instruction_offset"], 397);
        assert_eq!(
            reference["detail"]["source_offset_hex"],
            format!("{:064x}", 492)
        );
        assert_eq!(reference["detail"]["length_hex"], format!("{:064x}", 100));
        let reconstructed = fetch_context(
            &legacy,
            encoding,
            reference["context_at"].as_str().unwrap(),
            false,
        );
        assert_eq!(
            reconstructed["detail"]["reconstruction"]["status"],
            "success"
        );
        assert!(
            reconstructed["detail"]["reconstruction"]["text"]
                .as_str()
                .unwrap()
                .contains("code_copy(")
        );
        assert_original_segments(
            &reconstructed,
            &fs::read(&legacy).unwrap(),
            &fs::read(root.join("authored-navigation-legacy.runtime.bin")).unwrap(),
            encoding,
        );
    }
}
