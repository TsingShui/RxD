use clap::{Parser, Subcommand};
#[cfg(any(
    feature = "backend-js",
    feature = "backend-asar",
    feature = "backend-html",
    feature = "backend-jvm",
    feature = "backend-dotnet",
    feature = "backend-evm",
    feature = "backend-wasm",
    feature = "backend-pyc"
))]
use rxd_core::Error;
use rxd_core::Result;
#[cfg(any(
    feature = "backend-js",
    feature = "backend-asar",
    feature = "backend-html",
    feature = "backend-dotnet",
    feature = "backend-evm",
    feature = "backend-wasm",
    feature = "backend-pyc"
))]
use rxd_core::perf::Perf;
#[cfg(any(
    feature = "backend-js",
    feature = "backend-asar",
    feature = "backend-html",
    feature = "backend-jvm",
    feature = "backend-dotnet",
    feature = "backend-evm",
    feature = "backend-wasm",
    feature = "backend-pyc"
))]
use std::{
    io::{self, Write},
    path::Path,
};
#[cfg(feature = "backend-asar")]
mod asar;
#[cfg(feature = "backend-dex")]
mod dex;
#[cfg(feature = "backend-dotnet")]
mod dotnet;
#[cfg(feature = "backend-evm")]
mod evm;
/// `--case`, the one flag that governs case folding for every filter.
mod filter;
#[cfg(feature = "backend-html")]
mod html;
#[cfg(feature = "backend-js")]
mod js;
#[cfg(feature = "backend-jvm")]
mod jvm;
#[cfg(feature = "backend-kuna")]
pub(crate) mod kuna;
#[cfg(feature = "backend-pyc")]
mod pyc;
mod skill;
#[cfg(feature = "backend-wasm")]
mod wasm;
/// The `xref` target surface shared by every backend that answers "who references this?".
#[cfg(any(
    feature = "backend-dex",
    feature = "backend-jvm",
    feature = "backend-dotnet"
))]
mod xref;

#[derive(Parser)]
#[command(
    version,
    about = "Analyze JavaScript, JVM, .NET IL, APK/DEX, EVM, WASM, PYC, ASAR and HTML files without executing input code"
)]
pub struct Cli {
    #[arg(long, hide = true, global = true)]
    pub internal_worker: bool,
    #[command(subcommand)]
    backend: Backend,
}
#[derive(Subcommand)]
enum Backend {
    /// Install the embedded RxD skill for coding agents.
    Skill {
        #[command(subcommand)]
        command: skill::Command,
    },
    #[cfg(feature = "mcp")]
    /// Register or serve typed analysis tools over local MCP stdio.
    Mcp {
        #[command(subcommand)]
        command: crate::mcp::Command,
    },
    #[cfg(feature = "backend-kuna")]
    /// Run the bundled Kuna CLI with native arguments.
    #[command(disable_help_flag = true)]
    Kuna {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
    },
    #[cfg(feature = "backend-dotnet")]
    /// Analyze managed .NET assemblies without executing them.
    Dotnet {
        #[command(subcommand)]
        command: dotnet::Command,
    },
    #[cfg(feature = "backend-evm")]
    /// Analyze local EVM bytecode through the native EVMole library.
    Evm {
        #[command(subcommand)]
        command: evm::Command,
    },
    #[cfg(feature = "backend-js")]
    /// Analyze JavaScript files without executing them.
    Js {
        #[command(subcommand)]
        command: js::Command,
    },
    #[cfg(feature = "backend-html")]
    /// Enumerate lexical_naive_v1 HTML structure (not DOM/browser-complete).
    Html {
        #[command(subcommand)]
        command: html::Command,
    },
    #[cfg(feature = "backend-asar")]
    /// List and extract ASAR files without executing their contents.
    Asar {
        #[command(subcommand)]
        command: asar::Command,
    },
    #[cfg(feature = "backend-dex")]
    /// Analyze APK archives and standalone DEX files.
    #[command(visible_alias = "apk")]
    Dex {
        #[command(subcommand)]
        command: dex::Command,
    },
    #[cfg(feature = "backend-jvm")]
    /// Analyze JVM class files and JAR archives without Java or D8.
    #[command(name = "jar")]
    Jvm {
        #[command(subcommand)]
        command: jvm::Command,
    },
    #[cfg(feature = "backend-pyc")]
    /// Decompile Python bytecode without loading a Python interpreter.
    Pyc {
        #[command(subcommand)]
        command: pyc::Command,
    },
    #[cfg(feature = "backend-wasm")]
    /// Analyze WebAssembly modules without instantiating or executing them.
    Wasm {
        #[command(subcommand)]
        command: wasm::Command,
    },
}
impl Cli {
    pub fn bypasses_worker(&self) -> bool {
        match self.backend {
            Backend::Skill { .. } => true,
            #[cfg(feature = "backend-dex")]
            Backend::Dex { .. } => true,
            #[cfg(feature = "backend-kuna")]
            Backend::Kuna { .. } => true,
            #[cfg(feature = "mcp")]
            Backend::Mcp { .. } => true,
            #[cfg(any(
                feature = "backend-js",
                feature = "backend-asar",
                feature = "backend-html",
                feature = "backend-jvm",
                feature = "backend-dotnet",
                feature = "backend-evm",
                feature = "backend-wasm",
                feature = "backend-pyc"
            ))]
            _ => false,
        }
    }
    pub fn perf(&self) -> bool {
        match &self.backend {
            #[cfg(feature = "mcp")]
            Backend::Mcp { .. } => false,
            Backend::Skill { .. } => false,
            #[cfg(feature = "backend-kuna")]
            Backend::Kuna { .. } => false,
            #[cfg(feature = "backend-dotnet")]
            Backend::Dotnet { command } => command.perf(),
            #[cfg(feature = "backend-js")]
            Backend::Js { command } => command.perf(),
            #[cfg(feature = "backend-asar")]
            Backend::Asar { command } => command.perf(),
            #[cfg(feature = "backend-html")]
            Backend::Html { command } => command.perf(),
            #[cfg(feature = "backend-dex")]
            Backend::Dex { .. } => false,
            #[cfg(feature = "backend-jvm")]
            Backend::Jvm { .. } => false,
            #[cfg(feature = "backend-wasm")]
            Backend::Wasm { command } => command.perf(),
            #[cfg(feature = "backend-evm")]
            Backend::Evm { command } => command.perf(),
            #[cfg(feature = "backend-pyc")]
            Backend::Pyc { command } => command.perf(),
        }
    }
    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            #[cfg(feature = "mcp")]
            Backend::Mcp { .. } => "mcp",
            Backend::Skill { .. } => "skill",
            #[cfg(feature = "backend-kuna")]
            Backend::Kuna { .. } => "kuna",
            #[cfg(feature = "backend-dotnet")]
            Backend::Dotnet { .. } => "dotnet",
            #[cfg(feature = "backend-js")]
            Backend::Js { .. } => "js",
            #[cfg(feature = "backend-asar")]
            Backend::Asar { .. } => "asar",
            #[cfg(feature = "backend-html")]
            Backend::Html { .. } => "html",
            #[cfg(feature = "backend-dex")]
            Backend::Dex { .. } => "dex",
            #[cfg(feature = "backend-jvm")]
            Backend::Jvm { .. } => "jvm",
            #[cfg(feature = "backend-wasm")]
            Backend::Wasm { .. } => "wasm",
            #[cfg(feature = "backend-evm")]
            Backend::Evm { .. } => "evm",
            #[cfg(feature = "backend-pyc")]
            Backend::Pyc { .. } => "pyc",
        }
    }
    pub fn command_name(&self) -> &'static str {
        match &self.backend {
            #[cfg(feature = "mcp")]
            Backend::Mcp { .. } => "mcp",
            Backend::Skill { .. } => "skill",
            #[cfg(feature = "backend-kuna")]
            Backend::Kuna { .. } => "kuna",
            #[cfg(feature = "backend-dotnet")]
            Backend::Dotnet { command } => command.name(),
            #[cfg(feature = "backend-js")]
            Backend::Js { command } => command.name(),
            #[cfg(feature = "backend-asar")]
            Backend::Asar { command } => command.name(),
            #[cfg(feature = "backend-html")]
            Backend::Html { command } => command.name(),
            #[cfg(feature = "backend-dex")]
            Backend::Dex { command } => command.name(),
            #[cfg(feature = "backend-jvm")]
            Backend::Jvm { command } => command.name(),
            #[cfg(feature = "backend-wasm")]
            Backend::Wasm { command } => command.name(),
            #[cfg(feature = "backend-evm")]
            Backend::Evm { command } => command.name(),
            #[cfg(feature = "backend-pyc")]
            Backend::Pyc { command } => command.name(),
        }
    }
}
/// Turn one backend failure into one coded error.
///
/// This is the only place that decides a code it was not handed: an error that already
/// carries one keeps it, a device failure is `IO_ERROR`, and anything else is what a
/// parser produces when the bytes do not make sense - `INVALID_INPUT`. Backends that know
/// better say so with `rxd_core::Error::failure`, so no code is ever guessed from prose.
pub(crate) fn classify(error: anyhow::Error) -> rxd_core::Error {
    use rxd_core::{Error, Failure};
    match error.downcast::<Error>() {
        Ok(error) => error,
        Err(error) => match error.downcast::<std::io::Error>() {
            Ok(error) => Error::failure(Failure::Io, error.to_string()),
            Err(error) => {
                let message = format!("{error:#}");
                match rxd_core::leading_code(&message) {
                    // A backend that spells its code into a context is still reporting a
                    // class, and this is the one place that reads it. Repeated prefixes
                    // collapse, so the printed diagnostic carries the code once.
                    (Some(kind), rest) => Error::coded(kind.code(), rest),
                    (None, _) => Error::failure(Failure::Input, message),
                }
            }
        },
    }
}

/// Restore the default `SIGPIPE` disposition so a closed consumer (`| head`,
/// `| grep -m1`) ends this process quietly instead of turning a normal Unix idiom
/// into `IO_ERROR: Broken pipe`.
#[cfg(unix)]
pub fn restore_default_sigpipe() {
    // SAFETY: setting a signal disposition to SIG_DFL is async-signal-safe and
    // affects only this process.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
pub fn restore_default_sigpipe() {}

pub fn execute(cli: Cli) -> Result<i32> {
    match cli.backend {
        #[cfg(feature = "mcp")]
        Backend::Mcp { command } => crate::mcp::execute(command).map(|()| 0),
        Backend::Skill { command } => skill::execute(command).map(|()| 0),
        #[cfg(feature = "backend-kuna")]
        Backend::Kuna { args } => Ok(kuna::run(args)),
        #[cfg(feature = "backend-dotnet")]
        Backend::Dotnet { command } => dotnet::execute(command).map(|()| 0),
        #[cfg(feature = "backend-js")]
        Backend::Js { command } => js::execute(command).map(|()| 0),
        #[cfg(feature = "backend-asar")]
        Backend::Asar { command } => asar::execute(command).map(|()| 0),
        #[cfg(feature = "backend-html")]
        Backend::Html { command } => html::execute(command).map(|()| 0),
        #[cfg(feature = "backend-dex")]
        Backend::Dex { command } => dex::execute(command),
        #[cfg(feature = "backend-jvm")]
        Backend::Jvm { command } => jvm::execute(command).map(|()| 0),
        #[cfg(feature = "backend-wasm")]
        Backend::Wasm { command } => wasm::execute(command).map(|()| 0),
        #[cfg(feature = "backend-evm")]
        Backend::Evm { command } => evm::execute(command).map(|()| 0),
        #[cfg(feature = "backend-pyc")]
        Backend::Pyc { command } => pyc::execute(command).map(|()| 0),
    }
}
#[cfg(feature = "mcp")]
pub(crate) fn execute_query(cli: Cli, input_limit: usize) -> Result<i32> {
    #[cfg(not(feature = "backend-dex"))]
    let _ = input_limit;
    match cli.backend {
        #[cfg(feature = "backend-dex")]
        Backend::Dex { command } => dex::execute_query(command, input_limit),
        _ => execute(cli),
    }
}

/// Default worker count for a parallel scan phase.
///
/// One worker per logical CPU. Each phase here holds one parsed unit resident per worker
/// (one inflated DEX entry, or one parsed JavaScript input with its AST), so this value
/// decides both how fast the phase runs and how much memory it holds at once. Measured on a
/// 6P+6E machine: DEX `findrefs type View` over a 359 MiB archive scans in 955 / 152 / 129
/// ms at 1 / 8 / 12 threads, with 16 threads slower again (145 ms); JavaScript directory
/// scans follow the same curve over whole files. A hardcoded count cannot know any of that;
/// the machine can.
#[cfg(any(feature = "backend-js", feature = "backend-dex"))]
pub(crate) fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(8, std::num::NonZero::get)
}

#[cfg(any(
    feature = "mcp",
    feature = "backend-js",
    feature = "backend-asar",
    feature = "backend-html",
    feature = "backend-jvm",
    feature = "backend-dotnet",
    feature = "backend-evm",
    feature = "backend-pyc"
))]
pub(crate) fn positive(value: &str) -> std::result::Result<usize, String> {
    let n = value
        .parse::<usize>()
        .map_err(|_| "expected a positive integer".to_owned())?;
    if n == 0 || n > u32::MAX as usize {
        Err("expected an integer between 1 and 4294967295".into())
    } else {
        Ok(n)
    }
}

#[cfg(any(
    feature = "backend-js",
    feature = "backend-asar",
    feature = "backend-html",
    feature = "backend-dotnet",
    feature = "backend-evm",
    feature = "backend-wasm",
    feature = "backend-pyc"
))]
fn finish(mut perf: Perf, emit_perf: bool, result: Result<()>) -> Result<()> {
    perf.finish(result.as_ref().err().map(|error| error.code));
    if emit_perf {
        perf.emit();
    }
    result
}

#[cfg(any(
    feature = "backend-js",
    feature = "backend-asar",
    feature = "backend-html",
    feature = "backend-jvm",
    feature = "backend-dotnet",
    feature = "backend-evm",
    feature = "backend-wasm",
    feature = "backend-pyc"
))]
fn write_output(bytes: &[u8], destination: Option<&Path>) -> Result<()> {
    #[cfg(feature = "mcp")]
    if destination.is_none()
        && let Some(result) = crate::query::capture_write(bytes)
    {
        return result;
    }
    if let Some(path) = destination {
        let directory = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(bytes)?;
        temporary.flush()?;
        temporary.persist(path).map_err(|e| Error::from(e.error))?;
    } else {
        io::stdout().lock().write_all(bytes)?;
    }
    Ok(())
}
