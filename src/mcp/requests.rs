//! Request ownership and response delivery; suppressed replies release targets and storage.
use super::{
    resources::{ByteBudget, Reservation, json_envelope},
    targets::PublishedTarget,
};
use rmcp::model::RequestId;
use rxd_core::Result;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub struct Outbound {
    pub memory: Reservation,
    pub target: Option<PublishedTarget>,
}
impl Outbound {
    pub fn new(memory: Reservation) -> Self {
        Self {
            memory,
            target: None,
        }
    }
    pub fn delivered(&mut self) {
        if let Some(target) = self.target.as_mut() {
            target.delivered();
        }
    }
}
struct Pending {
    generation: u64,
    data: Option<Outbound>,
}

#[derive(Default)]
struct Registry {
    pending: HashMap<RequestId, Pending>,
    serial: u64,
}

#[derive(Clone)]
pub struct Responses {
    pub bytes: ByteBudget,
    registry: Arc<Mutex<Registry>>,
    active: usize,
}
impl Responses {
    pub fn new(limit: usize, active: usize) -> Self {
        Self {
            bytes: ByteBudget::new(limit),
            registry: Arc::new(Mutex::new(Registry::default())),
            active,
        }
    }
    pub fn admit(&self, id: RequestId, memory: Reservation) -> bool {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if registry.pending.len() >= self.active || registry.pending.contains_key(&id) {
            return false;
        }
        let Some(generation) = registry.serial.checked_add(1) else {
            return false;
        };
        registry.serial = generation;
        registry.pending.insert(
            id,
            Pending {
                generation,
                data: Some(Outbound::new(memory)),
            },
        );
        true
    }
    pub fn request(&self, id: RequestId) -> RequestLease {
        let generation = self
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .get(&id)
            .map(|entry| entry.generation);
        RequestLease {
            responses: self.clone(),
            id,
            generation,
            completed: false,
        }
    }
    pub fn cancel(&self, id: &RequestId) {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .remove(id);
    }
    pub fn sending(
        &self,
        id: Option<&RequestId>,
        value: &impl serde::Serialize,
    ) -> Result<Outbound> {
        let retained = id
            .and_then(|id| {
                self.registry
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pending
                    .remove(id)
            })
            .and_then(|entry| entry.data);
        let size = json_envelope(value, self.bytes.limit())?;
        let mut outbound = match retained {
            Some(outbound) => outbound,
            None => Outbound::new(self.bytes.reserve(size)?),
        };
        outbound.memory.resize(size)?;
        Ok(outbound)
    }
}

pub struct RequestLease {
    responses: Responses,
    id: RequestId,
    generation: Option<u64>,
    completed: bool,
}
impl RequestLease {
    pub fn respond<T>(mut self, build: impl FnOnce(&RequestId) -> (T, Option<Outbound>)) -> T {
        {
            let mut registry = self
                .responses
                .registry
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = registry
                .pending
                .get_mut(&self.id)
                .filter(|entry| Some(entry.generation) == self.generation)
            {
                entry.data.take();
            }
        }
        let (result, outbound) = build(&self.id);
        {
            let mut registry = self
                .responses
                .registry
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = registry
                .pending
                .get_mut(&self.id)
                .filter(|entry| Some(entry.generation) == self.generation)
            {
                entry.data = outbound;
            }
        }
        self.completed = true;
        result
    }
}
impl Drop for RequestLease {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let mut registry = self
            .responses
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if registry
            .pending
            .get(&self.id)
            .is_some_and(|entry| Some(entry.generation) == self.generation)
        {
            registry.pending.remove(&self.id);
        }
    }
}
