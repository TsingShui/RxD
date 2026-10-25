//! Cancellation-safe request framing and shared in-flight response admission.
use super::resources::{Responses, json_envelope};
use rmcp::{
    RoleServer,
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::Transport,
};
use serde_json::{Value, json};
use std::{io, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdin, Stdout},
    sync::{Mutex, Semaphore},
};

pub struct StdioTransport {
    input: BufReader<Stdin>,
    output: Arc<Mutex<Stdout>>,
    responses: Responses,
    partial: Vec<u8>,
    frame_limit: usize,
    active_limit: usize,
    shutdown: tokio_util::sync::CancellationToken,
    error_slots: Arc<Semaphore>,
    send_slots: Arc<Semaphore>,
}
impl StdioTransport {
    pub fn new(
        frame_limit: usize,
        responses: Responses,
        active_limit: usize,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            input: BufReader::new(tokio::io::stdin()),
            output: Arc::new(Mutex::new(tokio::io::stdout())),
            responses,
            partial: Vec::new(),
            frame_limit,
            active_limit,
            shutdown,
            error_slots: Arc::new(Semaphore::new(active_limit)),
            send_slots: Arc::new(Semaphore::new(active_limit)),
        }
    }
    fn error(&self, id: Value, code: i32, message: &str) {
        let packet = json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}});
        let memory = json_envelope(&packet, self.responses.bytes.limit())
            .and_then(|n| self.responses.bytes.reserve(n));
        let Ok(memory) = memory else {
            self.shutdown.cancel();
            return;
        };
        let Ok(bytes) = serde_json::to_vec(&packet) else {
            self.shutdown.cancel();
            return;
        };
        let Ok(permit) = self.error_slots.clone().try_acquire_owned() else {
            self.shutdown.cancel();
            return;
        };
        let output = self.output.clone();
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _memory = memory;
            tokio::select! {_=shutdown.cancelled()=>{},_=async {let mut output=output.lock().await;let _=output.write_all(&bytes).await;let _=output.write_all(b"\n").await;let _=output.flush().await;}=>{}}
        });
    }
}
impl Transport<RoleServer> for StdioTransport {
    type Error = io::Error;
    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = io::Result<()>> + Send + 'static {
        let output = self.output.clone();
        let responses = self.responses.clone();
        let shutdown = self.shutdown.clone();
        let permit = self.send_slots.clone().try_acquire_owned().ok();
        if permit.is_none() {
            self.shutdown.cancel();
        }
        async move {
            let _permit = permit.ok_or_else(|| {
                io::Error::other("RESOURCE_LIMIT: outgoing response admission exceeded")
            })?;
            // Handler reservations cover the complete JSON-RPC envelope.
            let id = match &item {
                rmcp::model::JsonRpcMessage::Response(response) => Some(&response.id),
                rmcp::model::JsonRpcMessage::Error(error) => error.id.as_ref(),
                _ => None,
            };
            let mut memory = id.and_then(|id| {
                responses.take(&serde_json::to_string(id).expect("request ID serialization"))
            });
            let size = json_envelope(&item, responses.bytes.limit()).map_err(io::Error::other)?;
            if let Some(memory) = memory.as_mut() {
                memory.resize(size).map_err(io::Error::other)?;
            } else {
                memory = Some(responses.bytes.reserve(size).map_err(io::Error::other)?);
            }
            let bytes = serde_json::to_vec(&item)?;
            tokio::select! {
                _=shutdown.cancelled()=>return Err(io::Error::new(io::ErrorKind::Interrupted,"MCP transport closed")),
                result=async {let mut output=output.lock().await;output.write_all(&bytes).await?;output.write_all(b"\n").await?;output.flush().await}=>result?,
            }
            if let Some(memory) = memory.as_mut() {
                memory.delivered();
            }
            Ok(())
        }
    }
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            if self.shutdown.is_cancelled() {
                return None;
            }
            let available = match self.input.fill_buf().await {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("MCP_ERROR: {error}");
                    self.shutdown.cancel();
                    return None;
                }
            };
            if available.is_empty() {
                self.shutdown.cancel();
                return None;
            }
            let newline = available.iter().position(|b| *b == b'\n');
            let take = newline.map_or(available.len(), |n| n + 1);
            if self.partial.len().saturating_add(take) > self.frame_limit {
                eprintln!("RESOURCE_LIMIT: MCP input frame exceeds budget");
                self.shutdown.cancel();
                return None;
            }
            self.partial.extend_from_slice(&available[..take]);
            self.input.consume(take);
            if newline.is_none() {
                continue;
            }
            let line = std::mem::take(&mut self.partial);
            let value: Value = match serde_json::from_slice(&line) {
                Ok(v) => v,
                Err(_) => {
                    self.error(Value::Null, -32700, "Parse error");
                    continue;
                }
            };
            let id = value.get("id").cloned();
            let message = match serde_json::from_value(value.clone()) {
                Ok(v) => v,
                Err(_) => {
                    self.error(id.unwrap_or(Value::Null), -32600, "Invalid Request");
                    continue;
                }
            };
            if value.get("method").is_some()
                && let Some(id) = id
            {
                let memory = json_envelope(&value, self.responses.bytes.limit())
                    .and_then(|size| self.responses.bytes.reserve(size));
                let Ok(memory) = memory else {
                    self.error(
                        id,
                        -32000,
                        "RESOURCE_LIMIT: in-flight request storage exceeded",
                    );
                    continue;
                };
                if !self.responses.admit(id.to_string(), memory) {
                    self.error(
                        id,
                        -32000,
                        "RESOURCE_LIMIT: request admission budget exceeded",
                    );
                    continue;
                }
            }
            if value["method"] == "notifications/cancelled"
                && let Some(id) = value["params"].get("requestId")
            {
                self.responses.take(&id.to_string());
            }
            return Some(message);
        }
    }
    async fn close(&mut self) -> io::Result<()> {
        tokio::select! {_=self.shutdown.cancelled()=>Ok(()),result=async {let _drain=self.error_slots.acquire_many(self.active_limit as u32).await;self.output.lock().await.flush().await}=>result}
    }
}
