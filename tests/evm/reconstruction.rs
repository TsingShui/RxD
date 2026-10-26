//! Selected reconstruction verified through the actual CLI and independent bytecode facts.
use super::*;

fn fetch(path: &Path, encoding: &str, id: &str, extra: &[&str]) -> Value {
    let mut args = vec!["--at", id];
    args.extend_from_slice(extra);
    let output = run("get-function", path, encoding, &args);
    success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn evm_default_fetch_reconstructs_compiled_getters_and_keeps_raw_evidence() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/evm");
    for encoding in ["raw", "hex"] {
        let suffix = if encoding == "raw" { "bin" } else { "hex" };
        let path = root.join(format!("openzeppelin-erc20.runtime.{suffix}"));
        let rows = jsonl(&run("interface", &path, encoding, &[]));
        for (selector, entry, expected, forbidden, instructions) in [
            ("313ce567", 598, "0x12", "persistent_storage[", 23),
            ("18160ddd", 849, "persistent_storage[0x2]", "0x12", 25),
        ] {
            let function = rows
                .iter()
                .find(|r| r["record"] == "function" && r["detail"]["selector"] == selector)
                .unwrap();
            let id = function["at"].as_str().unwrap();
            let row = fetch(&path, encoding, id, &[]);
            let reconstruction = &row["detail"]["reconstruction"];
            assert_eq!(reconstruction["status"], "success", "{row}");
            assert_eq!(reconstruction["complete"], true);
            assert_eq!(reconstruction["reason"], Value::Null);
            let text = reconstruction["text"].as_str().unwrap();
            assert!(text.contains(expected), "{text}");
            assert!(!text.contains(forbidden), "{text}");
            for operation in [
                "callvalue()",
                "calldata_size()",
                "memory_word[",
                "return memory_slice(",
                "revert memory_slice(",
                "if (",
            ] {
                assert!(text.contains(operation), "{operation}: {text}");
            }
            assert!(!text.contains("PUSH"), "{text}");
            assert_eq!(row["detail"]["decoded_entry"], entry);
            assert_eq!(
                reconstruction["reconstructed_instruction_count"],
                instructions
            );
            assert_eq!(reconstruction["reconstructed_block_count"], 4);
            assert_eq!(row["detail"]["complete"], true);
            let raw = fetch(&path, encoding, id, &["--raw"]);
            assert_eq!(raw["detail"]["reconstruction"]["status"], "not_requested");
            assert_eq!(raw["detail"]["reconstruction"]["text"], Value::Null);
            for field in ["instructions", "segments", "control_flow"] {
                assert_eq!(raw["detail"][field], row["detail"][field]);
            }
            // Original solc bytecode facts, independent of reconstructed text.
            let return_pc = if selector == "313ce567" { 624 } else { 877 };
            assert!(
                row["detail"]["instructions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|i| i["decoded_offset"] == return_pc && i["opcode"] == 243)
            );
        }
    }
}

fn authored(body: &str) -> (tempfile::TempDir, PathBuf) {
    // Dispatcher retains exactly the selected selector; entry 32 POPs it.
    let prefix = "5f3560e01c8063112233441460205700";
    fixture(
        format!("{prefix}{}5b50{body}", "00".repeat(32 - prefix.len() / 2)),
        "authored.hex",
    )
}

#[test]
fn evm_reconstruction_retains_top_first_word_operations_and_calldata() {
    // Authored byte sequences with independently worked top-first operands. Intrinsics
    // denote EVM word semantics, including zero division, wrapping and signed results.
    let max = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    let minus_seven = "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9";
    let minus_one = max;
    let minimum = "8000000000000000000000000000000000000000000000000000000000000000";
    for (body, expression) in [
        (
            "60026003025f5200".to_owned(),
            "u256_mul(0x3, 0x2)".to_owned(),
        ),
        (
            "60026007065f5200".to_owned(),
            "u256_mod(0x7, 0x2)".to_owned(),
        ),
        (
            format!("60027f{minus_seven}075f5200"),
            format!("i256_mod(0x{minus_seven}, 0x2)"),
        ),
        (
            "60026003105f5200".to_owned(),
            "u256_lt(0x3, 0x2)".to_owned(),
        ),
        (
            "60026003115f5200".to_owned(),
            "u256_gt(0x3, 0x2)".to_owned(),
        ),
        (
            format!("60017f{minus_seven}135f5200"),
            format!("i256_gt(0x{minus_seven}, 0x1)"),
        ),
        (
            "60026003145f5200".to_owned(),
            "u256_eq(0x3, 0x2)".to_owned(),
        ),
        ("5f155f5200".to_owned(), "u256_iszero(0x0)".to_owned()),
        (
            "60026003165f5200".to_owned(),
            "u256_and(0x3, 0x2)".to_owned(),
        ),
        (
            "60026003175f5200".to_owned(),
            "u256_or(0x3, 0x2)".to_owned(),
        ),
        (
            "60026003185f5200".to_owned(),
            "u256_xor(0x3, 0x2)".to_owned(),
        ),
        ("6002195f5200".to_owned(), "u256_not(0x2)".to_owned()),
        (
            format!("7f{minus_seven}6101001d5f5200"),
            format!("i256_sar(0x100, 0x{minus_seven})"),
        ),
        (
            "60026003035f5200".to_owned(),
            "u256_sub(0x3, 0x2)".to_owned(),
        ),
        (
            "60026007045f5200".to_owned(),
            "u256_div(0x7, 0x2)".to_owned(),
        ),
        ("5f6007045f5200".to_owned(), "u256_div(0x7, 0x0)".to_owned()),
        (
            "600860011c5f5200".to_owned(),
            "u256_shr(0x1, 0x8)".to_owned(),
        ),
        (
            "60086101001b5f5200".to_owned(),
            "u256_shl(0x100, 0x8)".to_owned(),
        ),
        (
            format!("7f{max}6001015f5200"),
            format!("u256_add(0x1, 0x{max})"),
        ),
        (
            format!("60027f{minus_seven}055f5200"),
            format!("i256_div(0x{minus_seven}, 0x2)"),
        ),
        (
            format!("7f{minus_one}7f{minimum}055f5200"),
            format!("i256_div(0x{minimum}, 0x{minus_one})"),
        ),
        (
            format!("60017f{minus_seven}125f5200"),
            format!("i256_lt(0x{minus_seven}, 0x1)"),
        ),
        (
            "600560036004085f5200".to_owned(),
            "u256_addmod(0x4, 0x3, 0x5)".to_owned(),
        ),
        (
            "600560036004095f5200".to_owned(),
            "u256_mulmod(0x4, 0x3, 0x5)".to_owned(),
        ),
        (
            "600260030a5f5200".to_owned(),
            "u256_exp(0x3, 0x2)".to_owned(),
        ),
        (
            "608060000b5f5200".to_owned(),
            "u256_signextend(0x0, 0x80)".to_owned(),
        ),
        (
            "610102601f1a5f5200".to_owned(),
            "u256_byte(0x1f, 0x102)".to_owned(),
        ),
        (
            "6004356007035f5200".to_owned(),
            "calldata_word(0x4)".to_owned(),
        ),
    ] {
        let (_dir, path) = authored(&body);
        let rows = jsonl(&run("interface", &path, "hex", &[]));
        let function = rows.iter().find(|r| r["record"] == "function").unwrap();
        let row = fetch(&path, "hex", function["at"].as_str().unwrap(), &[]);
        assert_eq!(
            row["detail"]["reconstruction"]["status"], "success",
            "{row}"
        );
        let text = row["detail"]["reconstruction"]["text"].as_str().unwrap();
        assert!(text.contains(&expression), "{expression}: {text}");
        assert!(text.contains("memory_word[0x0] = "), "{text}");
        assert!(text.contains("stop;"), "{text}");
    }
}

#[test]
fn evm_reconstruction_materializes_state_reads_before_writes_and_preserves_copy_order() {
    for (body, read, write, destination) in [
        (
            "600154602a6001555f5200",
            "persistent_storage[0x1]",
            "persistent_storage[0x1] = 0x2a;",
            "memory_word[0x0]",
        ),
        (
            "60425c602b60425d5f5200",
            "transient_storage[0x42]",
            "transient_storage[0x42] = 0x2b;",
            "memory_word[0x0]",
        ),
        (
            "5f51602c5f525f5200",
            "memory_word[0x0]",
            "memory_word[0x0] = 0x2c;",
            "memory_word[0x0]",
        ),
        (
            "5a60015f555f5200",
            "gas_remaining()",
            "persistent_storage[0x0] = 0x1;",
            "memory_word[0x0]",
        ),
        (
            "3d60015f555f5200",
            "returndata_size()",
            "persistent_storage[0x0] = 0x1;",
            "memory_word[0x0]",
        ),
    ] {
        let (_dir, path) = authored(body);
        let rows = jsonl(&run("interface", &path, "hex", &[]));
        let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
            .as_str()
            .unwrap();
        let row = fetch(&path, "hex", id, &[]);
        assert_eq!(
            row["detail"]["reconstruction"]["status"], "success",
            "{row}"
        );
        let text = row["detail"]["reconstruction"]["text"].as_str().unwrap();
        let line = text
            .lines()
            .find(|line| line.contains(&format!(" = {read};")))
            .unwrap();
        let value = line
            .trim()
            .strip_prefix("let ")
            .unwrap()
            .split(" = ")
            .next()
            .unwrap();
        assert!(
            text.find(line).unwrap() < text.find(write).unwrap(),
            "{text}"
        );
        assert!(
            text.contains(&format!("{destination} = {value};")),
            "{text}"
        );
        assert_eq!(text.matches(&format!(" = {read};")).count(), 1, "{text}");
    }
    for (opcode, intrinsic) in [
        ("37", "calldata_copy"),
        ("39", "code_copy"),
        ("3e", "returndata_copy"),
        ("5e", "memory_copy"),
    ] {
        let (_dir, path) = authored(&format!("602060046010{opcode}61abcd60085360206010f3"));
        let row = fetch(
            &path,
            "hex",
            jsonl(&run("interface", &path, "hex", &[]))
                .iter()
                .find(|r| r["record"] == "function")
                .unwrap()["at"]
                .as_str()
                .unwrap(),
            &[],
        );
        assert_eq!(
            row["detail"]["reconstruction"]["status"], "success",
            "{row}"
        );
        let text = row["detail"]["reconstruction"]["text"].as_str().unwrap();
        assert!(
            text.contains(&format!(
                "{intrinsic}(destination=0x10, source=0x4, length=0x20);"
            )),
            "{text}"
        );
        assert!(
            text.contains("memory_byte[0x8] = low_byte(0xabcd);"),
            "{text}"
        );
        assert!(text.contains("return memory_slice(0x10, 0x20);"), "{text}");
    }
}

#[test]
fn evm_reconstruction_only_lowers_selected_contexts_and_reports_separate_work() {
    for padding in [0, 4_000] {
        // Unrelated terminal blocks are 24,000 additional decoded bytes. Neither
        // selected function can reach them. Shared helper evidence remains unchanged.
        let carrier = format!("{SHARED_HELPER}{}", "5b60aa5f5500".repeat(padding));
        let (_dir, path) = fixture(&carrier, "selected.hex");
        let rows = jsonl(&run("interface", &path, "hex", &[]));
        for (selector, slot, other_slot) in
            [("11223344", "0xa", "0x14"), ("55667788", "0x14", "0xa")]
        {
            let id = rows
                .iter()
                .find(|r| r["record"] == "function" && r["detail"]["selector"] == selector)
                .unwrap()["at"]
                .as_str()
                .unwrap();
            let output = run("get-function", &path, "hex", &["--at", id, "--perf"]);
            success(&output);
            let row: Value = serde_json::from_slice(&output.stdout).unwrap();
            let reconstruction = &row["detail"]["reconstruction"];
            assert_eq!(reconstruction["status"], "success");
            let text = reconstruction["text"].as_str().unwrap();
            assert!(
                text.contains(&format!("persistent_storage[{slot}]")),
                "{text}"
            );
            assert!(
                !text.contains(&format!("persistent_storage[{other_slot}]")),
                "{text}"
            );
            assert!(!text.contains("0xaa"), "{text}");
            assert_eq!(reconstruction["reconstructed_instruction_count"], 15);
            assert_eq!(reconstruction["reconstructed_block_count"], 3);
            assert_eq!(reconstruction["reconstructed_context_count"], 3);
            let reports = perf_reports(&output);
            let counters = &reports[0]["counters"];
            assert_eq!(counters["reconstructed_instructions"], 15);
            assert_eq!(counters["reconstructed_blocks"], 3);
            assert_eq!(counters["reconstructed_contexts"], 3);
            assert_eq!(counters["reconstruction_work"], reconstruction["work"]);
            assert_eq!(
                counters["reconstruction_values"],
                reconstruction["value_count"]
            );
            assert!(counters["instructions"].as_u64().unwrap() > 15 + padding as u64);
            assert!(reports[0]["stages_ns"]["reconstruct"].as_u64().unwrap() > 0);
            assert!(reports[0]["stages_ns"]["query"].as_u64().unwrap() > 0);
            // Work scales with admitted input; the live value arena does not.
            assert!(
                reports[0]["resource_limits"]
                    .get("reconstruction_work")
                    .is_none()
            );
            assert_eq!(reports[0]["resource_limits"]["values"], 1_048_576);
            let plain = run("get-function", &path, "hex", &["--at", id]);
            assert_eq!(plain.stdout, output.stdout);
        }
    }
}

#[test]
fn evm_reconstruction_unsupported_and_incomplete_targets_keep_complete_raw_fallback() {
    for (body, reason, offset, boundary) in [
        // Seven operands for CALL: success/result and aliasing effects are outside
        // this milestone's declared semantics; never invent an ABI call expression.
        ("5f5f5f5f5f5f5ff100", "unsupported_opcode", 41, true),
        ("aa00", "undecoded_or_unindexed_successor", 34, false),
        ("60043556", "dynamic_jump_destination", 37, false),
        ("61ff", "undecoded_or_unindexed_successor", 34, false),
    ] {
        let (_dir, path) = authored(body);
        let rows = jsonl(&run("interface", &path, "hex", &[]));
        let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
            .as_str()
            .unwrap();
        let row = fetch(&path, "hex", id, &[]);
        let raw = fetch(&path, "hex", id, &["--raw"]);
        assert_eq!(row["detail"]["complete"], true);
        assert_eq!(row["detail"]["boundary_complete"], boundary);
        assert_eq!(row["detail"]["reconstruction"]["status"], "raw_fallback");
        assert_eq!(row["detail"]["reconstruction"]["reason"], reason);
        assert_eq!(row["detail"]["reconstruction"]["failure_offset"], offset);
        assert_eq!(row["detail"]["reconstruction"]["complete"], false);
        assert_eq!(row["detail"]["reconstruction"]["text"], Value::Null);
        assert_eq!(row["detail"]["segments"], raw["detail"]["segments"]);
        assert_eq!(row["detail"]["instructions"], raw["detail"]["instructions"]);
        assert_eq!(row["detail"]["control_flow"], raw["detail"]["control_flow"]);
        if body == "61ff" {
            assert_eq!(
                row["detail"]["unresolved_evidence"],
                serde_json::json!([{
                    "decoded_start":34, "decoded_end":36, "carrier_start":68, "carrier_end":72,
                    "raw_hex":"61ff", "reason":"undecoded_or_unindexed_successor"
                }])
            );
        }
    }
}

#[test]
fn evm_reconstruction_budgets_are_atomic_and_arena_growth_is_linear() {
    let body = format!("5f{}5f5200", "8001".repeat(256));
    let (dir, path) = authored(&body);
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
        .as_str()
        .unwrap();
    let plain = run("get-function", &path, "hex", &["--at", id]);
    success(&plain);
    let row: Value = serde_json::from_slice(&plain.stdout).unwrap();
    let reconstruction = &row["detail"]["reconstruction"];
    assert_eq!(reconstruction["status"], "success");
    assert_eq!(reconstruction["reconstructed_instruction_count"], 518);
    assert!(reconstruction["value_count"].as_u64().unwrap() < 300);
    assert!(reconstruction["text"].as_str().unwrap().len() < 20_000);
    let destination = dir.path().join("preserved.json");
    // Reconstruction and arena ceilings live in the shared budget table.
    for flag in [
        "--max-reconstruction-work",
        "--max-values",
        "--max-output-bytes",
    ] {
        fs::write(&destination, "preserved").unwrap();
        failure(
            run(
                "get-function",
                &path,
                "hex",
                &["--at", id, flag, "1", "-o", destination.to_str().unwrap()],
            ),
            2,
            flag,
        );
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
    let raw = run(
        "get-function",
        &path,
        "hex",
        &["--at", id, "--raw", "--perf"],
    );
    success(&raw);
    let reports = perf_reports(&raw);
    assert!(
        reports[0]["resource_limits"]
            .get("reconstruction_work")
            .is_none()
    );
    assert!(reports[0]["resource_limits"].get("values").is_none());
    assert_eq!(reports[0]["counters"]["reconstruction_work"], Value::Null);
    assert!(reports[0]["stages_ns"].get("reconstruct").is_none());
    let raw: Value = serde_json::from_slice(&raw.stdout).unwrap();
    assert_eq!(raw["detail"]["reconstruction"]["work"], 0);
    assert_eq!(raw["detail"]["reconstruction"]["value_count"], 0);
    assert_record_keys(
        reconstruction,
        &[
            "engine",
            "language",
            "status",
            "reason",
            "failure_offset",
            "complete",
            "text",
            "control_flow_status",
            "structuring_reason",
            "structured_context_count",
            "structured_edge_count",
            "structuring_work",
            "structured_loop_count",
            "structured_backedge_count",
            "reconstructed_instruction_count",
            "reconstructed_block_count",
            "reconstructed_context_count",
            "value_count",
            "work",
        ],
    );
    assert_record_keys(
        &raw["detail"]["reconstruction"],
        &[
            "engine",
            "language",
            "status",
            "reason",
            "failure_offset",
            "complete",
            "text",
            "control_flow_status",
            "structuring_reason",
            "structured_context_count",
            "structured_edge_count",
            "structuring_work",
            "structured_loop_count",
            "structured_backedge_count",
            "reconstructed_instruction_count",
            "reconstructed_block_count",
            "reconstructed_context_count",
            "value_count",
            "work",
        ],
    );
}

#[test]
fn evm_reconstruction_separates_known_boundary_from_ambiguous_incoming_stack() {
    // Two dispatcher paths reach entry 80 with selector-only and (7, selector)
    // stacks. They establish one candidate, but no single incoming stack precondition.
    let mut carrier = "346020575f3560e01c8063112233441460505700".to_owned();
    carrier.push_str(&"00".repeat(32 - carrier.len() / 2));
    carrier.push_str("5b60075f3560e01c8063112233441460505700");
    carrier.push_str(&"00".repeat(80 - carrier.len() / 2));
    carrier.push_str("5b5000");
    let (_dir, path) = fixture(&carrier, "two-states.hex");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let function = rows.iter().find(|r| r["record"] == "function").unwrap();
    assert_eq!(rows.iter().filter(|r| r["record"] == "function").count(), 1);
    let row = fetch(&path, "hex", function["at"].as_str().unwrap(), &[]);
    assert_eq!(row["detail"]["entry_states"].as_array().unwrap().len(), 2);
    assert_eq!(row["detail"]["boundary_complete"], true);
    assert_eq!(row["detail"]["reconstruction"]["status"], "raw_fallback");
    assert_eq!(
        row["detail"]["reconstruction"]["reason"],
        "multiple_or_unknown_entry_states"
    );
    assert_eq!(row["detail"]["reconstruction"]["failure_offset"], 80);
    assert_eq!(row["detail"]["reconstruction"]["complete"], false);
    assert_eq!(row["detail"]["reconstruction"]["text"], Value::Null);
    assert_eq!(row["detail"]["complete"], true);
    assert_eq!(row["detail"]["segments"][0]["raw_hex"], "5b5000");
    // INVALID is a supported explicit exceptional terminal, not an unknown opcode.
    let (_dir, path) = authored("fe");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
        .as_str()
        .unwrap();
    let row = fetch(&path, "hex", id, &[]);
    assert_eq!(row["detail"]["reconstruction"]["status"], "success");
    assert!(
        row["detail"]["reconstruction"]["text"]
            .as_str()
            .unwrap()
            .contains("invalid;")
    );
}

#[test]
fn evm_branch_fetch_structures_compiled_conditions_and_terminal_arms() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/evm");
    for encoding in ["raw", "hex"] {
        let suffix = if encoding == "raw" { "bin" } else { "hex" };
        let path = root.join(format!("authored-navigation.runtime.{suffix}"));
        let rows = jsonl(&run("interface", &path, encoding, &[]));
        let function = rows
            .iter()
            .find(|r| r["record"] == "function" && r["detail"]["selector"] == "0bb0b13a")
            .unwrap();
        let id = function["at"].as_str().unwrap();
        let row = fetch(&path, encoding, id, &[]);
        let reconstruction = &row["detail"]["reconstruction"];
        assert_eq!(reconstruction["status"], "success", "{row}");
        assert_eq!(
            reconstruction["control_flow_status"], "structured_acyclic",
            "{row}"
        );
        let text = reconstruction["text"].as_str().unwrap();
        assert!(!text.contains("goto "), "{text}");
        // Independently authored solc semantics: x<10, inverted by ISZERO for
        // JUMPI true -> x-3; false -> x+7. Both arms return, no joined tail.
        for expression in [
            "u256_lt(v_624_c3, 0xa)",
            "u256_iszero(v_628_c3)",
            "if (v_629_c3 != 0)",
            "u256_add(v_643_c4, s_4_3)",
            "u256_add(0x7, s_5_3)",
        ] {
            assert!(text.contains(expression), "{expression}: {text}");
        }
        assert!(
            text.find("u256_add(v_643_c4").unwrap() < text.find("u256_add(0x7, s_5_3)").unwrap(),
            "{text}"
        );
        assert_eq!(text.matches("return memory_slice(").count(), 2, "{text}");
        assert_eq!(row["detail"]["decoded_entry"], 601);
        assert!(
            row["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["decoded_offset"] == 633 && i["opcode"] == 87)
        );
        let raw = fetch(&path, encoding, id, &["--raw"]);
        for field in ["instructions", "segments", "control_flow"] {
            assert_eq!(raw["detail"][field], row["detail"][field]);
        }
        assert_eq!(raw["detail"]["reconstruction"]["status"], "not_requested");
    }
}

#[test]
fn evm_branch_join_assigns_each_predecessor_value_before_reusing_it() {
    // Both branch values reach ADD67 and feed the same storage/memory writes.
    let prefix = "5f3560e01c8063112233441460205700";
    let mut carrier = format!(
        "{prefix}{}5b50600435600a8110603657506022604056",
        "00".repeat(32 - prefix.len() / 2)
    );
    carrier.push_str(&"00".repeat(54 - carrier.len() / 2));
    carrier.push_str("5b506011604056");
    carrier.push_str(&"00".repeat(64 - carrier.len() / 2));
    carrier.push_str("5b600301805f555f5200");
    let (_dir, path) = fixture(&carrier, "join-values.hex");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
        .as_str()
        .unwrap();
    let output = run("get-function", &path, "hex", &["--at", id, "--perf"]);
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    let reconstruction = &row["detail"]["reconstruction"];
    assert_eq!(
        reconstruction["control_flow_status"], "structured_acyclic",
        "{row}"
    );
    let text = reconstruction["text"].as_str().unwrap();
    assert!(!text.contains("goto "), "{text}");
    assert_eq!(text.matches("// c_3: pc 64").count(), 1, "{text}");
    let true_value = text.find("(s_3_0) = (0x11);").unwrap();
    let false_value = text.find("(s_3_0) = (0x22);").unwrap();
    let alternate = text.find("} else {").unwrap();
    let join = text.find("// c_3: pc 64").unwrap();
    assert!(
        true_value < alternate && alternate < false_value && false_value < join,
        "{text}"
    );
    for expected in [
        "u256_lt(v_36_c0, 0xa)",
        "if (v_40_c0 != 0)",
        "u256_add(0x3, s_3_0)",
        "persistent_storage[0x0] = v_67_c3;",
        "memory_word[0x0] = v_67_c3;",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert_eq!(
        row["detail"]["segments"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["raw_hex"],
        "5b600301805f555f5200"
    );
    let reports = perf_reports(&output);
    assert_eq!(reports[0]["counters"]["structured_contexts"], 4);
    assert_eq!(reports[0]["counters"]["structured_edges"], 5);
    assert_eq!(
        reports[0]["counters"]["structuring_work"],
        reconstruction["structuring_work"]
    );
    assert!(reconstruction["structuring_work"].as_u64().unwrap() > 0);
}

#[test]
fn evm_branch_incompatible_join_heights_keep_explicit_complete_gotos() {
    // Unequal predecessor stacks must degrade rather than drop an extra word.
    let prefix = "5f3560e01c8063112233441460205700";
    let mut carrier = format!(
        "{prefix}{}5b50600435600a81106036575060226033604056",
        "00".repeat(32 - prefix.len() / 2)
    );
    carrier.push_str(&"00".repeat(54 - carrier.len() / 2));
    carrier.push_str("5b506011604056");
    carrier.push_str(&"00".repeat(64 - carrier.len() / 2));
    carrier.push_str("5b00");
    let (_dir, path) = fixture(&carrier, "join-heights.hex");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
        .as_str()
        .unwrap();
    let row = fetch(&path, "hex", id, &[]);
    let reconstruction = &row["detail"]["reconstruction"];
    assert_eq!(reconstruction["status"], "success", "{row}");
    assert_eq!(reconstruction["complete"], true);
    assert_eq!(
        reconstruction["control_flow_status"], "contextual_gotos",
        "{row}"
    );
    assert_eq!(
        reconstruction["structuring_reason"],
        "incompatible_join_stack_heights"
    );
    assert!(
        reconstruction["text"]
            .as_str()
            .unwrap()
            .contains("0x22, 0x33")
    );
    assert_eq!(row["detail"]["complete"], true);
    assert_eq!(row["detail"]["boundary_complete"], true);
    assert_eq!(
        row["detail"]["segments"],
        fetch(&path, "hex", id, &["--raw"])["detail"]["segments"]
    );
}

#[test]
fn evm_nested_branch_join_keeps_state_reads_and_all_three_edge_values() {
    // Read slot1 before branching; preserve the selected write and old loaded value.
    let prefix = "5f3560e01c8063112233441460205700";
    let mut carrier = format!(
        "{prefix}{}5b506001546004356050576022600155602435603c576066606056",
        "00".repeat(32 - prefix.len() / 2)
    );
    carrier.push_str(&"00".repeat(60 - carrier.len() / 2));
    carrier.push_str("5b6077606056");
    carrier.push_str(&"00".repeat(80 - carrier.len() / 2));
    carrier.push_str("5b60336001556088606056");
    carrier.push_str(&"00".repeat(96 - carrier.len() / 2));
    carrier.push_str("5b5f555f5200");
    let (_dir, path) = fixture(&carrier, "nested-state.hex");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
        .as_str()
        .unwrap();
    let row = fetch(&path, "hex", id, &[]);
    let reconstruction = &row["detail"]["reconstruction"];
    assert_eq!(
        reconstruction["control_flow_status"], "structured_acyclic",
        "{row}"
    );
    let text = reconstruction["text"].as_str().unwrap();
    assert_eq!(
        text.matches(" = persistent_storage[0x1];").count(),
        1,
        "{text}"
    );
    assert_eq!(text.matches("if (").count(), 2, "{text}");
    assert_eq!(text.matches("// c_3: pc 96").count(), 1, "{text}");
    let first_read = text.find("let v_36_c0 = persistent_storage[0x1];").unwrap();
    for write in [
        "persistent_storage[0x1] = 0x33;",
        "persistent_storage[0x1] = 0x22;",
    ] {
        assert!(first_read < text.find(write).unwrap(), "{text}");
    }
    for transfer in [
        "(s_3_0, s_3_1) = (s_1_0, 0x88);",
        "(s_3_0, s_3_1) = (s_4_0, 0x77);",
        "(s_3_0, s_3_1) = (s_5_0, 0x66);",
    ] {
        assert!(text.contains(transfer), "{transfer}: {text}");
        assert!(text.find(transfer).unwrap() < text.find("// c_3: pc 96").unwrap());
    }
    assert!(text.contains("persistent_storage[0x0] = s_3_1;"), "{text}");
    assert!(text.contains("memory_word[0x0] = s_3_0;"), "{text}");
    assert_eq!(
        row["detail"]["segments"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["raw_hex"],
        "5b5f555f5200"
    );
}

#[test]
fn evm_branch_unresolved_true_paths_preserve_raw_failure_evidence() {
    for (body, reason, pc) in [
        ("60043560ff5700", "invalid_jump_destination", 39),
        ("6004356004355700", "dynamic_jump_destination", 40),
    ] {
        let (_dir, path) = authored(body);
        let rows = jsonl(&run("interface", &path, "hex", &[]));
        let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
            .as_str()
            .unwrap();
        let row = fetch(&path, "hex", id, &[]);
        let reconstruction = &row["detail"]["reconstruction"];
        assert_eq!(reconstruction["status"], "raw_fallback", "{row}");
        assert_eq!(reconstruction["reason"], reason);
        assert_eq!(reconstruction["failure_offset"], pc);
        assert_eq!(reconstruction["complete"], false);
        assert_eq!(reconstruction["text"], Value::Null);
        assert_eq!(row["detail"]["boundary_complete"], false);
        assert_eq!(row["detail"]["complete"], true);
        assert!(
            row["detail"]["unresolved_evidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["decoded_start"] == pc && e["raw_hex"] == "57" && e["reason"] == reason)
        );
        let raw = fetch(&path, "hex", id, &["--raw"]);
        for field in [
            "segments",
            "instructions",
            "control_flow",
            "unresolved_evidence",
        ] {
            assert_eq!(raw["detail"][field], row["detail"][field]);
        }
    }
    // A proven acyclic target and unrelated unknown JUMP region stay isolated.
    let (_dir, path) = authored("5f60ff57005b60043556");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
        .as_str()
        .unwrap();
    let row = fetch(&path, "hex", id, &[]);
    assert_eq!(
        row["detail"]["reconstruction"]["status"], "success",
        "{row}"
    );
    assert_eq!(
        row["detail"]["reconstruction"]["control_flow_status"],
        "structured_acyclic"
    );
    assert!(
        !row["detail"]["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["opcode"] == 86)
    );
}

#[test]
fn evm_loop_fetch_structures_compiled_sum_with_simultaneous_carried_values() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/evm");
    for encoding in ["raw", "hex"] {
        let suffix = if encoding == "raw" { "bin" } else { "hex" };
        let path = root.join(format!("authored-navigation.runtime.{suffix}"));
        let rows = jsonl(&run("interface", &path, encoding, &[]));
        let id = rows
            .iter()
            .find(|r| r["record"] == "function" && r["detail"]["selector"] == "188b85b4")
            .unwrap()["at"]
            .as_str()
            .unwrap();
        let row = fetch(&path, encoding, id, &[]);
        let reconstruction = &row["detail"]["reconstruction"];
        assert_eq!(reconstruction["status"], "success", "{row}");
        assert_eq!(reconstruction["complete"], true);
        assert_eq!(
            reconstruction["control_flow_status"], "structured_loops",
            "{row}"
        );
        assert_eq!(reconstruction["structuring_reason"], Value::Null);
        let text = reconstruction["text"].as_str().unwrap();
        assert!(!text.contains("goto "), "{text}");
        // Independent solc 0.8.26 facts: carried [selector,i,result,n]; PC591
        // increments i, PC595 sums the old i, PC600 returns to header569.
        let initial = text
            .find("(s_4_0, s_4_1, s_4_2, s_4_3) = (s_3_0, 0x0, 0x0, v_568_c3);")
            .unwrap();
        let start = text.find("while (true) {").unwrap();
        let condition = text.find("u256_lt(s_4_1, s_4_3)").unwrap();
        let increment = text.find("u256_add(s_5_1, 0x1)").unwrap();
        let sum = text.find("u256_add(s_5_2, s_5_1)").unwrap();
        let backedge = text
            .find("(s_4_0, s_4_1, s_4_2, s_4_3) = (s_5_0, v_591_c5, v_595_c5, s_5_3);")
            .unwrap();
        assert!(
            initial < start
                && start < condition
                && condition < increment
                && increment < sum
                && sum < backedge
                && backedge < text.find("continue;").unwrap(),
            "{text}"
        );
        assert!(text.contains("break;"), "{text}");
        assert!(text.contains("memory_word[v_582_c6] = s_6_2;"), "{text}");
        assert!(text.contains("return memory_slice("), "{text}");
        assert_eq!(reconstruction["structured_loop_count"], 1);
        assert_eq!(reconstruction["structured_backedge_count"], 1);
        assert_eq!(reconstruction["reconstructed_context_count"], 7);
        assert!(
            row["detail"]["control_flow"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|c| c["edges"].as_array().unwrap())
                .any(|e| e["instruction_offset"] == 600
                    && e["target"] == 569
                    && e["resolution"] == "resolved")
        );
        let raw = fetch(&path, encoding, id, &["--raw"]);
        assert_eq!(raw["detail"]["reconstruction"]["status"], "not_requested");
        for field in ["instructions", "segments", "control_flow"] {
            assert_eq!(raw["detail"][field], row["detail"][field]);
        }
        assert!(
            row["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["decoded_offset"] == 600 && i["opcode"] == 86)
        );
    }
}

#[test]
fn evm_branch_structuring_covers_every_nested_guard_and_revert_arm() {
    // Sixty-four nested guards share one revert region; structuring must stay iterative.
    let prefix = "5f3560e01c8063112233441460205700";
    let mut carrier = format!(
        "{prefix}{}5b50{}00",
        "00".repeat(32 - prefix.len() / 2),
        "60043561180057".repeat(64)
    );
    carrier.push_str(&"00".repeat(6144 - carrier.len() / 2));
    carrier.push_str("5b5f80fd");
    let (dir, path) = fixture(&carrier, "nested-budget.hex");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
        .as_str()
        .unwrap();
    let plain = run("get-function", &path, "hex", &["--at", id, "--perf"]);
    success(&plain);
    let row: Value = serde_json::from_slice(&plain.stdout).unwrap();
    let reconstruction = &row["detail"]["reconstruction"];
    assert_eq!(
        reconstruction["control_flow_status"], "structured_acyclic",
        "{row}"
    );
    assert_eq!(reconstruction["structured_context_count"], 66);
    let work = reconstruction["work"].as_u64().unwrap();
    let structural = reconstruction["structuring_work"].as_u64().unwrap();
    let text = reconstruction["text"].as_str().unwrap();
    assert_eq!(text.matches("if (").count(), 64);
    assert_eq!(text.matches("revert memory_slice(").count(), 64);
    let lowering = work - structural - text.len() as u64;
    assert!(lowering > 0);
    let reports = perf_reports(&plain);
    assert!(reports[0]["counters"]["structuring_work"].as_u64().unwrap() > 0);
    assert!(
        reports[0]["counters"]["reconstructed_instructions"]
            .as_u64()
            .unwrap()
            > 64
    );
    assert!(reports[0]["stages_ns"]["structure"].as_u64().unwrap() > 0);
    // Reconstruction ceilings live in the shared budget table; the flags are gone.
    let destination = dir.path().join("preserved.json");
    for flag in [
        "--max-reconstruction-work",
        "--max-values",
        "--max-output-bytes",
    ] {
        fs::write(&destination, "preserved").unwrap();
        failure(
            run(
                "get-function",
                &path,
                "hex",
                &["--at", id, flag, "1", "-o", destination.to_str().unwrap()],
            ),
            2,
            flag,
        );
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
    let raw = run(
        "get-function",
        &path,
        "hex",
        &["--at", id, "--raw", "--perf"],
    );
    success(&raw);
    let reports = perf_reports(&raw);
    for key in [
        "structured_contexts",
        "structured_edges",
        "structuring_work",
    ] {
        assert_eq!(reports[0]["counters"][key], Value::Null);
    }
}

fn positioned_loop(regions: &[(usize, &str)]) -> (tempfile::TempDir, PathBuf) {
    let mut hex = "5f3560e01c8063112233441460205700".to_owned();
    for &(pc, bytes) in regions {
        assert!(hex.len() / 2 <= pc);
        hex.push_str(&"00".repeat(pc - hex.len() / 2));
        hex.push_str(bytes);
    }
    fixture(hex, "loop.hex")
}

#[test]
fn evm_loop_header_and_latch_keep_original_effect_order_and_zero_iteration_path() {
    for (regions, header_read, write, condition) in [
        (
            vec![
                (32, "5b505f602856"),
                (40, "5b5f54811015604057600101805f55602856"),
                (64, "5b5000"),
            ],
            Some("let v_42_c1 = persistent_storage[0x0];"),
            "persistent_storage[0x0] = v_51_c3;",
            "if (v_45_c1 != 0)",
        ),
        (
            // The body stores its increment before reading calldata at PC49.
            vec![(32, "5b505f602856"), (40, "5b600101805f5560043560285700")],
            None,
            "persistent_storage[0x0] = v_43_c1;",
            "if (v_49_c1 != 0)",
        ),
    ] {
        let (_dir, path) = positioned_loop(&regions);
        let rows = jsonl(&run("interface", &path, "hex", &[]));
        let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
            .as_str()
            .unwrap();
        let row = fetch(&path, "hex", id, &[]);
        let rec = &row["detail"]["reconstruction"];
        assert_eq!(rec["control_flow_status"], "structured_loops", "{row}");
        let text = rec["text"].as_str().unwrap();
        let start = text.find("while (true)").unwrap();
        let test = text.find(condition).unwrap();
        let store = text.find(write).unwrap();
        if let Some(read) = header_read {
            assert!(
                start < text.find(read).unwrap()
                    && text.find(read).unwrap() < test
                    && test < text.find("break;").unwrap()
                    && text.find("break;").unwrap() < store,
                "{text}"
            );
        } else {
            assert!(start < store && store < test, "{text}");
        }
        assert!(text.contains("continue;"), "{text}");
        assert!(!text.contains("goto "), "{text}");
    }
}

#[test]
fn evm_loop_and_nested_shared_helpers_keep_each_selected_return_and_target_scale() {
    let nested = format!(
        "{}5b6046604a56000000005b9056005b906001019056",
        &SHARED_HELPER[..120]
    );
    let mut looped = SHARED_HELPER.to_string();
    looped.replace_range(64..82, "5b5060525600000000");
    looped.push_str(&"00".repeat(82 - looped.len() / 2));
    looped.push_str("5b60295f5b600101600435605657603c56");
    let mut helper_loop = SHARED_HELPER.to_string();
    helper_loop.replace_range(64..82, "5b5060525600000000");
    helper_loop.push_str(&"00".repeat(82 - helper_loop.len() / 2));
    helper_loop.push_str("5b5f605856005b606090603c56005b600435605857602956");
    for (carrier, expected_flow, starts, other_starts) in [
        (
            nested,
            "structured_acyclic",
            vec![32, 41, 60, 70, 74],
            vec![46, 55, 60, 70, 74],
        ),
        (
            looped,
            "structured_loops",
            vec![32, 41, 60, 82, 86, 96],
            vec![46, 55, 60],
        ),
        (
            helper_loop,
            "structured_loops",
            vec![32, 41, 60, 82, 88, 96, 103],
            vec![46, 55, 60],
        ),
    ] {
        let mut baseline = Vec::new();
        for padding in [0, 4000] {
            let carrier = format!(
                "{carrier}{}{}",
                "00".repeat(1024 - carrier.len() / 2),
                "5b60aa5f5500".repeat(padding)
            );
            let (_dir, path) = fixture(&carrier, "loop-shared.hex");
            let rows = jsonl(&run("interface", &path, "hex", &[]));
            for (selector, slot, forbidden, selected_starts) in [
                ("11223344", "0xa", "0x14", starts.clone()),
                ("55667788", "0x14", "0xa", other_starts.clone()),
            ] {
                let id = rows
                    .iter()
                    .find(|r| r["record"] == "function" && r["detail"]["selector"] == selector)
                    .unwrap()["at"]
                    .as_str()
                    .unwrap();
                let row = fetch(&path, "hex", id, &[]);
                let rec = &row["detail"]["reconstruction"];
                assert_eq!(rec["status"], "success", "{row}");
                assert_eq!(
                    rec["control_flow_status"],
                    if selector == "11223344" {
                        expected_flow
                    } else {
                        "structured_acyclic"
                    },
                    "{row}"
                );
                let text = rec["text"].as_str().unwrap();
                assert!(
                    text.contains(&format!("persistent_storage[{slot}]")),
                    "{text}"
                );
                assert!(
                    !text.contains(&format!("persistent_storage[{forbidden}]"))
                        && !text.contains("0xaa"),
                    "{text}"
                );
                let actual: Vec<_> = row["detail"]["blocks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|b| b["detail"]["decoded_start"].as_u64().unwrap())
                    .collect();
                assert_eq!(actual, selected_starts);
                let raw = fetch(&path, "hex", id, &["--raw"]);
                assert_eq!(raw["detail"]["control_flow"], row["detail"]["control_flow"]);
                if padding == 0 {
                    baseline.push(rec.clone());
                } else {
                    assert_eq!(rec, &baseline[usize::from(selector == "55667788")]);
                }
            }
        }
    }
}

#[test]
fn evm_loop_multiple_backedges_swap_carried_words_and_render_infinite_flow_boundedly() {
    let (_dir, path) = positioned_loop(&[
        (32, "5b5060016002603056"),
        (48, "5b60043560405790603056"),
        (64, "5b600101603056"),
    ]);
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
        .as_str()
        .unwrap();
    let row = fetch(&path, "hex", id, &[]);
    let rec = &row["detail"]["reconstruction"];
    assert_eq!(rec["control_flow_status"], "structured_loops", "{row}");
    assert_eq!(rec["structured_backedge_count"], 2);
    let text = rec["text"].as_str().unwrap();
    assert_eq!(text.matches("continue;").count(), 2, "{text}");
    assert_eq!(text.matches("while (true)").count(), 1, "{text}");
    assert!(
        !text.contains("break;") && !text.contains("stop;") && !text.contains("goto "),
        "{text}"
    );
    let swap = text.find("(s_1_0, s_1_1) = (s_3_1, s_3_0);").unwrap();
    assert!(swap < text.rfind("continue;").unwrap(), "{text}");
    assert!(text.len() < 3000);
    assert_eq!(row["detail"]["boundary_complete"], true);
}

#[test]
fn evm_loop_terminal_arms_stay_inside_iterations_before_normal_exit() {
    let (_dir, path) = positioned_loop(&[
        (32, "5b505f603056"),
        (48, "5b6004356080575f54606057602435607057600101603056"),
        (96, "5b5f80fd"),
        (112, "5b5f80f3"),
        (128, "5b5000"),
    ]);
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
        .as_str()
        .unwrap();
    let row = fetch(&path, "hex", id, &[]);
    let rec = &row["detail"]["reconstruction"];
    assert_eq!(rec["control_flow_status"], "structured_loops", "{row}");
    let text = rec["text"].as_str().unwrap();
    let start = text.find("while (true)").unwrap();
    let end = text.find(": pc 128").unwrap();
    for terminal in [
        "revert memory_slice(0x0, 0x0);",
        "return memory_slice(0x0, 0x0);",
    ] {
        assert!(
            start < text.find(terminal).unwrap() && text.find(terminal).unwrap() < end,
            "{text}"
        );
    }
    assert_eq!(text.matches("continue;").count(), 1, "{text}");
    assert_eq!(text.matches("break;").count(), 1, "{text}");
    assert_eq!(text.matches("stop;").count(), 1, "{text}");
}

#[test]
fn evm_loop_unsupported_complete_flow_keeps_reasoned_gotos_and_all_raw_edges() {
    for (regions, reason) in [
        (
            vec![
                (32, "5b50600435604057603056"),
                (48, "5b604056"),
                (64, "5b603056"),
            ],
            "multiple_entry_loop",
        ),
        (
            vec![
                (32, "5b50603056"),
                (48, "5b600435608057604056"),
                (64, "5b602435605057604056"),
                (80, "5b603056"),
                (128, "5b00"),
            ],
            "unsupported_nested_loop",
        ),
        (
            vec![
                (32, "5b50603056"),
                (48, "5b60043560805760243560a057603056"),
                (128, "5b609056"),
                (144, "5b00"),
                (160, "5b60b056"),
                (176, "5b00"),
            ],
            "unsupported_loop_exits",
        ),
    ] {
        let (_dir, path) = positioned_loop(&regions);
        let rows = jsonl(&run("interface", &path, "hex", &[]));
        let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
            .as_str()
            .unwrap();
        let row = fetch(&path, "hex", id, &[]);
        let rec = &row["detail"]["reconstruction"];
        assert_eq!(rec["status"], "success", "{row}");
        assert_eq!(rec["complete"], true);
        assert_eq!(rec["control_flow_status"], "contextual_gotos", "{row}");
        assert_eq!(rec["structuring_reason"], reason, "{row}");
        assert_eq!(rec["reason"], Value::Null);
        let text = rec["text"].as_str().unwrap();
        assert!(
            text.contains("goto ") && !text.contains("while ("),
            "{text}"
        );
        let raw = fetch(&path, "hex", id, &["--raw"]);
        for field in ["instructions", "segments", "control_flow"] {
            assert_eq!(raw["detail"][field], row["detail"][field]);
        }
    }
}

#[test]
fn evm_loop_work_and_output_limits_are_exact_atomic_and_report_attempted_structure() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/evm/authored-navigation.runtime.hex");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let id = rows
        .iter()
        .find(|r| r["record"] == "function" && r["detail"]["selector"] == "188b85b4")
        .unwrap()["at"]
        .as_str()
        .unwrap();
    let output = run("get-function", &path, "hex", &["--at", id, "--perf"]);
    success(&output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    let rec = &row["detail"]["reconstruction"];
    let reports = perf_reports(&output);
    assert_eq!(reports[0]["counters"]["structured_loops"], 1);
    assert_eq!(reports[0]["counters"]["structured_backedges"], 1);
    assert_eq!(reports[1]["counters"]["structured_loops"], Value::Null);
    let work = rec["work"].as_u64().unwrap();
    let structural = rec["structuring_work"].as_u64().unwrap();
    let lower = work - structural - rec["text"].as_str().unwrap().len() as u64;
    assert!(lower > 0);
    assert!(reports[0]["counters"]["structuring_work"].as_u64().unwrap() > 0);
    assert!(reports[0]["stages_ns"]["structure"].as_u64().unwrap() > 0);
    // Reconstruction ceilings live in the shared budget table; the flags are gone.
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("preserved.json");
    for flag in ["--max-reconstruction-work", "--max-output-bytes"] {
        fs::write(&destination, "preserved").unwrap();
        failure(
            run(
                "get-function",
                &path,
                "hex",
                &["--at", id, flag, "1", "-o", destination.to_str().unwrap()],
            ),
            2,
            flag,
        );
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
    let raw = run(
        "get-function",
        &path,
        "hex",
        &["--at", id, "--raw", "--perf"],
    );
    success(&raw);
    for key in [
        "structured_loops",
        "structured_backedges",
        "structuring_work",
    ] {
        assert_eq!(perf_reports(&raw)[0]["counters"][key], Value::Null);
    }
}

#[test]
fn evm_loop_unknown_backedge_and_unsupported_effect_retain_reasoned_raw_fallback() {
    for (bytes, reason, pc) in [
        ("5b60043556", "dynamic_jump_destination", 44),
        ("5b4650602856", "unsupported_opcode", 41),
    ] {
        let (_dir, path) = positioned_loop(&[(32, "5b50602856"), (40, bytes)]);
        let rows = jsonl(&run("interface", &path, "hex", &[]));
        let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
            .as_str()
            .unwrap();
        let row = fetch(&path, "hex", id, &[]);
        let rec = &row["detail"]["reconstruction"];
        assert_eq!(rec["status"], "raw_fallback", "{row}");
        assert_eq!(rec["complete"], false);
        assert_eq!(rec["reason"], reason);
        assert_eq!(rec["failure_offset"], pc);
        assert_eq!(rec["text"], Value::Null);
        assert_eq!(rec["structured_loop_count"], 0);
        let raw = fetch(&path, "hex", id, &["--raw"]);
        for field in [
            "instructions",
            "segments",
            "control_flow",
            "unresolved_evidence",
        ] {
            assert_eq!(raw["detail"][field], row["detail"][field]);
        }
    }
}

#[test]
fn evm_loop_self_backedge_aliases_are_simultaneous_and_serial_loops_stay_separate() {
    for (regions, loops) in [
        (
            vec![(32, "5b5060016002603056"), (48, "5b9060043560305700")],
            1,
        ),
        (
            vec![
                (32, "5b50603056"),
                (48, "5b600435603057605056"),
                (80, "5b60243560505700"),
            ],
            2,
        ),
    ] {
        let (_dir, path) = positioned_loop(&regions);
        let rows = jsonl(&run("interface", &path, "hex", &[]));
        let id = rows.iter().find(|r| r["record"] == "function").unwrap()["at"]
            .as_str()
            .unwrap();
        let row = fetch(&path, "hex", id, &[]);
        let rec = &row["detail"]["reconstruction"];
        assert_eq!(rec["control_flow_status"], "structured_loops", "{row}");
        assert_eq!(rec["structured_loop_count"], loops);
        assert_eq!(rec["structured_backedge_count"], loops);
        let text = rec["text"].as_str().unwrap();
        assert_eq!(text.matches("while (true)").count(), loops);
        if loops == 1 {
            // SWAP1 at49 returns to48: destinations alias the old RHS words.
            let transfer = text.find("(s_1_0, s_1_1) = (s_1_1, s_1_0);").unwrap();
            assert!(transfer < text.find("continue;").unwrap(), "{text}");
            assert!(
                text.find("(s_1_0, s_1_1) = (0x1, 0x2);").unwrap()
                    < text.find("while (true)").unwrap(),
                "{text}"
            );
        } else {
            let first = text.find("while (true)").unwrap();
            let second = text.rfind("while (true)").unwrap();
            assert!(
                first < text.find("calldata_word(0x4)").unwrap()
                    && text.find("calldata_word(0x4)").unwrap() < second
                    && second < text.find("calldata_word(0x24)").unwrap(),
                "{text}"
            );
        }
    }
}
