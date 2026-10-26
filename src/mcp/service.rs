//! Tools and analysis scheduling are independent of the protocol transport.
use super::{
    Options,
    targets::{
        CaptureCheck, CaptureMode, CapturedInput, InputRef, TargetInfo, TargetLimits, TargetOrigin,
        TargetStore,
    },
    worker::{self, WorkerPayload, WorkerRequest},
};
use super::{
    requests::{Outbound, RequestLease, Responses},
    resources::json_envelope,
};
use crate::query::{QueryError, QueryOptions};
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
    options: QueryOptions,
    deadline: Duration,
    native_rss_limit: usize,
    responses: Responses,
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
    pub fn new(
        options: &Options,
        shutdown: CancellationToken,
        responses: Responses,
    ) -> Result<Self> {
        if options.max_inflight_bytes < 16 * 1024 || options.max_frame_bytes < 128 {
            return Err(Error::new(
                "INVALID_QUERY",
                "MCP shared byte/frame budgets must hold protocol errors",
            ));
        }
        let catalog = crate::query::catalog();
        let mut tools=vec![
            Tool::new("rxd_open","Capture immutable local input from absolute paths; backend hint establishes recursive JS or ASAR dependency scope.",open_schema()).with_raw_output_schema(Arc::new(with_error(schema::<TargetInfo>()))).with_annotations(ToolAnnotations::new().read_only(false).destructive(false).open_world(false)),
            Tool::new("rxd_close","Invalidate a process-local target handle; acquired requests retain their immutable bytes.",schema::<CloseArgs>()).with_raw_output_schema(Arc::new(with_error(schema::<Closed>()))).with_annotations(ToolAnnotations::new().read_only(false).destructive(false).open_world(false)),
        ];
        for definition in catalog {
            let creates_target = matches!(
                definition.name.as_str(),
                "asar_extract" | "zip_extract" | "html_extract"
            );
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
        let limits = TargetLimits {
            max_total_bytes: options.max_snapshot_bytes,
            max_depth: options.max_snapshot_depth,
        };
        let worker_slots = options.max_workers.min(options.max_threads);
        let query_options = QueryOptions {
            threads: options.max_threads / worker_slots,
            inflight_bytes: options.max_inflight_bytes,
        };
        Ok(Self {
            tools: Arc::new(tools),
            store: Arc::new(TargetStore::new(options.allow_paths.clone(), limits)?),
            admission: Arc::new(Semaphore::new(worker_slots)),
            worker_slots,
            shutdown,
            options: query_options,
            deadline: Duration::from_millis(options.deadline_ms as u64),
            native_rss_limit: options.max_native_worker_bytes,
            responses,
        })
    }

    async fn execute_tool(
        &self,
        name: String,
        arguments: Value,
        cancellation: CancellationToken,
        _permit: OwnedSemaphorePermit,
    ) -> std::result::Result<(Value, Outbound), QueryError> {
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
            let target = self.store.publish(capture).map_err(QueryError::from)?;
            let memory = json_envelope(&target.info, self.responses.bytes.limit())
                .and_then(|size| self.responses.bytes.reserve(size))
                .map_err(QueryError::from)?;
            let value = serde_json::to_value(&target.info)
                .map_err(|error| QueryError::new("INVALID_INPUT", error.to_string()))?;
            return Ok((
                value,
                Outbound {
                    memory,
                    target: Some(target),
                },
            ));
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
        let (reply, memory) = worker::run(
            WorkerRequest::Query {
                name,
                arguments: arguments.clone(),
                inputs,
                options: self.options,
            },
            self.responses.bytes.clone(),
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
        let mut outbound = Outbound::new(memory);
        if let Some(artifact) = result.artifact {
            if arguments["include_bytes"] == true {
                outbound
                    .memory
                    .grow(artifact.bytes.len().checked_mul(16).ok_or_else(|| {
                        QueryError::new("RESOURCE_LIMIT", "artifact envelope overflow")
                    })?)
                    .map_err(QueryError::from)?;
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
            outbound.target = Some(info);
            self.check_live(start, &cancellation)?;
            if artifact.source_id != expected_artifact_source_id {
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
                    serde_json::to_value(&outbound.target.as_ref().unwrap().info)
                        .map_err(|e| QueryError::new("WORKER_ERROR", e.to_string()))?,
                );
        }
        Ok((result.structured, outbound))
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
        let (reply, _memory) = worker::run(
            WorkerRequest::AsarPlan {
                input: capture.worker_input(),
            },
            self.responses.bytes.clone(),
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
        result: std::result::Result<(Value, Outbound), QueryError>,
        request: RequestLease,
    ) -> CallToolResult {
        #[derive(Serialize)]
        struct Envelope<'a> {
            jsonrpc: &'static str,
            id: &'a rmcp::model::RequestId,
            result: &'a CallToolResult,
        }
        request.respond(|id| {
            let prepared = result.and_then(|(value, mut outbound)| {
                outbound
                    .memory
                    .resize(
                        json_envelope(&value, self.responses.bytes.limit())
                            .map_err(QueryError::from)?,
                    )
                    .map_err(QueryError::from)?;
                let result = CallToolResult::structured(value);
                outbound
                    .memory
                    .resize(
                        json_envelope(
                            &Envelope {
                                jsonrpc: "2.0",
                                id,
                                result: &result,
                            },
                            self.responses.bytes.limit(),
                        )
                        .map_err(QueryError::from)?,
                    )
                    .map_err(QueryError::from)?;
                Ok((result, outbound))
            });
            match prepared {
                Ok((result, outbound)) => (result, Some(outbound)),
                Err(error) => {
                    let result = tool_error(error);
                    let outbound = json_envelope(
                        &Envelope {
                            jsonrpc: "2.0",
                            id,
                            result: &result,
                        },
                        self.responses.bytes.limit(),
                    )
                    .and_then(|size| self.responses.bytes.reserve(size))
                    .ok()
                    .map(Outbound::new);
                    (result, outbound)
                }
            }
        })
    }
}
impl ServerHandler for AnalysisService {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(Implementation::new("rxd",env!("CARGO_PKG_VERSION"))).with_instructions("rxd analyzes local inputs. Query tools accept absolute paths or process-local immutable targets; optional --allow-path grants apply to every capture.")
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
        let lease = self.responses.request(context.id.clone());
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        if request.name == "rxd_close" {
            return Ok(self
                .response(
                    decode::<CloseArgs>(arguments).and_then(|args| {
                        self.store
                            .close(&args.target_id)
                            .map_err(QueryError::from)?;
                        let value = json!({"target_id":args.target_id,"closed":true});
                        let memory = self
                            .responses
                            .bytes
                            .reserve(json_envelope(&value, self.responses.bytes.limit())?)
                            .map_err(QueryError::from)?;
                        Ok((value, Outbound::new(memory)))
                    }),
                    lease,
                )
                .into());
        }
        let permit = match self.admission.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                return Ok(self
                    .response(
                        Err(QueryError::new(
                            "RESOURCE_LIMIT",
                            "analysis worker admission budget exceeded",
                        )),
                        lease,
                    )
                    .into());
            }
        };
        let service = self.clone();
        let request_memory = json_envelope(&arguments, self.responses.bytes.limit())
            .and_then(|bytes| self.responses.bytes.reserve(bytes));
        let request_memory = match request_memory {
            Ok(memory) => memory,
            Err(error) => return Ok(self.response(Err(error.into()), lease).into()),
        };
        // The task owns its permit through capture cleanup and worker reaping even if a client disconnects.
        let result = tokio::spawn(async move {
            let _request_memory = request_memory;
            service
                .execute_tool(request.name.to_string(), arguments, context.ct, permit)
                .await
        })
        .await
        .map_err(join_error)
        .and_then(|result| result);
        Ok(self.response(result, lease).into())
    }
}
fn capture_mode(backend: &str) -> std::result::Result<CaptureMode, QueryError> {
    match backend {
        "asar" => Ok(CaptureMode::Asar),
        "dex" => Ok(CaptureMode::Bundle),
        "file" | "zip" | "html" | "jar" | "dotnet" | "wasm" | "pyc" | "evm" | "kuna" => {
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

fn open_schema() -> serde_json::Map<String, Value> {
    let backends: Vec<String> = crate::query::catalog()
        .into_iter()
        .map(|definition| definition.backend)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    json!({"type":"object","additionalProperties":false,"properties":{"path":{"type":"string","minLength":1,"description":"Absolute local input path."},"paths":{"type":"array","items":{"type":"string","minLength":1,"description":"Absolute local input path."},"minItems":1},"backend":{"type":"string","enum":backends}},"oneOf":[{"required":["path"],"not":{"required":["paths"]}},{"required":["paths"],"not":{"required":["path"]}}]}).as_object().unwrap().clone()
}
