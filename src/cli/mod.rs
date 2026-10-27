use clap::{Parser, Subcommand};
mod output;
pub(crate) use output::Output;
#[cfg(any(
    feature = "unpack",
    feature = "backend-jvm",
    feature = "backend-dotnet",
    feature = "backend-evm",
    feature = "backend-wasm",
    feature = "backend-pyc",
    feature = "backend-lua",
    feature = "backend-flutter",
    feature = "backend-dex"
))]
use rxd_core::Error;
use rxd_core::Result;
#[cfg(any(
    feature = "unpack",
    feature = "backend-dotnet",
    feature = "backend-evm",
    feature = "backend-wasm",
    feature = "backend-pyc",
    feature = "backend-lua",
    feature = "backend-flutter"
))]
use rxd_core::perf::Perf;
#[cfg(any(
    feature = "unpack",
    feature = "backend-jvm",
    feature = "backend-dotnet",
    feature = "backend-evm",
    feature = "backend-wasm",
    feature = "backend-pyc",
    feature = "backend-lua",
    feature = "backend-flutter",
    feature = "backend-dex"
))]
use std::{io::Write, path::Path};
#[cfg(feature = "backend-dex")]
mod dex;
#[cfg(feature = "backend-dotnet")]
mod dotnet;
#[cfg(feature = "backend-evm")]
mod evm;
/// `--case`, the one flag that governs case folding for every filter.
mod filter;
#[cfg(feature = "backend-flutter")]
mod flutter;
#[cfg(feature = "backend-jvm")]
mod jvm;
#[cfg(feature = "backend-kuna")]
pub(crate) mod kuna;
#[cfg(feature = "backend-lua")]
mod lua;
#[cfg(feature = "backend-pyc")]
mod pyc;
mod skill;
mod table;
#[cfg(feature = "unpack")]
mod unpack;
#[cfg(feature = "backend-wasm")]
mod wasm;
/// Identity selectors are shared by FETCH as well as reference queries.
#[cfg(any(
    feature = "backend-dex",
    feature = "backend-jvm",
    feature = "backend-dotnet",
    feature = "backend-evm",
    feature = "backend-wasm",
    feature = "backend-pyc",
    feature = "backend-lua",
    feature = "backend-flutter"
))]
mod xref;

#[derive(Parser)]
#[command(
    version,
    about = "Analyze APK/DEX, Class/JAR, DLL/EXE, WASM, PYC, Lua, Dart/Flutter, EVM bytecode and ELF/PE/Mach-O; unpack ASAR/ZIP/PyInstaller/wxapkg archives",
    after_long_help = "Coordinates: at and context_at chain the next fetch (kuna selects by address_hex).
Output: one record per line; exit 0 required; zero rows means no matches.
Filters: -f narrows listings; -m with --kind searches xref targets.
Listings: TAB-separated columns per command help; text JSON-quoted; \\N absent.
Raw: --raw selects original instructions where reconstruction exists.
Details: rxd BACKEND COMMAND --help; typed tools through rxd mcp.
Engines: source builds may omit Kuna/ILSpy."
)]
pub struct Cli {
    #[arg(long, hide = true, global = true)]
    pub internal_worker: bool,
    #[command(subcommand)]
    backend: Backend,
}
#[derive(Subcommand)]
enum Backend {
    /// Print Agent skill.
    #[command(
        display_order = 100,
        long_about = "Print Agent skill: embedded query workflows, coordinate and exit-code contracts"
    )]
    Skill,
    #[cfg(feature = "mcp")]
    /// Serve MCP stdio.
    #[command(
        display_order = 90,
        long_about = "Serve MCP stdio: typed analysis tools, immutable targets and disposable workers"
    )]
    Mcp(crate::mcp::Options),
    #[cfg(feature = "backend-kuna")]
    /// Decompile ELF/PE/Mach-O.
    #[command(
        disable_help_flag = true,
        visible_alias = "bin",
        display_order = 70,
        long_about = "Decompile ELF/PE/Mach-O: native functions and C through the optional linked engine"
    )]
    Kuna {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
    },
    #[cfg(feature = "backend-dotnet")]
    /// Analyze DLL/EXE.
    #[command(
        name = "ilspy",
        after_long_help = "Examples:
  rxd ilspy methods app.dll -f Read
  rxd ilspy get-method app.dll --at '0x0600000b' --raw
  rxd ilspy strings app.dll -f Authorization
  rxd ilspy xref app.dll -m Authorization --kind string
Copy methods.at into get-method; copy xref.context_at to fetch its caller.
Tokens such as 0x0600000b are module-scoped; use actual returned values.
C# requires ILSpy; missing/failed reconstruction returns original IL with a reason."
    )]
    #[command(display_order = 30, long_about = "Analyze DLL/EXE: metadata, IL, C#")]
    Dotnet {
        #[command(subcommand)]
        command: dotnet::Command,
    },
    #[cfg(feature = "backend-evm")]
    /// Analyze EVM bytecode.
    #[command(after_long_help = "Examples:
  rxd evmole interface runtime.hex --encoding hex
  rxd evmole disassemble runtime.hex --encoding hex
  rxd evmole strings runtime.hex --encoding hex --kind text --query Authorization
  rxd evmole metadata runtime.hex --encoding hex
--encoding raw means binary bytes; hex means ASCII hexadecimal. Never guessed.
interface selectors/types/mutability are inferred candidates, not a verified ABI.")]
    #[command(
        display_order = 60,
        long_about = "Analyze EVM bytecode: interface, disassembly, literals, compiler metadata"
    )]
    Evmole {
        #[command(subcommand)]
        command: evm::Command,
    },
    #[cfg(feature = "unpack")]
    /// Unpack ASAR/ZIP/PyInstaller/wxapkg.
    #[command(after_long_help = "Examples:
  rxd unpack app.asar -o app
  rxd unpack archive.zip -o archive
  rxd unpack app.bin -o app
  rxd unpack app_onedir -o app
  rxd unpack app.wxapkg -o app
  rxd unpack app.enc.wxapkg --appid wx1234567890abcdef -o app
Output: extracted files under the new directory; one JSON receipt on stdout.
Format is detected from content. The output directory must not exist.
ZIP entries must be regular, unencrypted Stored/Deflate files.
PyInstaller one-file executables yield CArchive files and a PYZ.pyz_extracted tree of .pyc files.
For a PyInstaller one-dir root, its launcher is found and only its CArchive/PYZ is
unpacked; loose _internal files remain in the input directory.
wxapkg extracts the indexed files; V1MMWX PC/Mac packages require --appid or an
appid path component.
No entry selection, nested extraction beyond PYZ, links, or encrypted ASAR/ZIP/PyInstaller archives.")]
    #[command(
        display_order = 80,
        long_about = "Unpack ASAR/ZIP/PyInstaller/wxapkg: every entry to a new directory"
    )]
    Unpack {
        #[command(flatten)]
        command: unpack::Command,
    },
    #[cfg(feature = "backend-dex")]
    /// Analyze APK/DEX.
    #[command(
        name = "rasc",
        after_long_help = "Examples:
  rxd rasc methods app.apk -f onCreate
  rxd rasc get-method app.apk --at 'Lx/y;->onCreate(Landroid/os/Bundle;)V'
  rxd rasc strings app.apk -f Authorization
  rxd rasc xref app.apk -m Authorization --kind string
  rxd rasc manifest app.apk
Copy methods.at into get-method; copy xref.context_at to fetch its caller.
Method coordinates include parameters AND return type after ). Use actual returned values.
classes.at selects get-class; -f filters names, --class is one exact owner.
Input is one APK/ZIP with root DEX entries, or one bare DEX. Java may contain unsupported stubs."
    )]
    #[command(
        display_order = 10,
        long_about = "Analyze APK/DEX: classes, strings, references, Java"
    )]
    Dex {
        #[command(subcommand)]
        command: dex::Command,
    },
    #[cfg(feature = "backend-jvm")]
    /// Analyze Class/JAR.
    #[command(
        name = "jar",
        after_long_help = "Examples:
  rxd jar methods library.jar -f parse
  rxd jar get-method library.jar --at 'Lx/y;->parse(Ljava/lang/String;)I' --raw
  rxd jar strings library.jar -f Authorization
  rxd jar xref library.jar -m Authorization --kind string
Copy methods.at or xref.context_at into get-method; classes.at selects get-class.
Coordinates retain JVM overload descriptors, including return type; use returned values.
--release N selects multi-release JAR versions. No Java runtime or D8 conversion needed."
    )]
    #[command(
        display_order = 20,
        long_about = "Analyze Class/JAR: members, references, Java/bytecode"
    )]
    Jvm {
        #[command(subcommand)]
        command: jvm::Command,
    },
    #[cfg(feature = "backend-pyc")]
    /// Analyze PYC.
    #[command(after_long_help = "Examples:
  rxd pyc codes module.pyc -f token
  rxd pyc strings module.pyc -f Authorization
  rxd pyc xref module.pyc --at '<string.at>'
  rxd pyc get-code module.pyc --at '<reference.context_at>'
  rxd pyc decompile module.pyc --json
Static navigation supports final CPython 3.8-3.13; raw marshal needs --version X.Y.
code:N selects get-code; const:N:PATH selects constant xref, not get-code.
Name xref is explicit bytecode evidence, not resolved Python runtime dispatch.
Decompile reconstructs the whole module; JSON detail.complete reports recovery status.")]
    #[command(
        display_order = 50,
        long_about = "Analyze PYC: codes, constants, Python reconstruction"
    )]
    Pyc {
        #[command(subcommand)]
        command: pyc::Command,
    },
    #[cfg(feature = "backend-flutter")]
    /// Analyze Dart/Flutter AOT.
    #[command(after_long_help = "Examples:
  rxd dae info libapp.so
  rxd dae functions libapp.so -f fl_clash
  rxd dae strings libapp.so -f Authorization
  rxd dae xref libapp.so --at 'str:0x6590'
  rxd dae decompile libapp.so --lib fl_clash
Input is an ELF ARM64 libapp.so from a Flutter Android release build; there is no encoding flag.
The Dart SDK profile comes from the snapshot fingerprint and may be unverified.
functions.at is a code entry offset; strings.at is a pool offset that xref consumes.
xref decodes every function to find pool loads, so it takes seconds.
decompile renders machine-level pseudocode per library.")]
    #[command(
        display_order = 56,
        long_about = "Analyze Dart/Flutter AOT: info, functions, pool strings, references, pseudocode"
    )]
    Dae {
        #[command(subcommand)]
        command: flutter::Command,
    },
    #[cfg(feature = "backend-lua")]
    /// Analyze Lua bytecode.
    #[command(after_long_help = "Examples:
  rxd unluac-rs codes module.luac
  rxd unluac-rs strings module.luac -f token
  rxd unluac-rs xref module.luac --at 'proto:1:const:0'
  rxd unluac-rs instructions module.luac --at 'proto:1'
  rxd unluac-rs decompile module.luac
Dialect (PUC Lua 5.1-5.5, LuaJIT 2.1, Luau) comes from the chunk header, never a flag.
codes.at is proto:N; strings.at is proto:N:const:I and selects xref, not a fetch.
Decompile reconstructs the whole chunk; listings never run the decompiler.")]
    #[command(
        display_order = 55,
        long_about = "Analyze Lua bytecode: codes, constants, references, source reconstruction"
    )]
    UnluacRs {
        #[command(subcommand)]
        command: lua::Command,
    },
    #[cfg(feature = "backend-wasm")]
    /// Analyze WASM.
    #[command(after_long_help = "Examples:
  rxd wasm functions module.wasm -f sum
  rxd wasm xref module.wasm --at '<function.at>'
  rxd wasm get-function module.wasm --at '<reference.context_at>' --raw
Copy functions.at (func:N) into xref/get-function; xref.context_at names the caller.
exports.detail.target_at also names the exported function; imported functions have no body.
Input is a binary core module, not WAT or Component Model.
Default FETCH attempts C-like reconstruction and falls back to original instructions.")]
    #[command(
        display_order = 40,
        long_about = "Analyze WASM: functions, instructions, C-like reconstruction"
    )]
    Wasm {
        #[command(subcommand)]
        command: wasm::Command,
    },
}
impl Cli {
    pub fn bypasses_worker(&self) -> bool {
        match self.backend {
            Backend::Skill => true,
            #[cfg(feature = "backend-dex")]
            Backend::Dex { .. } => true,
            #[cfg(feature = "backend-kuna")]
            Backend::Kuna { .. } => true,
            #[cfg(feature = "mcp")]
            Backend::Mcp(_) => true,
            #[cfg(feature = "unpack")]
            Backend::Unpack { .. } => true,
            #[cfg(any(
                feature = "backend-jvm",
                feature = "backend-dotnet",
                feature = "backend-evm",
                feature = "backend-wasm",
                feature = "backend-pyc",
                feature = "backend-lua",
                feature = "backend-flutter"
            ))]
            _ => false,
        }
    }
    pub fn perf(&self) -> bool {
        match &self.backend {
            #[cfg(feature = "mcp")]
            Backend::Mcp(_) => false,
            Backend::Skill => false,
            #[cfg(feature = "backend-kuna")]
            Backend::Kuna { .. } => false,
            #[cfg(feature = "backend-dotnet")]
            Backend::Dotnet { command } => command.perf(),
            #[cfg(feature = "unpack")]
            Backend::Unpack { command } => command.perf,
            #[cfg(feature = "backend-dex")]
            Backend::Dex { .. } => false,
            #[cfg(feature = "backend-jvm")]
            Backend::Jvm { .. } => false,
            #[cfg(feature = "backend-wasm")]
            Backend::Wasm { command } => command.perf(),
            #[cfg(feature = "backend-evm")]
            Backend::Evmole { command } => command.perf(),
            #[cfg(feature = "backend-pyc")]
            Backend::Pyc { command } => command.perf(),
            #[cfg(feature = "backend-lua")]
            Backend::UnluacRs { command } => command.perf(),
            #[cfg(feature = "backend-flutter")]
            Backend::Dae { command } => command.perf(),
        }
    }
    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            #[cfg(feature = "mcp")]
            Backend::Mcp(_) => "mcp",
            Backend::Skill => "skill",
            #[cfg(feature = "backend-kuna")]
            Backend::Kuna { .. } => "kuna",
            #[cfg(feature = "backend-dotnet")]
            Backend::Dotnet { .. } => "dotnet",
            #[cfg(feature = "unpack")]
            Backend::Unpack { .. } => "unpack",
            #[cfg(feature = "backend-dex")]
            Backend::Dex { .. } => "dex",
            #[cfg(feature = "backend-jvm")]
            Backend::Jvm { .. } => "jvm",
            #[cfg(feature = "backend-wasm")]
            Backend::Wasm { .. } => "wasm",
            #[cfg(feature = "backend-evm")]
            Backend::Evmole { .. } => "evm",
            #[cfg(feature = "backend-pyc")]
            Backend::Pyc { .. } => "pyc",
            #[cfg(feature = "backend-lua")]
            Backend::UnluacRs { .. } => "lua",
            #[cfg(feature = "backend-flutter")]
            Backend::Dae { .. } => "flutter",
        }
    }
    pub fn command_name(&self) -> &'static str {
        match &self.backend {
            #[cfg(feature = "mcp")]
            Backend::Mcp(_) => "mcp",
            Backend::Skill => "skill",
            #[cfg(feature = "backend-kuna")]
            Backend::Kuna { .. } => "kuna",
            #[cfg(feature = "backend-dotnet")]
            Backend::Dotnet { command } => command.name(),
            #[cfg(feature = "unpack")]
            Backend::Unpack { .. } => "unpack",
            #[cfg(feature = "backend-dex")]
            Backend::Dex { command } => command.name(),
            #[cfg(feature = "backend-jvm")]
            Backend::Jvm { command } => command.name(),
            #[cfg(feature = "backend-wasm")]
            Backend::Wasm { command } => command.name(),
            #[cfg(feature = "backend-evm")]
            Backend::Evmole { command } => command.name(),
            #[cfg(feature = "backend-pyc")]
            Backend::Pyc { command } => command.name(),
            #[cfg(feature = "backend-lua")]
            Backend::UnluacRs { command } => command.name(),
            #[cfg(feature = "backend-flutter")]
            Backend::Dae { command } => command.name(),
        }
    }
}
/// Preserve typed diagnostic codes; classify IO separately and default parse errors to input.
pub(crate) fn classify(error: anyhow::Error) -> rxd_core::Error {
    use rxd_core::{Error, Failure};
    match error.downcast::<Error>() {
        Ok(error) => error,
        Err(error) => match error.downcast::<std::io::Error>() {
            Ok(error) => Error::failure(Failure::Io, error.to_string()),
            Err(error) => {
                let message = format!("{error:#}");
                match rxd_core::leading_code(&message) {
                    (Some(kind), rest) => Error::coded(kind.code(), rest),
                    (None, _) => Error::failure(Failure::Input, message),
                }
            }
        },
    }
}

/// Restore SIGPIPE so closed pipeline consumers exit quietly.
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

pub fn execute(cli: Cli, output: &mut Output<'_>) -> Result<i32> {
    let _ = &output;
    let policy = rxd_core::BudgetPolicy::InputScaled;
    let _ = policy;
    match cli.backend {
        #[cfg(feature = "mcp")]
        Backend::Mcp(options) => crate::mcp::execute(options).map(|()| 0),
        Backend::Skill => skill::execute().map(|()| 0),
        #[cfg(feature = "backend-kuna")]
        Backend::Kuna { args } => Ok(kuna::run(args)),
        #[cfg(feature = "backend-dotnet")]
        Backend::Dotnet { command } => dotnet::execute(command, policy, output).map(|()| 0),
        #[cfg(feature = "unpack")]
        Backend::Unpack { command } => unpack::execute(command, policy, output).map(|()| 0),
        #[cfg(feature = "backend-dex")]
        Backend::Dex { command } => dex::execute(command, policy, output),
        #[cfg(feature = "backend-jvm")]
        Backend::Jvm { command } => jvm::execute(command, policy, output).map(|()| 0),
        #[cfg(feature = "backend-wasm")]
        Backend::Wasm { command } => wasm::execute(command, policy, output).map(|()| 0),
        #[cfg(feature = "backend-evm")]
        Backend::Evmole { command } => evm::execute(command, policy, output).map(|()| 0),
        #[cfg(feature = "backend-pyc")]
        Backend::Pyc { command } => pyc::execute(command, policy, output).map(|()| 0),
        #[cfg(feature = "backend-lua")]
        Backend::UnluacRs { command } => lua::execute(command, policy, output).map(|()| 0),
        #[cfg(feature = "backend-flutter")]
        Backend::Dae { command } => flutter::execute(command, policy, output).map(|()| 0),
    }
}
/// One analysis worker per available logical CPU.
#[cfg(feature = "backend-dex")]
pub(crate) fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(8, std::num::NonZero::get)
}

#[cfg(feature = "mcp")]
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
    feature = "unpack",
    feature = "backend-dotnet",
    feature = "backend-evm",
    feature = "backend-wasm",
    feature = "backend-pyc",
    feature = "backend-lua",
    feature = "backend-flutter"
))]
fn finish(mut perf: Perf, emit_perf: bool, result: Result<()>) -> Result<()> {
    perf.finish(result.as_ref().err().map(|error| error.code));
    if emit_perf {
        perf.emit();
    }
    result
}

#[cfg(any(
    feature = "unpack",
    feature = "backend-jvm",
    feature = "backend-dotnet",
    feature = "backend-evm",
    feature = "backend-wasm",
    feature = "backend-pyc",
    feature = "backend-lua",
    feature = "backend-flutter",
    feature = "backend-dex"
))]
fn write_output(bytes: &[u8], destination: Option<&Path>, output: &mut Output<'_>) -> Result<()> {
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
        output.write_all(bytes)?;
    }
    Ok(())
}
