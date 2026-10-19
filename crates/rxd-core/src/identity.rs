//! Scoped identities for immutable server-owned input snapshots.
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// A precomputed identity of a private immutable captured file.
///
/// The caller must own the snapshot and keep its bytes immutable for the entire
/// scoped operation. Original caller-controlled paths must never be registered.
#[derive(Clone)]
pub struct CapturedIdentity {
    pub path: PathBuf,
    pub input_bytes: usize,
    pub source_id: String,
}
thread_local! { static CAPTURES: RefCell<BTreeMap<PathBuf, (usize, String)>> = const { RefCell::new(BTreeMap::new()) }; }

pub fn with_captured_identities<T>(
    identities: Vec<CapturedIdentity>,
    query: impl FnOnce() -> T,
) -> T {
    struct Restore(BTreeMap<PathBuf, (usize, String)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CAPTURES.with(|slot| *slot.borrow_mut() = std::mem::take(&mut self.0));
        }
    }
    let identities = identities
        .into_iter()
        .map(|identity| (identity.path, (identity.input_bytes, identity.source_id)))
        .collect();
    let previous = CAPTURES.with(|slot| slot.replace(identities));
    let _restore = Restore(previous);
    query()
}

/// Return the captured identity when this buffer is from the scoped immutable
/// snapshot; ordinary CLI and newly derived container buffers are hashed normally.
pub fn captured_id(path: &Path, input_bytes: usize) -> Option<String> {
    CAPTURES.with(|slot| {
        slot.borrow()
            .get(path)
            .filter(|(bytes, source_id)| {
                *bytes == input_bytes
                    && source_id.len() == 64
                    && source_id
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            .map(|(_, source_id)| source_id.clone())
    })
}

pub fn content_id(path: &Path, bytes: &[u8]) -> String {
    captured_id(path, bytes.len()).unwrap_or_else(|| lower_hex(&Sha256::digest(bytes)))
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
