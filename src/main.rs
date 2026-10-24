mod cli;
#[cfg(feature = "mcp")]
mod mcp;
#[cfg(feature = "mcp")]
mod query;

use std::{
    io::{self, Seek, SeekFrom, Write},
    process::{Command, ExitCode, Stdio},
    time::Instant,
};

use clap::Parser;
use rxd_core::perf::{Perf, ns};
use rxd_core::{Error, Result};

fn main() -> ExitCode {
    #[cfg(feature = "mcp")]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--internal-mcp-worker")
    {
        let limit = std::env::args()
            .nth(2)
            .and_then(|arg| arg.parse::<usize>().ok())
            .filter(|limit| *limit > 0 && *limit <= u32::MAX as usize)
            .unwrap_or(64 * 1024 * 1024);
        return mcp::worker::entry(limit);
    }
    #[cfg(feature = "backend-kuna")]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "kuna" || arg == "bin")
    {
        return ExitCode::from(cli::kuna::run(std::env::args_os().skip(2).collect()) as u8);
    }
    // Every CLI process, parent or isolated worker, writes to a possibly closed
    // consumer; make `| head` terminate quietly like a standard Unix filter.
    cli::restore_default_sigpipe();
    let process_start = Instant::now();
    let cli = cli::Cli::parse();
    let args_ns = ns(process_start.elapsed());
    if cli.bypasses_worker() {
        return match cli::execute(cli) {
            Ok(code) => ExitCode::from(code as u8),
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(error.exit_code())
            }
        };
    }
    if !cli.internal_worker {
        let emit_perf = cli.perf();
        let mut perf = Perf::new(emit_perf, "cli", cli.backend_name(), cli.command_name());
        perf.include_args(process_start, args_ns);
        let result = run_worker(&mut perf);
        let (code, error_code) = match result {
            Ok(0) => (0u32, None),
            Ok(code) => (code as u32, Some("WORKER_ERROR")),
            Err(error) => {
                eprintln!("{error}");
                (u32::from(error.exit_code()), Some(error.code))
            }
        };
        perf.finish(error_code);
        if emit_perf {
            perf.emit();
        }
        return ExitCode::from(code as u8);
    }
    let result = {
        // Isolate recursive parser failures in a worker process.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !info
                .payload()
                .downcast_ref::<Error>()
                .is_some_and(|e| e.code == "RESOURCE_LIMIT")
            {
                hook(info);
            }
        }));
        std::thread::Builder::new()
            .name("analysis".into())
            .stack_size(32 * 1024 * 1024)
            .spawn(move || cli::execute(cli))
            .map_err(Error::from)
            .and_then(|worker| {
                worker.join().unwrap_or_else(|_| {
                    Err(Error::new("INTERNAL_ERROR", "analysis worker panicked"))
                })
            })
    };
    if let Err(error) = result {
        eprintln!("{error}");
        // The status belongs to the failure class, in one place (ADR 0015).
        return ExitCode::from(error.exit_code());
    }
    ExitCode::SUCCESS
}

/// The isolated worker deliberately has no stdin; analysis input is never a selector
/// stream that some other command produced.
fn run_worker(perf: &mut Perf) -> Result<i32> {
    // Do not retain another whole result in the parent. Publish the private spool
    // only after successful completion, and discard it on worker failure.
    let mut payload = tempfile::tempfile()?;
    let result = perf.measure("spawn_wait", || {
        Command::new(std::env::current_exe()?)
            .arg("--internal-worker")
            .args(std::env::args_os().skip(1))
            .stdin(Stdio::null())
            .stdout(Stdio::from(payload.try_clone()?))
            .stderr(Stdio::piped())
            .spawn()?
            .wait_with_output()
    })?;
    if let Some(code) = result.status.code() {
        perf.counters.output_bytes =
            Some(usize::try_from(payload.metadata()?.len()).map_err(|_| {
                Error::failure(rxd_core::Failure::Resource, "worker output length overflow")
            })?);
        perf.measure("forward", || -> Result<()> {
            io::stderr().lock().write_all(&result.stderr)?;
            if code == 0 {
                payload.seek(SeekFrom::Start(0))?;
                io::copy(&mut payload, &mut io::stdout().lock())?;
            }
            Ok(())
        })?;
        Ok(code)
    } else {
        Err(Error::new(
            "RESOURCE_LIMIT",
            "analysis worker terminated unexpectedly; possible parser stack exhaustion",
        ))
    }
}
