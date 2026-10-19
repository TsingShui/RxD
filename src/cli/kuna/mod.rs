#[cfg(all(feature = "mcp", feature = "bundled-kuna"))]
pub(crate) fn run_mcp_captured(
    args: Vec<String>,
    limit: usize,
) -> rxd_core::Result<(i32, Vec<u8>)> {
    bundled::run_mcp_captured(args, limit)
        .map_err(|error| rxd_core::Error::new("KUNA_ERROR", error.to_string()))
}

use std::ffi::OsString;

pub fn run(args: Vec<OsString>) -> i32 {
    #[cfg(feature = "bundled-kuna")]
    {
        match bundled::run(args) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("KUNA_BUNDLE_ERROR: {error}");
                1
            }
        }
    }
    #[cfg(not(feature = "bundled-kuna"))]
    {
        let _ = args;
        eprintln!("KUNA_NOT_BUNDLED: build with python3 scripts/build-bundled.py");
        127
    }
}

#[cfg(feature = "bundled-kuna")]
mod bundled {
    use std::{
        env,
        ffi::OsString,
        io::{self, Write},
    };

    use rxd_core::bundle::{Manifest, Result, materialize};
    use serde_json::Value;

    mod embed {
        include!(env!("RXD_KUNA_EMBED_RS"));
    }

    #[cfg(feature = "mcp")]
    pub(super) fn run_mcp_captured(args: Vec<String>, limit: usize) -> Result<(i32, Vec<u8>)> {
        if !matches!(
            args.first().map(String::as_str),
            Some("functions" | "decompile")
        ) {
            return Err("unsupported native MCP command".into());
        }
        let manifest: Manifest = serde_json::from_str(embed::MANIFEST)?;
        let cache = materialize("rxd-kuna", embed::DATA, embed::TARGET, &manifest)?;
        // SAFETY: this entry runs only in a one-shot isolated MCP analysis
        // worker before native analysis threads start; its main thread joins it.
        unsafe {
            env::set_var("KUNA_SPECS", cache.join("specs"));
            env::set_var("SLEIGHHOME", cache.join("specs"));
            // Serial upstream dispatch leaves an inherited lane override intact.
            // The MCP worker owns exactly one native analysis allocation.
            env::set_var("KUNA_DECODE_JOBS", "1");
            for name in [
                "KUNA_ROOT",
                "KUNA_RUST_PROFILE",
                "KUNA_DECOMP_DBG",
                "KUNA_ENGINE",
            ] {
                env::remove_var(name);
            }
        }
        let mut argv = vec!["kuna".to_owned()];
        argv.extend(args);
        Ok(rxd_kuna_cli::run_captured_bounded(argv, limit)?)
    }

    fn emit(bytes: &[u8]) -> Result<i32> {
        match io::stdout().lock().write_all(bytes) {
            Ok(()) => Ok(0),
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(0),
            Err(error) => Err(error.into()),
        }
    }

    fn catalog(args: &[OsString]) -> Result<i32> {
        let mut option = None;
        let mut tier = None;
        let mut json = false;
        let mut check = false;
        let mut i = 1;
        while i < args.len() {
            let flag = args[i].to_str().ok_or("catalog arguments must be UTF-8")?;
            match flag {
                "--json" => json = true,
                "--check" => check = true,
                "--option" | "--tier" | "--sleighpath" | "--engine" => {
                    i += 1;
                    let Some(value) = args.get(i).and_then(|x| x.to_str()) else {
                        eprintln!("error: {flag} requires a value");
                        return Ok(2);
                    };
                    match flag {
                        "--option" => option = Some(value),
                        "--tier" => tier = Some(value),
                        _ => {}
                    }
                }
                _ => {
                    eprintln!("error: unknown catalog argument: {flag}");
                    return Ok(2);
                }
            }
            i += 1;
        }
        if json && check {
            eprintln!("error: --json and --check are mutually exclusive");
            return Ok(2);
        }
        let all: Vec<Value> = serde_json::from_str(&rxd_kuna_cli::catalog_json())?;
        if check {
            return emit(b"catalog OK: documents exactly the registered kuna options\n");
        }
        if let Some(name) = option
            && !all.iter().any(|v| v["option"] == name)
        {
            eprintln!("error: unknown option: {name:?}");
            return Ok(1);
        }
        let selected: Vec<&Value> = all
            .iter()
            .filter(|v| {
                option.is_none_or(|name| v["option"] == name)
                    && tier.is_none_or(|name| v["tier"] == name)
            })
            .collect();
        if json {
            let mut bytes = serde_json::to_vec_pretty(&selected)?;
            bytes.push(b'\n');
            return emit(&bytes);
        }
        let mut text = String::new();
        for item in selected {
            use std::fmt::Write;
            let field = |name: &str| item[name].as_str().unwrap_or("");
            let values = item["values"]
                .as_array()
                .ok_or("invalid catalog values")?
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("|");
            let flag = if item["destructive_as_default"] == true {
                "  (opt-in: destructive as global default)"
            } else {
                ""
            };
            writeln!(
                text,
                "{:<20} {:<20} default={:<10} [{}/{}] tier={}{}",
                field("option"),
                format!("{{{values}}}"),
                field("default"),
                field("phase"),
                field("subphase"),
                field("tier"),
                flag
            )?;
            writeln!(
                text,
                "    {}\n    when: {}",
                field("summary"),
                field("use_when")
            )?;
            if !field("source_decompiler").is_empty() || !field("change_kind").is_empty() {
                writeln!(
                    text,
                    "    from: {} ({})",
                    if field("source_decompiler").is_empty() {
                        "?"
                    } else {
                        field("source_decompiler")
                    },
                    if field("change_kind").is_empty() {
                        "?"
                    } else {
                        field("change_kind")
                    }
                )?;
            }
            if !field("inspiration").is_empty() {
                writeln!(text, "    inspiration: {}", field("inspiration"))?;
            }
        }
        emit(text.as_bytes())
    }

    pub fn run(mut args: Vec<OsString>) -> Result<i32> {
        let has = |name: &str| args.iter().any(|a| a == name);
        let help = has("--help") || has("-h");
        let external =
            has("--decomp-dbg") || env::var_os("KUNA_DECOMP_DBG").is_some_and(|s| !s.is_empty());
        if args.first().is_some_and(|a| a == "catalog") && !help && !has("--markdown") && !external
        {
            return catalog(&args);
        }
        let text =
            args.first().is_some_and(|a| a == "decompile") && !help && !has("--json") && !external;
        if text {
            args.push("--json".into());
        }
        let resource_free = args
            .first()
            .and_then(|arg| arg.to_str())
            .is_some_and(|arg| {
                matches!(
                    arg,
                    "--version"
                        | "-V"
                        | "version"
                        | "--help"
                        | "-h"
                        | "help"
                        | "docs"
                        | "install-skill"
                        | "modes"
                        | "catalog"
                )
            });
        if (!resource_free || external) && !help {
            let manifest: Manifest = serde_json::from_str(embed::MANIFEST)?;
            let cache = materialize("rxd-kuna", embed::DATA, embed::TARGET, &manifest)?;
            // SAFETY: main dispatches Kuna before spawning threads or loading engines.
            // The upstream CLI also configures process-wide state at this entry point.
            unsafe {
                env::set_var("KUNA_SPECS", cache.join("specs"));
                env::set_var("SLEIGHHOME", cache.join("specs"));
                env::remove_var("KUNA_ROOT");
                env::remove_var("KUNA_RUST_PROFILE");
            }
        }
        let mut argv = vec!["kuna".to_owned()];
        for arg in args {
            argv.push(
                arg.into_string()
                    .map_err(|_| "Kuna arguments must be UTF-8")?,
            );
        }
        if text {
            let (status, bytes) = rxd_kuna_cli::run_captured(argv);
            let document = serde_json::from_slice::<Value>(&bytes);
            match document {
                Ok(value) => {
                    let mut output = String::new();
                    for function in value["functions"]
                        .as_array()
                        .ok_or("invalid Kuna JSON response")?
                    {
                        if let Some(code) = function["code"].as_str() {
                            output.push_str(code);
                            if !code.ends_with('\n') {
                                output.push('\n');
                            }
                        }
                    }
                    emit(output.as_bytes())?;
                }
                Err(error) if status == 0 => return Err(error.into()),
                Err(_) => {}
            }
            return Ok(status);
        }
        Ok(rxd_kuna_cli::run(argv))
    }
}
