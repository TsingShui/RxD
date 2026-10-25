//! Input identity for coordinate validation.
use sha2::{Digest, Sha256};

const SAMPLE_BYTES: usize = 16 * 1024;

/// Hash input length and the first/last 16 KiB.
/// Same-length interior edits are not detected; this is not a whole-file digest.
pub fn input_id(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update((bytes.len() as u64).to_le_bytes());
    if bytes.len() <= 2 * SAMPLE_BYTES {
        hasher.update(bytes);
    } else {
        hasher.update(&bytes[..SAMPLE_BYTES]);
        hasher.update(&bytes[bytes.len() - SAMPLE_BYTES..]);
    }
    lower_hex(&hasher.finalize())
}

pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 15) as usize] as char);
    }
    result
}
