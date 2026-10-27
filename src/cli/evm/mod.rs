use std::path::PathBuf;

use super::filter::CaseArgs;
use clap::{Args, Subcommand, ValueEnum};
use rxd_core::{Error, Result, perf::Perf};
use rxd_evm::{
    Analysis, FunctionNavigation, Limits, Listing, LiteralInventory, LiteralKind, LiteralQuery,
    Navigation, Query,
};

#[derive(Subcommand)]
pub enum Command {
    /// Search decoded PUSH literals and data candidates.
    #[command(
        after_long_help = "Examples:\nrxd evm strings runtime.hex --encoding hex --kind text --query Authorization\nrxd evm strings runtime.bin --encoding raw --kind bytes --query 4142\nrxd evm strings runtime.hex --encoding hex --kind number --query 42\nRequired: --encoding and --kind. Omit --query to inventory the selected literal kind.\ntext matches readable fragments; bytes is exact even hex; number is decimal/0x U256.\nOutput: analysis then literal/data_literal JSONL; find the constant behind a selector or address.\nThese are static PUSH/CODECOPY candidates, not proven runtime strings."
    )]
    Strings(StringsOptions),

    /// List function candidates with ABI fields.
    #[command(
        after_long_help = "Examples:\nrxd evm interface runtime.hex --encoding hex -f 313ce567\nOutput: analysis plus function JSONL; at is a file-scoped candidate coordinate.\nSelectors/types/mutability are inferred candidates, not a verified source ABI."
    )]
    Interface(ListOptions),
    /// List decoded instructions as offset/opcode.
    #[command(
        after_long_help = "Examples:\nrxd evm disassemble runtime.hex --encoding hex\nOutput: native JSON offset/opcode pairs. Offsets are decoded bytecode bytes, not hex characters.\nOffsets index decoded bytecode, so a range is start..start+length."
    )]
    Disassemble(Options),

    /// Extract terminal CBOR compiler metadata.
    #[command(
        after_long_help = "Examples:\nrxd evm metadata runtime.hex --encoding hex\nOutput: native terminal CBOR/compiler metadata evidence. Not a deployed-contract identity.\nRaw bytes remain explicit when metadata is unknown/undecodable."
    )]
    Metadata(Options),
}

#[derive(Clone, Copy, ValueEnum)]
enum QueryKind {
    Text,
    Bytes,
    Number,
}

#[derive(Args)]
pub struct StringsOptions {
    #[command(flatten)]
    options: Options,
    #[arg(long, value_enum)]
    kind: QueryKind,
    /// Text substring, exact hex bytes, or decimal/0x U256; omit to inventory this kind.
    #[arg(long, allow_hyphen_values = true)]
    query: Option<String>,
    /// Substring over the value a literal row is named by, folded unless `--case sensitive`.
    #[arg(short = 'f', long)]
    filter: Option<String>,
    #[command(flatten)]
    case: CaseArgs,
}

#[derive(Clone, Copy, ValueEnum)]
enum Encoding {
    Raw,
    Hex,
}

#[derive(Args)]
pub struct ListOptions {
    #[command(flatten)]
    options: Options,
    /// Substring over the value a row is named by, folded unless `--case sensitive`.
    #[arg(short = 'f', long)]
    filter: Option<String>,
    #[command(flatten)]
    case: CaseArgs,
}

#[derive(Args)]
pub struct Options {
    /// One regular local carrier: binary bytes (raw) or ASCII hexadecimal (hex).
    input: PathBuf,
    /// Select raw bytes or ASCII hex explicitly; no encoding is guessed.
    #[arg(long, value_enum)]
    encoding: Encoding,
    /// Include the exact decoded bytecode as lowercase hex in the result.
    #[arg(long)]
    include_bytecode: bool,
    /// JSON is the default and only output format.
    #[arg(long)]
    json: bool,
    /// Atomically write the result to FILE instead of stdout; never overwrite input.
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Write timing/resource JSONL to stderr without changing the result payload.
    #[arg(long)]
    perf: bool,
    // Every ceiling comes from rxd_core::Budget; no CLI flag exposes one.
}

impl Command {
    fn options(&self) -> &Options {
        match self {
            Self::Strings(options) => &options.options,
            Self::Interface(options) => &options.options,
            Self::Disassemble(options) | Self::Metadata(options) => options,
        }
    }

    fn query(&self) -> Option<Query> {
        match self {
            Self::Disassemble(_) => Some(Query::Disassemble),
            Self::Metadata(_) => Some(Query::Metadata),
            Self::Strings(_) | Self::Interface(_) => None,
        }
    }

    /// Serialization stages measured by the backend itself, not by this adapter.
    fn measures_own_serialization(&self) -> bool {
        matches!(self, Self::Strings(_))
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Interface(_) => "interface",
            Self::Disassemble(_) => "disassemble",
            Self::Metadata(_) => "metadata",
            Self::Strings(_) => "strings",
        }
    }

    pub fn perf(&self) -> bool {
        self.options().perf
    }
}

pub fn execute(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    sink: &mut super::Output<'_>,
) -> Result<()> {
    let options = command.options();
    let mut perf = Perf::new(options.perf, "analysis", "evm", command.name());
    let budget = policy.for_file(&options.input)?;
    let mut limits = Limits::from_budget(budget);
    if matches!(policy, rxd_core::BudgetPolicy::InputScaled) {
        // Archive-size scaling must not enlarge the abstract interpreter's live state.
        limits.stack_cells = rxd_core::MAX_ARCHIVE_ENTRIES;
        limits.values = rxd_core::MAX_ARCHIVE_ENTRIES;
    }
    let mut effective_limits = [("input_bytes", limits.input_bytes)]
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    if command.query().is_none()
        && let Some(work) = limits.work
    {
        effective_limits.insert("work", work);
    }
    if matches!(command, Command::Interface(_) | Command::Strings(_)) {
        effective_limits.insert("states", limits.states);
        effective_limits.insert("stack_cells", limits.stack_cells);
    }
    perf.resource_limits = Some(effective_limits);
    let result = (|| {
        if let Some(output) = &options.output
            && let (Ok(input), Ok(output)) = (options.input.canonicalize(), output.canonicalize())
            && input == output
        {
            return Err(Error::new(
                "INVALID_QUERY",
                "output must not replace the input carrier",
            ));
        }
        let encoding = match options.encoding {
            Encoding::Raw => rxd_evm::Encoding::Raw,
            Encoding::Hex => rxd_evm::Encoding::Hex,
        };
        let bytes = if let Some(query) = command.query() {
            let analysis = Analysis::read(&options.input, encoding, query, limits, &mut perf)?;
            perf.measure("serialize", || {
                analysis.json(command.name(), options.include_bytecode)
            })?
        } else {
            let navigation = Navigation::read(&options.input, encoding, limits, &mut perf)?;
            let with_abi = matches!(command, Command::Interface(_));
            let functions = if with_abi || matches!(command, Command::Strings(_)) {
                Some(FunctionNavigation::discover(
                    &navigation,
                    with_abi,
                    limits,
                    &mut perf,
                )?)
            } else {
                None
            };
            let serialization = if command.measures_own_serialization() {
                None
            } else {
                Some(perf.start_stage())
            };
            let bytes = match &command {
                Command::Interface(interface) => {
                    let filter = interface.filter.as_deref().map(|needle| {
                        rxd_core::filter::Contains::new(needle, interface.case.sensitive())
                    });
                    functions.as_ref().unwrap().interface_json(
                        &navigation,
                        options.include_bytecode,
                        filter.as_ref(),
                        limits,
                        &mut perf,
                    )
                }
                Command::Strings(strings) => {
                    LiteralInventory::discover(&navigation, limits, &mut perf)?.strings_json(
                        &navigation,
                        functions.as_ref().unwrap(),
                        &LiteralQuery::new(
                            match strings.kind {
                                QueryKind::Text => LiteralKind::Text,
                                QueryKind::Bytes => LiteralKind::Bytes,
                                QueryKind::Number => LiteralKind::Number,
                            },
                            strings.query.as_deref(),
                        )?,
                        Listing {
                            include_bytecode: options.include_bytecode,
                            filter: strings.filter.as_deref(),
                            case_sensitive: strings.case.sensitive(),
                        },
                        limits,
                        &mut perf,
                    )
                }
                _ => unreachable!("native query handled above"),
            };
            if let Some(stage) = serialization {
                perf.record("serialize", stage);
            }
            bytes?
        };
        perf.counters.output_bytes = Some(bytes.len());
        perf.measure("write", || {
            super::write_output(&bytes, options.output.as_deref(), sink)
        })
    })();
    super::finish(perf, options.perf, result)
}
