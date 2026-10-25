//! Local MCP entry point; transport-independent handlers live in service.
mod framing;
mod native_memory;
mod requests;
mod resources;
mod service;
pub(crate) mod targets;
pub(crate) mod worker;

use clap::Args;
use rmcp::ServiceExt;
use rxd_core::{Error, Result};
use std::path::PathBuf;

#[derive(Args)]
#[command(after_long_help = "Examples:
  rxd mcp --root /absolute/authorized/inputs
  rxd mcp --base /workspace --root /workspace --backends js,jar,wasm
Launch as a stdio MCP server; stdout is JSON-RPC, not CLI query output.
Clients discover tools/list before tools/call. CLI flags use kebab-case; MCP fields use snake_case.
Ordinary tools take input: {path: ...} or input: {target_id: ...}, not a bare path string.
For repeated queries: rxd_open -> use returned target_id -> rxd_close.
Schemas/descriptions include query examples; copy result at/context_at for navigation.
Analysis budgets scale with input exactly as in CLI queries; no per-query limits object.
Work, instruction count, expanded-entry bytes and output have no input-derived quotas.
Shared in-flight storage still applies; linked engines retain their own protections.
Targets remain valid until rxd_close or shutdown; no target TTL or count quota.
Service byte envelopes include metadata and protocol copies, not hard process RSS.
Raise the listed storage/deadline/RSS settings for larger workloads.")]
pub struct Options {
    /// Local roots permitted for input capture. Defaults to the server base.
    #[arg(long)]
    pub root: Vec<PathBuf>,
    /// Backend tool catalog filter (comma separated).
    #[arg(long, value_delimiter = ',')]
    pub backends: Vec<String>,
    /// Base for relative caller paths.
    #[arg(long)]
    pub base: Option<PathBuf>,
    /// Maximum incoming JSON-RPC frame bytes, including its newline.
    #[arg(long, default_value = "1048576", value_parser = crate::cli::positive)]
    pub max_frame_bytes: usize,
    /// Shared byte envelope for all in-flight requests, worker IPC and responses.
    #[arg(long, default_value = "1073741824", value_parser = crate::cli::positive)]
    pub max_inflight_bytes: usize,
    /// Maximum requests retained by the protocol process.
    #[arg(long, default_value = "16", value_parser = crate::cli::positive)]
    pub max_active_requests: usize,
    /// Maximum simultaneous analysis workers.
    #[arg(long, default_value = "2", value_parser = crate::cli::positive)]
    pub max_workers: usize,
    /// Total analysis thread budget across workers.
    #[arg(long, default_value = "4", value_parser = crate::cli::positive)]
    pub max_threads: usize,
    /// Per-call deadline in milliseconds, including capture and worker execution.
    #[arg(long, default_value = "30000", value_parser = crate::cli::positive)]
    pub deadline_ms: usize,
    /// Sampled RSS limit per linked Kuna worker; adjustable for large targets.
    #[arg(long, default_value = "1073741824", value_parser = crate::cli::positive)]
    pub max_native_worker_bytes: usize,
    /// Shared snapshot storage, including metadata and empty-file/handle overhead.
    #[arg(long, default_value = "1073741824", value_parser = crate::cli::positive)]
    pub max_snapshot_bytes: usize,
    /// Format/traversal guard, not a file-count quota (maximum 256).
    #[arg(long, default_value = "256", value_parser = crate::cli::positive)]
    pub max_snapshot_depth: usize,
}

pub fn execute(options: Options) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(Error::from)?;
    let result = runtime.block_on(async move {
        let shutdown = tokio_util::sync::CancellationToken::new();
        let responses =
            requests::Responses::new(options.max_inflight_bytes, options.max_active_requests);
        let transport = framing::StdioTransport::new(
            options.max_frame_bytes,
            responses.clone(),
            options.max_active_requests,
            shutdown.clone(),
        );
        let handler = service::AnalysisService::new(&options, shutdown.clone(), responses)?;
        let admission = handler.admission.clone();
        let slots = handler.worker_slots;
        let result = match handler.serve(transport).await {
            Ok(running) => running
                .waiting()
                .await
                .map(|_| ())
                .map_err(|e| Error::new("MCP_ERROR", e.to_string())),
            Err(rmcp::service::ServerInitializeError::ConnectionClosed(_))
                if shutdown.is_cancelled() =>
            {
                Ok(())
            }
            Err(error) => Err(Error::new("MCP_ERROR", error.to_string())),
        };
        shutdown.cancel();
        let _drain = admission.acquire_many(slots as u32).await;
        result
    });
    // Tokio stdio uses blocking system IO. A client may keep an unread stdout pipe open after stdin EOF.
    // Analysis tasks and workers were drained above; process exit releases remaining blocked IO threads.
    runtime.shutdown_timeout(std::time::Duration::from_millis(100));
    result
}
