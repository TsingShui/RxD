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
    #[command(
        after_long_help = "Examples:\nrxd evm strings runtime.hex --encoding hex --kind text --query Authorization\nrxd evm strings runtime.bin --encoding raw --kind bytes --query 4142\nrxd evm strings runtime.hex --encoding hex --kind number --query 42\nRequired: --encoding and --kind. Omit --query to inventory the selected literal kind.\ntext matches readable fragments; bytes is exact even hex; number is decimal/0x U256.\nOutput: analysis then literal/data_literal JSONL; copy literal.at into xref.\nThese are static PUSH/CODECOPY candidates, not proven runtime strings."
    )]
    Strings(StringsOptions),
    /// Find evidenced references to a literal, block or function coordinate.
    #[command(
        after_long_help = "Examples:\nrxd evm xref runtime.hex --encoding hex --at '<literal.at>'\nrxd evm xref runtime.hex --encoding hex -m 313ce567 --kind function\nRequired: --encoding and exactly one of --at/--match; --match needs --kind.\n--at copies interface.at/blocks.at/strings.at. --match must identify one candidate:\nzero matches is NOT_FOUND, multiple matches AMBIGUOUS; narrow or list then copy at.\nOutput: analysis plus reference rows. context_at is a function or block caller.\nNext: get-function for function context_at, get-block for block context_at; never guess."
    )]
    Xref(XrefOptions),
    /// Return all native EVMole analyses as one JSON object.
    #[command(
        after_long_help = "Examples:\nrxd evm analyze runtime.hex --encoding hex\nrxd evm analyze runtime.bin --encoding raw --include-bytecode\nRequired: --encoding raw/hex. Output: one evm_analysis JSON record with native results.\nFor navigable coordinates use interface/blocks/strings."
    )]
    Analyze(Options),
    /// List function candidates with EVMole's own ABI fields.
    #[command(
        after_long_help = "Examples:\nrxd evm interface runtime.hex --encoding hex -f 313ce567\nOutput: analysis plus function JSONL; at is a file-scoped candidate coordinate.\nNext: rxd evm get-function runtime.hex --encoding hex --at '<function.at>'.\nSelectors/types/mutability are inferred candidates, not a verified source ABI."
    )]
    Interface(ListOptions),
    /// Infer persistent and transient storage, retaining the function candidates.
    #[command(
        after_long_help = "Examples:\nrxd evm storage runtime.hex --encoding hex\nOutput: one native analysis JSON record with persistent/transient storage evidence.\nThis infers static candidates; it does not read chain state or perform RPC calls."
    )]
    Storage(Options),
    /// Decode bytecode into offset/opcode pairs.
    #[command(
        after_long_help = "Examples:\nrxd evm disassemble runtime.hex --encoding hex\nOutput: native JSON offset/opcode pairs. Offsets are decoded bytecode bytes, not hex characters.\nFor a selected range list blocks, then get-block --at '<block.at>'."
    )]
    Disassemble(Options),
    /// Enumerate native basic-block ranges. Each end is the start offset of its last opcode.
    #[command(
        after_long_help = "Examples:\nrxd evm basic-blocks runtime.hex --encoding hex\nOutput: native EVMole range array; its end is the last opcode START, not exclusive EOF.\nFor navigable coordinates use blocks, which returns block.at and exclusive ranges."
    )]
    BasicBlocks(Options),
    /// Infer a control-flow graph. Destinations are upstream block IDs.
    #[command(
        after_long_help = "Examples:\nrxd evm cfg runtime.hex --encoding hex\nOutput: native JSON control-flow graph. Destinations are producer block IDs, not PCs.\nFor original selected instructions use blocks -> get-block --at."
    )]
    Cfg(Options),
    /// Extract terminal CBOR compiler metadata.
    #[command(
        after_long_help = "Examples:\nrxd evm metadata runtime.hex --encoding hex\nOutput: native terminal CBOR/compiler metadata evidence. Not a deployed-contract identity.\nRaw bytes remain explicit when metadata is unknown/undecodable."
    )]
    Metadata(Options),
    /// List basic blocks.
    #[command(
        after_long_help = "Examples:\nrxd evm blocks runtime.hex --encoding hex\nOutput: analysis plus basic_block JSONL with at and decoded ranges.\nNext: rxd evm get-block runtime.hex --encoding hex --at '<basic_block.at>'.\nCoordinates refer to this raw/hex carrier, not arbitrary PC values."
    )]
    Blocks(ListOptions),
    /// Fetch one basic block using the coordinate a blocks row prints.
    #[command(
        after_long_help = "Examples:\nrxd evm blocks runtime.hex --encoding hex\nrxd evm get-block runtime.hex --encoding hex --at '<basic_block.at>'\nRequired: --encoding and --at copied from blocks or block-kind xref.context_at.\nOutput: one block JSON record with original instructions and exact carrier ranges.\nDo not use a function coordinate or fabricate a block from a PC."
    )]
    GetBlock(BlockOptions),
    /// Fetch native reachable regions for one selector-dispatched function candidate.
    #[command(
        after_long_help = "Examples:\nrxd evm interface runtime.hex --encoding hex -f 313ce567\nrxd evm get-function runtime.hex --encoding hex --at '<function.at>' --raw\nrxd evm get-function runtime.hex --encoding hex --selector 313ce567\nRequired: --encoding and exactly one of --at/--selector. Selector is four-byte ABI hex.\nPrefer interface.at or function-kind xref.context_at from this same input.\nOutput: one function_code JSON record; default bounded pseudo-code with explicit raw fallback.\n--raw returns original selected regions; reconstruction is not original Solidity."
    )]
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

#[derive(Clone, Copy, ValueEnum)]
enum XrefKind {
    Function,
    Block,
    Literal,
}

impl XrefKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Block => "block",
            Self::Literal => "literal",
        }
    }
}

#[derive(Args)]
#[command(group(clap::ArgGroup::new("anchor").required(true).args(["at", "filter"])))]
pub struct XrefOptions {
    #[command(flatten)]
    options: Options,
    /// The coordinate a block, function or literal row prints.
    #[arg(long, conflicts_with = "filter")]
    at: Option<String>,
    /// Substring over the target's selector, block coordinate or literal value; requires --kind.
    #[arg(short = 'm', long = "match", requires = "kind", conflicts_with = "at")]
    filter: Option<String>,
    /// Target kind for a content search: function, block or literal.
    #[arg(long, value_enum, requires = "filter")]
    kind: Option<XrefKind>,
    #[command(flatten)]
    case: CaseArgs,
}

#[derive(Clone, Copy, ValueEnum)]
enum Encoding {
    Raw,
    Hex,
}

#[derive(Args)]
#[command(mut_arg("at", |arg| arg.required(true).value_name("BLOCK_AT").help("Copy blocks.at or block-kind xref.context_at from this EVM input")))]
pub struct BlockOptions {
    #[command(flatten)]
    options: Options,
    #[command(flatten)]
    selector: Selector,
}

#[derive(Args)]
#[command(group(clap::ArgGroup::new("function_anchor").required(true).args(["at", "selector"])))]
pub struct FunctionOptions {
    #[command(flatten)]
    options: Options,
    /// The coordinate an interface row prints.
    #[arg(long, conflicts_with = "selector")]
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
    if command.query().is_none()
        && let Some(work) = limits.work
    {
        effective_limits.insert("work", work);
    }
    if matches!(
        command,
        Command::Interface(_) | Command::GetFunction(_) | Command::Strings(_) | Command::Xref(_)
    ) {
        effective_limits.insert("states", limits.states);
        effective_limits.insert("stack_cells", limits.stack_cells);
    }
    if matches!(&command, Command::GetFunction(function) if !function.raw) {
        if let Some(work) = limits.reconstruction_work {
            effective_limits.insert("reconstruction_work", work);
        }
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
                Command::Xref(refs) => {
                    let at = match (&refs.at, &refs.filter) {
                        (Some(at), None) => at.clone(),
                        (None, Some(filter)) => {
                            let kind = refs.kind.expect("clap requires --kind with --match");
                            resolve_xref_search(
                                &navigation,
                                functions.as_ref().unwrap(),
                                kind,
                                filter,
                                refs.case.sensitive(),
                                limits,
                                &mut perf,
                            )?
                        }
                        _ => unreachable!("clap enforces exactly one xref anchor"),
                    };
                    rxd_evm::references::refs_json(
                        &navigation,
                        functions.as_ref().unwrap(),
                        &at,
                        Listing {
                            include_bytecode: options.include_bytecode,
                            filter: None,
                            case_sensitive: false,
                        },
                        limits,
                        &mut perf,
                    )
                }
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

fn resolve_xref_search(
    navigation: &rxd_evm::Navigation,
    functions: &rxd_evm::FunctionNavigation,
    kind: XrefKind,
    filter: &str,
    case_sensitive: bool,
    limits: rxd_evm::Limits,
    perf: &mut Perf,
) -> Result<String> {
    let needle = rxd_core::filter::Contains::new(filter, case_sensitive);
    let mut targets: Vec<String> = match kind {
        XrefKind::Function => functions
            .functions
            .iter()
            .filter(|function| needle.matches(&function.selector))
            .map(|function| functions.function_coordinate(navigation, function))
            .collect(),
        XrefKind::Block => navigation
            .blocks
            .iter()
            .filter(|block| needle.matches(&navigation.block_coordinate(block)))
            .map(|block| navigation.block_coordinate(block))
            .collect(),
        XrefKind::Literal => {
            rxd_evm::matching_targets(navigation, functions, &needle, limits, perf)?
        }
    };
    targets.sort();
    targets.dedup();
    match targets.len() {
        0 => Err(Error::failure(
            rxd_core::Failure::NotFound,
            format!("no {} target matches --match {filter:?}", kind.as_str()),
        )),
        1 => Ok(targets.pop().expect("one target")),
        count => Err(Error::failure(
            rxd_core::Failure::Ambiguous,
            format!(
                "--match {filter:?} matches {count} {} targets; narrow it or use --at",
                kind.as_str()
            ),
        )),
    }
}
