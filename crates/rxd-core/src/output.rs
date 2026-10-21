//! Complete-result buffering with explicit amplification bounds and fallible growth.
use std::io::{self, Write};

use crate::{Error, Failure, Result};

pub struct Output {
    bytes: Vec<u8>,
    limit: usize,
}

impl Output {
    pub fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }

    pub fn json_line(&mut self, value: &impl serde::Serialize) -> Result<()> {
        serde_json::to_writer(&mut *self, value).map_err(|error| {
            Error::failure(
                if error.io_error_kind() == Some(io::ErrorKind::OutOfMemory) {
                    Failure::Resource
                } else {
                    Failure::Input
                },
                error.to_string(),
            )
        })?;
        self.write_all(b"\n")?;
        Ok(())
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl std::ops::Deref for Output {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::new(io::ErrorKind::OutOfMemory, "output bytes"));
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "output allocation"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_output_counts_newlines_and_preserves_resource_class() {
        let value = "x"; // three JSON bytes plus newline
        let mut output = Output::new(4);
        output.json_line(&value).unwrap();
        assert_eq!(&*output, b"\"x\"\n");
        for remaining in [0, 2, 3] {
            let mut output = Output::new(remaining);
            let error = output.json_line(&value).unwrap_err();
            assert_eq!(error.kind(), Some(Failure::Resource));
            assert!(output.len() <= remaining);
        }
    }
}
