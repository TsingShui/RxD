//! Offline, native EVMole analysis of an explicitly encoded local bytecode carrier.
use std::{io, path::Path};

use evmole::{Contract, ContractInfoArgs};
use rxd_core::{Error, Result, perf::Perf};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub mod code_references;
mod data;
mod functions;
mod literals;
mod navigation;
pub mod reconstruction;
pub mod references;
pub mod structuring;
mod text;
pub use functions::{
    AbstractState, AbstractValue, Context, Edge, Evidence, FunctionCandidate, FunctionNavigation,
    FunctionSelection, MemoryWord,
};
pub use literals::{LiteralInventory, LiteralKind, LiteralQuery};
pub use navigation::{BasicBlock, Instruction, Listing, Navigation};

pub const ENGINE_VERSION: &str = "0.9.4";

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    Raw,
    Hex,
}

#[derive(Clone, Copy)]
pub enum Query {
    Analyze,
    Storage,
    Disassemble,
    BasicBlocks,
    Cfg,
    Metadata,
}

/// Abstract-interpreter states one path may hold. Bound on the analysis machine, not on
/// the answer: it is the same number the previous entity budget gave this interpreter.
const STATES: usize = 100_000;

#[derive(Clone, Copy)]
pub struct Limits {
    pub input_bytes: usize,
    /// Explicit work budget for navigation; native producer queries retain their own budgets.
    pub work: usize,
    pub states: usize,
    pub stack_cells: usize,
    pub reconstruction_work: usize,
    pub values: usize,
}

impl Limits {
    /// Project the one shared budget table onto the EVM ceilings.
    ///
    /// The abstract interpreter's context graph, retained stack cells and value arena keep
    /// their own state-machine ceilings - they bound what one path may hold while it runs,
    /// not how much a query may answer - and the archive/work numbers come from the shared
    /// table. Limits are reported in the `--perf` sidecar and are never CLI knobs.
    pub fn from_budget(budget: rxd_core::Budget) -> Self {
        Self {
            input_bytes: budget.input_bytes,
            work: budget.work,
            states: STATES,
            stack_cells: budget.entries,
            reconstruction_work: budget.work,
            values: budget.entries,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::from_budget(rxd_core::Budget::default())
    }
}

/// Expand the copy-only coordinate a record prints into the canonical content-bound ID.
///
/// A record prints short, self-describing coordinates such as `hex:block:0:4` because the
/// stream header already carries the source identity. A full `rxd1:evm:<sha256>:...` ID is
/// returned unchanged so a foreign hash still fails `SOURCE_MISMATCH` during validation.
pub(crate) fn canonical_id(source_id: &str, at: &str) -> String {
    if at.starts_with("rxd1:") {
        at.to_string()
    } else {
        format!("rxd1:evm:{source_id}:{at}")
    }
}

/// Strip the schema prefix from a content-bound ID for printing next to its record.
pub(crate) fn coordinate(id: &str) -> String {
    rxd_core::envelope::at_of(id).into_owned()
}

/// One fixed carrier and its explicitly selected decoded interpretation.
pub struct Carrier {
    pub path: String,
    pub encoding: Encoding,
    pub source_id: String,
    pub bytes: Vec<u8>,
    pub bytecode: Vec<u8>,
    pub bytecode_sha256: String,
    decoded_carrier_start: usize,
}

impl Encoding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Hex => "hex",
        }
    }
}

impl Carrier {
    pub fn read(path: &Path, encoding: Encoding, limits: Limits, perf: &mut Perf) -> Result<Self> {
        let bytes = perf.measure("read", || rxd_core::read_bytes(path, limits.input_bytes))?;
        let source_id = perf.measure("hash", || rxd_core::identity::content_id(path, &bytes));
        perf.source_id = Some(source_id.clone());
        perf.counters.input_bytes = Some(bytes.len());
        let bytecode = perf.measure("decode", || decode(&bytes, encoding))?;
        if bytecode.starts_with(&[0xef, 0x00]) {
            return Err(Error::new(
                "UNSUPPORTED_INPUT",
                "EOF (EF00) containers are outside the flat EVM bytecode profile",
            ));
        }
        let bytecode_sha256 = perf.measure("hash", || format!("{:x}", Sha256::digest(&bytecode)));
        let decoded_carrier_start = match encoding {
            Encoding::Raw => 0,
            Encoding::Hex => {
                let trimmed = bytes.trim_ascii_start();
                bytes.len() - trimmed.len()
                    + usize::from(trimmed.starts_with(b"0x") || trimmed.starts_with(b"0X")) * 2
            }
        };
        Ok(Self {
            path: path.to_string_lossy().into_owned(),
            encoding,
            source_id,
            bytes,
            bytecode,
            bytecode_sha256,
            decoded_carrier_start,
        })
    }

    /// Map validated decoded byte boundaries to the original raw/hex carrier.
    pub fn carrier_range(&self, start: usize, end: usize) -> (usize, usize) {
        let stride = if self.encoding == Encoding::Hex { 2 } else { 1 };
        (
            self.decoded_carrier_start + start * stride,
            self.decoded_carrier_start + end * stride,
        )
    }
}

pub struct Analysis {
    path: String,
    encoding: Encoding,
    pub source_id: String,
    pub input_bytes: usize,
    pub bytecode: Vec<u8>,
    bytecode_sha256: String,
    pub result: Contract,
}

fn decode(carrier: &[u8], encoding: Encoding) -> Result<Vec<u8>> {
    if matches!(encoding, Encoding::Raw) {
        return Ok(carrier.to_vec());
    }
    let mut hex = carrier.trim_ascii();
    if hex.starts_with(b"0x") || hex.starts_with(b"0X") {
        hex = &hex[2..];
    }
    if !hex.len().is_multiple_of(2) {
        return Err(Error::new(
            "INVALID_INPUT",
            "hex bytecode has an odd number of digits",
        ));
    }
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    };
    hex.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| match (digit(pair[0]), digit(pair[1])) {
            (Some(high), Some(low)) => Ok((high << 4) | low),
            _ => Err(Error::new(
                "INVALID_INPUT",
                "expected ASCII hex digits without interior separators or whitespace",
            )),
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 15) as usize] as char);
    }
    output
}

impl Analysis {
    pub fn read(
        path: &Path,
        encoding: Encoding,
        query: Query,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<Self> {
        let carrier = Carrier::read(path, encoding, limits, perf)?;
        let bytecode = &carrier.bytecode;
        let result = perf.measure("analyze", || {
            let args = ContractInfoArgs::new(bytecode);
            let args = match query {
                Query::Analyze => args
                    .with_selectors()
                    .with_arguments()
                    .with_state_mutability()
                    .with_storage()
                    .with_disassemble()
                    .with_basic_blocks()
                    .with_control_flow_graph()
                    .with_metadata(),
                Query::Storage => args.with_storage().with_state_mutability().with_metadata(),
                Query::Disassemble => args.with_disassemble(),
                Query::BasicBlocks => args.with_basic_blocks(),
                Query::Cfg => args.with_control_flow_graph(),
                Query::Metadata => args.with_metadata(),
            };
            evmole::contract_info(args)
        });
        let records = result.functions.as_ref().map_or(0, Vec::len)
            + result.storage.as_ref().map_or(0, Vec::len)
            + result.transient_storage.as_ref().map_or(0, Vec::len)
            + result.disassembled.as_ref().map_or(0, Vec::len)
            + result.basic_blocks.as_ref().map_or(0, Vec::len)
            + result
                .control_flow_graph
                .as_ref()
                .map_or(0, |cfg| cfg.blocks.len())
            + result
                .metadata
                .as_ref()
                .map_or(0, |metadata| metadata.entries.len());
        perf.counters.records = Some(records);
        if let Some(instructions) = &result.disassembled {
            perf.counters.instructions = Some(instructions.len());
        }
        Ok(Self {
            path: carrier.path,
            encoding,
            source_id: carrier.source_id,
            input_bytes: carrier.bytes.len(),
            bytecode: carrier.bytecode,
            bytecode_sha256: carrier.bytecode_sha256,
            result,
        })
    }

    pub fn json(&self, command: &str, include_bytecode: bool) -> Result<Vec<u8>> {
        #[derive(Serialize)]
        struct Engine {
            name: &'static str,
            version: &'static str,
        }
        #[derive(Serialize)]
        struct Carrier<'a> {
            path: &'a str,
            encoding: Encoding,
            bytes: usize,
        }
        #[derive(Serialize)]
        struct Bytecode<'a> {
            sha256: &'a str,
            bytes: usize,
            hex: Option<String>,
        }
        #[derive(Serialize)]
        struct Tail {
            start: usize,
            bytes: usize,
            hex: String,
        }
        #[derive(Serialize)]
        struct Detail<'a> {
            engine: Engine,
            carrier: Carrier<'a>,
            bytecode: Bytecode<'a>,
            offset_unit: &'static str,
            cfg_destination_unit: &'static str,
            basic_block_end: &'static str,
            disassembly_complete: Option<bool>,
            undecoded_tail: Option<Tail>,
            abi_verified: bool,
            discovery_complete: Option<bool>,
            input_role: &'static str,
            hardfork: Option<&'static str>,
            result: &'a Contract,
            complete: bool,
        }
        #[derive(Serialize)]
        struct Report<'a> {
            record: &'static str,
            backend: &'static str,
            command: &'a str,
            schema_version: u32,
            id: Option<&'a str>,
            source_id: &'a str,
            parent_at: Option<&'a str>,
            context_at: Option<&'a str>,
            start: usize,
            end: usize,
            detail: Detail<'a>,
        }
        let decoded_end = self.result.disassembled.as_ref().map(|instructions| {
            instructions.last().map_or(0, |(offset, _)| {
                let opcode = self.bytecode.get(*offset).copied().unwrap_or(0);
                let width = 1 + if (0x60..=0x7f).contains(&opcode) {
                    usize::from(opcode - 0x5f)
                } else {
                    0
                };
                offset.saturating_add(width).min(self.bytecode.len())
            })
        });
        let report = Report {
            record: "evm_analysis",
            backend: "evm",
            command,
            schema_version: rxd_core::envelope::SCHEMA_VERSION,
            id: None,
            source_id: &self.source_id,
            parent_at: None,
            context_at: None,
            start: 0,
            end: self.input_bytes,
            detail: Detail {
                engine: Engine {
                    name: "evmole",
                    version: ENGINE_VERSION,
                },
                carrier: Carrier {
                    path: &self.path,
                    encoding: self.encoding,
                    bytes: self.input_bytes,
                },
                bytecode: Bytecode {
                    sha256: &self.bytecode_sha256,
                    bytes: self.bytecode.len(),
                    hex: include_bytecode.then(|| hex(&self.bytecode)),
                },
                offset_unit: "decoded_bytecode_byte",
                cfg_destination_unit: "upstream_block_id",
                basic_block_end: "last_opcode_start",
                disassembly_complete: decoded_end.map(|end| end == self.bytecode.len()),
                undecoded_tail: decoded_end
                    .filter(|&end| end < self.bytecode.len())
                    .map(|start| Tail {
                        start,
                        bytes: self.bytecode.len() - start,
                        hex: hex(&self.bytecode[start..]),
                    }),
                abi_verified: false,
                discovery_complete: None,
                input_role: "unknown",
                hardfork: None,
                result: &self.result,
                complete: true,
            },
        };
        let mut output = Output { bytes: Vec::new() };
        if let Err(error) = serde_json::to_writer(&mut output, &report) {
            return Err(Error::new("INVALID_INPUT", error.to_string()));
        }
        io::Write::write_all(&mut output, b"\n")
            .map_err(|_| Error::new("RESOURCE_LIMIT", "EVM output byte budget exceeded"))?;
        Ok(output.bytes)
    }
}

struct Output {
    bytes: Vec<u8>,
}

impl io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
