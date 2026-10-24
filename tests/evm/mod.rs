//! Real CLI checks against explicit bytecode and independently known instruction/ABI facts.
use super::*;

const ARGUMENTS: &str = "6080604052348015600e575f80fd5b50600436106030575f3560e01c80632125b65b146034578063b69ef8a8146044575b5f80fd5b6044603f3660046046565b505050565b005b5f805f606084860312156057575f80fd5b833563ffffffff811681146069575f80fd5b925060208401356001600160a01b03811681146083575f80fd5b915060408401356001600160e01b0381168114609d575f80fd5b80915050925092509256";

fn run(command: &str, path: &Path, encoding: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["evm", command])
        .arg(path)
        .args(["--encoding", encoding])
        .args(args)
        .env_remove("RXD_CACHE_DIR")
        .output()
        .unwrap()
}

fn result(output: &Output) -> Value {
    success(output);
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["record"], "evm_analysis");
    assert_eq!(row["backend"], "evm");
    assert_eq!(row["schema_version"], 2);
    assert_eq!(
        row["detail"]["engine"],
        serde_json::json!({"name":"evmole","version":"0.9.4"})
    );
    assert_eq!(row["detail"]["complete"], true);
    assert_eq!(row["detail"]["discovery_complete"], Value::Null);
    assert_eq!(row["detail"]["abi_verified"], false);
    assert_eq!(row["detail"]["input_role"], "unknown");
    assert_eq!(row["detail"]["hardfork"], Value::Null);
    for key in ["id", "parent_id", "context_id"] {
        assert_eq!(row[key], Value::Null);
    }
    row
}

fn input(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/evm")
        .join(name)
}

#[test]
fn evm_interface_lists_carrier_bound_functions_with_native_abi_fields() {
    let carrier = format!(" \n0X{}\r\n", ARGUMENTS.to_uppercase());
    let (dir, path) = fixture(&carrier, "contract.hex");
    let bytes: Vec<u8> = ARGUMENTS
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    let raw = dir.path().join("renamed.bin");
    fs::write(&raw, &bytes).unwrap();
    let hex_output = run("interface", &path, "hex", &["--include-bytecode"]);
    let hex = jsonl(&hex_output);
    let raw_rows = jsonl(&run("interface", &raw, "raw", &["--include-bytecode"]));
    let header = &hex[0];
    assert_eq!(header["command"], "interface");
    assert_eq!(
        header["source_id"],
        rxd_core::identity::input_id(carrier.as_bytes())
    );
    assert_eq!(
        raw_rows[0]["source_id"],
        rxd_core::identity::input_id(&bytes)
    );
    assert_ne!(header["source_id"], raw_rows[0]["source_id"]);
    assert_eq!(
        header["detail"]["bytecode"],
        raw_rows[0]["detail"]["bytecode"]
    );
    assert_eq!(header["detail"]["bytecode"]["hex"], ARGUMENTS);
    assert_eq!(
        header["detail"]["bytecode"]["sha256"],
        format!("{:x}", Sha256::digest(&bytes))
    );
    assert_eq!(header["detail"]["offset_unit"], "decoded_bytecode_byte");
    assert_eq!(header["input_bytes"], carrier.len());
    assert_eq!(
        header["detail"]["metadata"],
        raw_rows[0]["detail"]["metadata"]
    );
    let functions: Vec<_> = hex.iter().filter(|r| r["record"] == "function").collect();
    assert_eq!(functions.len(), 2);
    let function = functions
        .iter()
        .find(|f| f["detail"]["selector"] == "2125b65b")
        .unwrap();
    // EVMole's own ABI fields sit next to the navigation evidence on the same row.
    assert_eq!(function["detail"]["arguments"], "uint32,address,uint224");
    assert_eq!(function["detail"]["stateMutability"], "pure");
    assert_eq!(function["detail"]["dispatch"], "abi");
    assert_eq!(
        run("interface", &path, "hex", &["--include-bytecode", "--json"]).stdout,
        hex_output.stdout
    );
    // One spelling: the old `functions` alias is gone.
    let alias = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["evm", "functions"])
        .arg(&path)
        .args(["--encoding", "hex"])
        .output()
        .unwrap();
    failure(alias, 2, "functions");
    let profiled = run("interface", &path, "hex", &["--include-bytecode", "--perf"]);
    assert_eq!(profiled.stdout, hex_output.stdout);
    let reports = perf_reports(&profiled);
    assert_eq!(reports.len(), 2);
    assert!(reports[0]["stages_ns"]["analyze"].as_u64().unwrap() > 0);
    assert_eq!(reports[0]["counters"]["input_bytes"], carrier.len());
}

#[test]
fn evm_all_native_views_include_storage_transient_storage_disassembly_and_cfg() {
    let storage = input("storage-runtime.hex");
    let analysis = result(&run("analyze", &storage, "hex", &[]));
    let value = &analysis["detail"]["result"];
    for field in [
        "functions",
        "storage",
        "transientStorage",
        "disassembled",
        "basicBlocks",
    ] {
        assert!(value[field].is_array(), "{field}: {value}");
    }
    assert!(value["controlFlowGraph"]["blocks"].is_array());
    let store = result(&run("storage", &storage, "hex", &[]));
    let record = &store["detail"]["result"]["storage"][0];
    assert_eq!(record["slot"], format!("{:064x}", 1));
    assert_eq!(record["type"], "uint256");
    assert_eq!(record["offset"], 0);
    assert_eq!(record["writes"], serde_json::json!(["11223344"]));
    let transient = result(&run(
        "storage",
        &input("transient-storage-runtime.hex"),
        "hex",
        &[],
    ));
    assert_eq!(
        transient["detail"]["result"]["transientStorage"][0]["slot"],
        format!("{:064x}", 1)
    );
    let (_directory, path) = fixture("6003565b00", "jump.hex");
    let disassembly = result(&run("disassemble", &path, "hex", &[]));
    assert_eq!(
        disassembly["detail"]["result"]["disassembled"],
        serde_json::json!([[0, "PUSH1 03"], [2, "JUMP"], [3, "JUMPDEST"], [4, "STOP"]])
    );
    let blocks = result(&run("basic-blocks", &path, "hex", &[]));
    assert_eq!(
        blocks["detail"]["result"]["basicBlocks"],
        serde_json::json!([[0, 2], [3, 4]])
    );
    assert_eq!(disassembly["detail"]["disassembly_complete"], true);
    let (_truncated_directory, truncated) = fixture("0061ff", "short-push.hex");
    let partial = result(&run("disassemble", &truncated, "hex", &[]));
    assert_eq!(partial["detail"]["disassembly_complete"], false);
    assert_eq!(
        partial["detail"]["undecoded_tail"],
        serde_json::json!({"start":1,"bytes":2,"hex":"61ff"})
    );
    let cfg = result(&run("cfg", &path, "hex", &[]));
    assert_eq!(cfg["detail"]["cfg_destination_unit"], "upstream_block_id");
    assert_eq!(cfg["detail"]["basic_block_end"], "last_opcode_start");
    let blocks = cfg["detail"]["result"]["controlFlowGraph"]["blocks"]
        .as_array()
        .unwrap();
    let branch = blocks.iter().find(|b| b["start"] == 0).unwrap();
    assert_eq!(branch["type"], "Jump");
    let target = branch["data"]["to"].as_u64().unwrap();
    assert!(blocks.iter().any(|b| b["id"] == target && b["start"] == 3));
}

#[test]
fn evm_metadata_retains_native_bytes_and_large_integer_encodings() {
    let metadata = result(&run("metadata", &input("metadata-runtime.hex"), "hex", &[]));
    let metadata = &metadata["detail"]["result"]["metadata"];
    assert_eq!(metadata["bytecodeOffset"], 1);
    assert_eq!(metadata["cborLength"], 10);
    assert_eq!(
        metadata["entries"][0],
        serde_json::json!({"key":"solc","value":{"type":"bytes","value":[0,8,26]}})
    );
    let (_directory, path) = fixture("00a161691b0020000000000000000c", "large-cbor.hex");
    let row = result(&run("metadata", &path, "hex", &[]));
    assert_eq!(
        row["detail"]["result"]["metadata"]["entries"][0]["value"],
        serde_json::json!({"type":"undecoded","value":[27,0,32,0,0,0,0,0,0]})
    );
}

#[test]
fn evm_invalid_encodings_eof_and_non_files_fail_atomically() {
    let (directory, path) = fixture("00", "input.hex");
    let destination = directory.path().join("keep.json");
    fs::write(&destination, "original output").unwrap();
    for invalid in [
        b"0".as_slice(),
        b"0x001",
        b"60 00",
        b"60_00",
        b"0xgg",
        b"\xef\xbb\xbf00",
        b"\xff\xff",
        b"\xc2\xa000",
    ] {
        fs::write(&path, invalid).unwrap();
        failure(
            run(
                "analyze",
                &path,
                "hex",
                &["-o", destination.to_str().unwrap()],
            ),
            1,
            "INVALID_INPUT",
        );
        assert_eq!(fs::read(&destination).unwrap(), b"original output");
    }
    fs::write(&path, [0xef, 0x00, 0x01]).unwrap();
    failure(run("analyze", &path, "raw", &[]), 1, "UNSUPPORTED_INPUT");
    failure(
        run("interface", directory.path(), "raw", &[]),
        1,
        "INVALID_INPUT",
    );
    failure(
        run("metadata", &directory.path().join("missing"), "raw", &[]),
        1,
        "IO_ERROR",
    );
    fs::write(&path, "00").unwrap();
    failure(
        run("interface", &path, "hex", &["-o", path.to_str().unwrap()]),
        2,
        "INVALID_QUERY",
    );
    assert_eq!(fs::read(&path).unwrap(), b"00");
    fs::write(&path, "0xgg").unwrap();
    failure(run("blocks", &path, "hex", &[]), 1, "INVALID_INPUT");
    fs::write(&path, [0xef, 0x00]).unwrap();
    failure(run("blocks", &path, "raw", &[]), 1, "UNSUPPORTED_INPUT");
    let missing = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["evm", "interface"])
        .arg(&path)
        .output()
        .unwrap();
    failure(missing, 2, "--encoding");
}

#[test]
fn evm_empty_inputs_and_budgets_keep_complete_delivery_and_previous_outputs() {
    let (directory, path) = fixture(" \n0x\t", "empty.hex");
    let empty = result(&run("analyze", &path, "hex", &["--include-bytecode"]));
    assert_eq!(empty["detail"]["bytecode"]["bytes"], 0);
    assert_eq!(empty["detail"]["bytecode"]["hex"], "");
    assert_eq!(
        empty["detail"]["result"]["functions"],
        serde_json::json!([])
    );
    assert_eq!(empty["detail"]["discovery_complete"], Value::Null);
    fs::write(&path, "6003565b00").unwrap();
    let plain = run("analyze", &path, "hex", &[]);
    result(&plain);
    let destination = directory.path().join("saved.json");
    let saved = run(
        "analyze",
        &path,
        "hex",
        &["-o", destination.to_str().unwrap()],
    );
    success(&saved);
    assert!(saved.stdout.is_empty());
    assert_eq!(fs::read(&destination).unwrap(), plain.stdout);
    // Ceilings live in rxd_core::Budget; native analysis exposes no knob for them.
    for budget in [
        ["--max-input-bytes", "1"],
        ["--max-records", "1"],
        ["--max-output-bytes", "1"],
    ] {
        fs::write(&destination, "preserved").unwrap();
        let mut unknown = vec![budget[0], budget[1], "-o"];
        unknown.push(destination.to_str().unwrap());
        failure(run("analyze", &path, "hex", &unknown), 2, budget[0]);
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
}

#[test]
fn evm_outline_ids_fetch_exact_blocks_across_raw_and_hex_carriers() {
    // Independent instruction facts: PUSH2 operand contains 5b, which is not JUMPDEST.
    // Block [0,4): PUSH2 005b; JUMP. Block [4,8): JUMPDEST; PUSH1 aa; STOP.
    let (directory, path) = fixture(" \n0X61005B565B60AA00\r\n", "blocks.hex");
    let raw = directory.path().join("blocks.bin");
    fs::write(&raw, [0x61, 0x00, 0x5b, 0x56, 0x5b, 0x60, 0xaa, 0x00]).unwrap();
    for (input, encoding, carrier_start, stride) in [(&path, "hex", 4, 2), (&raw, "raw", 0, 1)] {
        let output = run("blocks", input, encoding, &[]);
        let rows = jsonl(&output);
        assert_eq!(rows[0]["schema_version"], 2);
        assert_eq!(rows[0]["detail"]["disassembly_complete"], true);
        let blocks: Vec<_> = rows
            .iter()
            .filter(|r| r["record"] == "basic_block")
            .collect();
        assert_eq!(blocks.len(), 2);
        for (block, start, end, raw_hex, texts) in [
            (blocks[0], 0, 4, "61005b56", vec!["PUSH2 005b", "JUMP"]),
            (
                blocks[1],
                4,
                8,
                "5b60aa00",
                vec!["JUMPDEST", "PUSH1 aa", "STOP"],
            ),
        ] {
            assert_eq!(block["detail"]["decoded_start"], start);
            assert_eq!(block["detail"]["decoded_end"], end);
            assert_eq!(block["start"], carrier_start + start * stride);
            assert_eq!(block["end"], carrier_start + end * stride);
            assert_eq!(block["detail"]["upstream_block_id"], Value::Null);
            let id = block["at"].as_str().unwrap();
            let fetched = run("get-block", input, encoding, &["--at", id]);
            success(&fetched);
            let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
            assert_eq!(fetched["record"], "block");
            assert_eq!(fetched["at"], id);
            assert_eq!(fetched["detail"]["raw_hex"], raw_hex);
            assert_eq!(fetched["start"], block["start"]);
            assert_eq!(fetched["end"], block["end"]);
            assert_eq!(fetched["detail"]["complete"], true);
            let actual: Vec<_> = fetched["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["text"].as_str().unwrap())
                .collect();
            assert_eq!(actual, texts);
        }
    }
}

#[test]
fn evm_block_ids_survive_copy_and_reject_encodings_and_forged_bounds() {
    let (directory, path) = fixture("6003565b00", "original.hex");
    let rows = jsonl(&run("blocks", &path, "hex", &[]));
    let id = rows[1]["at"].as_str().unwrap();
    let copy = directory.path().join("copy.hex");
    fs::copy(&path, &copy).unwrap();
    let copy_rows = jsonl(&run("blocks", &copy, "hex", &[]));
    assert_eq!(copy_rows[1]["at"], id);
    success(&run("get-block", &copy, "hex", &["--at", id]));
    // The copy-only coordinate names a block in whichever carrier is queried; it does
    // not bind the content it was read from.
    let full = id.to_owned();
    let destination = directory.path().join("keep.json");
    fs::write(&destination, "preserved").unwrap();
    for forged in [
        format!("{full}:0"),
        full.replace(":block:", ":function:"),
        full.replace(":0:3", ":1:3"),
        full.replace(":0:3", ":0:2"),
        full.replace(":0:3", ":0:300"),
        full.replace(":0:3", ":00:3"),
        full.replace(":0:3", ":0:184467440737095516160"),
    ] {
        failure(
            run(
                "get-block",
                &path,
                "hex",
                &["--at", &forged, "-o", destination.to_str().unwrap()],
            ),
            1,
            "INVALID_QUERY",
        );
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
    failure(
        run("get-block", &path, "raw", &["--at", id]),
        1,
        "SOURCE_MISMATCH",
    );
    fs::write(&copy, "6004565b00").unwrap();
    success(&run("get-block", &copy, "hex", &["--at", &full]));
    fs::write(&path, "6003565b01").unwrap();
    success(&run("get-block", &path, "hex", &["--at", &full]));
}

#[test]
fn evm_block_navigation_preserves_incomplete_push_tail_and_empty_delivery() {
    for (carrier, block_count, tail) in [
        (" \n0x\t", 0, Value::Null),
        (
            "0X61ff",
            0,
            serde_json::json!({"decoded_start":0,"decoded_end":2,"carrier_start":2,"carrier_end":6,"hex":"61ff"}),
        ),
        (
            "0061ff",
            1,
            serde_json::json!({"decoded_start":1,"decoded_end":3,"carrier_start":2,"carrier_end":6,"hex":"61ff"}),
        ),
    ] {
        let (_directory, path) = fixture(carrier, "tail.hex");
        let rows = jsonl(&run("blocks", &path, "hex", &[]));
        assert_eq!(rows.len(), block_count + 2);
        assert_eq!(rows[0]["detail"]["undecoded_tail"], tail);
        assert_eq!(rows[0]["detail"]["disassembly_complete"], tail.is_null());
        assert_eq!(rows[0]["detail"]["discovery_complete"], Value::Null);
        if block_count != 0 {
            let output = run(
                "get-block",
                &path,
                "hex",
                &["--at", rows[1]["at"].as_str().unwrap()],
            );
            success(&output);
            let fetched: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(fetched["detail"]["raw_hex"], "00");
            assert_eq!(
                fetched["detail"]["instructions"],
                serde_json::json!([{
                    "decoded_offset":0,"decoded_end":1,"opcode":0,"operand_hex":"","text":"STOP"
                }])
            );
            assert_eq!(fetched["detail"]["disassembly_complete"], true);
        }
    }
}

#[test]
fn evm_block_navigation_reports_effective_limits_and_publishes_atomically() {
    let (directory, path) = fixture("6003565b00", "jump.hex");
    let plain = run("blocks", &path, "hex", &[]);
    let rows = jsonl(&plain);
    let id = rows[1]["at"].as_str().unwrap();
    let profiled = run("blocks", &path, "hex", &["--perf"]);
    assert_eq!(profiled.stdout, plain.stdout);
    let reports = perf_reports(&profiled);
    // The shared budget table supplies every ceiling; no CLI flag overrides one.
    assert_eq!(reports[0]["resource_limits"]["work"], 1_001_280);
    assert!(reports[0]["resource_limits"].get("records").is_none());
    // Five decoded bytes, four complete instructions, two native blocks.
    assert_eq!(reports[0]["counters"]["work"], 11);
    assert_eq!(reports[0]["counters"]["instructions"], 4);
    assert_eq!(reports[0]["counters"]["records"], 2);
    for stage in ["decode", "analyze", "navigate", "serialize"] {
        assert!(
            reports[0]["stages_ns"][stage].as_u64().unwrap() > 0,
            "{stage}"
        );
    }
    let fetched = run("get-block", &path, "hex", &["--at", id]);
    success(&fetched);
    let destination = directory.path().join("saved.json");
    for (command, args, expected) in [
        ("blocks", vec![], plain.stdout.as_slice()),
        ("get-block", vec!["--at", id], fetched.stdout.as_slice()),
    ] {
        let mut file_args = args.clone();
        file_args.extend(["-o", destination.to_str().unwrap()]);
        let saved = run(command, &path, "hex", &file_args);
        success(&saved);
        assert!(saved.stdout.is_empty());
        assert_eq!(fs::read(&destination).unwrap(), expected);
        let mut overwrite = args.clone();
        overwrite.extend(["-o", path.to_str().unwrap()]);
        failure(run(command, &path, "hex", &overwrite), 2, "INVALID_QUERY");
        assert_eq!(fs::read(&path).unwrap(), b"6003565b00");
        // Every ceiling lives in rxd_core::Budget, so no `--max-*` flag exists.
        for budget in [
            ["--max-input-bytes", "1"],
            ["--max-output-bytes", "1"],
            ["--max-work", "1"],
            ["--max-records", "1"],
            ["--max-states", "1"],
            ["--max-stack-cells", "1"],
            ["--max-reconstruction-work", "1"],
            ["--max-values", "1"],
        ] {
            let mut unknown = args.clone();
            unknown.extend(budget);
            failure(run(command, &path, "hex", &unknown), 2, budget[0]);
        }
    }
    // Failures keep the previous output file intact.
    fs::write(&destination, "preserved").unwrap();
    failure(
        run(
            "get-block",
            &path,
            "hex",
            &[
                "--at",
                "hex:block:99:100",
                "-o",
                destination.to_str().unwrap(),
            ],
        ),
        1,
        "INVALID_QUERY",
    );
    assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    fs::write(&path, "0xgg").unwrap();
    failure(
        run(
            "blocks",
            &path,
            "hex",
            &["-o", destination.to_str().unwrap()],
        ),
        1,
        "INVALID_INPUT",
    );
    assert_eq!(fs::read(&destination).unwrap(), b"preserved");
}

#[test]
fn evm_final_push_block_fetch_includes_the_full_operand_and_fixed_schema() {
    let (_directory, path) = fixture(" 0x5b61005B\t", "final-push.hex");
    let rows = jsonl(&run("blocks", &path, "hex", &["--include-bytecode"]));
    assert_eq!(rows[0]["detail"]["bytecode"]["hex"], "5b61005b");
    assert_eq!(rows[1]["detail"]["last_opcode_start"], 1);
    assert_eq!(rows[1]["detail"]["decoded_end"], 4);
    assert_eq!(rows[1]["start"], 3);
    assert_eq!(rows[1]["end"], 11);
    assert_js_keys(
        &rows[0],
        &[
            "record",
            "backend",
            "command",
            "source_id",
            "input_bytes",
            "schema_version",
            "query",
            "detail",
        ],
    );
    assert_js_keys(&rows[0]["detail"], acceptance::HEADER_DETAIL_KEYS);
    assert_js_keys(
        &rows[1],
        &[
            "record",
            "at",
            "source_id",
            "parent_at",
            "context_at",
            "start",
            "end",
            "detail",
        ],
    );
    assert_js_keys(
        &rows[1]["detail"],
        &[
            "unit_kind",
            "decoded_start",
            "decoded_end",
            "last_opcode_start",
            "instruction_count",
            "upstream_block_id",
            "boundary_status",
            "owner_ids",
            "shared",
        ],
    );
    let output = run(
        "get-block",
        &path,
        "hex",
        &["--at", rows[1]["at"].as_str().unwrap()],
    );
    success(&output);
    let fetched: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fetched["detail"]["raw_hex"], "5b61005b");
    assert_eq!(
        fetched["detail"]["instructions"][1],
        serde_json::json!({
            "decoded_offset":1,"decoded_end":4,"opcode":97,"operand_hex":"005b","text":"PUSH2 005b"
        })
    );
    assert_js_keys(
        &fetched,
        &[
            "record",
            "at",
            "source_id",
            "parent_at",
            "context_at",
            "start",
            "end",
            "detail",
            "backend",
            "command",
            "schema_version",
        ],
    );
    assert_js_keys(
        &fetched["detail"],
        &[
            "unit_kind",
            "decoded_start",
            "decoded_end",
            "last_opcode_start",
            "instruction_count",
            "upstream_block_id",
            "boundary_status",
            "owner_ids",
            "shared",
            "carrier",
            "bytecode",
            "raw_hex",
            "instructions",
            "disassembly_complete",
            "undecoded_tail",
            "complete",
        ],
    );
    assert_js_keys(&rows[2]["detail"], acceptance::SUMMARY_DETAIL_KEYS);
}

// Authored dispatcher, two caller-specific returns through helper [60,66).
// A writes 2 to slot 10; B writes 4 to slot 20. No bytecode is executed.
const SHARED_HELPER: &str = "5f3560e01c8063112233441460205780635566778814602e57000000000000005b5060296001603c565b600a55005b5060376003603c565b601455005b6001019056";

#[test]
fn evm_function_ids_fetch_only_selected_disjoint_regions_and_shared_helper_return() {
    let (_directory, path) = fixture(SHARED_HELPER, "shared.hex");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let functions: Vec<_> = rows.iter().filter(|r| r["record"] == "function").collect();
    assert_eq!(functions.len(), 2);
    let a = functions
        .iter()
        .find(|r| r["detail"]["selector"] == "11223344")
        .unwrap();
    let b = functions
        .iter()
        .find(|r| r["detail"]["selector"] == "55667788")
        .unwrap();
    for (function, starts, segments) in [
        (
            *a,
            vec![32, 41, 60],
            vec!["5b5060296001603c565b600a5500", "5b6001019056"],
        ),
        (
            *b,
            vec![46, 55, 60],
            vec!["5b5060376003603c565b601455005b6001019056"],
        ),
    ] {
        let output = run(
            "get-function",
            &path,
            "hex",
            &["--at", function["at"].as_str().unwrap(), "--raw"],
        );
        success(&output);
        let fetched: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(fetched["at"], function["at"]);
        assert_eq!(fetched["record"], "function_code");
        assert_eq!(fetched["command"], "get-function");
        assert_eq!(fetched["detail"]["complete"], true);
        assert_eq!(fetched["detail"]["boundary_complete"], true);
        assert_eq!(fetched["detail"]["discovery_complete"], Value::Null);
        assert_eq!(
            fetched["detail"]["reconstruction"]["status"],
            "not_requested"
        );
        let actual: Vec<_> = fetched["detail"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["detail"]["decoded_start"].as_u64().unwrap())
            .collect();
        assert_eq!(actual, starts);
        let actual: Vec<_> = fetched["detail"]["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["raw_hex"].as_str().unwrap())
            .collect();
        assert_eq!(actual, segments);
        let helper = fetched["detail"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["detail"]["decoded_start"] == 60)
            .unwrap();
        assert_eq!(helper["detail"]["owner_ids"].as_array().unwrap().len(), 2);
        assert_eq!(helper["detail"]["shared"], true);
        let selector = function["detail"]["selector"].as_str().unwrap();
        let by_selector = run(
            "get-function",
            &path,
            "hex",
            &["--selector", selector, "--raw"],
        );
        assert_eq!(by_selector.stdout, output.stdout);
        if selector == "11223344" {
            assert_eq!(fetched["start"], Value::Null);
            assert_eq!(fetched["end"], Value::Null);
        }
    }
}

#[test]
fn evm_selector_selection_rejects_multiple_evidenced_dispatch_entries() {
    // CALLVALUE chooses two dispatcher paths for the same selector. Their terminal
    // JUMPDEST entries are 64 and 80, so a selector alone cannot pick one target.
    let mut bytecode =
        "346012575f3560e01c6311223344146040575b5f3560e01c63112233441460505700".to_string();
    bytecode.push_str(&"00".repeat(64 - bytecode.len() / 2));
    bytecode.push_str("5b00");
    bytecode.push_str(&"00".repeat(80 - bytecode.len() / 2));
    bytecode.push_str("5b00");
    let (_directory, path) = fixture(&bytecode, "ambiguous.hex");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let functions: Vec<_> = rows.iter().filter(|r| r["record"] == "function").collect();
    assert_eq!(functions.len(), 2);
    failure(
        run("get-function", &path, "hex", &["--selector", "11223344"]),
        1,
        "AMBIGUOUS_SELECTOR",
    );
    for f in functions {
        let fetched = run(
            "get-function",
            &path,
            "hex",
            &["--at", f["at"].as_str().unwrap(), "--raw"],
        );
        success(&fetched);
        let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
        assert_eq!(fetched["detail"]["segments"][0]["raw_hex"], "5b00");
    }
}

#[test]
fn evm_function_fetch_reports_effective_limits_and_keeps_previous_output() {
    let (directory, path) = fixture(SHARED_HELPER, "shared.hex");
    let destination = directory.path().join("previous.json");
    // Every ceiling comes from rxd_core::Budget, so the flags no longer exist.
    for budget in [["--max-states", "1"], ["--max-stack-cells", "1"]] {
        fs::write(&destination, "preserved").unwrap();
        let mut query = vec![
            "--selector",
            "11223344",
            "--raw",
            "--perf",
            "-o",
            destination.to_str().unwrap(),
        ];
        query.extend(budget);
        failure(run("get-function", &path, "hex", &query), 2, budget[0]);
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
    // A failing selector keeps the previous output and still reports why.
    fs::write(&destination, "preserved").unwrap();
    let failed = run(
        "get-function",
        &path,
        "hex",
        &[
            "--selector",
            "ffffffff",
            "--perf",
            "-o",
            destination.to_str().unwrap(),
        ],
    );
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
    assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    let reports = perf_reports(&failed);
    assert_eq!(reports[0]["error_code"], "NOT_FOUND");
    let profiled = run(
        "get-function",
        &path,
        "hex",
        &["--selector", "11223344", "--raw", "--perf"],
    );
    success(&profiled);
    let reports = perf_reports(&profiled);
    assert_eq!(reports[0]["resource_limits"]["states"], 100_000);
    assert_eq!(reports[0]["resource_limits"]["stack_cells"], 1_048_576);
    assert_eq!(reports[0]["counters"]["matches"], 15); // selected original instructions
    assert_eq!(reports[0]["counters"]["semantic_nodes"], 3); // selected contexts
    // Native single-object analysis reports no navigation ceiling.
    let native = run("analyze", &path, "hex", &["--perf"]);
    success(&native);
    let native_reports = perf_reports(&native);
    assert!(native_reports[0]["resource_limits"].get("work").is_none());
    assert!(native_reports[0]["resource_limits"].get("states").is_none());
}

#[test]
fn evm_function_raw_fallback_preserves_unknown_and_invalid_jump_evidence() {
    for (entry_hex, reason, destination, expected_starts) in [
        (
            "5b505f3556",
            "dynamic_jump_destination",
            Value::Null,
            vec![32],
        ),
        // PUSH1 5b payload at 35 is not an instruction-aligned JUMPDEST.
        (
            "5b50605b602356",
            "invalid_jump_destination",
            serde_json::json!(format!("{:064x}", 35)),
            vec![32],
        ),
        // Unknown condition; invalid true destination POP at 33; false STOP is valid.
        (
            "5b505f3560215700",
            "invalid_jump_destination",
            serde_json::json!(format!("{:064x}", 33)),
            vec![32, 39],
        ),
        (
            "5b5061ff",
            "undecoded_or_unindexed_successor",
            Value::Null,
            vec![32],
        ),
    ] {
        let prefix = "5f3560e01c8063112233441460205700";
        let carrier = format!("{prefix}{}{entry_hex}", "00".repeat(32 - prefix.len() / 2));
        let (_directory, path) = fixture(&carrier, "unknown.hex");
        let rows = jsonl(&run("interface", &path, "hex", &[]));
        let function = rows.iter().find(|r| r["record"] == "function").unwrap();
        let output = run(
            "get-function",
            &path,
            "hex",
            &["--at", function["at"].as_str().unwrap()],
        );
        success(&output);
        let fetched: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(fetched["detail"]["complete"], true);
        assert_eq!(fetched["detail"]["boundary_complete"], false);
        assert_eq!(fetched["detail"]["boundary_status"], "incomplete");
        assert_eq!(
            fetched["detail"]["reconstruction"]["status"],
            "raw_fallback"
        );
        assert_eq!(fetched["detail"]["reconstruction"]["reason"], reason);
        assert_eq!(fetched["detail"]["reconstruction"]["text"], Value::Null);
        assert_eq!(fetched["detail"]["reconstruction"]["complete"], false);
        let unknown = fetched["detail"]["unknown_successors"].as_array().unwrap();
        assert!(
            unknown
                .iter()
                .any(|r| r["reason"] == reason && r["destination_hex"] == destination),
            "{fetched}"
        );
        let starts: Vec<_> = fetched["detail"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["detail"]["decoded_start"].as_u64().unwrap())
            .collect();
        assert_eq!(starts, expected_starts);
        for block in fetched["detail"]["blocks"].as_array().unwrap() {
            let raw = run(
                "get-block",
                &path,
                "hex",
                &["--at", block["at"].as_str().unwrap()],
            );
            success(&raw);
        }
    }
}

#[test]
fn evm_function_ids_validate_discovered_units_carriers_and_selector_queries() {
    let (directory, path) = fixture(
        format!(" \n0X{}\r\n", SHARED_HELPER.to_uppercase()),
        "carrier.hex",
    );
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let function = rows
        .iter()
        .find(|r| r["record"] == "function" && r["detail"]["selector"] == "11223344")
        .unwrap();
    let id = function["at"].as_str().unwrap();
    let copy = directory.path().join("renamed.hex");
    fs::copy(&path, &copy).unwrap();
    let fetch = run("get-function", &copy, "hex", &["--at", id, "--raw"]);
    success(&fetch);
    let fetched: Value = serde_json::from_slice(&fetch.stdout).unwrap();
    assert_eq!(fetched["detail"]["segments"][0]["carrier_start"], 68);
    assert_eq!(fetched["detail"]["segments"][0]["carrier_end"], 96);
    assert_eq!(fetched["detail"]["segments"][1]["carrier_start"], 124);
    assert_eq!(fetched["detail"]["segments"][1]["carrier_end"], 136);
    let full = id.to_owned();
    for invalid in [
        format!("{full}:0"),
        full.replace(":function:", ":block:"),
        full.replace(":abi:32", ":abi:33"),
        full.replace(":abi:32", ":abi:032"),
        full.replace("11223344:abi", "ffffffff:abi"),
        full.replace(":abi:32", ":abi:184467440737095516160"),
    ] {
        failure(
            run("get-function", &path, "hex", &["--at", &invalid]),
            1,
            "INVALID_QUERY",
        );
    }
    failure(
        run("get-function", &path, "raw", &["--at", id]),
        1,
        "SOURCE_MISMATCH",
    );
    fs::write(&copy, SHARED_HELPER).unwrap();
    success(&run("get-function", &copy, "hex", &["--at", &full]));
    for selector in ["", "0x0011", "gg223344"] {
        failure(
            run("get-function", &path, "hex", &["--selector", selector]),
            2,
            "INVALID_QUERY",
        );
    }
    failure(
        run("get-function", &path, "hex", &["--selector", "ffffffff"]),
        1,
        "NOT_FOUND",
    );
    success(&run(
        "get-function",
        &path,
        "hex",
        &["--selector", "0X11223344", "--raw"],
    ));
    let raw = directory.path().join("runtime.bin");
    let bytes: Vec<_> = SHARED_HELPER
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    fs::write(&raw, bytes).unwrap();
    let raw_rows = jsonl(&run("interface", &raw, "raw", &[]));
    let raw_function = raw_rows
        .iter()
        .find(|r| r["record"] == "function" && r["detail"]["selector"] == "11223344")
        .unwrap();
    let raw_output = run(
        "get-function",
        &raw,
        "raw",
        &["--at", raw_function["at"].as_str().unwrap(), "--raw"],
    );
    success(&raw_output);
    let raw_fetch: Value = serde_json::from_slice(&raw_output.stdout).unwrap();
    assert_ne!(raw_fetch["at"], fetched["at"]);
    assert_eq!(
        raw_fetch["detail"]["bytecode"],
        fetched["detail"]["bytecode"]
    );
    assert_eq!(
        raw_fetch["detail"]["instructions"],
        fetched["detail"]["instructions"]
    );
    assert_eq!(raw_fetch["detail"]["segments"][0]["carrier_start"], 32);
    assert_eq!(raw_fetch["detail"]["segments"][0]["carrier_end"], 46);
    fs::write(&raw, []).unwrap();
    let empty = jsonl(&run("interface", &raw, "raw", &[]));
    assert_eq!(empty.last().unwrap()["detail"]["functions"], 0);
    failure(
        run("get-function", &raw, "raw", &["--selector", "11223344"]),
        1,
        "NOT_FOUND",
    );
}

#[test]
fn evm_compiled_function_navigation_retains_dispatcher_stack_and_memory_prelude() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/evm/authored-navigation.runtime.hex");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let function = rows
        .iter()
        .find(|r| r["record"] == "function" && r["detail"]["selector"] == "1ba64846")
        .unwrap();
    let output = run(
        "get-function",
        &path,
        "hex",
        &["--at", function["at"].as_str().unwrap(), "--raw"],
    );
    success(&output);
    let fetched: Value = serde_json::from_slice(&output.stdout).unwrap();
    let detail = &fetched["detail"];
    // Independent solc methodIdentifiers, original ASM and trusted source facts.
    assert_eq!(
        detail["bytecode"]["sha256"],
        "57b5cd6175ac081a520fbb9cae66f5700b5cc09ea8d60802f54a4de12bdb4602"
    );
    assert_eq!(detail["decoded_entry"], 514);
    assert_eq!(detail["boundary_complete"], true);
    let starts: Vec<_> = detail["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["detail"]["decoded_start"].as_u64().unwrap())
        .collect();
    assert_eq!(starts, vec![152, 514, 520, 532]);
    assert_eq!(
        detail["entry_states"][0]["stack"],
        serde_json::json!([{"kind":"constant","hex":format!("{:064x}",0x1ba64846)}])
    );
    assert_eq!(
        detail["entry_states"][0]["memory_words"],
        serde_json::json!([{"offset":64,"value_hex":format!("{:064x}",128),"write_pc":5}])
    );
    assert_eq!(detail["instruction_count"], 26);
    assert!(
        detail["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["decoded_offset"] == 542 && i["text"] == "ADD")
    );
    assert_eq!(
        detail["blocks"][0]["detail"]["owner_ids"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    assert_eq!(detail["blocks"][0]["detail"]["shared"], true);
}

#[test]
fn evm_function_membership_keeps_return_context_through_nested_helpers_and_data_loop() {
    // Nested H calls N with return 70; N preserves deeper caller return 41 or 55.
    let nested = format!(
        "{}5b6046604a56000000005b9056005b906001019056",
        &SHARED_HELPER[..120]
    );
    // A's data counter widens at loop [82,96), preserving saved return 41.
    let mut looped = SHARED_HELPER.to_string();
    looped.replace_range(64..82, "5b5060525600000000");
    looped.push_str(&"00".repeat(82 - looped.len() / 2));
    looped.push_str("5b60295f5b600101600435605657603c56");
    for (carrier, starts) in [
        (nested, vec![32, 41, 60, 70, 74]),
        (looped, vec![32, 41, 60, 82, 86, 96]),
    ] {
        let (_directory, path) = fixture(&carrier, "context.hex");
        let rows = jsonl(&run("interface", &path, "hex", &[]));
        let function = rows
            .iter()
            .find(|r| r["record"] == "function" && r["detail"]["selector"] == "11223344")
            .unwrap();
        let output = run(
            "get-function",
            &path,
            "hex",
            &["--at", function["at"].as_str().unwrap(), "--raw"],
        );
        success(&output);
        let fetched: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(fetched["detail"]["boundary_complete"], true, "{fetched}");
        let actual: Vec<_> = fetched["detail"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["detail"]["decoded_start"].as_u64().unwrap())
            .collect();
        assert_eq!(actual, starts);
        assert!(!actual.contains(&55));
        assert_eq!(
            fetched["detail"]["unknown_successors"],
            serde_json::json!([])
        );
    }
}

#[test]
fn evm_function_navigation_has_fixed_schema_atomic_output_and_separate_completeness() {
    let (directory, path) = fixture(SHARED_HELPER, "schema.hex");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let function = rows.iter().find(|r| r["record"] == "function").unwrap();
    let id = function["at"].as_str().unwrap();
    let function_keys = [
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
    ];
    let mut interface_keys = function_keys.to_vec();
    interface_keys.extend(["arguments", "stateMutability"]);
    assert_js_keys(&function["detail"], &interface_keys);
    let plain = run("get-function", &path, "hex", &["--at", id, "--raw"]);
    success(&plain);
    let fetched: Value = serde_json::from_slice(&plain.stdout).unwrap();
    assert_js_keys(
        &fetched,
        &[
            "record",
            "backend",
            "command",
            "schema_version",
            "at",
            "source_id",
            "parent_at",
            "context_at",
            "start",
            "end",
            "detail",
        ],
    );
    let mut fetch_keys = function_keys.to_vec();
    fetch_keys.extend([
        "carrier",
        "bytecode",
        "instructions",
        "control_flow",
        "unresolved_evidence",
        "reconstruction",
        "complete",
    ]);
    assert_js_keys(&fetched["detail"], &fetch_keys);
    assert_js_keys(
        &fetched["detail"]["segments"][0],
        &[
            "decoded_start",
            "decoded_end",
            "carrier_start",
            "carrier_end",
            "raw_hex",
        ],
    );
    let context = &fetched["detail"]["control_flow"][0];
    assert_js_keys(
        context,
        &[
            "context",
            "block_id",
            "entry_state",
            "exit_state",
            "edges",
            "problems",
        ],
    );
    assert_js_keys(&context["entry_state"], &["stack", "memory_words"]);
    assert_js_keys(
        &context["edges"][0],
        &[
            "instruction_offset",
            "kind",
            "target",
            "target_context",
            "resolution",
        ],
    );
    assert_js_keys(
        &fetched["detail"]["reconstruction"],
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
    let profiled = run(
        "get-function",
        &path,
        "hex",
        &["--at", id, "--raw", "--perf"],
    );
    assert_eq!(plain.stdout, profiled.stdout);
    let reports = perf_reports(&profiled);
    assert_eq!(reports[0]["counters"]["records"], 1);
    let destination = directory.path().join("saved.json");
    success(&run(
        "get-function",
        &path,
        "hex",
        &["--at", id, "--raw", "-o", destination.to_str().unwrap()],
    ));
    assert_eq!(fs::read(&destination).unwrap(), plain.stdout);
    for budget in [
        ["--max-output-bytes", "1"],
        ["--max-work", "1"],
        ["--max-input-bytes", "1"],
        ["--max-records", "1"],
    ] {
        fs::write(&destination, "preserved").unwrap();
        failure(
            run(
                "get-function",
                &path,
                "hex",
                &[
                    "--at",
                    id,
                    "--raw",
                    budget[0],
                    budget[1],
                    "-o",
                    destination.to_str().unwrap(),
                ],
            ),
            2,
            budget[0],
        );
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
    failure(
        run(
            "get-function",
            &path,
            "hex",
            &["--at", id, "-o", path.to_str().unwrap()],
        ),
        2,
        "INVALID_QUERY",
    );
    assert_eq!(fs::read(&path).unwrap(), SHARED_HELPER.as_bytes());
}

#[test]
fn evm_function_stack_storage_ceiling_comes_from_the_shared_budget() {
    let prefix = "5f3560e01c8063112233441460205700";
    let carrier = format!(
        "{prefix}{}5b50{}00",
        "00".repeat(32 - prefix.len() / 2),
        "5f".repeat(128)
    );
    let (_directory, path) = fixture(&carrier, "exit-stack.hex");
    failure(
        run(
            "get-function",
            &path,
            "hex",
            &[
                "--selector",
                "11223344",
                "--raw",
                "--max-stack-cells",
                "100",
            ],
        ),
        2,
        "--max-stack-cells",
    );
    // 128 stack words stay far below the shared ceiling and fetch normally.
    success(&run(
        "get-function",
        &path,
        "hex",
        &["--selector", "11223344", "--raw"],
    ));
}

/// A linear scan does not fail merely because input exceeds a fixed work quota.
#[test]
fn evm_input_scaled_work_allows_large_linear_scans_with_complete_output() {
    // Over 1.1M decoded bytes but only one block. Do not turn this regression into
    // a million-row output stress test: PUSH32 payloads are original input bytes.
    let carrier = format!("{}00", format!("7f{}", "00".repeat(32)).repeat(33_334));
    let (directory, path) = fixture(carrier, "large.hex");
    let destination = directory.path().join("preserved.json");
    fs::write(&destination, "preserved").unwrap();
    let output = run(
        "blocks",
        &path,
        "hex",
        &["--perf", "-o", destination.to_str().unwrap()],
    );
    success(&output);
    assert!(output.stdout.is_empty());
    let rows: Vec<Value> = fs::read_to_string(&destination)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.last().unwrap()["complete"], true);
    assert_eq!(rows.last().unwrap()["records"], 1);
    let reports = perf_reports(&output);
    assert_eq!(reports[0]["error_code"], Value::Null);
    assert_eq!(
        reports[0]["resource_limits"]["work"],
        rxd_core::Budget::for_input_bytes(fs::metadata(&path).unwrap().len() as usize)
            .unwrap()
            .work
    );
    assert!(reports[0]["counters"]["work"].as_u64().unwrap() > 1_000_000);
}

/// The removed mixed stream and the platform-native range view that stays.
#[test]
fn evm_lists_one_kind_per_command_and_filters_by_the_row_name() {
    let (_directory, path) = fixture(SHARED_HELPER, "filter.hex");
    for removed in ["outline", "functions"] {
        let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["evm", removed])
            .arg(&path)
            .args(["--encoding", "hex"])
            .output()
            .unwrap();
        failure(output, 2, removed);
    }
    // `basic-blocks` stays the native EVMole range view, not a second block listing.
    let native = result(&run("basic-blocks", &path, "hex", &[]));
    let ranges = native["detail"]["result"]["basicBlocks"]
        .as_array()
        .unwrap()
        .clone();
    let blocks = jsonl(&run("blocks", &path, "hex", &[]));
    assert_eq!(
        blocks
            .iter()
            .filter(|r| r["record"] == "basic_block")
            .count(),
        ranges.len()
    );
    let filtered = jsonl(&run("blocks", &path, "hex", &["-f", "block:6"]));
    assert_eq!(filtered[0]["query"]["filter"], "block:6");
    let selected: Vec<_> = filtered
        .iter()
        .filter(|r| r["record"] == "basic_block")
        .collect();
    assert_eq!(selected.len(), 1);
    assert!(selected[0]["at"].as_str().unwrap().contains("block:6"));
    assert_eq!(filtered.last().unwrap()["detail"]["blocks"], 1);
    // The interface listing filters on the selector each row is named by.
    let one = jsonl(&run("interface", &path, "hex", &["-f", "55667788"]));
    let selected: Vec<_> = one.iter().filter(|r| r["record"] == "function").collect();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0]["detail"]["selector"], "55667788");
    assert_eq!(one.last().unwrap()["detail"]["functions"], 1);
    // Literal rows are named by their decoded value: text when printable, else hex.
    let literals = jsonl(&run(
        "strings",
        &path,
        "hex",
        &["--kind", "bytes", "-f", "1122"],
    ));
    let selected: Vec<_> = literals
        .iter()
        .filter(|r| r["record"] == "literal")
        .collect();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0]["detail"]["value"], "11223344");
    // `index` is the literal's position in the whole decoded inventory (PUSH0, PUSH1).
    assert_eq!(selected[0]["detail"]["index"], 2);
    // Reference rows filter on the caller coordinate.
    let all = jsonl(&run("xref", &path, "hex", &["--at", "hex:block:60:66"]));
    assert_eq!(
        all.iter()
            .filter(|r| r["record"] == "code_reference")
            .count(),
        2
    );
}

mod reconstruction;

#[test]
fn evm_code_refs_route_unowned_static_jumps_to_exact_raw_blocks() {
    // Byte 2 is JUMP to instruction-aligned JUMPDEST at 3.
    let (directory, path) = fixture(" \n0X6003565B00\r\n", "refs.hex");
    let raw = directory.path().join("refs.bin");
    fs::write(&raw, [0x60, 0x03, 0x56, 0x5b, 0x00]).unwrap();
    for (input, encoding, offset, stride) in [(&path, "hex", 4, 2), (&raw, "raw", 0, 1)] {
        let outline = jsonl(&run("blocks", input, encoding, &[]));
        let target = outline
            .iter()
            .find(|r| r["record"] == "basic_block" && r["detail"]["decoded_start"] == 3)
            .unwrap();
        let target_id = target["at"].as_str().unwrap();
        let rows = jsonl(&run("xref", input, encoding, &["--at", target_id]));
        assert_eq!(rows[0]["query"]["at"], target_id);
        assert_eq!(rows.len(), 3);
        let hit = &rows[1];
        assert_eq!(hit["record"], "code_reference");
        assert_eq!(hit["at"], Value::Null);
        assert_eq!(hit["detail"]["target"], target_id);
        assert_eq!(hit["detail"]["target_kind"], "block");
        assert_eq!(hit["detail"]["target_decoded_offset"], 3);
        assert_eq!(hit["detail"]["instruction_offset"], 2);
        assert_eq!(hit["detail"]["reference_kind"], "jump");
        assert_eq!(hit["detail"]["resolution"], "resolved");
        assert_eq!(hit["detail"]["context_kind"], "block");
        assert_eq!(hit["detail"]["ownership_status"], "unresolved");
        assert_eq!(hit["start"], offset + 2 * stride);
        assert_eq!(hit["end"], offset + 3 * stride);
        let fetched = run(
            "get-block",
            input,
            encoding,
            &["--at", hit["context_at"].as_str().unwrap()],
        );
        success(&fetched);
        let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
        assert_eq!(fetched["detail"]["raw_hex"], "600356");
        assert_eq!(fetched["detail"]["instructions"][1]["text"], "JUMP");
        assert_eq!(rows.last().unwrap()["detail"]["resolution_complete"], true);
        assert_eq!(rows.last().unwrap()["detail"]["unknown_jumps"], 0);
    }
}

#[test]
fn evm_code_refs_preserve_caller_specific_shared_helper_routes_and_entry_semantics() {
    let (directory, path) = fixture(SHARED_HELPER, "shared-refs.hex");
    let raw = directory.path().join("shared-refs.bin");
    let bytes: Vec<_> = SHARED_HELPER
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect();
    fs::write(&raw, bytes).unwrap();
    for (input, encoding) in [(&path, "hex"), (&raw, "raw")] {
        let outline = jsonl(&run("interface", input, encoding, &[]));
        let blocks = jsonl(&run("blocks", input, encoding, &[]));
        let a = outline
            .iter()
            .find(|r| r["record"] == "function" && r["detail"]["selector"] == "11223344")
            .unwrap();
        let b = outline
            .iter()
            .find(|r| r["record"] == "function" && r["detail"]["selector"] == "55667788")
            .unwrap();
        let a_id = a["at"].as_str().unwrap();
        let b_id = b["at"].as_str().unwrap();
        for (target_start, sites, owners) in [
            (60, vec![40, 54], vec![a_id, b_id]),
            (41, vec![65], vec![a_id]),
            (55, vec![65], vec![b_id]),
        ] {
            let block = blocks
                .iter()
                .find(|r| {
                    r["record"] == "basic_block" && r["detail"]["decoded_start"] == target_start
                })
                .unwrap();
            let rows = jsonl(&run(
                "xref",
                input,
                encoding,
                &["--at", block["at"].as_str().unwrap()],
            ));
            let hits: Vec<_> = rows
                .iter()
                .filter(|r| r["record"] == "code_reference")
                .collect();
            assert_eq!(hits.len(), sites.len());
            for ((hit, pc), owner) in hits.iter().zip(sites).zip(owners) {
                assert_eq!(hit["context_at"], owner);
                assert_eq!(hit["detail"]["context_kind"], "function");
                assert_eq!(hit["detail"]["instruction_offset"], pc);
                assert_eq!(hit["detail"]["target_decoded_offset"], target_start);
                // The shared helper has two owners, but only this contextual return
                // witness may route to this return target.
                assert_eq!(hit["detail"]["owner_ids"], serde_json::json!([owner]));
                assert_eq!(hit["detail"]["source_call_verified"], false);
                let fetch = run(
                    "get-function",
                    input,
                    encoding,
                    &["--at", hit["context_at"].as_str().unwrap(), "--raw"],
                );
                success(&fetch);
                let fetched: Value = serde_json::from_slice(&fetch.stdout).unwrap();
                assert!(
                    fetched["detail"]["instructions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|i| i["decoded_offset"] == pc && i["opcode"] == 0x56)
                );
                let contexts = fetched["detail"]["control_flow"].as_array().unwrap();
                let witness = contexts
                    .iter()
                    .find(|c| c["context"] == hit["detail"]["referencing_context_index"])
                    .unwrap();
                assert!(
                    witness["edges"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|e| e["instruction_offset"] == pc && e["target"] == target_start)
                );
                let starts: Vec<_> = fetched["detail"]["blocks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r["detail"]["decoded_start"].as_u64().unwrap())
                    .collect();
                assert!(!starts.contains(&(if owner == a_id { 55 } else { 41 })));
            }
        }
        let rows = jsonl(&run("xref", input, encoding, &["--at", a_id]));
        let hits: Vec<_> = rows
            .iter()
            .filter(|r| r["record"] == "code_reference")
            .collect();
        assert_eq!(hits.len(), 1); // Reference to entry32, not its helper/return blocks.
        assert_eq!(hits[0]["detail"]["target_kind"], "function");
        assert_eq!(hits[0]["detail"]["instruction_offset"], 14);
        assert_eq!(hits[0]["detail"]["reference_kind"], "selector_dispatch");
        assert_eq!(hits[0]["detail"]["context_kind"], "block");
        let fetch = run(
            "get-block",
            input,
            encoding,
            &["--at", hits[0]["context_at"].as_str().unwrap()],
        );
        success(&fetch);
        let fetch: Value = serde_json::from_slice(&fetch.stdout).unwrap();
        assert_eq!(
            fetch["detail"]["instructions"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["decoded_offset"],
            14
        );
    }
}

#[test]
fn evm_code_refs_do_not_resolve_a_block_that_requires_an_impossible_incoming_stack() {
    // An EVM stack holds at most 1024 words. These 1025 POPs cannot reach the jump.
    // The unowned block begins at 1. JUMP at 1030 targets JUMPDEST at 1031.
    let carrier = format!("005b{}610407565b00", "50".repeat(1025));
    let (_directory, path) = fixture(&carrier, "impossible-stack.hex");
    let outline = jsonl(&run("blocks", &path, "hex", &[]));
    let target = outline
        .iter()
        .find(|r| r["record"] == "basic_block" && r["detail"]["decoded_start"] == 1031)
        .unwrap();
    let refs = jsonl(&run(
        "xref",
        &path,
        "hex",
        &["--at", target["at"].as_str().unwrap()],
    ));
    assert_eq!(refs.last().unwrap()["records"], 0);
    assert_eq!(refs.last().unwrap()["detail"]["resolution_complete"], false);
    assert_eq!(refs.last().unwrap()["detail"]["analysis_problems"], 1);
}

#[test]
fn evm_code_refs_distinguish_empty_unknown_invalid_conditional_and_external_call_facts() {
    for (carrier, target_start, unknown, invalid, expected_kind, external) in [
        ("005b00".to_string(), 1, 0, 0, None, 0),
        ("5f35565b00".to_string(), 3, 1, 0, None, 0),
        // Jump destination1 is PUSH data5b, not an instruction JUMPDEST.
        ("605b6001565b00".to_string(), 5, 0, 1, None, 0),
        // Unknown condition and destination; the independent false edge remains usable.
        (
            "5f355f3557005b00".to_string(),
            5,
            1,
            0,
            Some("branch_false"),
            0,
        ),
        // Known false condition does not take the invalid true destination.
        (
            "5f600157005b00".to_string(),
            4,
            0,
            0,
            Some("branch_false"),
            0,
        ),
        // PUSH20 address36, CALL, and PUSH4 selector do not reference block36.
        (
            format!(
                "{}73{}245af150631122334450005b00",
                "5f".repeat(5),
                "00".repeat(19)
            ),
            36,
            0,
            0,
            None,
            1,
        ),
    ] {
        let (_directory, path) = fixture(&carrier, "resolution.hex");
        let outline = jsonl(&run("blocks", &path, "hex", &[]));
        let block = outline
            .iter()
            .find(|r| r["record"] == "basic_block" && r["detail"]["decoded_start"] == target_start)
            .unwrap();
        let rows = jsonl(&run(
            "xref",
            &path,
            "hex",
            &["--at", block["at"].as_str().unwrap()],
        ));
        let summary = rows.last().unwrap();
        assert_eq!(summary["complete"], true);
        assert_eq!(summary["records"], usize::from(expected_kind.is_some()));
        assert_eq!(summary["detail"]["unknown_jumps"], unknown);
        assert_eq!(summary["detail"]["invalid_jumps"], invalid);
        assert_eq!(summary["detail"]["external_call_sites"], external);
        assert_eq!(
            summary["detail"]["resolution_complete"],
            unknown == 0 && invalid == 0
        );
        assert_eq!(summary["detail"]["source_call_graph_complete"], false);
        if let Some(kind) = expected_kind {
            assert_eq!(rows[1]["detail"]["reference_kind"], kind);
            assert_eq!(rows[1]["detail"]["source_call_verified"], false);
            assert_eq!(rows[1]["detail"]["abi_call_verified"], false);
            let fetched = run(
                "get-block",
                &path,
                "hex",
                &["--at", rows[1]["context_at"].as_str().unwrap()],
            );
            success(&fetched);
            let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
            assert_eq!(
                fetched["detail"]["instructions"]
                    .as_array()
                    .unwrap()
                    .last()
                    .unwrap()["opcode"],
                0x57
            );
        }
    }
    let (_directory, path) = fixture("0061ff", "tail-refs.hex");
    let outline = jsonl(&run("blocks", &path, "hex", &[]));
    let rows = jsonl(&run(
        "xref",
        &path,
        "hex",
        &["--at", outline[1]["at"].as_str().unwrap()],
    ));
    assert_eq!(rows.last().unwrap()["records"], 0);
    assert_eq!(rows.last().unwrap()["complete"], true);
    assert_eq!(rows.last().unwrap()["detail"]["resolution_complete"], false);
    assert_eq!(rows[0]["detail"]["undecoded_tail"]["hex"], "61ff");
}

#[test]
fn evm_code_refs_reject_wrong_kind_and_forged_ids_without_partial_delivery() {
    let (directory, path) = fixture(SHARED_HELPER, "identity-refs.hex");
    let rows = jsonl(&run("interface", &path, "hex", &[]));
    let function = rows
        .iter()
        .find(|r| r["record"] == "function" && r["detail"]["selector"] == "11223344")
        .unwrap();
    let id = function["at"].as_str().unwrap();
    let destination = directory.path().join("previous.jsonl");
    let copy = directory.path().join("copy.hex");
    fs::copy(&path, &copy).unwrap();
    success(&run("xref", &copy, "hex", &["--at", id]));
    let full = id.to_owned();
    for forged in [
        format!("{full}:0"),
        full.replace(":function:", ":literal:"),
        full.replace(":function:", ":block:"),
        full.replace(":abi:32", ":abi:33"),
        full.replace(":abi:32", ":abi:032"),
        full.replace("11223344:abi", "ffffffff:abi"),
    ] {
        fs::write(&destination, "preserved").unwrap();
        failure(
            run(
                "xref",
                &path,
                "hex",
                &["--at", &forged, "-o", destination.to_str().unwrap()],
            ),
            1,
            "INVALID_QUERY",
        );
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
    failure(
        run("xref", &path, "raw", &["--at", id]),
        1,
        "SOURCE_MISMATCH",
    );
    fs::write(&copy, format!("{SHARED_HELPER}00")).unwrap();
    success(&run("xref", &copy, "hex", &["--at", &full]));
    let blocks = jsonl(&run("blocks", &path, "hex", &[]));
    let block = blocks
        .iter()
        .find(|r| r["record"] == "basic_block" && r["detail"]["decoded_start"] == 60)
        .unwrap();
    let forged = block["at"].as_str().unwrap().replace(":60:66", ":61:66");
    failure(
        run("xref", &path, "hex", &["--at", &forged]),
        1,
        "INVALID_QUERY",
    );
    failure(
        run(
            "xref",
            &path,
            "hex",
            &["--at", id, "-o", path.to_str().unwrap()],
        ),
        2,
        "INVALID_QUERY",
    );
    assert_eq!(fs::read(&path).unwrap(), SHARED_HELPER.as_bytes());
}

#[test]
fn evm_code_refs_have_fixed_schema_atomic_budgets_and_separate_query_perf() {
    let (directory, path) = fixture(SHARED_HELPER, "budget-refs.hex");
    let outline = jsonl(&run("blocks", &path, "hex", &[]));
    let target = outline
        .iter()
        .find(|r| r["record"] == "basic_block" && r["detail"]["decoded_start"] == 60)
        .unwrap();
    let id = target["at"].as_str().unwrap();
    let plain = run("xref", &path, "hex", &["--at", id]);
    let rows = jsonl(&plain);
    assert_js_keys(
        &rows[0],
        &[
            "record",
            "backend",
            "command",
            "source_id",
            "input_bytes",
            "schema_version",
            "query",
            "detail",
        ],
    );
    assert_js_keys(&rows[0]["query"], &["encoding", "at", "reference_scope"]);
    assert_js_keys(&rows[0]["detail"], acceptance::HEADER_DETAIL_KEYS);
    assert_js_keys(
        &rows[1],
        &[
            "record",
            "at",
            "source_id",
            "parent_at",
            "context_at",
            "start",
            "end",
            "detail",
        ],
    );
    assert_js_keys(
        &rows[1]["detail"],
        &[
            "target",
            "target_kind",
            "target_decoded_offset",
            "reference_kind",
            "resolution",
            "instruction_offset",
            "opcode",
            "operand_hex",
            "context_kind",
            "referencing_block_id",
            "referencing_context_index",
            "target_context_index",
            "evidence_kind",
            "incoming_stack_words",
            "owner_ids",
            "shared",
            "ownership_status",
            "source_call_verified",
            "abi_call_verified",
        ],
    );
    assert_js_keys(
        &rows.last().unwrap()["detail"],
        acceptance::SUMMARY_DETAIL_KEYS,
    );
    let profiled = run("xref", &path, "hex", &["--at", id, "--perf"]);
    assert_eq!(profiled.stdout, plain.stdout);
    let reports = perf_reports(&profiled);
    assert_eq!(reports[0]["resource_limits"]["states"], 100_000);
    assert_eq!(reports[0]["resource_limits"]["stack_cells"], 1_048_576);
    assert_eq!(reports[0]["counters"]["records"], 2);
    assert_eq!(reports[0]["counters"]["matches"], 2);
    for stage in ["query", "serialize", "navigate"] {
        assert!(reports[0]["stages_ns"][stage].as_u64().unwrap() > 0);
    }
    let destination = directory.path().join("saved.jsonl");
    success(&run(
        "xref",
        &path,
        "hex",
        &["--at", id, "-o", destination.to_str().unwrap()],
    ));
    assert_eq!(fs::read(&destination).unwrap(), plain.stdout);
    // Every ceiling lives in rxd_core::Budget, so no `--max-*` flag exists.
    for budget in [
        ["--max-input-bytes", "1"],
        ["--max-output-bytes", "1"],
        ["--max-records", "1"],
        ["--max-work", "1"],
        ["--max-states", "1"],
        ["--max-stack-cells", "1"],
    ] {
        fs::write(&destination, "preserved").unwrap();
        failure(
            run(
                "xref",
                &path,
                "hex",
                &[
                    "--at",
                    id,
                    budget[0],
                    budget[1],
                    "-o",
                    destination.to_str().unwrap(),
                ],
            ),
            2,
            budget[0],
        );
        assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    }
    // A stale selector keeps the previous file and reports the failure code.
    fs::write(&destination, "preserved").unwrap();
    let failed = run(
        "xref",
        &path,
        "hex",
        &[
            "--at",
            "hex:block:9999:99999",
            "--perf",
            "-o",
            destination.to_str().unwrap(),
        ],
    );
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
    assert_eq!(fs::read(&destination).unwrap(), b"preserved");
    let reports = perf_reports(&failed);
    assert_eq!(reports[0]["error_code"], "INVALID_QUERY");
}

#[test]
fn evm_code_refs_route_compiled_shared_revert_guards_to_all_supported_functions() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/evm");
    for encoding in ["raw", "hex"] {
        let path = corpus.join(if encoding == "raw" {
            "authored-navigation.runtime.bin"
        } else {
            "authored-navigation.runtime.hex"
        });
        let outline = jsonl(&run("interface", &path, encoding, &[]));
        let blocks = jsonl(&run("blocks", &path, encoding, &[]));
        let revert = blocks
            .iter()
            .find(|r| r["record"] == "basic_block" && r["detail"]["decoded_start"] == 152)
            .unwrap();
        let rows = jsonl(&run(
            "xref",
            &path,
            encoding,
            &["--at", revert["at"].as_str().unwrap()],
        ));
        let hits: Vec<_> = rows
            .iter()
            .filter(|r| {
                r["record"] == "code_reference" && r["detail"]["context_kind"] == "function"
            })
            .collect();
        let owners: std::collections::BTreeSet<_> = hits
            .iter()
            .map(|r| r["context_at"].as_str().unwrap())
            .collect();
        assert_eq!(owners.len(), 8); // Independent compiler methodIdentifiers and common revert ASM.
        let add = outline
            .iter()
            .find(|r| r["record"] == "function" && r["detail"]["selector"] == "1ba64846")
            .unwrap();
        let add_hits: Vec<_> = hits
            .iter()
            .filter(|r| r["context_at"] == add["at"])
            .collect();
        let pcs: Vec<_> = add_hits
            .iter()
            .map(|r| r["detail"]["instruction_offset"].as_u64().unwrap())
            .collect();
        assert_eq!(pcs, vec![519, 531]); // Trusted solc guard blocks [514,520),[520,532).
        for owner in owners {
            let fetched = run("get-function", &path, encoding, &["--at", owner, "--raw"]);
            success(&fetched);
            let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
            for hit in hits.iter().filter(|r| r["context_at"] == owner) {
                let pc = hit["detail"]["instruction_offset"].as_u64().unwrap();
                assert_eq!(hit["detail"]["resolution"], "resolved");
                assert_eq!(hit["detail"]["reference_kind"], "branch_true");
                assert!(
                    fetched["detail"]["instructions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|i| i["decoded_offset"] == pc && i["opcode"] == 0x57)
                );
            }
            let revert = fetched["detail"]["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["detail"]["decoded_start"] == 152)
                .unwrap();
            let raw = run(
                "get-block",
                &path,
                encoding,
                &["--at", revert["at"].as_str().unwrap()],
            );
            success(&raw);
            let raw: Value = serde_json::from_slice(&raw.stdout).unwrap();
            assert_eq!(raw["detail"]["raw_hex"], "5b5f80fd");
        }
    }
}

#[test]
fn evm_code_refs_keep_raw_block_stack_preconditions_and_actual_pc_zero_underflow_explicit() {
    for (carrier, target, records, words) in [
        ("506004565b00", 4, 0, None),
        ("005b506006565b00", 6, 1, Some(1)),
    ] {
        let (_directory, path) = fixture(carrier, "precondition-refs.hex");
        let outline = jsonl(&run("blocks", &path, "hex", &[]));
        let target = outline
            .iter()
            .find(|r| r["record"] == "basic_block" && r["detail"]["decoded_start"] == target)
            .unwrap();
        let rows = jsonl(&run(
            "xref",
            &path,
            "hex",
            &["--at", target["at"].as_str().unwrap()],
        ));
        assert_eq!(rows.last().unwrap()["records"], records);
        if let Some(words) = words {
            assert_eq!(rows[1]["detail"]["incoming_stack_words"], words);
        } else {
            assert_eq!(rows.last().unwrap()["detail"]["analysis_problems"], 1);
        }
    }
}

#[test]
fn evm_code_refs_keep_unvalidated_producer_candidate_entries_incomplete() {
    // The native producer retains selector 11223344 at POP at 33. It is neither a
    // block start nor JUMPDEST, so this valid candidate ID has no resolved refs.
    let prefix = "5f3560e01c8063112233441460215700";
    let carrier = format!("{prefix}{}5b5000", "00".repeat(32 - prefix.len() / 2));
    let (_directory, path) = fixture(&carrier, "invalid-entry-refs.hex");
    let outline = jsonl(&run("interface", &path, "hex", &[]));
    let target = outline.iter().find(|r| r["record"] == "function").unwrap();
    assert_eq!(target["detail"]["decoded_entry"], 33);
    let rows = jsonl(&run(
        "xref",
        &path,
        "hex",
        &["--at", target["at"].as_str().unwrap()],
    ));
    assert_eq!(
        rows[0]["detail"]["target_semantics"],
        "unvalidated_candidate_entry"
    );
    assert_eq!(rows.last().unwrap()["records"], 0);
    assert_eq!(rows.last().unwrap()["complete"], true);
    assert_eq!(rows.last().unwrap()["detail"]["resolution_complete"], false);
    assert_eq!(rows.last().unwrap()["detail"]["invalid_jumps"], 1);
    assert!(
        rows.last().unwrap()["detail"]["analysis_problems"]
            .as_u64()
            .unwrap()
            > 0
    );
}

#[test]
fn evm_immediate_text_search_refs_fetch_compiler_owned_contexts() {
    // Independent solc ASM: PUSH32 at 213 has operand [214,246), within message().
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/evm");
    for encoding in ["raw", "hex"] {
        let extension = if encoding == "raw" { "bin" } else { "hex" };
        let path = root.join(format!("authored-navigation.runtime.{extension}"));
        let rows = jsonl(&run(
            "strings",
            &path,
            encoding,
            &["--kind", "text", "--query", "RXD_DATA_NAVIGATION"],
        ));
        let literals: Vec<_> = rows.iter().filter(|r| r["record"] == "literal").collect();
        assert_eq!(literals.len(), 1);
        assert_eq!(rows[0]["detail"]["excluded_unindexed_pushes"], 3);
        let metadata = jsonl(&run(
            "strings",
            &path,
            encoding,
            &["--kind", "bytes", "--query", "6970667358"],
        ));
        assert_eq!(metadata.last().unwrap()["records"], 0);
        let literal = literals[0];
        assert_eq!(literal["detail"]["instruction_offset"], 213);
        assert_eq!(literal["detail"]["decoded_start"], 214);
        assert_eq!(literal["detail"]["decoded_end"], 246);
        assert_eq!(literal["detail"]["width_bytes"], 32);
        assert_eq!(literal["detail"]["runtime_string_proven"], false);
        assert_eq!(
            literal["detail"]["evidence_kind"],
            "push_immediate_text_candidate"
        );
        let id = literal["at"].as_str().unwrap();
        let references = jsonl(&run("xref", &path, encoding, &["--at", id]));
        let references: Vec<_> = references
            .iter()
            .filter(|r| r["record"] == "literal_reference")
            .collect();
        assert_eq!(references.len(), 1);
        let reference = references[0];
        assert_eq!(reference["detail"]["instruction_offset"], 213);
        assert_eq!(reference["detail"]["target"], id);
        assert_eq!(reference["context_at"], literal["context_at"]);
        let fetched = run(
            "get-function",
            &path,
            encoding,
            &["--at", reference["context_at"].as_str().unwrap(), "--raw"],
        );
        success(&fetched);
        let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
        assert_eq!(fetched["detail"]["selector"], "e21f37ce");
        let instruction = fetched["detail"]["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["decoded_offset"] == 213)
            .unwrap();
        assert_eq!(instruction["opcode"], 0x7f);
        assert_eq!(
            instruction["operand_hex"],
            "5258445f444154415f4e415649474154494f4e3a207472757374656420617574"
        );
        // The plaintext marker was transformed by solc; parsing must not invent it.
        let absent = jsonl(&run(
            "strings",
            &path,
            encoding,
            &["--kind", "text", "--query", "RXD_LITERAL_NAVIGATION"],
        ));
        assert_eq!(absent.last().unwrap()["records"], 0);
    }
}

#[test]
fn evm_immediate_search_preserves_utf8_boundaries_and_overlapping_matches() {
    // PUSH4 "aaaa"; PUSH4 UTF-8 "éé"; PUSH1 incomplete UTF-8 c3;
    // PUSH3 aa 5f bb (5f is an operand, never a PUSH0 use); PUSH2 truncated.
    let (_directory, path) = fixture("636161616163c3a9c3a960c362aa5fbb61ff", "utf8.hex");
    let rows = jsonl(&run(
        "strings",
        &path,
        "hex",
        &["--kind", "text", "--query", "aa"],
    ));
    assert_eq!(rows.last().unwrap()["records"], 1);
    assert_eq!(
        rows[1]["detail"]["matches"],
        serde_json::json!([
            {"decoded_start":1,"decoded_end":3,"carrier_start":2,"carrier_end":6},
            {"decoded_start":2,"decoded_end":4,"carrier_start":4,"carrier_end":8},
            {"decoded_start":3,"decoded_end":5,"carrier_start":6,"carrier_end":10}
        ])
    );
    let unicode = jsonl(&run(
        "strings",
        &path,
        "hex",
        &["--kind", "text", "--query", "é"],
    ));
    assert_eq!(unicode.last().unwrap()["records"], 1);
    assert_eq!(unicode[1]["detail"]["bytes_hex"], "c3a9c3a9");
    assert_eq!(
        unicode[1]["detail"]["matches"],
        serde_json::json!([
            {"decoded_start":6,"decoded_end":8,"carrier_start":12,"carrier_end":16},
            {"decoded_start":8,"decoded_end":10,"carrier_start":16,"carrier_end":20}
        ])
    );
    assert_eq!(unicode[0]["detail"]["disassembly_complete"], false);
    assert_eq!(unicode[0]["detail"]["undecoded_tail"]["hex"], "61ff");
    let zeros = jsonl(&run(
        "strings",
        &path,
        "hex",
        &["--kind", "number", "--query", "0"],
    ));
    assert_eq!(zeros.last().unwrap()["records"], 0);
    for row in [&rows[1], &unicode[1]] {
        let refs = jsonl(&run(
            "xref",
            &path,
            "hex",
            &["--at", row["at"].as_str().unwrap()],
        ));
        assert_eq!(refs[1]["detail"]["context_kind"], "block");
        let fetched = run(
            "get-block",
            &path,
            "hex",
            &["--at", refs[1]["context_at"].as_str().unwrap()],
        );
        success(&fetched);
        let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
        assert_eq!(
            fetched["detail"]["raw_hex"],
            "636161616163c3a9c3a960c362aa5fbb"
        );
        assert_eq!(
            fetched["detail"]["instructions"].as_array().unwrap().len(),
            4
        );
    }
}

#[test]
fn evm_immediate_numbers_keep_push_widths_and_route_each_real_shared_owner() {
    let (directory, path) = fixture(" \n0X5F6000610000\r\n", "zero.hex");
    let raw = directory.path().join("zero.bin");
    fs::write(&raw, [0x5f, 0x60, 0x00, 0x61, 0x00, 0x00]).unwrap();
    for (path, encoding, origin, stride) in [(&path, "hex", 4, 2), (&raw, "raw", 0, 1)] {
        let zero = jsonl(&run(
            "strings",
            path,
            encoding,
            &["--kind", "number", "--query", "0X0000"],
        ));
        assert_eq!(zero[0]["query"]["query"], "0".repeat(64));
        assert_eq!(zero[0]["query"]["match_mode"], "equal_u256");
        assert_eq!(zero.last().unwrap()["records"], 3);
        let literals: Vec<_> = zero.iter().filter(|r| r["record"] == "literal").collect();
        for (row, pc, start, end, width, bytes) in [
            (literals[0], 0, 1, 1, 0, ""),
            (literals[1], 1, 2, 3, 1, "00"),
            (literals[2], 3, 4, 6, 2, "0000"),
        ] {
            assert_eq!(row["detail"]["instruction_offset"], pc);
            assert_eq!(row["detail"]["width_bytes"], width);
            assert_eq!(row["detail"]["bytes_hex"], bytes);
            assert_eq!(row["start"], origin + start * stride);
            assert_eq!(row["end"], origin + end * stride);
            let refs = jsonl(&run(
                "xref",
                path,
                encoding,
                &["--at", row["at"].as_str().unwrap()],
            ));
            assert_eq!(refs.last().unwrap()["records"], 1);
            assert_eq!(refs[1]["detail"]["instruction_offset"], pc);
            let fetched = run(
                "get-block",
                path,
                encoding,
                &["--at", refs[1]["context_at"].as_str().unwrap()],
            );
            success(&fetched);
            let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
            assert_eq!(fetched["detail"]["raw_hex"], "5f6000610000");
        }
        let bytes = jsonl(&run(
            "strings",
            path,
            encoding,
            &["--kind", "bytes", "--query", "0X00"],
        ));
        assert_eq!(bytes.last().unwrap()["records"], 1);
        assert_eq!(bytes[1]["detail"]["width_bytes"], 1);
        let empty_bytes = jsonl(&run(
            "strings",
            path,
            encoding,
            &["--kind", "bytes", "--query", ""],
        ));
        assert_eq!(empty_bytes.last().unwrap()["records"], 1);
        assert_eq!(empty_bytes[1]["detail"]["width_bytes"], 0);
    }
    let (_directory, path) = fixture(SHARED_HELPER, "shared.hex");
    let rows = jsonl(&run(
        "strings",
        &path,
        "hex",
        &["--kind", "number", "--query", "1"],
    ));
    assert_eq!(rows.last().unwrap()["detail"]["literals"], 2);
    assert_eq!(rows.last().unwrap()["records"], 3);
    let shared: Vec<_> = rows
        .iter()
        .filter(|r| r["record"] == "literal" && r["detail"]["instruction_offset"] == 61)
        .collect();
    assert_eq!(shared.len(), 2);
    assert_eq!(shared[0]["at"], shared[1]["at"]);
    let refs = jsonl(&run(
        "xref",
        &path,
        "hex",
        &["--at", shared[0]["at"].as_str().unwrap()],
    ));
    assert_eq!(refs.last().unwrap()["records"], 2);
    for row in refs.iter().filter(|r| r["record"] == "literal_reference") {
        assert_eq!(row["detail"]["owner_count"], 2);
        let fetched = run(
            "get-function",
            &path,
            "hex",
            &["--at", row["context_at"].as_str().unwrap(), "--raw"],
        );
        success(&fetched);
        let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
        let starts: Vec<_> = fetched["detail"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["detail"]["decoded_start"].as_u64().unwrap())
            .collect();
        match fetched["detail"]["selector"].as_str().unwrap() {
            "11223344" => assert_eq!(starts, [32, 41, 60]),
            "55667788" => assert_eq!(starts, [46, 55, 60]),
            other => panic!("invented owner {other}"),
        }
    }
    let selector = jsonl(&run(
        "strings",
        &path,
        "hex",
        &["--kind", "bytes", "--query", "11223344"],
    ));
    assert_eq!(selector.last().unwrap()["records"], 1);
    assert_eq!(selector[1]["detail"]["instruction_offset"], 6);
    assert_eq!(selector[1]["detail"]["context_kind"], "block");
}

#[test]
fn evm_immediate_ids_queries_budgets_and_fixed_envelopes_remain_atomic() {
    let (directory, path) = fixture(SHARED_HELPER, "original.hex");
    let plain = run(
        "strings",
        &path,
        "hex",
        &["--kind", "number", "--query", "1"],
    );
    let rows = jsonl(&plain);
    let literal = rows
        .iter()
        .find(|r| r["record"] == "literal" && r["detail"]["instruction_offset"] == 61)
        .unwrap();
    let id = literal["at"].as_str().unwrap();
    let references = run("xref", &path, "hex", &["--at", id]);
    let reference_rows = jsonl(&references);
    assert_js_keys(
        &rows[0]["query"],
        &["encoding", "kind", "query", "match_mode", "filter"],
    );
    assert_js_keys(
        &reference_rows[0]["query"],
        &["encoding", "at", "reference_scope"],
    );
    assert_js_keys(&rows[0]["detail"], acceptance::HEADER_DETAIL_KEYS);
    assert_js_keys(
        literal,
        &[
            "record",
            "at",
            "source_id",
            "parent_at",
            "context_at",
            "start",
            "end",
            "detail",
        ],
    );
    assert_js_keys(
        &literal["detail"],
        &[
            "value",
            "index",
            "literal_kind",
            "instruction_offset",
            "instruction_end",
            "opcode",
            "width_bytes",
            "decoded_start",
            "decoded_end",
            "bytes_hex",
            "number_hex",
            "block_id",
            "owner_count",
            "context_kind",
            "context_boundary_status",
            "context_boundary_complete",
            "runtime_string_proven",
            "query_kind",
            "evidence_kind",
            "match_status",
            "text_candidates",
            "matches",
        ],
    );
    assert_js_keys(
        &reference_rows[1]["detail"],
        &[
            "target",
            "literal_kind",
            "instruction_offset",
            "instruction_end",
            "opcode",
            "width_bytes",
            "decoded_start",
            "decoded_end",
            "bytes_hex",
            "number_hex",
            "block_id",
            "owner_count",
            "context_kind",
            "context_boundary_status",
            "context_boundary_complete",
            "runtime_string_proven",
            "evidence_kind",
            "reference_kind",
            "instruction_carrier_start",
            "instruction_carrier_end",
        ],
    );
    assert_js_keys(
        &rows.last().unwrap()["detail"],
        acceptance::SUMMARY_DETAIL_KEYS,
    );
    let copy = directory.path().join("copy.hex");
    fs::copy(&path, &copy).unwrap();
    let copy_rows = jsonl(&run(
        "strings",
        &copy,
        "hex",
        &["--kind", "number", "--query", "1"],
    ));
    assert_eq!(copy_rows[1]["at"], rows[1]["at"]);
    success(&run("xref", &copy, "hex", &["--at", id]));
    let full = id.to_owned();
    for invalid in [
        format!("{full}:0"),
        full.replace(":literal:", ":block:"),
        full.replace(":push:", ":data:"),
        full.replace(":61:1", ":62:1"),
        full.replace(":61:1", ":61:2"),
        full.replace(":61:1", ":061:1"),
        full.replace(":61:1", ":61:01"),
        full.replace(":61:1", ":61:33"),
        full.replace(":61:1", ":184467440737095516160:1"),
    ] {
        failure(
            run("xref", &path, "hex", &["--at", &invalid]),
            1,
            "INVALID_QUERY",
        );
    }
    failure(
        run("xref", &path, "raw", &["--at", id]),
        1,
        "SOURCE_MISMATCH",
    );
    fs::write(&copy, format!("{SHARED_HELPER}00")).unwrap();
    success(&run("xref", &copy, "hex", &["--at", &full]));
    for (kind, query) in [
        ("text", ""),
        ("bytes", "0"),
        ("bytes", "zz"),
        ("number", "-1"),
        ("number", "0x"),
        (
            "number",
            "0x10000000000000000000000000000000000000000000000000000000000000000",
        ),
    ] {
        failure(
            run("strings", &path, "hex", &["--kind", kind, "--query", query]),
            2,
            "INVALID_QUERY",
        );
    }
    let destination = directory.path().join("previous.jsonl");
    for (command, args, expected) in [
        (
            "strings",
            vec!["--kind", "number", "--query", "1"],
            plain.stdout.as_slice(),
        ),
        ("xref", vec!["--at", id], references.stdout.as_slice()),
    ] {
        let mut profiled_args = args.clone();
        profiled_args.push("--perf");
        let profiled = run(command, &path, "hex", &profiled_args);
        assert_eq!(profiled.stdout, expected);
        let reports = perf_reports(&profiled);
        assert_eq!(reports[0]["resource_limits"]["states"], 100_000);
        assert_eq!(reports[0]["resource_limits"]["stack_cells"], 1_048_576);
        assert!(reports[0]["stages_ns"]["search"].as_u64().unwrap() > 0);
        let mut saved_args = args.clone();
        saved_args.extend(["-o", destination.to_str().unwrap()]);
        success(&run(command, &path, "hex", &saved_args));
        assert_eq!(fs::read(&destination).unwrap(), expected);
        // Every ceiling lives in rxd_core::Budget, so no `--max-*` flag exists.
        for budget in [
            ["--max-input-bytes", "1"],
            ["--max-records", "1"],
            ["--max-output-bytes", "1"],
            ["--max-work", "1"],
            ["--max-states", "1"],
            ["--max-stack-cells", "1"],
        ] {
            fs::write(&destination, "preserved").unwrap();
            let mut failed_args = saved_args.clone();
            failed_args.extend(budget);
            failure(run(command, &path, "hex", &failed_args), 2, budget[0]);
            assert_eq!(fs::read(&destination).unwrap(), b"preserved");
        }
        let mut overwrite = args.clone();
        overwrite.extend(["-o", path.to_str().unwrap()]);
        failure(run(command, &path, "hex", &overwrite), 2, "INVALID_QUERY");
    }
    fs::write(&copy, " \n0x\t").unwrap();
    for kind in ["text", "bytes", "number"] {
        let empty = jsonl(&run("strings", &copy, "hex", &["--kind", kind]));
        assert_eq!(empty.len(), 2);
        assert_eq!(empty.last().unwrap()["records"], 0);
    }
}

#[test]
fn evm_immediate_clues_do_not_upgrade_unknown_function_boundaries_or_address_semantics() {
    // Dispatcher points at PC32; PUSH4 ABCD at34; CALLDATALOAD/JUMP is unresolved.
    let prefix = "5f3560e01c8063112233441460205700";
    let carrier = format!(
        "{prefix}{}5b5063414243445f3556",
        "00".repeat(32 - prefix.len() / 2)
    );
    let (_directory, path) = fixture(carrier, "unknown.hex");
    let rows = jsonl(&run(
        "strings",
        &path,
        "hex",
        &["--kind", "text", "--query", "ABCD"],
    ));
    assert_eq!(rows.last().unwrap()["records"], 1);
    assert_eq!(rows[1]["detail"]["context_kind"], "function");
    assert_eq!(rows[1]["detail"]["context_boundary_complete"], false);
    let refs = jsonl(&run(
        "xref",
        &path,
        "hex",
        &["--at", rows[1]["at"].as_str().unwrap()],
    ));
    assert_eq!(refs[1]["detail"]["context_boundary_status"], "incomplete");
    let fetched = run(
        "get-function",
        &path,
        "hex",
        &["--at", refs[1]["context_at"].as_str().unwrap(), "--raw"],
    );
    success(&fetched);
    let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
    assert_eq!(fetched["detail"]["boundary_complete"], false);
    assert_eq!(
        fetched["detail"]["segments"][0]["raw_hex"],
        "5b5063414243445f3556"
    );
    assert!(
        fetched["detail"]["unknown_successors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reason"] == "dynamic_jump_destination")
    );
    // Authored PUSH20 clue; an address-sized value never establishes a call target.
    let (_directory, path) = fixture(
        "73112233445566778899aabbccddeeff001122334400",
        "address.hex",
    );
    let address = jsonl(&run(
        "strings",
        &path,
        "hex",
        &[
            "--kind",
            "bytes",
            "--query",
            "0X112233445566778899AABBCCDDEEFF0011223344",
        ],
    ));
    assert_eq!(address.last().unwrap()["records"], 1);
    assert_eq!(address[1]["detail"]["width_bytes"], 20);
    assert_eq!(address[1]["detail"]["context_kind"], "block");
    let refs = jsonl(&run(
        "xref",
        &path,
        "hex",
        &["--at", address[1]["at"].as_str().unwrap()],
    ));
    assert_eq!(refs[1]["detail"]["reference_kind"], "push_immediate");
    let fetched = run(
        "get-block",
        &path,
        "hex",
        &["--at", refs[1]["context_at"].as_str().unwrap()],
    );
    success(&fetched);
    let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
    assert_eq!(
        fetched["detail"]["instructions"].as_array().unwrap().len(),
        2
    );
    assert_eq!(fetched["detail"]["instructions"][1]["text"], "STOP");
}

#[test]
fn evm_real_compiler_data_id_refs_fetch_prove_codecopy_source_length_and_owner() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/evm");
    for (name, encoding, stride) in [
        ("authored-navigation-legacy.runtime.bin", "raw", 1),
        ("authored-navigation-legacy.runtime.hex", "hex", 2),
    ] {
        let path = root.join(name);
        let rows = jsonl(&run(
            "strings",
            &path,
            encoding,
            &["--kind", "text", "--query", "RXD_DATA_NAVIGATION"],
        ));
        let data = rows
            .iter()
            .find(|r| r["record"] == "data_literal")
            .expect("compiler data candidate");
        assert_eq!(data["detail"]["decoded_start"], 492);
        assert_eq!(data["detail"]["decoded_end"], 592);
        assert_eq!(data["start"], 492 * stride);
        assert_eq!(data["end"], 592 * stride);
        assert_eq!(data["detail"]["runtime_string_proven"], false);
        assert_eq!(
            data["detail"]["evidence_kind"],
            "unindexed_data_text_candidate"
        );
        let refs = jsonl(&run(
            "xref",
            &path,
            encoding,
            &["--at", data["at"].as_str().unwrap()],
        ));
        let evidence = refs
            .iter()
            .find(|r| r["record"] == "data_reference")
            .expect("constant CODECOPY witness");
        assert_eq!(evidence["detail"]["instruction_offset"], 397);
        assert_eq!(
            evidence["detail"]["source_offset_hex"],
            format!("{:064x}", 492)
        );
        assert_eq!(evidence["detail"]["length_hex"], format!("{:064x}", 100));
        assert_eq!(evidence["detail"]["copied_decoded_start"], 492);
        assert_eq!(evidence["detail"]["copied_decoded_end"], 592);
        assert_eq!(
            evidence["detail"]["zero_padding_hex"],
            format!("{:064x}", 0)
        );
        let fetched = run(
            "get-function",
            &path,
            encoding,
            &["--at", evidence["context_at"].as_str().unwrap(), "--raw"],
        );
        success(&fetched);
        let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
        assert_eq!(fetched["detail"]["selector"], "e21f37ce");
        assert_eq!(fetched["detail"]["decoded_entry"], 276);
        let instructions = fetched["detail"]["instructions"].as_array().unwrap();
        for (pc, text) in [(391, "PUSH2 01ec"), (394, "PUSH1 64"), (397, "CODECOPY")] {
            assert!(
                instructions
                    .iter()
                    .any(|i| i["decoded_offset"] == pc && i["text"] == text)
            );
        }
    }
}

#[test]
fn evm_data_unknown_copy_operands_remain_coverage_evidence_without_fabricated_routes() {
    // Entry stack is empty. CODECOPY PC5 consumes destination=0, source=calldata[0], length=4.
    let (_directory, path) = fixture(b"\x60\x04\x5f\x35\x5f\x39\x00CLUE", "unknown-source.bin");
    let rows = jsonl(&run(
        "strings",
        &path,
        "raw",
        &["--kind", "text", "--query", "CLUE"],
    ));
    let data = rows.iter().find(|r| r["record"] == "data_literal").unwrap();
    assert_eq!(data["context_at"], Value::Null);
    assert_eq!(
        data["detail"]["reference_status"],
        "unreferenced_or_unknown"
    );
    assert_eq!(rows[0]["detail"]["unknown_copy_witnesses"], 1);
    let unknown = &rows[0]["detail"]["unknown_copy_evidence"][0];
    assert_eq!(unknown["instruction_offset"], 5);
    assert_eq!(unknown["source_offset_hex"], Value::Null);
    assert_eq!(unknown["length_hex"], format!("{:064x}", 4));
    assert_eq!(unknown["reason"], "source_offset_unknown");
    let refs = jsonl(&run(
        "xref",
        &path,
        "raw",
        &["--at", data["at"].as_str().unwrap()],
    ));
    assert_eq!(refs.last().unwrap()["records"], 0);
    assert_eq!(
        refs.last().unwrap()["detail"]["reference_status"],
        "unreferenced_or_unknown"
    );
    assert_eq!(refs.len(), 2);
}

#[test]
fn evm_data_utf8_overlaps_charge_matches_without_consuming_record_budget() {
    let (_directory, path) = fixture("00c3a9c3a9c3a9004141414141ff434c5545", "utf8-data.hex");
    // Every ceiling lives in rxd_core::Budget, so no `--max-*` flag exists.
    failure(
        run(
            "strings",
            &path,
            "hex",
            &["--kind", "text", "--query", "éé", "--max-records", "1"],
        ),
        2,
        "--max-records",
    );
    let rows = jsonl(&run(
        "strings",
        &path,
        "hex",
        &["--kind", "text", "--query", "éé"],
    ));
    let data = rows.iter().find(|r| r["record"] == "data_literal").unwrap();
    assert_eq!(data["detail"]["decoded_start"], 1);
    assert_eq!(data["detail"]["decoded_end"], 7);
    assert_eq!(
        data["detail"]["matches"],
        serde_json::json!([
            {"decoded_start":1,"decoded_end":5,"carrier_start":2,"carrier_end":10},
            {"decoded_start":3,"decoded_end":7,"carrier_start":6,"carrier_end":14}
        ])
    );
    assert_eq!(data["context_at"], Value::Null);
    assert_eq!(data["detail"]["reference_status"], "unreferenced");
    assert_eq!(rows.last().unwrap()["records"], 1);
    let absent = jsonl(&run(
        "strings",
        &path,
        "hex",
        &["--kind", "text", "--query", "AAAAAC"],
    ));
    assert_eq!(absent.last().unwrap()["records"], 0);
}

#[test]
fn evm_data_shared_helper_copy_witnesses_keep_each_callers_actual_source() {
    // Both selectors call helper PC60. Its stack is [return_pc, source]; it copies
    // four source bytes to memory zero and returns using the caller's own address.
    // Source80 is AAAA, source85 is BBBB: block ownership alone cannot associate them.
    for second_source in [80, 85] {
        let mut bytes: Vec<u8> = SHARED_HELPER
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        bytes[37] = 80;
        bytes[51] = second_source;
        bytes[42..46].fill(0);
        bytes[56..60].fill(0);
        bytes.truncate(60);
        bytes.extend([0x5b, 0x60, 4, 0x90, 0x5f, 0x39, 0x56]);
        bytes.resize(80, 0);
        bytes.extend(b"AAAA\0BBBB");
        let (_directory, path) = fixture(bytes, "shared-copy.bin");
        let rows = jsonl(&run(
            "strings",
            &path,
            "raw",
            &["--kind", "text", "--query", "AAAA"],
        ));
        let literal = rows.iter().find(|r| r["record"] == "data_literal").unwrap();
        let refs = jsonl(&run(
            "xref",
            &path,
            "raw",
            &["--at", literal["at"].as_str().unwrap()],
        ));
        let references: Vec<_> = refs
            .iter()
            .filter(|r| r["record"] == "data_reference")
            .collect();
        assert_eq!(references.len(), if second_source == 80 { 2 } else { 1 });
        // Every ceiling lives in rxd_core::Budget, so no `--max-*` flag exists.
        failure(
            run(
                "strings",
                &path,
                "raw",
                &["--kind", "text", "--query", "AAAA", "--max-records", "1"],
            ),
            2,
            "--max-records",
        );
        failure(
            run(
                "xref",
                &path,
                "raw",
                &[
                    "--at",
                    literal["at"].as_str().unwrap(),
                    "--max-records",
                    "1",
                ],
            ),
            2,
            "--max-records",
        );
        let mut selectors = Vec::new();
        for reference in references {
            assert_eq!(reference["detail"]["instruction_offset"], 65);
            assert_eq!(
                reference["detail"]["source_offset_hex"],
                format!("{:064x}", 80)
            );
            assert_eq!(reference["detail"]["copied_decoded_start"], 80);
            assert_eq!(reference["detail"]["copied_decoded_end"], 84);
            let fetched = run(
                "get-function",
                &path,
                "raw",
                &["--at", reference["context_at"].as_str().unwrap(), "--raw"],
            );
            success(&fetched);
            let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
            selectors.push(fetched["detail"]["selector"].as_str().unwrap().to_owned());
        }
        assert_eq!(
            selectors,
            if second_source == 80 {
                vec!["11223344", "55667788"]
            } else {
                vec!["11223344"]
            }
        );
    }
}

#[test]
fn evm_data_copy_intersections_preserve_zero_padding_and_huge_length_without_allocation() {
    // CODECOPY PC5 source10,length8,destination0; source length is14, so copies
    // [10,14) and pads four zeros. The queried candidate begins one byte earlier.
    let (_directory, path) = fixture(b"\x60\x08\x60\x0a\x5f\x39\x00xxABCDE", "partial-copy.bin");
    let rows = jsonl(&run(
        "strings",
        &path,
        "raw",
        &["--kind", "text", "--query", "ABCDE"],
    ));
    let data = rows.iter().find(|r| r["record"] == "data_literal").unwrap();
    let refs = jsonl(&run(
        "xref",
        &path,
        "raw",
        &["--at", data["at"].as_str().unwrap()],
    ));
    let reference = refs
        .iter()
        .find(|r| r["record"] == "data_reference")
        .unwrap();
    assert_eq!(reference["detail"]["instruction_offset"], 5);
    assert_eq!(reference["detail"]["available_source_start"], 10);
    assert_eq!(reference["detail"]["available_source_end"], 14);
    assert_eq!(reference["detail"]["copied_decoded_start"], 10);
    assert_eq!(reference["detail"]["copied_decoded_end"], 14);
    assert_eq!(reference["detail"]["copied_bytes_hex"], "42434445");
    assert_eq!(
        reference["detail"]["zero_padding_hex"],
        format!("{:064x}", 4)
    );
    let fetched = run(
        "get-block",
        &path,
        "raw",
        &["--at", reference["context_at"].as_str().unwrap()],
    );
    success(&fetched);
    let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
    assert_eq!(fetched["detail"]["raw_hex"], "6008600a5f3900");
    // Length is MAX_U256; the original source intersection is only four bytes.
    let mut giant = vec![0x7f];
    giant.extend([0xff; 32]);
    giant.extend([0x60, 38, 0x5f, 0x39, 0x00]);
    giant.extend(b"CLUE");
    let (_directory, path) = fixture(giant, "giant-copy.bin");
    let rows = jsonl(&run(
        "strings",
        &path,
        "raw",
        &["--kind", "text", "--query", "CLUE"],
    ));
    let data = rows.iter().find(|r| r["record"] == "data_literal").unwrap();
    let refs = jsonl(&run(
        "xref",
        &path,
        "raw",
        &["--at", data["at"].as_str().unwrap()],
    ));
    let reference = refs
        .iter()
        .find(|r| r["record"] == "data_reference")
        .unwrap();
    assert_eq!(reference["detail"]["length_hex"], "ff".repeat(32));
    assert_eq!(reference["detail"]["copied_bytes_hex"], "434c5545");
    assert_eq!(
        reference["detail"]["zero_padding_hex"],
        format!("{}fb", "ff".repeat(31))
    );
}

#[test]
fn evm_data_metadata_and_copied_push_bytes_keep_distinct_candidate_evidence() {
    // STOP, orphan CLUE, CBOR {CLUE:CLUE}, 11-byte suffix. Compiler metadata is
    // designated by validated terminal CBOR syntax, with input role still unknown.
    let (_directory, path) = fixture(b"\x00CLUE\xa1\x64CLUE\x64CLUE\x00\x0b", "metadata-data.bin");
    let rows = jsonl(&run(
        "strings",
        &path,
        "raw",
        &["--kind", "text", "--query", "CLUE"],
    ));
    let data: Vec<_> = rows
        .iter()
        .filter(|r| r["record"] == "data_literal")
        .collect();
    assert_eq!(data.len(), 2);
    let kinds: Vec<_> = data
        .iter()
        .map(|r| r["detail"]["evidence_kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "compiler_metadata_text_candidate",
            "unindexed_data_text_candidate"
        ]
    );
    assert_eq!(
        rows[0]["detail"]["metadata_range"],
        serde_json::json!({"decoded_start":5,"decoded_end":18})
    );
    for literal in data {
        assert_eq!(literal["context_at"], Value::Null);
        let refs = jsonl(&run(
            "xref",
            &path,
            "raw",
            &["--at", literal["at"].as_str().unwrap()],
        ));
        assert_eq!(refs.last().unwrap()["records"], 0);
        assert_eq!(
            refs.last().unwrap()["detail"]["reference_status"],
            "unreferenced"
        );
    }
    // Exact source range [1,5) lies inside a PUSH5 operand; copies and decoded
    // immediate use remain two separate kinds of evidence for the same bytes.
    let (_directory, path) = fixture(
        b"\x64CLUE!\x50\x60\x04\x60\x01\x5f\x39\x00",
        "copied-operand.bin",
    );
    let rows = jsonl(&run(
        "strings",
        &path,
        "raw",
        &["--kind", "text", "--query", "CLUE"],
    ));
    let immediate = rows.iter().find(|r| r["record"] == "literal").unwrap();
    let data = rows.iter().find(|r| r["record"] == "data_literal").unwrap();
    assert_eq!(
        immediate["detail"]["evidence_kind"],
        "push_immediate_text_candidate"
    );
    assert_eq!(
        data["detail"]["evidence_kind"],
        "copied_bytecode_text_candidate"
    );
    assert_eq!(data["detail"]["decoded_start"], 1);
    assert_eq!(data["detail"]["decoded_end"], 5);
    let refs = jsonl(&run(
        "xref",
        &path,
        "raw",
        &["--at", data["at"].as_str().unwrap()],
    ));
    let reference = refs
        .iter()
        .find(|r| r["record"] == "data_reference")
        .unwrap();
    assert_eq!(reference["detail"]["instruction_offset"], 12);
    assert_eq!(reference["detail"]["copied_bytes_hex"], "434c5545");
}

#[test]
fn evm_data_ids_fixed_schemas_and_budgets_remain_atomic_with_perf_sidecars() {
    let (directory, path) = fixture(b"\x60\x04\x60\x07\x5f\x39\x00CLUE", "copy.bin");
    let plain = run(
        "strings",
        &path,
        "raw",
        &["--kind", "text", "--query", "CLUE"],
    );
    let rows = jsonl(&plain);
    let data = rows.iter().find(|r| r["record"] == "data_literal").unwrap();
    let id = data["at"].as_str().unwrap();
    assert_js_keys(
        &data["detail"],
        &[
            "value",
            "index",
            "literal_kind",
            "region_kind",
            "decoded_start",
            "decoded_end",
            "bytes_hex",
            "number_hex",
            "query_kind",
            "evidence_kind",
            "match_status",
            "text_candidates",
            "matches",
            "reference_status",
            "reference_count",
            "context_kind",
            "context_index",
            "runtime_string_proven",
        ],
    );
    let references = run("xref", &path, "raw", &["--at", id]);
    let refs = jsonl(&references);
    let reference = refs
        .iter()
        .find(|r| r["record"] == "data_reference")
        .unwrap();
    assert_js_keys(
        &reference["detail"],
        &[
            "target",
            "reference_kind",
            "evidence_kind",
            "instruction_offset",
            "instruction_end",
            "instruction_carrier_start",
            "instruction_carrier_end",
            "source_offset_hex",
            "length_hex",
            "destination_offset_hex",
            "available_source_start",
            "available_source_end",
            "zero_padding_hex",
            "copied_decoded_start",
            "copied_decoded_end",
            "copied_bytes_hex",
            "block_id",
            "context_kind",
            "context_index",
            "context_entry_state",
            "entry_evidence_block_ids",
            "resolution",
            "runtime_string_proven",
        ],
    );
    for (kind, query) in [("bytes", "434c5545"), ("number", "0x434c5545")] {
        let typed = jsonl(&run(
            "strings",
            &path,
            "raw",
            &["--kind", kind, "--query", query],
        ));
        let candidate = typed
            .iter()
            .find(|r| r["record"] == "data_literal")
            .unwrap();
        assert_eq!(candidate["at"], id);
        assert_js_keys(
            &candidate["detail"],
            &data["detail"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
    }
    let copy = directory.path().join("renamed.bin");
    fs::copy(&path, &copy).unwrap();
    success(&run("xref", &copy, "raw", &["--at", id]));
    let full = id.to_owned();
    for forged in [
        format!("{full}:0"),
        full.replace(":unindexed:", ":tail:"),
        full.replace(":7:11", ":07:11"),
        full.replace(":7:11", ":8:11"),
        full.replace(":7:11", ":7:100"),
        full.replace(":data:", ":push:"),
    ] {
        failure(
            run("xref", &path, "raw", &["--at", &forged]),
            1,
            "INVALID_QUERY",
        );
    }
    fs::write(&copy, b"\x60\x04\x60\x07\x5f\x39\x00CHANGED").unwrap();
    failure(
        run("xref", &copy, "raw", &["--at", &full]),
        2,
        "INVALID_QUERY",
    );
    let profiled = run(
        "strings",
        &path,
        "raw",
        &["--kind", "text", "--query", "CLUE", "--perf"],
    );
    assert_eq!(profiled.stdout, plain.stdout);
    let reports = perf_reports(&profiled);
    let work = reports[0]["counters"]["work"].as_u64().unwrap();
    assert!(work > 0);
    for stage in ["search", "analyze", "serialize"] {
        assert!(reports[0]["stages_ns"][stage].as_u64().unwrap() > 0);
    }
    let destination = directory.path().join("saved.jsonl");
    for (command, query, expected) in [
        (
            "strings",
            vec!["--kind", "text", "--query", "CLUE"],
            plain.stdout.as_slice(),
        ),
        ("xref", vec!["--at", id], references.stdout.as_slice()),
    ] {
        let mut save = query.clone();
        save.extend(["-o", destination.to_str().unwrap()]);
        success(&run(command, &path, "raw", &save));
        assert_eq!(fs::read(&destination).unwrap(), expected);
        for budget in [
            ["--max-work", "1"],
            ["--max-input-bytes", "1"],
            ["--max-output-bytes", "1"],
        ] {
            fs::write(&destination, "preserved").unwrap();
            let mut fail = query.clone();
            fail.extend([budget[0], budget[1], "-o", destination.to_str().unwrap()]);
            failure(run(command, &path, "raw", &fail), 2, budget[0]);
            assert_eq!(fs::read(&destination).unwrap(), b"preserved");
        }
    }
}

#[test]
fn evm_data_unknown_length_unvisited_copy_and_fully_padded_copy_preserve_honest_status() {
    for (bytes, unknown, unvisited, reason, expected_status) in [
        (
            b"\x5f\x35\x60\x07\x5f\x39\x00CLUE".as_slice(),
            1,
            0,
            Some("length_unknown"),
            "unreferenced_or_unknown",
        ),
        (b"\x39\x00CLUE", 0, 1, None, "unreferenced_or_unknown"),
        (
            b"\x60\x04\x60\xff\x5f\x39\x00CLUE",
            0,
            0,
            None,
            "unreferenced",
        ),
        (
            b"\x60\x00\x60\x07\x5f\x39\x00CLUE",
            0,
            0,
            None,
            "unreferenced",
        ),
    ] {
        let (_directory, path) = fixture(bytes, "copy-boundary.bin");
        let rows = jsonl(&run(
            "strings",
            &path,
            "raw",
            &["--kind", "text", "--query", "CLUE"],
        ));
        let data = rows.iter().find(|r| r["record"] == "data_literal").unwrap();
        assert_eq!(data["context_at"], Value::Null);
        assert_eq!(rows[0]["detail"]["unknown_copy_witnesses"], unknown);
        assert_eq!(rows[0]["detail"]["unvisited_copy_sites"], unvisited);
        if let Some(reason) = reason {
            assert_eq!(
                rows[0]["detail"]["unknown_copy_evidence"][0]["reason"],
                reason
            );
        }
        let refs = jsonl(&run(
            "xref",
            &path,
            "raw",
            &["--at", data["at"].as_str().unwrap()],
        ));
        assert_eq!(refs.last().unwrap()["records"], 0);
        assert_eq!(
            refs.last().unwrap()["detail"]["reference_status"],
            expected_status
        );
    }
    let (_directory, path) = fixture(b"\x00AAAA\0BBBB\0CCCC", "candidate-limit.bin");
    // Candidate storage ceilings come from the shared budget, not a CLI knob.
    failure(
        run(
            "strings",
            &path,
            "raw",
            &["--kind", "text", "--query", "AAAA", "--max-states", "1"],
        ),
        2,
        "--max-states",
    );
    let inventory = jsonl(&run(
        "strings",
        &path,
        "raw",
        &["--kind", "text", "--query", "AAAA"],
    ));
    assert!(inventory.len() >= 2);
}

#[test]
fn evm_refs_dispatch_code_and_literal_ids_with_the_same_fixed_stream_contract() {
    let (_directory, path) = fixture(SHARED_HELPER, "combined-refs.hex");
    let outline = jsonl(&run("blocks", &path, "hex", &[]));
    let block = outline
        .iter()
        .find(|r| r["record"] == "basic_block" && r["detail"]["decoded_start"] == 60)
        .unwrap();
    let strings = jsonl(&run(
        "strings",
        &path,
        "hex",
        &["--kind", "number", "--query", "1"],
    ));
    let literal = strings
        .iter()
        .find(|r| r["record"] == "literal" && r["detail"]["instruction_offset"] == 61)
        .unwrap();
    let code = jsonl(&run(
        "xref",
        &path,
        "hex",
        &["--at", block["at"].as_str().unwrap(), "--include-bytecode"],
    ));
    let literals = jsonl(&run(
        "xref",
        &path,
        "hex",
        &[
            "--at",
            literal["at"].as_str().unwrap(),
            "--include-bytecode",
        ],
    ));
    assert_eq!(code[1]["record"], "code_reference");
    assert_eq!(literals[1]["record"], "literal_reference");
    assert_eq!(code[0]["detail"]["bytecode"]["hex"], SHARED_HELPER);
    assert_eq!(literals[0]["detail"]["bytecode"]["hex"], SHARED_HELPER);
    let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(&code[0]["query"]), keys(&literals[0]["query"]));
    assert_eq!(keys(&code[0]["detail"]), keys(&literals[0]["detail"]));
    assert_eq!(
        keys(&code.last().unwrap()["detail"]),
        keys(&literals.last().unwrap()["detail"])
    );
    assert_eq!(code[0]["detail"]["target_kind"], "block");
    assert_eq!(literals[0]["detail"]["target_kind"], "literal");
    assert_eq!(code.last().unwrap()["detail"]["literals"], Value::Null);
    assert_eq!(
        literals.last().unwrap()["detail"]["resolved_edges"],
        Value::Null
    );
    assert_eq!(
        literals.last().unwrap()["detail"]["resolution_complete"],
        Value::Null
    );
    assert_eq!(literals.last().unwrap()["detail"]["references"], 2);
    let (_data_directory, data_path) = fixture(b"\x00CLUE", "combined-data.bin");
    let found = jsonl(&run(
        "strings",
        &data_path,
        "raw",
        &["--kind", "text", "--query", "CLUE"],
    ));
    let data_id = found
        .iter()
        .find(|r| r["record"] == "data_literal")
        .unwrap()["at"]
        .as_str()
        .unwrap();
    let data = jsonl(&run(
        "xref",
        &data_path,
        "raw",
        &["--at", data_id, "--include-bytecode"],
    ));
    assert_eq!(keys(&code[0]["query"]), keys(&data[0]["query"]));
    assert_eq!(keys(&code[0]["detail"]), keys(&data[0]["detail"]));
    assert_eq!(
        keys(&code.last().unwrap()["detail"]),
        keys(&data.last().unwrap()["detail"])
    );
    assert_eq!(data[0]["detail"]["bytecode"]["hex"], "00434c5545");
    assert_eq!(
        data[0]["detail"]["target_semantics"],
        "bytecode_data_candidate"
    );
    assert_eq!(data.last().unwrap()["detail"]["data_literals"], 1);
    assert_eq!(literals.last().unwrap()["detail"]["data_literals"], 0);
    assert_eq!(code.last().unwrap()["detail"]["data_literals"], Value::Null);
    assert_eq!(
        data.last().unwrap()["detail"]["reference_status"],
        "unreferenced"
    );
    assert_eq!(code[0]["detail"]["data_scope"], Value::Null);
    assert_eq!(literals[0]["detail"]["data_scope"], Value::Null);
}

#[test]
fn evm_data_ownerless_block_refs_preserve_observed_incoming_copy_preconditions() {
    // PC0 pushes length4/source11 before jumping to PC7. FETCH of [7,11) alone
    // cannot explain these constants: the observed incoming stack must accompany refs.
    let (_directory, path) = fixture(
        b"\x60\x04\x60\x0b\x60\x07\x56\x5b\x5f\x39\x00CLUE",
        "incoming-copy.bin",
    );
    let rows = jsonl(&run(
        "strings",
        &path,
        "raw",
        &["--kind", "text", "--query", "CLUE"],
    ));
    let data = rows.iter().find(|r| r["record"] == "data_literal").unwrap();
    let refs = jsonl(&run(
        "xref",
        &path,
        "raw",
        &["--at", data["at"].as_str().unwrap()],
    ));
    let reference = refs
        .iter()
        .find(|r| r["record"] == "data_reference")
        .unwrap();
    assert_eq!(reference["detail"]["context_kind"], "block");
    assert_eq!(
        reference["detail"]["context_entry_state"],
        serde_json::json!({"stack":[
        {"kind":"constant","hex":format!("{:064x}",4)},
        {"kind":"constant","hex":format!("{:064x}",11)}
    ],"memory_words":[]})
    );
    let fetched = run(
        "get-block",
        &path,
        "raw",
        &["--at", reference["context_at"].as_str().unwrap()],
    );
    success(&fetched);
    let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
    assert_eq!(fetched["detail"]["raw_hex"], "5b5f3900");
    let proof_blocks = reference["detail"]["entry_evidence_block_ids"]
        .as_array()
        .expect("actual predecessor FETCH IDs");
    assert_eq!(proof_blocks.len(), 2);
    let predecessor = run(
        "get-block",
        &path,
        "raw",
        &["--at", proof_blocks[0].as_str().unwrap()],
    );
    success(&predecessor);
    let predecessor: Value = serde_json::from_slice(&predecessor.stdout).unwrap();
    assert_eq!(predecessor["detail"]["raw_hex"], "6004600b600756");
    assert_eq!(proof_blocks[1], reference["context_at"]);

    assert_eq!(
        reference["detail"]["source_offset_hex"],
        format!("{:064x}", 11)
    );
    assert_eq!(reference["detail"]["length_hex"], format!("{:064x}", 4));
}

mod acceptance;

#[test]
fn evm_xref_content_search_resolves_one_target() {
    let (_directory, path) = fixture(SHARED_HELPER, "search.hex");
    let interface = jsonl(&run("interface", &path, "hex", &[]));
    let function = interface
        .iter()
        .find(|row| row["record"] == "function" && row["detail"]["selector"] == "11223344")
        .unwrap();
    let function_at = function["at"].as_str().unwrap();
    let direct = jsonl(&run("xref", &path, "hex", &["--at", function_at]));
    let searched = jsonl(&run(
        "xref",
        &path,
        "hex",
        &["-m", "11223344", "--kind", "function"],
    ));
    assert_eq!(searched[0]["query"]["kind"], "function");
    assert_eq!(searched[0]["query"]["match"], "11223344");
    assert_eq!(searched[0]["query"]["at"], function_at);
    assert_eq!(searched[1..searched.len() - 1], direct[1..direct.len() - 1]);

    let blocks = jsonl(&run("blocks", &path, "hex", &[]));
    let block = blocks
        .iter()
        .find(|row| row["record"] == "basic_block")
        .unwrap();
    let block_at = block["at"].as_str().unwrap();
    let direct = jsonl(&run("xref", &path, "hex", &["--at", block_at]));
    let searched = jsonl(&run(
        "xref",
        &path,
        "hex",
        &["-m", block_at, "--kind", "block"],
    ));
    assert_eq!(searched[1..searched.len() - 1], direct[1..direct.len() - 1]);

    let strings = jsonl(&run("strings", &path, "hex", &["--kind", "text"]));
    let literal = strings
        .iter()
        .find(|row| row["record"] == "literal" && row["detail"]["value"] == "11223344")
        .unwrap();
    let literal_at = literal["at"].as_str().unwrap();
    let direct = jsonl(&run("xref", &path, "hex", &["--at", literal_at]));
    let searched = jsonl(&run(
        "xref",
        &path,
        "hex",
        &["-m", "11223344", "--kind", "literal"],
    ));
    assert_eq!(searched[1..searched.len() - 1], direct[1..direct.len() - 1]);

    // Data candidates are literals too.
    let (_copy_directory, copy) = fixture(b"\x60\x04\x60\x07\x5f\x39\x00CLUE", "copy.bin");
    let strings = jsonl(&run("strings", &copy, "raw", &["--kind", "text"]));
    let data = strings
        .iter()
        .find(|row| row["record"] == "data_literal" && row["detail"]["value"] == "CLUE")
        .unwrap();
    let data_at = data["at"].as_str().unwrap();
    let direct = jsonl(&run("xref", &copy, "raw", &["--at", data_at]));
    let searched = jsonl(&run(
        "xref",
        &copy,
        "raw",
        &["-m", "CLUE", "--kind", "literal"],
    ));
    assert_eq!(searched[0]["query"]["kind"], "literal");
    assert_eq!(searched[1..searched.len() - 1], direct[1..direct.len() - 1]);

    failure(
        run("xref", &path, "hex", &["-m", "<", "--kind", "literal"]),
        2,
        "AMBIGUOUS",
    );
    failure(
        run("xref", &path, "hex", &["-m", "zzzz", "--kind", "function"]),
        2,
        "NOT_FOUND",
    );
    failure(run("xref", &path, "hex", &["-m", "11223344"]), 2, "--kind");
    failure(
        run(
            "xref",
            &path,
            "hex",
            &["--at", function_at, "-m", "x", "--kind", "function"],
        ),
        2,
        "cannot be used",
    );
}
