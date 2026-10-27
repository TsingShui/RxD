//! Real CLI checks for the four retained EVM commands: interface, disassemble,
//! strings and metadata. Rows are evidence, so assertions stay on facts that an
//! independent reading of the bytecode also gives.
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

fn input(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/evm")
        .join(name)
}

#[test]
fn interface_reports_both_selector_candidates() {
    let (_dir, path) = fixture(ARGUMENTS, "contract.hex");
    let out = run("interface", &path, "hex", &[]);
    success(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("2125b65b(uint32,address,uint224)"),
        "{stdout}"
    );
    assert!(stdout.contains("b69ef8a8()"), "{stdout}");
}

#[test]
fn metadata_extracts_the_terminal_cbor_blob() {
    let out = run("metadata", &input("metadata-runtime.hex"), "hex", &[]);
    success(&out);
    let stdout = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
    assert!(
        stdout.contains("solc") || stdout.contains("ipfs"),
        "{stdout}"
    );
}

#[test]
fn disassemble_offsets_are_decoded_bytes_not_hex_characters() {
    let (_dir, path) = fixture(ARGUMENTS, "contract.hex");
    let out = run("disassemble", &path, "hex", &[]);
    success(&out);
    // 0x6080 is PUSH1 0x80; a hex-character offset would land mid-pair.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("0\tPUSH1 80\n2\tPUSH1 40\n"), "{stdout}");
    assert!(stdout.contains("29\tPUSH4 2125b65b\n"), "{stdout}");
}

#[test]
fn removed_commands_no_longer_parse() {
    let (_dir, path) = fixture(ARGUMENTS, "contract.hex");
    for command in [
        "analyze",
        "storage",
        "blocks",
        "basic-blocks",
        "cfg",
        "get-block",
        "get-function",
        "xref",
    ] {
        let out = run(command, &path, "hex", &[]);
        assert!(!out.status.success(), "{command} still runs");
    }
}
