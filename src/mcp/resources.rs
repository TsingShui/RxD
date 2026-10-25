//! Shared byte admission for resident snapshots and in-flight protocol data.
//! This accounts for retained payload/metadata envelopes, not process RSS.
use rxd_core::{Error, Result};
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct ByteBudget(Arc<Mutex<State>>);
struct State {
    used: usize,
    limit: usize,
}

pub struct Reservation {
    budget: ByteBudget,
    bytes: usize,
}
impl ByteBudget {
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(Mutex::new(State { used: 0, limit })))
    }
    pub fn limit(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).limit
    }
    pub fn reserve(&self, bytes: usize) -> Result<Reservation> {
        let mut reservation = Reservation {
            budget: self.clone(),
            bytes: 0,
        };
        reservation.grow(bytes)?;
        Ok(reservation)
    }
}
impl Reservation {
    pub fn grow(&mut self, bytes: usize) -> Result<()> {
        let mut state = self.budget.0.lock().unwrap_or_else(|e| e.into_inner());
        if bytes > state.limit.saturating_sub(state.used) {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "shared retained byte budget exceeded",
            ));
        }
        state.used += bytes;
        self.bytes += bytes;
        Ok(())
    }
    pub fn resize(&mut self, bytes: usize) -> Result<()> {
        if bytes > self.bytes {
            return self.grow(bytes - self.bytes);
        }
        let mut state = self.budget.0.lock().unwrap_or_else(|e| e.into_inner());
        state.used -= self.bytes - bytes;
        self.bytes = bytes;
        Ok(())
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self.budget.0.lock().unwrap_or_else(|e| e.into_inner());
        state.used -= self.bytes;
    }
}

/// Preflight JSON payload, metadata and temporary protocol copies before allocation.
pub fn json_envelope(value: &impl serde::Serialize, limit: usize) -> Result<usize> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::OutOfMemory,
                    "JSON envelope",
                ));
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value)
        .map_err(|_| Error::new("RESOURCE_LIMIT", "JSON envelope exceeds in-flight budget"))?;
    counter
        .bytes
        .checked_mul(12)
        .and_then(|n| n.checked_add(1024))
        .filter(|n| *n <= limit)
        .ok_or_else(|| {
            Error::new(
                "RESOURCE_LIMIT",
                "JSON metadata envelope exceeds in-flight budget",
            )
        })
}
