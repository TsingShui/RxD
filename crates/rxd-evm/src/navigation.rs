//! Bounded navigation over validated native block candidates.
use rxd_core::{
    Error, Result,
    envelope::{self, RecordMeta},
    perf::Perf,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::{io::Write, path::Path};

use crate::{Carrier, Encoding, Limits, Output, hex};

#[derive(Debug, Serialize)]
pub struct Instruction {
    pub decoded_offset: usize,
    pub decoded_end: usize,
    pub opcode: u8,
    pub operand_hex: String,
    pub text: String,
}

#[derive(Debug)]
pub struct BasicBlock {
    pub start: usize,
    pub end: usize,
    pub last_opcode_start: usize,
    pub instruction_start: usize,
    pub instruction_end: usize,
}

/// The two output options every EVM row listing shares.
#[derive(Clone, Copy)]
pub struct Listing<'a> {
    /// Emit the exact decoded bytecode as lowercase hex in the header.
    pub include_bytecode: bool,
    /// Substring over the value the row is named by, folded unless `case_sensitive`.
    pub filter: Option<&'a str>,
    /// Whether `filter` folds case; the CLI's `--case` decides it.
    pub case_sensitive: bool,
}

pub struct Navigation {
    pub carrier: Carrier,
    pub instructions: Vec<Instruction>,
    pub blocks: Vec<BasicBlock>,
    pub decoded_end: usize,
}

pub(crate) fn work(perf: &mut Perf, amount: usize, limit: usize) -> Result<()> {
    let count = perf.counters.work.get_or_insert(0);
    *count = count.saturating_add(amount);
    if *count > limit {
        return Err(Error::new(
            "RESOURCE_LIMIT",
            "EVM navigation work budget exceeded",
        ));
    }
    Ok(())
}

fn width(opcode: u8) -> usize {
    1 + if (0x60..=0x7f).contains(&opcode) {
        usize::from(opcode - 0x5f)
    } else {
        0
    }
}

pub(crate) fn object(value: Value) -> Map<String, Value> {
    value
        .as_object()
        .expect("fixed record schema is an object")
        .clone()
}

impl Navigation {
    pub fn read(path: &Path, encoding: Encoding, limits: Limits, perf: &mut Perf) -> Result<Self> {
        let carrier = Carrier::read(path, encoding, limits, perf)?;
        Self::from_carrier(carrier, limits, perf)
    }

    pub fn from_carrier(carrier: Carrier, limits: Limits, perf: &mut Perf) -> Result<Self> {
        let scan = perf.start_stage();
        let scan_result: Result<usize> = (|| {
            work(perf, carrier.bytecode.len(), limits.work)?;
            let mut pc = 0;
            while pc < carrier.bytecode.len() {
                let end = pc + width(carrier.bytecode[pc]);
                if end > carrier.bytecode.len() {
                    break;
                }
                work(perf, 1, limits.work)?;
                pc = end;
            }
            Ok(pc)
        })();
        perf.record("navigate", scan);
        let decoded_end = scan_result?;
        // The bounded preflight prevents unbounded instruction allocations. No selector,
        // storage or graph traversal is requested by basic-block navigation.
        let native = perf.measure("analyze", || {
            evmole::contract_info(
                evmole::ContractInfoArgs::new(&carrier.bytecode)
                    .with_disassemble()
                    .with_basic_blocks(),
            )
        });
        let validation = perf.start_stage();
        let result = (|| {
            let instructions: Vec<_> = native
                .disassembled
                .unwrap_or_default()
                .into_iter()
                .map(|(pc, text)| {
                    let end = pc + width(carrier.bytecode[pc]);
                    Instruction {
                        decoded_offset: pc,
                        decoded_end: end,
                        opcode: carrier.bytecode[pc],
                        operand_hex: hex(&carrier.bytecode[pc + 1..end]),
                        text,
                    }
                })
                .collect();
            perf.counters.instructions = Some(instructions.len());
            let mut blocks = Vec::new();
            for (start, last) in native.basic_blocks.unwrap_or_default() {
                work(perf, 1, limits.work)?;
                // EVMole may report an initial range even when the first PUSH is truncated.
                // Such a range has no complete instruction and is never a fetchable block.
                if start >= decoded_end {
                    continue;
                }
                let first = instructions.binary_search_by_key(&start, |i| i.decoded_offset);
                let final_instruction =
                    instructions.binary_search_by_key(&last, |i| i.decoded_offset);
                let (Ok(first), Ok(final_instruction)) = (first, final_instruction) else {
                    return Err(Error::new(
                        "INVALID_INPUT",
                        "native basic block is not instruction-aligned",
                    ));
                };
                if final_instruction < first {
                    return Err(Error::new(
                        "INVALID_INPUT",
                        "native basic block has reversed bounds",
                    ));
                }
                blocks.push(BasicBlock {
                    start,
                    end: instructions[final_instruction].decoded_end,
                    last_opcode_start: last,
                    instruction_start: first,
                    instruction_end: final_instruction + 1,
                });
            }
            Ok(Self {
                carrier,
                instructions,
                blocks,
                decoded_end,
            })
        })();
        perf.record("navigate", validation);
        result
    }

    /// The copy-only coordinate a row prints and `--at` accepts.
    pub fn block_coordinate(&self, block: &BasicBlock) -> String {
        format!(
            "{}:block:{}:{}",
            self.carrier.encoding.as_str(),
            block.start,
            block.end
        )
    }

    pub fn block_id(&self, block: &BasicBlock) -> String {
        self.block_coordinate(block)
    }

    pub fn select_block(&self, id: &str) -> Result<&BasicBlock> {
        let canonical = id.to_string();
        let fields: Vec<_> = canonical.split(':').collect();
        let invalid = || Error::new("INVALID_QUERY", "expected an EVM block coordinate");
        if fields.len() != 4 || fields[1] != "block" || !matches!(fields[0], "raw" | "hex") {
            return Err(invalid());
        }
        let offset = |s: &str| -> Result<usize> {
            let n = s.parse::<usize>().map_err(|_| invalid())?;
            if n.to_string() != s {
                return Err(invalid());
            }
            Ok(n)
        };
        let start = offset(fields[2])?;
        let end = offset(fields[3])?;
        if fields[0] != self.carrier.encoding.as_str() {
            return Err(Error::new(
                "SOURCE_MISMATCH",
                "EVM block coordinate does not match the carrier encoding",
            ));
        }
        self.blocks
            .iter()
            .find(|b| b.start == start && b.end == end)
            .ok_or_else(invalid)
    }

    pub(crate) fn carrier_detail(&self) -> Value {
        json!({"path": self.carrier.path, "encoding": self.carrier.encoding, "bytes": self.carrier.bytes.len()})
    }

    pub(crate) fn bytecode_detail(&self, include: bool) -> Value {
        json!({"sha256": self.carrier.bytecode_sha256, "bytes": self.carrier.bytecode.len(),
            "hex": include.then(|| hex(&self.carrier.bytecode))})
    }

    pub fn undecoded_tail(&self) -> Value {
        if self.decoded_end == self.carrier.bytecode.len() {
            return Value::Null;
        }
        let (start, end) = self
            .carrier
            .carrier_range(self.decoded_end, self.carrier.bytecode.len());
        json!({"decoded_start": self.decoded_end, "decoded_end": self.carrier.bytecode.len(),
            "carrier_start": start, "carrier_end": end, "hex": hex(&self.carrier.bytecode[self.decoded_end..])})
    }

    fn block_detail(&self, block: &BasicBlock) -> Map<String, Value> {
        object(
            json!({"unit_kind": "block", "decoded_start": block.start, "decoded_end": block.end,
            "last_opcode_start": block.last_opcode_start,
            "instruction_count": block.instruction_end - block.instruction_start,
            "upstream_block_id": null, "boundary_status": "producer_candidate", "owner_ids": null, "shared": null}),
        )
    }

    pub fn block_record(&self, block: &BasicBlock, fetch: bool) -> Value {
        let id = self.block_id(block);
        let (start, end) = self.carrier.carrier_range(block.start, block.end);
        let mut detail = self.block_detail(block);
        if fetch {
            detail.extend(object(
                json!({"carrier": self.carrier_detail(), "bytecode": self.bytecode_detail(false),
                "raw_hex": hex(&self.carrier.bytecode[block.start..block.end]),
                "instructions": &self.instructions[block.instruction_start..block.instruction_end],
                "disassembly_complete": true, "undecoded_tail": null, "complete": true}),
            ));
        }
        let mut record = envelope::record(
            if fetch { "block" } else { "basic_block" },
            RecordMeta {
                at: Some(&id),
                start: Some(start),
                end: Some(end),
                ..RecordMeta::default()
            },
            detail,
        );
        if fetch {
            record
                .as_object_mut()
                .unwrap()
                .extend(object(json!({"backend": "evm", "command": "get-block",
                "schema_version": envelope::SCHEMA_VERSION})));
        }
        record
    }

    pub fn blocks_json(
        &self,
        include: bool,
        filter: Option<&str>,
        case_sensitive: bool,
        perf: &mut Perf,
    ) -> Result<Vec<u8>> {
        let needle = filter.map(|needle| rxd_core::filter::Contains::new(needle, case_sensitive));
        let selected: Vec<_> = self
            .blocks
            .iter()
            .filter(|block| {
                needle
                    .as_ref()
                    .is_none_or(|needle| needle.matches(&self.block_coordinate(block)))
            })
            .collect();
        perf.counters.records = Some(selected.len());
        let header = envelope::header(
            "evm",
            "blocks",
            Some(&self.carrier.source_id),
            Some(self.carrier.bytes.len()),
            json!({"encoding": self.carrier.encoding, "filter": filter}),
            header_detail(object(json!({"carrier": self.carrier_detail(),
                "bytecode": self.bytecode_detail(include), "offset_unit": "decoded_bytecode_byte",
                "basic_block_end": "exclusive", "boundary_status": "producer_candidate", "discovery_complete": null,
                "disassembly_complete": self.decoded_end == self.carrier.bytecode.len(), "undecoded_tail": self.undecoded_tail()}))),
        );
        let summary = envelope::summary(
            selected.len(),
            true,
            summary_detail(object(json!({"blocks": selected.len(),
            "disassembly_complete": self.decoded_end == self.carrier.bytecode.len(), "discovery_complete": null}))),
        );
        let mut output = Output { bytes: Vec::new() };
        write_record(&mut output, &header)?;
        for block in selected {
            write_record(&mut output, &self.block_record(block, false))?;
        }
        write_record(&mut output, &summary)?;
        Ok(output.bytes)
    }

    pub fn block_json(&self, at: &str, perf: &mut Perf) -> Result<Vec<u8>> {
        let block = self.select_block(at)?;
        perf.counters.records = Some(1);
        let mut output = Output { bytes: Vec::new() };
        write_record(&mut output, &self.block_record(block, true))?;
        Ok(output.bytes)
    }
}

pub(crate) fn write_record(output: &mut Output, record: &Value) -> Result<()> {
    if let Err(error) = serde_json::to_writer(&mut *output, record) {
        return Err(Error::new("INVALID_INPUT", error.to_string()));
    }
    output
        .write_all(b"\n")
        .map_err(|_| Error::new("RESOURCE_LIMIT", "EVM output byte budget exceeded"))
}

/// Fixed nullable EVM JSONL header detail, shared by all navigation commands.
pub(crate) fn header_detail(
    detail: serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    let mut fields = object(json!({
        "carrier":null,"bytecode":null,"offset_unit":null,"basic_block_end":null,"boundary_status":null,
        "target_kind":null,"target_decoded_offset":null,"target_semantics":null,"coverage":null,"literal_scope":null,
        "excluded_unindexed_pushes":null,"discovery_complete":null,"disassembly_complete":null,"undecoded_tail":null,
        "source_call_graph_complete":null,"abi_verified":null,"data_scope":null,"metadata_range":null,"copy_witnesses":null,
        "unknown_copy_witnesses":null,"unvisited_copy_sites":null,"data_reference_coverage":null,"unknown_copy_evidence":null,"metadata":null
    }));
    fields.extend(detail);
    fields
}

/// Fixed nullable EVM JSONL summary detail, shared by all navigation commands.
pub(crate) fn summary_detail(
    detail: serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    let mut fields = object(json!({
        "blocks":null,"functions":null,"target_kind":null,"target_decoded_offset":null,"references":null,"literals":null,
        "data_literals":null,"reference_status":null,"excluded_unindexed_pushes":null,"resolved_edges":null,"unknown_jumps":null,
        "invalid_jumps":null,"analysis_problems":null,"scanned_contexts":null,"scanned_blocks":null,"external_call_sites":null,
        "resolution_complete":null,"discovery_complete":null,"disassembly_complete":null,"source_call_graph_complete":null
    }));
    fields.extend(detail);
    fields
}
