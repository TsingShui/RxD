//! Local MCP entry point; transport-independent handlers live in service.
mod framing;
mod install;
mod native_memory;
mod service;
pub(crate) mod targets;
pub(crate) mod worker;

use clap::{Args, Subcommand};
use rmcp::ServiceExt;
use rxd_core::{Error, Result};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum Command {
    /// Serve MCP on stdin/stdout. All diagnostics use stderr.
    Serve(Options),
    /// Register this executable as a user-level native MCP stdio server.
    Install(install::Options),
}

#[derive(Args)]
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
    /// Maximum complete response frame bytes, including compatibility text.
    #[arg(long, default_value = "33554432", value_parser = crate::cli::positive)]
    pub max_response_bytes: usize,
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
    /// Maximum worker request or result IPC bytes.
    #[arg(long, default_value = "67108864", value_parser = crate::cli::positive)]
    pub max_worker_bytes: usize,
    /// Maximum sampled RSS of each linked Kuna worker (1 through 268435456 bytes).
    #[arg(long, default_value = "268435456", value_parser = crate::cli::positive)]
    pub max_native_worker_bytes: usize,
    #[arg(long, default_value = "64", value_parser = crate::cli::positive)]
    pub max_targets: usize,
    #[arg(long, default_value = "128", value_parser = crate::cli::positive)]
    pub max_live_captures: usize,
    #[arg(long, default_value = "1073741824", value_parser = crate::cli::positive)]
    pub max_snapshot_bytes: usize,
    #[arg(long, default_value = "268435456", value_parser = crate::cli::positive)]
    pub max_capture_bytes: usize,
    #[arg(long, default_value = "10000", value_parser = crate::cli::positive)]
    pub max_snapshot_files: usize,
    #[arg(long, default_value = "100000", value_parser = crate::cli::positive)]
    pub max_snapshot_entries: usize,
    /// Maximum directory capture depth (1 through 256).
    #[arg(long, default_value = "64", value_parser = crate::cli::positive)]
    pub max_snapshot_depth: usize,
    #[arg(long, default_value = "600000", value_parser = crate::cli::positive)]
    pub target_ttl_ms: usize,
}

pub fn execute(command: Command) -> Result<()> {
    let options = match command {
        Command::Install(options) => return install::execute(options),
        Command::Serve(options) => options,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(Error::from)?;
    let result = runtime.block_on(async move {
        let shutdown = tokio_util::sync::CancellationToken::new();
        let transport = framing::StdioTransport::new(
            options.max_frame_bytes,
            options.max_response_bytes,
            options.max_active_requests,
            shutdown.clone(),
        );
        let handler = service::AnalysisService::new(&options, shutdown.clone())?;
        let admission = handler.admission.clone();
        let slots = handler.worker_slots;
        let reaper = handler.spawn_reaper(std::time::Duration::from_millis(
            options.target_ttl_ms.clamp(1, 1000) as u64,
        ));
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
        let _ = reaper.await;
        let _drain = admission.acquire_many(slots as u32).await;
        result
    });
    // Tokio stdio uses blocking system IO. A client may keep an unread stdout pipe open after stdin EOF.
    // Analysis tasks and workers were drained above; process exit releases remaining blocked IO threads.
    runtime.shutdown_timeout(std::time::Duration::from_millis(100));
    result
}
