//! Tools and analysis scheduling are independent of the protocol transport.
use super::{
    Options,
    targets::{
        CaptureCheck, CaptureMode, CapturedInput, InputRef, TargetInfo, TargetLimits, TargetOrigin,
        TargetStore,
    },
    worker::{self, WorkerPayload, WorkerRequest},
};
use crate::query::{QueryBudgets, QueryError};
use base64::Engine as _;
use rmcp::{
    RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
        ToolAnnotations,
    },
    service::RequestContext,
};
use rxd_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AnalysisService {
    tools: Arc<Vec<Tool>>,
    store: Arc<TargetStore>,
    pub admission: Arc<Semaphore>,
    pub worker_slots: usize,
    shutdown: CancellationToken,
    budgets: QueryBudgets,
    deadline: Duration,
    ipc_limit: usize,
    native_rss_limit: usize,
    response_limit: usize,
    max_files: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenArgs {
    path: Option<PathBuf>,
    paths: Option<Vec<PathBuf>>,
    backend: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CloseArgs {
    target_id: String,
}
#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Closed {
    target_id: String,
    closed: bool,
}

impl AnalysisService {
    pub fn new(options: &Options, shutdown: CancellationToken) -> Result<Self> {
        if options.max_response_bytes < 1024 || options.max_frame_bytes < 128 {
            return Err(Error::new(
                "INVALID_QUERY",
                "MCP response/frame budgets must hold error and lifecycle messages",
            ));
        }
        if options.max_native_worker_bytes > super::native_memory::MAX_RSS_BYTES {
            return Err(Error::new(
                "INVALID_QUERY",
                "native worker RSS limit must be 1..=268435456 bytes",
            ));
        }
        let catalog = crate::query::catalog();
        for backend in &options.backends {
            if !catalog.iter().any(|tool| &tool.backend == backend) {
                return Err(Error::new(
                    "INVALID_QUERY",
                    format!("unsupported MCP backend: {backend}"),
                ));
            }
        }
        let mut tools=vec![
            Tool::new("rxd_open","Capture immutable local input; backend hint establishes recursive JS or ASAR dependency scope.",open_schema(options)).with_raw_output_schema(Arc::new(with_error(schema::<TargetInfo>()))).with_annotations(ToolAnnotations::new().read_only(false).destructive(false).open_world(false)),
            Tool::new("rxd_close","Invalidate a process-local target handle; acquired requests retain their immutable bytes.",schema::<CloseArgs>()).with_raw_output_schema(Arc::new(with_error(schema::<Closed>()))).with_annotations(ToolAnnotations::new().read_only(false).destructive(false).open_world(false)),
        ];
        for definition in catalog.into_iter().filter(|definition| {
            options.backends.is_empty() || options.backends.contains(&definition.backend)
        }) {
            let creates_target =
                matches!(definition.name.as_str(), "asar_extract" | "html_extract");
            let mut tool = Tool::new(
                definition.name,
                definition.description,
                definition
                    .input_schema
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            )
            .with_raw_output_schema(Arc::new(
                definition
                    .output_schema
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            ))
            .with_annotations(
                ToolAnnotations::new()
                    .read_only(!creates_target)
                    .destructive(false)
                    .idempotent(!creates_target)
                    .open_world(false),
            );
            tool.meta = Some(rmcp::model::MetaObject(
                json!({"backend":definition.backend,"capabilities":definition.capabilities})
                    .as_object()
                    .unwrap()
                    .clone(),
            ));
            tools.push(tool);
        }
        let base = options.base.clone().unwrap_or(std::env::current_dir()?);
        let roots = if options.root.is_empty() {
            vec![base.clone()]
        } else {
            options.root.clone()
        };
        let limits = TargetLimits {
            max_targets: options.max_targets,
            max_live_captures: options.max_live_captures,
            max_total_bytes: options.max_snapshot_bytes,
            max_capture_bytes: options.max_capture_bytes,
            max_files: options.max_snapshot_files,
            max_depth: options.max_snapshot_depth,
            max_directory_entries: options.max_snapshot_entries,
            ttl: Duration::from_millis(options.target_ttl_ms as u64),
        };
        let worker_slots = options.max_workers.min(options.max_threads);
        let budgets = QueryBudgets {
            input_bytes: options
                .max_capture_bytes
                .min(QueryBudgets::default().input_bytes),
            output_bytes: (options.max_response_bytes / 4)
                .min(QueryBudgets::default().output_bytes),
            threads: options.max_threads / worker_slots,
            ..Default::default()
        };
        Ok(Self {
            tools: Arc::new(tools),
            store: Arc::new(TargetStore::new(base, roots, limits)?),
            admission: Arc::new(Semaphore::new(worker_slots)),
            worker_slots,
            shutdown,
            budgets,
            deadline: Duration::from_millis(options.deadline_ms as u64),
            ipc_limit: options.max_worker_bytes,
            native_rss_limit: options.max_native_worker_bytes,
            response_limit: options.max_response_bytes,
            max_files: options.max_snapshot_files,
        })
    }

    pub fn spawn_reaper(&self, cadence: Duration) -> tokio::task::JoinHandle<()> {
        let store = self.store.clone();
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(cadence);
            loop {
                tokio::select! {_=shutdown.cancelled()=>break,_=interval.tick()=>{if let Err(error)=store.purge_expired(){eprintln!("{error}");}}}
            }
        })
    }

    async fn execute_tool(
        &self,
        name: String,
        arguments: Value,
        cancellation: CancellationToken,
        _permit: OwnedSemaphorePermit,
    ) -> std::result::Result<Value, QueryError> {
        let start = Instant::now();
        if name == "rxd_open" {
            let args: OpenArgs = decode(arguments)?;
            let paths = match (args.path, args.paths) {
                (Some(path), None) => vec![path],
                (None, Some(paths)) if !paths.is_empty() => paths,
                _ => {
                    return Err(QueryError::new(
                        "INVALID_QUERY",
                        "rxd_open requires exactly one nonempty path or paths",
                    ));
                }
            };
            let mode = if paths.len() > 1 {
                if args.backend.as_deref() != Some("dex") {
                    return Err(QueryError::new(
                        "INVALID_QUERY",
                        "multiple paths require backend dex",
                    ));
                }
                CaptureMode::Bundle
            } else {
                capture_mode(args.backend.as_deref().unwrap_or("file"))?
            };
            if let Some(backend) = args.backend
                && !self.tools.iter().any(|tool| {
                    tool.meta
                        .as_ref()
                        .is_some_and(|meta| meta.get("backend") == Some(&json!(backend)))
                })
            {
                return Err(QueryError::new(
                    "INVALID_QUERY",
                    "capture backend is not enabled",
                ));
            }
            let store = self.store.clone();
            let check = self.capture_check(start, &cancellation);
            let capture =
                tokio::task::spawn_blocking(move || store.capture_paths(&paths, mode, check))
                    .await
                    .map_err(join_error)?
                    .map_err(QueryError::from)?;
            let capture = self.prepare(capture, mode, start, &cancellation).await?;
            self.check_live(start, &cancellation)?;
            let info = self.store.publish(capture).map_err(QueryError::from)?;
            return match serde_json::to_value(&info) {
                Ok(value) => Ok(value),
                Err(error) => {
                    let _ = self.store.close(&info.target_id);
                    Err(QueryError::new("INVALID_INPUT", error.to_string()))
                }
            };
        }
        crate::query::validate(&name, &arguments)?;
        let backend = name
            .split_once('_')
            .map(|p| p.0)
            .ok_or_else(|| QueryError::new("INVALID_QUERY", "tool has no backend"))?;
        let mode = if name == "asar_entries" {
            CaptureMode::File
        } else {
            capture_mode(backend)?
        };
        let mut references = Vec::new();
        if let Some(inputs) = arguments.get("inputs").and_then(Value::as_array) {
            for input in inputs {
                references.push(decode::<InputRef>(input.clone())?);
            }
        } else {
            references.push(decode::<InputRef>(
                arguments.get("input").cloned().unwrap_or(Value::Null),
            )?);
        }
        let mut captures = Vec::new();
        for reference in references {
            self.check_live(start, &cancellation)?;
            let store = self.store.clone();
            let check = self.capture_check(start, &cancellation);
            let capture =
                tokio::task::spawn_blocking(move || store.acquire(&reference, mode, check))
                    .await
                    .map_err(join_error)?
                    .map_err(QueryError::from)?;
            captures.push(self.prepare(capture, mode, start, &cancellation).await?);
        }
        self.check_live(start, &cancellation)?;
        let inputs = captures
            .iter()
            .map(|capture| capture.worker_input())
            .collect();
        let reply = worker::run(
            WorkerRequest::Query {
                name,
                arguments: arguments.clone(),
                inputs,
                budgets: self.budgets,
            },
            self.ipc_limit,
            self.native_rss_limit,
            self.remaining(start)?,
            cancellation.clone(),
            self.shutdown.clone(),
        )
        .await?;
        let WorkerPayload::Query(mut result) = reply else {
            return Err(QueryError::new(
                "WORKER_ERROR",
                "query worker returned an ASAR plan",
            ));
        };
        self.check_live(start, &cancellation)?;
        if let Some(artifact) = result.artifact {
            if arguments["include_bytes"] == true {
                result.structured["bytes_base64"] =
                    json!(base64::engine::general_purpose::STANDARD.encode(&artifact.bytes));
            }
            let parent = captures
                .first()
                .ok_or_else(|| QueryError::new("WORKER_ERROR", "artifact has no parent input"))?
                .worker_input();
            let origin = TargetOrigin {
                container_source_id: parent.source_id,
                entry: artifact.name.clone(),
                parent_target_id: arguments["input"]["target_id"].as_str().map(str::to_owned),
            };
            let store = self.store.clone();
            let check = self.capture_check(start, &cancellation);
            let expected_artifact_source_id = rxd_core::identity::input_id(&artifact.bytes);
            let child_name = artifact
                .name
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&artifact.name)
                .replace(':', "_");
            let info = tokio::task::spawn_blocking(move || {
                store.child(&artifact.bytes, &child_name, origin, check)
            })
            .await
            .map_err(join_error)?
            .map_err(QueryError::from)?;
            if let Err(error) = self.check_live(start, &cancellation) {
                let _ = self.store.close(&info.target_id);
                return Err(error);
            }
            // MCP target identity is a whole-file SHA-256; an extract receipt's
            // `source_id` is the sampled stream identity. Check the receipt against its
            // own bytes instead of comparing the two different identity domains.
            if artifact.source_id != expected_artifact_source_id {
                let _ = self.store.close(&info.target_id);
                return Err(QueryError::new(
                    "WORKER_ERROR",
                    "extracted artifact identity mismatch",
                ));
            }
            result
                .structured
                .as_object_mut()
                .ok_or_else(|| {
                    QueryError::new("WORKER_ERROR", "artifact receipt is not an object")
                })?
                .insert(
                    "child_target".into(),
                    serde_json::to_value(info)
                        .map_err(|e| QueryError::new("WORKER_ERROR", e.to_string()))?,
                );
        }
        Ok(result.structured)
    }

    async fn prepare(
        &self,
        capture: Arc<CapturedInput>,
        mode: CaptureMode,
        start: Instant,
        cancellation: &CancellationToken,
    ) -> std::result::Result<Arc<CapturedInput>, QueryError> {
        self.check_live(start, cancellation)?;
        if !matches!(mode, CaptureMode::Asar) || capture.worker_input().asar_dependencies_captured {
            return Ok(capture);
        }
        let reply = worker::run(
            WorkerRequest::AsarPlan {
                input: capture.worker_input(),
                max_files: self.max_files,
                output_bytes: self.budgets.output_bytes,
            },
            self.ipc_limit,
            self.native_rss_limit,
            self.remaining(start)?,
            cancellation.clone(),
            self.shutdown.clone(),
        )
        .await?;
        let WorkerPayload::AsarPlan(dependencies) = reply else {
            return Err(QueryError::new(
                "WORKER_ERROR",
                "ASAR planner returned query output",
            ));
        };
        self.check_live(start, cancellation)?;
        // Generic targets/child artifacts already have immutable bytes; packed-only ASAR requires no additional capture.
        if Arc::strong_count(&capture) > 1 {
            if dependencies.is_empty() {
                return Ok(capture);
            }
            return Err(QueryError::new(
                "INVALID_QUERY",
                "ASAR dependencies were not captured; open with backend asar",
            ));
        }
        let store = self.store.clone();
        let check = self.capture_check(start, cancellation);
        tokio::task::spawn_blocking(move || store.complete_asar(capture, &dependencies, check))
            .await
            .map_err(join_error)?
            .map_err(QueryError::from)
    }
    fn capture_check(&self, start: Instant, cancellation: &CancellationToken) -> CaptureCheck {
        let cancellation = cancellation.clone();
        let shutdown = self.shutdown.clone();
        let deadline = self.deadline;
        Arc::new(move || {
            if cancellation.is_cancelled() || shutdown.is_cancelled() {
                return Err(Error::new("CANCELLED", "input capture cancelled"));
            }
            if start.elapsed() >= deadline {
                return Err(Error::new("TIMEOUT", "input capture deadline exceeded"));
            }
            Ok(())
        })
    }
    fn remaining(&self, start: Instant) -> std::result::Result<Duration, QueryError> {
        self.deadline
            .checked_sub(start.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| QueryError::new("TIMEOUT", "analysis deadline exceeded"))
    }
    fn check_live(
        &self,
        start: Instant,
        cancellation: &CancellationToken,
    ) -> std::result::Result<(), QueryError> {
        if cancellation.is_cancelled() || self.shutdown.is_cancelled() {
            return Err(QueryError::new("CANCELLED", "analysis request cancelled"));
        }
        self.remaining(start).map(|_| ())
    }
    fn response(
        &self,
        result: std::result::Result<Value, QueryError>,
        rpc_overhead: usize,
    ) -> CallToolResult {
        let cleanup = result
            .as_ref()
            .ok()
            .and_then(|value| {
                value["child_target"]["target_id"]
                    .as_str()
                    .or_else(|| value["target_id"].as_str())
            })
            .map(str::to_owned);
        let result = match result {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => tool_error(error),
        };
        if serde_json::to_vec(&result).map_or(true, |bytes| {
            bytes.len().saturating_add(rpc_overhead) > self.response_limit
        }) {
            if let Some(target) = cleanup {
                let _ = self.store.close(&target);
            }
            tool_error(QueryError::new(
                "RESOURCE_LIMIT",
                "complete MCP response exceeds transport budget",
            ))
        } else {
            result
        }
    }
}
impl ServerHandler for AnalysisService {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(Implementation::new("rxd",env!("CARGO_PKG_VERSION"))).with_instructions("rxd analyzes local inputs. Query tools accept direct paths or process-local immutable targets; configured roots apply to every capture.")
    }
    async fn list_tools(
        &self,
        params: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        if params.is_some_and(|p| p.cursor.is_some()) {
            return Err(ErrorData::invalid_params(
                "rxd tool catalog has no cursors",
                None,
            ));
        }
        Ok(ListToolsResult {
            tools: self.tools.as_ref().clone(),
            ..Default::default()
        })
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|tool| tool.name == name).cloned()
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        if !self.tools.iter().any(|tool| tool.name == request.name) {
            return Err(ErrorData::invalid_params(
                "unknown or disabled rxd tool",
                None,
            ));
        }
        // Include the caller's correlation ID and fixed protocol/SDK metadata in the complete-response budget.
        let rpc_overhead = serde_json::to_vec(&context.id)
            .map_or(self.response_limit, |bytes| bytes.len())
            .saturating_add(512);
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        if request.name == "rxd_close" {
            return Ok(self
                .response(
                    decode::<CloseArgs>(arguments).and_then(|args| {
                        self.store
                            .close(&args.target_id)
                            .map_err(QueryError::from)?;
                        Ok(json!({"target_id":args.target_id,"closed":true}))
                    }),
                    rpc_overhead,
                )
                .into());
        }
        let permit = match self.admission.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                return Ok(tool_error(QueryError::new(
                    "RESOURCE_LIMIT",
                    "analysis worker admission budget exceeded",
                ))
                .into());
            }
        };
        let service = self.clone();
        // The task owns its permit through capture cleanup and worker reaping even if a client disconnects.
        let result = tokio::spawn(async move {
            service
                .execute_tool(request.name.to_string(), arguments, context.ct, permit)
                .await
        })
        .await
        .map_err(join_error)
        .and_then(|result| result);
        Ok(self.response(result, rpc_overhead).into())
    }
}
fn capture_mode(backend: &str) -> std::result::Result<CaptureMode, QueryError> {
    match backend {
        "js" => Ok(CaptureMode::JavaScriptDirectory),
        "asar" => Ok(CaptureMode::Asar),
        "dex" => Ok(CaptureMode::Bundle),
        "file" | "html" | "jar" | "dotnet" | "wasm" | "pyc" | "evm" | "kuna" => {
            Ok(CaptureMode::File)
        }
        _ => Err(QueryError::new(
            "INVALID_QUERY",
            "unsupported capture backend",
        )),
    }
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> std::result::Result<T, QueryError> {
    serde_json::from_value(value).map_err(|e| QueryError::new("INVALID_QUERY", e.to_string()))
}
fn join_error(error: tokio::task::JoinError) -> QueryError {
    QueryError::new("WORKER_ERROR", error.to_string())
}
fn tool_error(error: QueryError) -> CallToolResult {
    CallToolResult::structured_error(
        json!({"error":{"code":error.code,"message":error.message,"candidates":error.candidates}}),
    )
}
fn schema<T: JsonSchema>() -> serde_json::Map<String, Value> {
    schemars::schema_for!(T)
        .to_value()
        .as_object()
        .unwrap()
        .clone()
}
fn with_error(mut success: serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    let definitions = success.remove("$defs");
    let mut schema=json!({"type":"object","anyOf":[success,{"type":"object","required":["error"],"additionalProperties":false,"properties":{"error":{"type":"object","required":["code","message","candidates"],"additionalProperties":false,"properties":{"code":{"type":"string"},"message":{"type":"string"},"candidates":{"type":"null"}}}}}]}).as_object().unwrap().clone();
    if let Some(definitions) = definitions {
        schema.insert("$defs".into(), definitions);
    }
    schema
}

fn open_schema(options: &Options) -> serde_json::Map<String, Value> {
    let backends: Vec<String> = crate::query::catalog()
        .into_iter()
        .map(|definition| definition.backend)
        .filter(|backend| options.backends.is_empty() || options.backends.contains(backend))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    json!({"type":"object","additionalProperties":false,"properties":{"path":{"type":"string","minLength":1},"paths":{"type":"array","items":{"type":"string","minLength":1},"minItems":1,"maxItems":options.max_snapshot_files},"backend":{"type":"string","enum":backends}},"oneOf":[{"required":["path"],"not":{"required":["paths"]}},{"required":["paths"],"not":{"required":["path"]}}]}).as_object().unwrap().clone()
}
