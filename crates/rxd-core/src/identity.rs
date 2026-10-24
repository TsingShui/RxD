//! Input identity for coordinate validation.
use sha2::{Digest, Sha256};

/// Bytes taken from each end of an input when deriving its identity.
const SAMPLE_BYTES: usize = 16 * 1024;

/// Identity of an input buffer: a digest over its length and both ends.
///
/// Whole-file hashing cost one pass over every byte of every input on every
/// query — 5.5 ms on a 16 MB assembly, tens of milliseconds on a large APK. The
/// identity only has to separate one input from another, so it samples the
/// length plus the first and last 16 KiB. A copy or rename keeps the same
/// identity; a query never reads the interior twice.
///
/// The deliberate weakening: an edit that preserves the length and both ends is
/// not detected. Rebuilds change the size or the headers, and a substituted
/// artifact is not this tool's concern.
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
