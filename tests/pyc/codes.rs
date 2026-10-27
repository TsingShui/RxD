use super::*;

fn codes(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "codes"])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

// Diagnostic marshal fixture builder; no input is executed and no private parser is called.
fn bytes_field(value: &[u8]) -> Vec<u8> {
    let mut out = vec![b's'];
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value);
    out
}
fn metadata_module(body: &[u8], consts: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = vec![b'c'];
    for value in [0_u32, 0, 0, 2, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(body);
    out.extend_from_slice(consts);
    out.extend_from_slice(b")\0)\0");
    out.extend(bytes_field(b""));
    out.extend_from_slice(b"z\x0afixture.py");
    out.extend_from_slice(name);
    out.extend_from_slice(name);
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend(bytes_field(b""));
    out.extend(bytes_field(b""));
    out
}

#[test]
fn pyc_codes_rejects_non_ascii_marshal_ascii_metadata() {
    let name = b"z\x03\xe9\x9b\xaa"; // Valid UTF-8, invalid TYPE_SHORT_ASCII payload.
    let (_dir, path) = fixture(
        metadata_module(&bytes_field(&[121, 0]), b")\0", name),
        "ascii.marshal",
    );
    failure(codes(&path, &["--version", "3.12"]), 1, "INVALID_INPUT");
}
