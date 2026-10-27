//! `rxd lua`: Lua/LuaJIT/Luau bytecode navigation and reconstruction.

use std::path::PathBuf;

use super::Output;
use super::filter::CaseArgs;
use super::table::{self, Cell};
use clap::{Args, Subcommand};
use rxd_core::perf::Perf;
use rxd_core::{Error, Result};
use rxd_lua::Chunk;
use serde_json::json;

#[derive(Subcommand)]
pub enum Command {
    /// List code objects (columns: at, label).
    #[command(
        after_long_help = "Examples:\nrxd lua codes module.luac\nrxd lua codes module.luac -f vararg=true\nOutput: one row per function in compiler order; at is proto:N, parent_at is containment.\nLabels report params, vararg, source line range and instruction count.\nDialect comes from the chunk header; no encoding flag is needed."
    )]
    Codes(CodesOptions),
    /// List string constants (columns: at, value).
    #[command(
        after_long_help = "Examples:\nrxd lua strings module.luac -f token\nOutput: at is proto:N:const:I, the pool slot the owning prototype loads.\nNon-UTF-8 bytes appear as lossy text; use instructions for raw operand words."
    )]
    Strings(StringsOptions),
    /// List original instructions (columns: offset, instruction).
    #[command(
        after_long_help = "Examples:\nrxd lua codes module.luac\nrxd lua instructions module.luac --at 'proto:1'\nRequired: --at copied from codes.at. Offsets are chunk byte offsets, not instruction indices.\nNo reconstruction here; use decompile for reconstructed source."
    )]
    Instructions(InstructionsOptions),
    /// Reconstruct Lua source for the whole chunk.
    #[command(
        after_long_help = "Examples:\nrxd lua decompile module.luac\nOutput: one lua_decompile JSON record with source, dialect and completeness.\nReconstruction is a representation, not the original source file."
    )]
    Decompile(Options),
}

#[derive(Args)]
pub struct Options {
    /// One regular local chunk: PUC Lua 5.1-5.5, LuaJIT 2.1 or Luau bytecode.
    input: PathBuf,
    /// Atomically write the result to FILE instead of stdout.
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Write timing/resource JSONL to stderr without changing the payload.
    #[arg(long)]
    perf: bool,
}

#[derive(Args)]
pub struct CodesOptions {
    #[command(flatten)]
    options: Options,
    /// Optional existing codes.at coordinate, proto:N; omit to list every code object.
    #[arg(long)]
    at: Option<String>,
    /// Substring over the label, folded unless `--case sensitive`.
    #[arg(short = 'f', long)]
    filter: Option<String>,
    #[command(flatten)]
    case: CaseArgs,
}

#[derive(Args)]
pub struct StringsOptions {
    #[command(flatten)]
    options: Options,
    /// Substring over the literal text, folded unless `--case sensitive`.
    #[arg(short = 'f', long)]
    filter: Option<String>,
    #[command(flatten)]
    case: CaseArgs,
}

#[derive(Args)]
pub struct InstructionsOptions {
    #[command(flatten)]
    options: Options,
    /// Copy codes.at: proto:N of the function whose instructions to list.
    #[arg(long)]
    at: String,
}

impl Command {
    fn options(&self) -> &Options {
        match self {
            Self::Codes(options) => &options.options,
            Self::Strings(options) => &options.options,
            Self::Instructions(options) => &options.options,
            Self::Decompile(options) => options,
        }
    }

    pub fn perf(&self) -> bool {
        self.options().perf
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Codes(_) => "codes",
            Self::Strings(_) => "strings",
            Self::Instructions(_) => "instructions",
            Self::Decompile(_) => "decompile",
        }
    }
}

fn table_error(error: anyhow::Error) -> Error {
    Error::new("INVALID_QUERY", error.to_string())
}

pub fn execute(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    sink: &mut Output<'_>,
) -> Result<()> {
    let options = command.options();
    let mut perf = Perf::new(options.perf, "analysis", "lua", command.name());
    let perf_enabled = options.perf;
    let input = options.input.clone();
    let output_path = options.output.clone();
    let budget = policy.for_file(&input)?;
    perf.resource_limits = Some([("input_bytes", budget.input_bytes)].into_iter().collect());
    let result = (|| {
        let chunk = Chunk::read(&input)?;
        if chunk.input_bytes() > budget.input_bytes {
            return Err(Error::new(
                "INPUT_TOO_LARGE",
                "input exceeds the admitted size",
            ));
        }
        let bytes = match command {
            Command::Codes(codes) => {
                let filter = codes
                    .filter
                    .as_deref()
                    .map(|needle| rxd_core::filter::Contains::new(needle, codes.case.sensitive()));
                let mut out = String::new();
                for code in chunk.codes() {
                    if let Some(at) = &codes.at
                        && at != &code.at
                    {
                        continue;
                    }
                    if let Some(filter) = &filter
                        && !filter.matches(&code.label)
                    {
                        continue;
                    }
                    out.push_str(
                        &table::line(&[Cell::Atom(&code.at), Cell::Text(&code.label)])
                            .map_err(table_error)?,
                    );
                }
                out.into_bytes()
            }
            Command::Strings(strings) => {
                let filter = strings.filter.as_deref().map(|needle| {
                    rxd_core::filter::Contains::new(needle, strings.case.sensitive())
                });
                let mut out = String::new();
                for item in chunk.strings() {
                    if let Some(filter) = &filter
                        && !filter.matches(&item.value)
                    {
                        continue;
                    }
                    out.push_str(
                        &table::line(&[Cell::Atom(&item.at), Cell::Text(&item.value)])
                            .map_err(table_error)?,
                    );
                }
                out.into_bytes()
            }
            Command::Instructions(instructions) => {
                let mut out = String::new();
                for instruction in chunk.instructions(&instructions.at)? {
                    out.push_str(
                        &table::line(&[
                            Cell::Atom(&instruction.offset.to_string()),
                            Cell::Atom(&instruction.text),
                        ])
                        .map_err(table_error)?,
                    );
                }
                out.into_bytes()
            }
            Command::Decompile(_) => {
                let source = chunk.decompile()?;
                let mut record = serde_json::to_vec(&json!({
                    "record": "lua_decompile",
                    "backend": "lua",
                    "command": "decompile",
                    "source_id": null,
                    "dialect": chunk.dialect(),
                    "complete": true,
                    "source": source,
                }))
                .map_err(|error| Error::new("INVALID_QUERY", error.to_string()))?;
                record.push(b'\n');
                record
            }
        };
        perf.counters.output_bytes = Some(bytes.len());
        perf.measure("write", || {
            super::write_output(&bytes, output_path.as_deref(), sink)
        })
    })();
    super::finish(perf, perf_enabled, result)
}
