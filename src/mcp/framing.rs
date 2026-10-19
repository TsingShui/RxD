//! Finite, cancellation-safe line framing ahead of the SDK deserializer.
use rmcp::{
    RoleServer,
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::Transport,
};
use serde_json::{Value, json};
use std::{collections::HashSet, io, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdin, Stdout},
    sync::{Mutex, Semaphore},
};

pub struct StdioTransport {
    input: BufReader<Stdin>,
    output: Arc<Mutex<Stdout>>,
    pending: Arc<Mutex<HashSet<String>>>,
    partial: Vec<u8>,
    frame_limit: usize,
    response_limit: usize,
    active_limit: usize,
    shutdown: tokio_util::sync::CancellationToken,
    error_slots: Arc<Semaphore>,
    send_slots: Arc<Semaphore>,
}
impl StdioTransport {
    pub fn new(
        frame_limit: usize,
        response_limit: usize,
        active_limit: usize,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            input: BufReader::new(tokio::io::stdin()),
            output: Arc::new(Mutex::new(tokio::io::stdout())),
            pending: Arc::new(Mutex::new(HashSet::new())),
            partial: Vec::new(),
            frame_limit,
            response_limit,
            active_limit,
            shutdown,
            error_slots: Arc::new(Semaphore::new(active_limit)),
            send_slots: Arc::new(Semaphore::new(active_limit)),
        }
    }
    fn error(&self, id: Value, code: i32, message: &str) {
        let mut bytes = serde_json::to_vec(
            &json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
        )
        .unwrap_or_default();
        if bytes.len().saturating_add(1) > self.response_limit {
            bytes=serde_json::to_vec(&json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"RESOURCE_LIMIT: error correlation exceeds response budget"}})).unwrap_or_default();
        }
        if bytes.len().saturating_add(1) > self.response_limit {
            self.shutdown.cancel();
            return;
        }
        let Ok(permit) = self.error_slots.clone().try_acquire_owned() else {
            self.shutdown.cancel();
            return;
        };
        let output = self.output.clone();
        let shutdown = self.shutdown.clone();
        // The SDK cancels receive futures while sending. Already-consumed malformed frames must still receive their error.
        tokio::spawn(async move {
            let _permit = permit;
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
        let pending = self.pending.clone();
        let limit = self.response_limit;
        let shutdown = self.shutdown.clone();
        let permit = self.send_slots.clone().try_acquire_owned().ok();
        if permit.is_none() {
            self.shutdown.cancel();
        }
        async move {
            let _permit = permit.ok_or_else(|| {
                io::Error::other("RESOURCE_LIMIT: MCP outgoing response admission exceeded")
            })?;
            let value = serde_json::to_value(item)?;
            let id = value.get("id").map(Value::to_string);
            let mut bytes = serde_json::to_vec(&value)?;
            if bytes.len().saturating_add(1) > limit {
                bytes = serde_json::to_vec(
                    &json!({"jsonrpc":"2.0", "id":value.get("id").cloned().unwrap_or(Value::Null),"error":{"code":-32603,"message":"RESOURCE_LIMIT: complete response exceeds transport budget"}}),
                )?;
                if bytes.len().saturating_add(1) > limit {
                    return Err(io::Error::other("response budget cannot hold error"));
                }
            }
            tokio::select! {
                _=shutdown.cancelled()=>return Err(io::Error::new(io::ErrorKind::Interrupted,"MCP transport closed")),
                result=async {let mut output=output.lock().await;output.write_all(&bytes).await?;output.write_all(b"\n").await?;output.flush().await}=>result?,
            }
            if let Some(id) = id {
                pending.lock().await.remove(&id);
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
                Ok(v) => v,
                Err(e) => {
                    eprintln!("MCP_ERROR: {e}");
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
            if id
                .as_ref()
                .is_some_and(|id| id.to_string().len().saturating_add(768) > self.response_limit)
            {
                self.error(
                    Value::Null,
                    -32600,
                    "RESOURCE_LIMIT: request ID cannot fit response budget",
                );
                continue;
            }
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
                let mut pending = self.pending.lock().await;
                if pending.len() >= self.active_limit || !pending.insert(id.to_string()) {
                    drop(pending);
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
                self.pending.lock().await.remove(&id.to_string());
            }
            return Some(message);
        }
    }
    async fn close(&mut self) -> io::Result<()> {
        tokio::select! {
            _=self.shutdown.cancelled()=>Ok(()),
            result=async {let _drain=self.error_slots.acquire_many(self.active_limit as u32).await;self.output.lock().await.flush().await}=>result,
        }
    }
}
