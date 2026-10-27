//! Dynamic Ark ABC/HAP navigation; source records are not source-language classes.
use std::path::PathBuf;

use super::{
    Output,
    filter::CaseArgs,
    table::{self, Cell},
};
use clap::{Args, Subcommand};
use rxd_abc::{Abc, Coordinate, Input};
use rxd_core::{Budget, BudgetPolicy, Error, Limits, Result, perf::Perf};

#[derive(Subcommand)]
pub enum Command {
    /// Navigate module records and their methods.
    List {
        #[command(subcommand)]
        command: ListCommand,
    },
    #[command(
        about = "Reconstruct one method or fetch record/method instructions (--raw).",
        after_long_help = "Examples:\n  rxd ark decompile app.hap --at '<method.at>'\n  rxd ark decompile modules.abc --at '<record.at>' --raw\nOutput: reconstructed JS-like source, or original instructions with --raw.\nMethods using lexical environments are returned as a capture factory accepting {env, hole}.\nUnsupported source constructs fail explicitly; record-level source reconstruction is not yet available."
    )]
    Decompile(DecompileOptions),
}

#[derive(Subcommand)]
pub enum ListCommand {
    #[command(
        about = "List module records. Columns: at, name, methods.",
        after_long_help = "Examples:\n  rxd ark list record modules.abc\n  rxd ark list record app.hap -f entry\nOutput: one row per file record, including metadata records with no methods.\nCopy at into list method --at. Record names describe modules, not recovered source classes."
    )]
    Record(ListOptions),
    #[command(
        about = "List methods of one record. Columns: at, name, code_size, args, vregs.",
        after_long_help = "Examples:\n  rxd ark list record app.hap\n  rxd ark list method app.hap --at 'record:0x1234'\nRequired: --at copied from list record. code_size is bytecode bytes; args includes VM parameters.\nMissing code metadata is \\N; filtered zero hits are a complete empty result."
    )]
    Method(MethodOptions),
}

#[derive(Args)]
pub struct Options {
    /// Dynamic ABC 12.0.6.0 file or HAP/ZIP containing ABC entries.
    input: PathBuf,
    /// Exact ABC path inside HAP; required when more than one ABC exists.
    #[arg(long)]
    entry: Option<String>,
    /// Atomically write the result to FILE instead of stdout.
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Emit phase timings as JSONL on stderr.
    #[arg(long)]
    perf: bool,
}

#[derive(Args)]
pub struct ListOptions {
    #[command(flatten)]
    options: Options,
    /// Substring over names, folded unless --case sensitive.
    #[arg(short = 'f', long)]
    filter: Option<String>,
    #[command(flatten)]
    case: CaseArgs,
}

#[derive(Args)]
pub struct MethodOptions {
    #[command(flatten)]
    list: ListOptions,
    /// Copy an existing record coordinate from list record.
    #[arg(long)]
    at: String,
}

#[derive(Args)]
pub struct DecompileOptions {
    #[command(flatten)]
    options: Options,
    /// Copy a record or method coordinate from the corresponding list.
    #[arg(long)]
    at: String,
    /// Return original instructions instead of reconstructed source.
    #[arg(long)]
    raw: bool,
}

impl Command {
    fn options(&self) -> &Options {
        match self {
            Self::Decompile(options) => &options.options,
            Self::List {
                command: ListCommand::Record(list),
            } => &list.options,
            Self::List {
                command: ListCommand::Method(methods),
            } => &methods.list.options,
        }
    }
    pub fn perf(&self) -> bool {
        self.options().perf
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::List {
                command: ListCommand::Record(_),
            } => "list-record",
            Self::List {
                command: ListCommand::Method(_),
            } => "list-method",
            Self::Decompile(_) => "decompile",
        }
    }
}

fn append(output: &mut Vec<u8>, cells: &[Cell<'_>], budget: Budget) -> Result<()> {
    let line = table::line(cells).map_err(super::classify)?;
    let size = output
        .len()
        .checked_add(line.len())
        .filter(|size| budget.output_bytes.is_none_or(|limit| *size <= limit))
        .ok_or_else(|| Error::new("RESOURCE_LIMIT", "ABC output byte budget exceeded"))?;
    output
        .try_reserve(size - output.len())
        .map_err(|_| Error::new("RESOURCE_LIMIT", "ABC output allocation failed"))?;
    output.extend_from_slice(line.as_bytes());
    Ok(())
}

pub fn execute(command: Command, policy: BudgetPolicy, sink: &mut Output<'_>) -> Result<()> {
    let options = command.options();
    let mut perf = Perf::new(options.perf, "analysis", "abc", command.name());
    let enabled = options.perf;
    let output_path = options.output.clone();
    let result = (|| {
        if let Some(output) = &output_path
            && let (Ok(input), Ok(output)) = (options.input.canonicalize(), output.canonicalize())
            && input == output
        {
            return Err(Error::new(
                "INVALID_QUERY",
                "output must not replace the input ABC/HAP",
            ));
        }
        let input = Input::read(&options.input, options.entry.as_deref(), policy, &mut perf)?;
        perf.limits = Some(Limits::from(input.budget));
        let abc = perf.measure("parse", || Abc::parse(&input.bytes, input.budget))?;
        perf.source_id = Some(abc.source_id.clone());
        perf.counters.input_bytes = Some(input.bytes.len());
        let mut bytes = Vec::new();
        perf.measure("query", || -> Result<()> {
            match &command {
                Command::Decompile(options) => {
                    let at = Coordinate::parse(&options.at)?;
                    bytes = if options.raw {
                        abc.raw_target(at)?
                    } else {
                        abc.decompile_target(at)?
                    };
                }
                Command::List {
                    command: ListCommand::Record(list),
                } => {
                    let filter = list.filter.as_deref().map(|needle| {
                        rxd_core::filter::Contains::new(needle, list.case.sensitive())
                    });
                    for offset in abc.record_offsets() {
                        let record = abc.record(offset)?;
                        if filter
                            .as_ref()
                            .is_some_and(|filter| !filter.matches(&record.name.text))
                        {
                            continue;
                        }
                        append(
                            &mut bytes,
                            &[
                                Cell::Atom(&format!("record:0x{offset:x}")),
                                record
                                    .name
                                    .scalar_text()
                                    .map_or(Cell::Atom(&record.name.quoted()), Cell::Text),
                                Cell::Atom(&record.methods.to_string()),
                            ],
                            input.budget,
                        )?;
                    }
                }
                Command::List {
                    command: ListCommand::Method(methods),
                } => {
                    let Coordinate::Record(offset) = Coordinate::parse(&methods.at)? else {
                        return Err(Error::new(
                            "INVALID_QUERY",
                            "list method --at requires a record coordinate",
                        ));
                    };
                    let record = abc.record(offset)?;
                    let filter = methods.list.filter.as_deref().map(|needle| {
                        rxd_core::filter::Contains::new(needle, methods.list.case.sensitive())
                    });
                    for method in &abc.members(&record)?.methods {
                        if filter
                            .as_ref()
                            .is_some_and(|filter| !filter.matches(&method.name.text))
                        {
                            continue;
                        }
                        let code = method.code_offset.map(|_| abc.code(method)).transpose()?;
                        let size = code.as_ref().map(|code| code.bytes.len().to_string());
                        let args = code.as_ref().map(|code| code.args.to_string());
                        let vregs = code.as_ref().map(|code| code.vregs.to_string());
                        append(
                            &mut bytes,
                            &[
                                Cell::Atom(&format!("method:0x{:x}", method.offset)),
                                method
                                    .name
                                    .scalar_text()
                                    .map_or(Cell::Atom(&method.name.quoted()), Cell::Text),
                                size.as_deref().map_or(Cell::Null, Cell::Atom),
                                args.as_deref().map_or(Cell::Null, Cell::Atom),
                                vregs.as_deref().map_or(Cell::Null, Cell::Atom),
                            ],
                            input.budget,
                        )?;
                    }
                }
            }
            Ok(())
        })?;
        perf.counters.output_bytes = Some(bytes.len());
        perf.measure("write", || {
            super::write_output(&bytes, output_path.as_deref(), sink)
        })
    })();
    super::finish(perf, enabled, result)
}
