//! Bounded one-shot analysis IPC. No parser is invoked in the protocol process.
use super::targets::{AsarDependency, WorkerInput};
use crate::query::{CapturedInput, QueryBudgets, QueryError, QueryResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::{Read, Write},
    process::ExitCode,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Deserialize)]
pub enum WorkerRequest {
    Query {
        name: String,
        arguments: Value,
        inputs: Vec<WorkerInput>,
        budgets: QueryBudgets,
    },
    AsarPlan {
        input: WorkerInput,
        max_files: usize,
        output_bytes: usize,
    },
}
#[derive(Serialize, Deserialize)]
pub enum WorkerPayload {
    Query(QueryResult),
    AsarPlan(Vec<AsarDependency>),
}
#[derive(Serialize, Deserialize)]
struct WorkerReply {
    result: Result<WorkerPayload, QueryError>,
}

#[cfg(feature = "backend-asar")]
fn plan_asar(
    input: WorkerInput,
    max_files: usize,
    output_bytes: usize,
) -> Result<WorkerPayload, QueryError> {
    let path = input
        .paths
        .first()
        .ok_or_else(|| QueryError::new("INVALID_QUERY", "ASAR capture has no input"))?;
    let limits = rxd_core::Limits {
        records: max_files,
        output_bytes,
        ..Default::default()
    };
    let mut perf = rxd_core::perf::Perf::new(false, "analysis", "asar", "entries");
    let rxd_asar::Response::List(bytes) = rxd_asar::run(
        rxd_asar::Query::Entries {
            contains: None,
            case_sensitive: false,
        },
        rxd_asar::Options {
            input: path,
            output: None,
            limits,
            max_entries: max_files,
            max_header_bytes: 8 * 1024 * 1024,
        },
        &mut perf,
    )
    .map_err(QueryError::from)?
    else {
        return Err(QueryError::new(
            "WORKER_ERROR",
            "ASAR planning returned an artifact",
        ));
    };
    let mut dependencies = Vec::new();
    for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let record: Value = serde_json::from_slice(line)
            .map_err(|e| QueryError::new("WORKER_ERROR", e.to_string()))?;
        if record["record"] == "file" && record["detail"]["storage"] == "unpacked" {
            dependencies.push(AsarDependency {
                entry: record["detail"]["path"]
                    .as_str()
                    .ok_or_else(|| QueryError::new("WORKER_ERROR", "invalid ASAR path"))?
                    .to_owned(),
                size: record["detail"]["bytes"]
                    .as_u64()
                    .ok_or_else(|| QueryError::new("WORKER_ERROR", "invalid ASAR size"))?,
            });
        }
    }
    Ok(WorkerPayload::AsarPlan(dependencies))
}

#[cfg(not(feature = "backend-asar"))]
fn plan_asar(
    _input: WorkerInput,
    _max_files: usize,
    _output_bytes: usize,
) -> Result<WorkerPayload, QueryError> {
    Err(QueryError::new(
        "INVALID_QUERY",
        "ASAR backend is not enabled in this build",
    ))
}

pub fn entry(limit: usize) -> ExitCode {
    let result = std::thread::Builder::new()
        .name("mcp-analysis".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || -> Result<WorkerPayload, QueryError> {
            let mut bytes = Vec::new();
            std::io::stdin()
                .take(limit.saturating_add(1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|e| QueryError::new("WORKER_ERROR", e.to_string()))?;
            if bytes.len() > limit {
                return Err(QueryError::new(
                    "RESOURCE_LIMIT",
                    "worker request IPC budget exceeded",
                ));
            }
            let request: WorkerRequest = serde_json::from_slice(&bytes)
                .map_err(|e| QueryError::new("WORKER_ERROR", e.to_string()))?;
            match request {
                WorkerRequest::Query {
                    name,
                    arguments,
                    inputs,
                    budgets,
                } => {
                    let inputs: Vec<CapturedInput> =
                        inputs.into_iter().flat_map(query_inputs).collect();
                    crate::query::execute(&name, arguments, &inputs, budgets)
                        .map(WorkerPayload::Query)
                }
                WorkerRequest::AsarPlan {
                    input,
                    max_files,
                    output_bytes,
                } => plan_asar(input, max_files, output_bytes),
            }
        })
        .map_err(|e| QueryError::new("WORKER_ERROR", e.to_string()))
        .and_then(|worker| {
            worker.join().unwrap_or_else(|_| {
                Err(QueryError::new("WORKER_ERROR", "analysis worker panicked"))
            })
        });
    let mut output = LimitedBuffer {
        bytes: Vec::new(),
        limit,
    };
    if serde_json::to_writer(&mut output, &WorkerReply { result }).is_err() {
        output.bytes.clear();
        if serde_json::to_writer(
            &mut output,
            &WorkerReply {
                result: Err(QueryError::new(
                    "RESOURCE_LIMIT",
                    "worker response IPC budget exceeded",
                )),
            },
        )
        .is_err()
        {
            return ExitCode::FAILURE;
        }
    }
    if std::io::stdout().lock().write_all(&output.bytes).is_err() {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

struct LimitedBuffer {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for LimitedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("worker IPC budget exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub async fn run(
    request: WorkerRequest,
    limit: usize,
    native_rss_limit: usize,
    deadline: Duration,
    cancellation: CancellationToken,
    shutdown: CancellationToken,
) -> Result<WorkerPayload, QueryError> {
    let native = matches!(&request, WorkerRequest::Query {name, ..} if name.starts_with("kuna_"));
    let mut wire = LimitedBuffer {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut wire, &request)
        .map_err(|_| QueryError::new("RESOURCE_LIMIT", "worker request IPC budget exceeded"))?;
    let executable =
        std::env::current_exe().map_err(|e| QueryError::new("WORKER_ERROR", e.to_string()))?;
    let mut command = Command::new(executable);
    command
        .args(["--internal-mcp-worker", &limit.to_string()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| QueryError::new("WORKER_ERROR", e.to_string()))?;
    let monitor = if native {
        match super::native_memory::Monitor::new(&child, native_rss_limit) {
            Ok(monitor) => Some(monitor),
            Err(error) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(error);
            }
        }
    } else {
        None
    };
    let memory = async {
        match monitor {
            Some(monitor) => monitor.exceeded().await,
            None => std::future::pending().await,
        }
    };
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| QueryError::new("WORKER_ERROR", "worker input missing"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| QueryError::new("WORKER_ERROR", "worker output missing"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| QueryError::new("WORKER_ERROR", "worker diagnostics missing"))?;
    let operation = async {
        let send = async {
            stdin
                .write_all(&wire.bytes)
                .await
                .map_err(|e| QueryError::new("WORKER_ERROR", e.to_string()))?;
            stdin
                .shutdown()
                .await
                .map_err(|e| QueryError::new("WORKER_ERROR", e.to_string()))?;
            drop(stdin);
            Ok::<(), QueryError>(())
        };
        let wait = async {
            child
                .wait()
                .await
                .map_err(|e| QueryError::new("WORKER_ERROR", e.to_string()))
        };
        let (_, output, diagnostics, status) = tokio::try_join!(
            send,
            read_bounded(stdout, limit),
            read_bounded(stderr, 64 * 1024),
            wait
        )?;
        if !status.success() {
            return Err(QueryError::new(
                "WORKER_ERROR",
                format!(
                    "analysis worker terminated: {status}; {}",
                    String::from_utf8_lossy(&diagnostics)
                ),
            ));
        }
        let reply: WorkerReply = serde_json::from_slice(&output).map_err(|_| {
            QueryError::new("WORKER_ERROR", "analysis worker returned malformed IPC")
        })?;
        reply.result
    };
    let result = tokio::select! {
        error=memory=>Err(error),
        result=operation=>result,
        _=cancellation.cancelled()=>Err(QueryError::new("CANCELLED","analysis request cancelled")),
        _=shutdown.cancelled()=>Err(QueryError::new("CANCELLED","server transport closed")),
        _=tokio::time::sleep(deadline)=>Err(QueryError::new("TIMEOUT","analysis deadline exceeded")),
    };
    // Reap explicitly on every path; dropping a cancellation future alone is insufficient.
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.start_kill();
    }
    let _ = child.wait().await;
    result
}
async fn read_bounded(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, QueryError> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| QueryError::new("WORKER_ERROR", e.to_string()))?;
    if bytes.len() > limit {
        return Err(QueryError::new(
            "RESOURCE_LIMIT",
            "worker IPC stream exceeded budget",
        ));
    }
    Ok(bytes)
}

fn query_inputs(input: WorkerInput) -> Vec<CapturedInput> {
    let identities: Vec<crate::query::SnapshotIdentity> = input
        .files
        .iter()
        .filter_map(|file| {
            let path = if file.dependency {
                let mut unpacked = input.paths.first()?.as_os_str().to_owned();
                unpacked.push(".unpacked");
                std::path::PathBuf::from(unpacked).join(file.relative_path.as_ref()?)
            } else if input.kind == super::targets::TargetKind::Directory {
                input.paths.first()?.join(file.relative_path.as_ref()?)
            } else {
                input
                    .paths
                    .iter()
                    .zip(&input.original_paths)
                    .find(|(_, original)| **original == file.original_path)?
                    .0
                    .clone()
            };
            Some(crate::query::SnapshotIdentity {
                path,
                source_id: file.source_id.clone(),
                input_bytes: usize::try_from(file.input_bytes).ok()?,
            })
        })
        .collect();
    input
        .paths
        .iter()
        .zip(&input.original_paths)
        .map(|(path, original_path)| {
            let file = input
                .files
                .iter()
                .find(|file| !file.dependency && &file.original_path == original_path);
            CapturedInput {
                path: path.clone(),
                original_path: original_path.clone(),
                source_id: file.map(|file| file.source_id.clone()),
                input_bytes: file.and_then(|file| usize::try_from(file.input_bytes).ok()),
                identities: identities.clone(),
            }
        })
        .collect()
}
