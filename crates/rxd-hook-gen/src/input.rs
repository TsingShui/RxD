use crate::{Diagnostic, ErrorKind, Limits, Result, Stage};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Owns the sole analysis capture. The display name never substitutes for byte identity.
#[derive(Clone, Debug)]
pub struct CapturedInput {
    name: String,
    bytes: Vec<u8>,
    source_id: String,
}

impl CapturedInput {
    pub fn from_bytes(name: impl Into<String>, bytes: Vec<u8>, limits: Limits) -> Result<Self> {
        limits.validate()?;
        limits.check_input(bytes.len())?;
        let mut source_id = String::with_capacity(64);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in Sha256::digest(&bytes) {
            source_id.push(HEX[(byte >> 4) as usize] as char);
            source_id.push(HEX[(byte & 0x0f) as usize] as char);
        }
        Ok(Self {
            name: name.into(),
            bytes,
            source_id,
        })
    }

    pub fn read(path: &Path, limits: Limits) -> Result<Self> {
        limits.validate()?;
        // rxd-core captures are u32-sized; cap before its one-byte overflow probe.
        let bytes = rxd_core::read_bytes(path, limits.input_bytes.min(u32::MAX as usize)).map_err(
            |error| {
                Diagnostic::new(
                    match error.code {
                        "RESOURCE_LIMIT" => ErrorKind::ResourceLimit,
                        "INVALID_INPUT" => ErrorKind::InvalidInput,
                        _ => ErrorKind::Io,
                    },
                    Stage::Input,
                    error.message,
                )
            },
        )?;
        Self::from_bytes(path.to_string_lossy(), bytes, limits)
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
}
