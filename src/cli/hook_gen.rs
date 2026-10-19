//! Thin argument/output adapter; providers own all analysis and rendering.
use std::{io::Write, path::PathBuf};

use clap::{Args, Subcommand};
use rxd_hook_gen::{
    Artifact, CapturedInput, Diagnostic, ErrorKind, Language, Limits, Request, Stage, dex,
    generate, native, publish_extension, web,
};

#[derive(Subcommand)]
pub enum Command {
    /// Generate Frida source without attaching or executing it.
    Frida {
        #[command(subcommand)]
        command: Frida,
    },
    /// Generate browser page-realm JavaScript or a local MV3 extension.
    Web(Web),
}

#[derive(Subcommand)]
pub enum Frida {
    /// Select a declared APK/DEX method.
    Dex(Dex),
    /// Select a native function entry (static VMA, not a file offset).
    Native(Native),
}

#[derive(Args)]
pub struct Dex {
    input: PathBuf,
    #[arg(required_unless_present = "class", conflicts_with_all = ["class", "method_index"])]
    method: Option<String>,
    #[arg(long, requires = "method_index")]
    class: Option<String>,
    /// DEX method_ids index, not a class-local slot.
    #[arg(long, requires = "class")]
    method_index: Option<u32>,
    #[arg(long)]
    entry: Option<String>,
    #[arg(long, default_value = "js", value_parser = ["js", "ts"])]
    lang: String,
    /// Emit one runtime Java bridge import (ESM source, not a bundle).
    #[arg(long = "import")]
    import_java_bridge: bool,
}

#[derive(Args)]
pub struct Native {
    input: PathBuf,
    /// Function name or 0xHEX static virtual address (never a file offset).
    target: String,
    #[arg(long, default_value = "js", value_parser = ["js", "ts"])]
    lang: String,
    /// Runtime module name; otherwise derive it from the input.
    #[arg(long, conflicts_with = "main_module")]
    module: Option<String>,
    #[arg(long)]
    main_module: bool,
}

#[derive(Args)]
pub struct Web {
    #[arg(long)]
    target: String,
    #[arg(long, default_value = "snippet", value_parser = ["snippet", "extension"])]
    emit: String,
    #[arg(long = "match")]
    matches: Vec<String>,
    /// Bounded wait in milliseconds in the generated source, not during generation.
    #[arg(long)]
    wait: Option<u32>,
    #[arg(long)]
    out: Option<PathBuf>,
}

fn invalid(message: &str) -> Diagnostic {
    Diagnostic::new(ErrorKind::InvalidArguments, Stage::Input, message)
}

fn io_error(error: std::io::Error) -> Diagnostic {
    Diagnostic::new(ErrorKind::Io, Stage::Publication, error.to_string())
}

/// The same adapter is used by the product and independently built module driver.
/// Resource preparation is explicit and lazy; it must not mutate process globals.
pub fn run(
    command: Command,
    mut prepare_specs: impl FnMut() -> rxd_hook_gen::Result<Option<PathBuf>>,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> rxd_hook_gen::Result<()> {
    let limits = Limits::default();
    let mut destination = None;
    let mut request = match command {
        Command::Web(args) => {
            let emit: web::Emit = args.emit.parse()?;
            match emit {
                web::Emit::Extension if args.out.is_none() => {
                    return Err(invalid("--emit extension requires --out DIR"));
                }
                web::Emit::Snippet if args.out.is_some() => {
                    return Err(invalid("--out is only supported with --emit extension"));
                }
                _ => {}
            }
            destination = args.out;
            Request::Web(web::Request {
                target: args.target,
                emit,
                matches: args.matches,
                wait_ms: args.wait,
            })
        }
        Command::Frida {
            command: Frida::Dex(args),
        } => {
            let selector = match (args.method, args.class, args.method_index) {
                (Some(method), None, None) => dex::Selector::Signature(method),
                (None, Some(class), Some(method_index)) => dex::Selector::MethodIndex {
                    class,
                    method_index,
                },
                _ => return Err(invalid("provide METHOD or --class CLASS --method-index N")),
            };
            Request::Dex(dex::Request {
                input: CapturedInput::read(&args.input, limits)?,
                selector,
                options: dex::Options {
                    language: args.lang.parse()?,
                    import_java_bridge: args.import_java_bridge,
                    entry: args.entry,
                },
            })
        }
        Command::Frida {
            command: Frida::Native(args),
        } => {
            let target = if let Some(hex) = args.target.strip_prefix("0x") {
                if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(invalid("native address must be 0xHEX"));
                }
                native::Target::StaticAddress(
                    u64::from_str_radix(hex, 16)
                        .map_err(|_| invalid("native static address exceeds u64"))?,
                )
            } else if !args.target.is_empty() {
                native::Target::Name(args.target)
            } else {
                return Err(invalid("native target must be a function name or 0xHEX"));
            };
            let language = args.lang.parse()?;
            Request::Native(native::Request {
                input: CapturedInput::read(&args.input, limits)?,
                target,
                options: native::Options {
                    language,
                    module: match (args.module, args.main_module) {
                        (Some(name), false) => native::RuntimeModule::Named(name),
                        (None, true) => native::RuntimeModule::Main,
                        (None, false) => native::RuntimeModule::Auto,
                        _ => return Err(invalid("--module conflicts with --main-module")),
                    },
                    specs_dir: if language == Language::TypeScript {
                        prepare_specs()?
                    } else {
                        None
                    },
                },
            })
        }
    };
    let artifact = match generate(&request, limits) {
        Err(error) if error.kind == ErrorKind::EngineUnavailable => {
            if let Request::Native(native) = &mut request {
                if native.options.specs_dir.is_some() {
                    return Err(error);
                }
                native.options.specs_dir = prepare_specs()?;
                if native.options.specs_dir.is_none() {
                    return Err(error);
                }
                generate(&request, limits)?
            } else {
                return Err(error);
            }
        }
        result => result?,
    };
    match artifact {
        Artifact::Source(source) => stdout.write_all(source.source.as_bytes()).map_err(io_error),
        Artifact::Extension(files) => {
            let receipt = publish_extension(
                &files,
                &destination.ok_or_else(|| invalid("extension requires --out DIR"))?,
                limits,
            )?;
            writeln!(
                stderr,
                "hook-gen published {} bytes to {:?}",
                receipt.bytes, receipt.directory
            )
            .map_err(io_error)
        }
    }
}
