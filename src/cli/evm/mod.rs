use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use rxd_core::{Error, Result, perf::Perf};
use rxd_evm::{
    Analysis, FunctionNavigation, FunctionSelection, Limits, LiteralInventory, LiteralKind,
    LiteralQuery, Navigation, Query,
};

#[derive(Subcommand)]
pub enum Command {
    /// Search decoded PUSH literals and data candidates using an explicit query interpretation.
    Strings(StringsOptions),
    /// Find evidenced references and fetchable contexts for a literal or code-unit ID.
    Refs(RefsOptions),
    /// Return all native EVMole analyses as one JSON object.
    Analyze(Options),
    /// Infer selectors, argument types and state mutability.
    #[command(visible_alias = "functions")]
    Interface(Options),
    /// Infer persistent and transient storage, retaining the function candidates.
    Storage(Options),
    /// Decode bytecode into offset/opcode pairs.
    Disassemble(Options),
    /// Enumerate basic blocks. Each end is the start offset of its last opcode.
    BasicBlocks(Options),
    /// Infer a control-flow graph. Destinations are upstream block IDs.
    Cfg(Options),
    /// Extract terminal CBOR compiler metadata.
    Metadata(Options),
    /// Enumerate content-bound basic-block candidates as JSONL.
    Outline(Options),
    /// Fetch one exact basic block using an ID returned by outline.
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
}

#[derive(Args)]
pub struct RefsOptions {
    #[command(flatten)]
    options: Options,
    #[arg(long)]
    id: String,
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
    id: String,
}

#[derive(Args)]
pub struct FunctionOptions {
    #[command(flatten)]
    options: Options,
    #[arg(required_unless_present = "selector", conflicts_with = "selector")]
    id: Option<String>,
    /// Select exactly one candidate by its four-byte selector.
    #[arg(long, conflicts_with = "id")]
    selector: Option<String>,
    /// Return original byte segments/instructions without requesting reconstruction.
    #[arg(long)]
    raw: bool,
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
    #[arg(long)]
    debug: bool,
    /// Input byte cap in bytes (1 through 8589934592); 8 GiB when omitted.
    #[arg(long, value_parser = super::input_bytes)]
    max_input_bytes: Option<usize>,
    #[arg(long, default_value_t = 100_000, value_parser = super::positive)]
    max_records: usize,
    #[arg(long, default_value_t = 16 * 1024 * 1024, value_parser = super::positive)]
    max_output_bytes: usize,
    /// Navigation bytes, instructions and block visits; native queries retain engine budgets.
    #[arg(long, default_value_t = 16 * 1024 * 1024, value_parser = super::positive)]
    max_work: usize,
    /// Cumulative contextual graph states allocated during function navigation.
    #[arg(long, default_value_t = 100_000, value_parser = super::positive)]
    max_states: usize,
    /// Cumulative stored graph stack slots and known memory-word facts.
    #[arg(long, default_value_t = 1_048_576, value_parser = super::positive)]
    max_stack_cells: usize,
    /// Selected reconstruction transfer and rendered text work.
    #[arg(long, default_value_t = 1_048_576, value_parser = super::positive)]
    max_reconstruction_work: usize,
    /// Selected reconstruction value arena entries.
    #[arg(long, default_value_t = 1_048_576, value_parser = super::positive)]
    max_values: usize,
}

impl Command {
    fn parts(&self) -> (&Options, Option<Query>) {
        match self {
            Self::Analyze(options) => (options, Some(Query::Analyze)),
            Self::Interface(options) => (options, Some(Query::Interface)),
            Self::Storage(options) => (options, Some(Query::Storage)),
            Self::Disassemble(options) => (options, Some(Query::Disassemble)),
            Self::BasicBlocks(options) => (options, Some(Query::BasicBlocks)),
            Self::Cfg(options) => (options, Some(Query::Cfg)),
            Self::Metadata(options) => (options, Some(Query::Metadata)),
            Self::Outline(options) => (options, None),
            Self::Strings(options) => (&options.options, None),
            Self::Refs(options) => (&options.options, None),
            Self::GetBlock(options) => (&options.options, None),
            Self::GetFunction(options) => (&options.options, None),
        }
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
            Self::Outline(_) => "outline",
            Self::Strings(_) => "strings",
            Self::Refs(_) => "refs",
            Self::GetBlock(_) => "get-block",
            Self::GetFunction(_) => "get-function",
        }
    }

    pub fn perf(&self) -> bool {
        self.parts().0.perf
    }

    pub fn debug(&self) -> bool {
        self.parts().0.debug
    }
}

pub fn execute(command: Command) -> Result<()> {
    let (options, query) = command.parts();
    let mut perf = Perf::new(
        options.perf || options.debug,
        "analysis",
        "evm",
        command.name(),
    );
    let mut effective_limits = [
        ("input_bytes", super::input_limit(options.max_input_bytes)),
        ("records", options.max_records),
        ("output_bytes", options.max_output_bytes),
    ]
    .into_iter()
    .collect::<std::collections::BTreeMap<_, _>>();
    if query.is_none() {
        effective_limits.insert("work", options.max_work);
    }
    if matches!(
        command,
        Command::Outline(_) | Command::GetFunction(_) | Command::Strings(_) | Command::Refs(_)
    ) {
        effective_limits.insert("states", options.max_states);
        effective_limits.insert("stack_cells", options.max_stack_cells);
    }
    if matches!(&command, Command::GetFunction(function) if !function.raw) {
        effective_limits.insert("reconstruction_work", options.max_reconstruction_work);
        effective_limits.insert("values", options.max_values);
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
        let limits = Limits {
            input_bytes: super::input_limit(options.max_input_bytes),
            records: options.max_records,
            output_bytes: options.max_output_bytes,
            work: options.max_work,
            states: options.max_states,
            stack_cells: options.max_stack_cells,
            reconstruction_work: options.max_reconstruction_work,
            values: options.max_values,
        };
        let encoding = match options.encoding {
            Encoding::Raw => rxd_evm::Encoding::Raw,
            Encoding::Hex => rxd_evm::Encoding::Hex,
        };
        let bytes = if let Some(query) = query {
            let analysis = Analysis::read(&options.input, encoding, query, limits, &mut perf)?;
            perf.measure("serialize", || {
                analysis.json(
                    command.name(),
                    options.include_bytecode,
                    limits.output_bytes,
                )
            })?
        } else {
            let navigation = Navigation::read(&options.input, encoding, limits, &mut perf)?;
            let functions = if matches!(
                command,
                Command::Outline(_)
                    | Command::GetFunction(_)
                    | Command::Strings(_)
                    | Command::Refs(_)
            ) {
                Some(FunctionNavigation::discover(
                    &navigation,
                    limits,
                    &mut perf,
                )?)
            } else {
                None
            };
            let serialization = if matches!(
                &command,
                Command::GetFunction(_) | Command::Strings(_) | Command::Refs(_)
            ) {
                None
            } else {
                perf.start_stage()
            };
            let bytes = match &command {
                Command::Outline(_) => navigation.outline_json(
                    functions.as_ref().unwrap(),
                    options.include_bytecode,
                    limits,
                    &mut perf,
                ),
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
                        options.include_bytecode,
                        limits,
                        &mut perf,
                    )
                }
                Command::Refs(refs) => rxd_evm::references::refs_json(
                    &navigation,
                    functions.as_ref().unwrap(),
                    &refs.id,
                    options.include_bytecode,
                    limits,
                    &mut perf,
                ),
                Command::GetBlock(block) => navigation.block_json(&block.id, limits, &mut perf),
                Command::GetFunction(function) => functions.as_ref().unwrap().function_json(
                    &navigation,
                    if let Some(id) = &function.id {
                        FunctionSelection::Id(id)
                    } else {
                        FunctionSelection::Selector(function.selector.as_deref().unwrap())
                    },
                    function.raw,
                    limits,
                    &mut perf,
                ),
                _ => unreachable!("native query handled above"),
            };
            if !matches!(command, Command::Strings(_) | Command::Refs(_)) {
                perf.record("serialize", serialization);
            }
            bytes?
        };
        perf.counters.output_bytes = Some(bytes.len());
        perf.measure("write", || {
            super::write_output(&bytes, options.output.as_deref())
        })
    })();
    super::finish(perf, options.perf, options.debug, result)
}
