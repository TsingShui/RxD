//! Shared byte admission for resident snapshots and in-flight protocol data.
//! This accounts for retained payload/metadata envelopes, not process RSS.
use rxd_core::{Error, Result};
use std::{
    collections::HashMap,
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
    rollback: Option<Box<dyn FnOnce() + Send + Sync>>,
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
            rollback: None,
        };
        reservation.grow(bytes)?;
        Ok(reservation)
    }
}
impl Reservation {
    /// Roll back targets whose response is not delivered.
    pub fn rollback(&mut self, cleanup: impl FnOnce() + Send + Sync + 'static) {
        self.rollback = Some(Box::new(cleanup));
    }
    pub fn delivered(&mut self) {
        self.rollback = None;
    }
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
        drop(state);
        if let Some(cleanup) = self.rollback.take() {
            cleanup();
        }
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

/// Transfer response storage to transport; cancellation releases suppressed replies.
#[derive(Clone)]
pub struct Responses {
    pub bytes: ByteBudget,
    pending: Arc<Mutex<HashMap<String, Option<Reservation>>>>,
    active: usize,
}
impl Responses {
    pub fn new(limit: usize, active: usize) -> Self {
        Self {
            bytes: ByteBudget::new(limit),
            pending: Arc::new(Mutex::new(HashMap::new())),
            active,
        }
    }
    pub fn admit(&self, id: String, memory: Reservation) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.len() >= self.active || pending.contains_key(&id) {
            return false;
        }
        pending.insert(id, Some(memory));
        true
    }
    pub fn release_request(&self, id: &str) {
        if let Some(slot) = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(id)
        {
            slot.take();
        }
    }
    pub fn retain(&self, id: &str, memory: Reservation) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let Some(slot) = pending.get_mut(id) else {
            return false;
        };
        *slot = Some(memory);
        true
    }
    pub fn take(&self, id: &str) -> Option<Reservation> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .flatten()
    }
}
