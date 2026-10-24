use std::path::PathBuf;

use super::filter::CaseArgs;
use super::xref::Selector;
use clap::{Args, Subcommand, ValueEnum};
use rxd_core::{Error, Result, perf::Perf};
use rxd_evm::{
    Analysis, FunctionNavigation, FunctionSelection, Limits, Listing, LiteralInventory,
    LiteralKind, LiteralQuery, Navigation, Query,
};

#[derive(Subcommand)]
pub enum Command {
    /// Search decoded PUSH literals and data candidates using an explicit query interpretation.
    Strings(StringsOptions),
    /// Find evidenced references to a literal, block or function coordinate.
    Xref(XrefOptions),
    /// Return all native EVMole analyses as one JSON object.
    Analyze(Options),
    /// List function candidates with EVMole's own ABI fields.
    Interface(ListOptions),
    /// Infer persistent and transient storage, retaining the function candidates.
    Storage(Options),
    /// Decode bytecode into offset/opcode pairs.
    Disassemble(Options),
    /// Enumerate native basic-block ranges. Each end is the start offset of its last opcode.
    BasicBlocks(Options),
    /// Infer a control-flow graph. Destinations are upstream block IDs.
    Cfg(Options),
    /// Extract terminal CBOR compiler metadata.
    Metadata(Options),
    /// List basic blocks.
    Blocks(ListOptions),
    /// Fetch one basic block using the coordinate a blocks row prints.
    GetBlock(BlockOptions),
    /// Fetch native reachable regions for one selector-dispatched function candidate.
    GetFunction(FunctionOptions),
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

#[derive(Args)]
pub struct XrefOptions {
    #[command(flatten)]
    options: Options,
    /// The coordinate a block, function or literal row prints.
    #[arg(long)]
    at: String,
}

#[derive(Clone, Copy, ValueEnum)]
enum Encoding {
    Raw,
    Hex,
}

#[derive(Args)]
pub struct BlockOptions {
    #[command(flatten)]
    options: Options,
    #[command(flatten)]
    selector: Selector,
}

#[derive(Args)]
pub struct FunctionOptions {
    #[command(flatten)]
    options: Options,
    /// The coordinate an interface row prints.
    #[arg(
        long,
        required_unless_present = "selector",
        conflicts_with = "selector"
    )]
    at: Option<String>,
    /// Select exactly one candidate by its four-byte selector.
    #[arg(long, conflicts_with = "at")]
    selector: Option<String>,
    /// Return original byte segments/instructions without requesting reconstruction.
    #[arg(long)]
    raw: bool,
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
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    #[arg(long)]
    perf: bool,
    // Every ceiling comes from rxd_core::Budget; no CLI flag exposes one.
}

impl Command {
    fn options(&self) -> &Options {
        match self {
            Self::Strings(options) => &options.options,
            Self::Xref(options) => &options.options,
            Self::Interface(options) | Self::Blocks(options) => &options.options,
            Self::GetBlock(options) => &options.options,
            Self::GetFunction(options) => &options.options,
            Self::Analyze(options)
            | Self::Storage(options)
            | Self::Disassemble(options)
            | Self::BasicBlocks(options)
            | Self::Cfg(options)
            | Self::Metadata(options) => options,
        }
    }

    fn query(&self) -> Option<Query> {
        match self {
            Self::Analyze(_) => Some(Query::Analyze),
            Self::Storage(_) => Some(Query::Storage),
            Self::Disassemble(_) => Some(Query::Disassemble),
            Self::BasicBlocks(_) => Some(Query::BasicBlocks),
            Self::Cfg(_) => Some(Query::Cfg),
            Self::Metadata(_) => Some(Query::Metadata),
            Self::Strings(_)
            | Self::Xref(_)
            | Self::Interface(_)
            | Self::Blocks(_)
            | Self::GetBlock(_)
            | Self::GetFunction(_) => None,
        }
    }

    /// Serialization stages measured by the backend itself, not by this adapter.
    fn measures_own_serialization(&self) -> bool {
        matches!(
            self,
            Self::Strings(_) | Self::Xref(_) | Self::GetFunction(_)
        )
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Analyze(_) => "analyze",
            Self::Interface(_) => "interface",
            Self::Storage(_) => "storage",
            Self::Disassemble(_) => "disassemble",
            Self::BasicBlocks(_) => "basic-blocks",
            Self::Cfg(_) => "cfg",
            Self::Metadata(_) => "metadata",
            Self::Blocks(_) => "blocks",
            Self::Strings(_) => "strings",
            Self::Xref(_) => "xref",
            Self::GetBlock(_) => "get-block",
            Self::GetFunction(_) => "get-function",
        }
    }

    pub fn perf(&self) -> bool {
        self.options().perf
    }
}

pub fn execute(command: Command, policy: rxd_core::BudgetPolicy) -> Result<()> {
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
    if command.query().is_none() {
        effective_limits.insert("work", limits.work);
    }
    if matches!(
        command,
        Command::Interface(_) | Command::GetFunction(_) | Command::Strings(_) | Command::Xref(_)
    ) {
        effective_limits.insert("states", limits.states);
        effective_limits.insert("stack_cells", limits.stack_cells);
    }
    if matches!(&command, Command::GetFunction(function) if !function.raw) {
        effective_limits.insert("reconstruction_work", limits.reconstruction_work);
        effective_limits.insert("values", limits.values);
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
            let functions = if with_abi
                || matches!(
                    command,
                    Command::GetFunction(_) | Command::Strings(_) | Command::Xref(_)
                ) {
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
                Command::Blocks(blocks) => navigation.blocks_json(
                    options.include_bytecode,
                    blocks.filter.as_deref(),
                    blocks.case.sensitive(),
                    &mut perf,
                ),
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
                Command::Xref(refs) => rxd_evm::references::refs_json(
                    &navigation,
                    functions.as_ref().unwrap(),
                    &refs.at,
                    Listing {
                        include_bytecode: options.include_bytecode,
                        filter: None,
                        case_sensitive: false,
                    },
                    limits,
                    &mut perf,
                ),
                Command::GetBlock(block) => {
                    let at = block.selector.resolve().map_err(super::classify)?;
                    navigation.block_json(&at, &mut perf)
                }
                Command::GetFunction(function) => {
                    // `--at` and `--selector` name one candidate.
                    let selection = match &function.at {
                        Some(at) => FunctionSelection::At(at),
                        None => FunctionSelection::Selector(function.selector.as_deref().unwrap()),
                    };
                    functions.as_ref().unwrap().function_json(
                        &navigation,
                        selection,
                        function.raw,
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
            super::write_output(&bytes, options.output.as_deref())
        })
    })();
    super::finish(perf, options.perf, result)
}
