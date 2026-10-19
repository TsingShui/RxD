//! Bytecode/data ambiguity stays explicit; only contextual CODECOPY evidence routes to code.
use std::collections::BTreeSet;

use alloy_primitives::U256;
use rxd_core::{
    Error, Result,
    envelope::{self, RecordMeta},
    perf::Perf,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    AbstractState, FunctionNavigation, Limits, Navigation,
    functions::CopyWitness,
    hex,
    literals::{LiteralKind, LiteralQuery},
    navigation::work,
};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DataLiteral {
    kind: &'static str,
    pub start: usize,
    pub end: usize,
    text: bool,
}

#[derive(Serialize)]
pub(crate) struct DataRange {
    pub decoded_start: usize,
    pub decoded_end: usize,
}

#[derive(Serialize)]
pub(crate) struct DataCoverage {
    pub data_scope: &'static str,
    pub metadata_range: Option<DataRange>,
    pub copy_witnesses: usize,
    pub unknown_copy_witnesses: usize,
    pub unvisited_copy_sites: usize,
    pub data_reference_coverage: &'static str,
    pub unknown_copy_evidence: Vec<Value>,
}

pub(crate) struct DataInventory {
    literals: Vec<DataLiteral>,
    witnesses: Vec<CopyWitness>,
    metadata: Option<(usize, usize)>,
    unknown: usize,
    unvisited: usize,
}

pub(crate) struct DataMatch<'a> {
    literal: &'a DataLiteral,
    matches: Vec<Value>,
    routes: Vec<Option<usize>>,
}

// Keep temporary-run storage bounded before the caller applies the minimum text length.
fn text_ranges(bytes: &[u8], start: usize, limits: Limits) -> Result<Vec<(usize, usize)>> {
    let mut ranges = Vec::new();
    for (offset, text) in crate::text::printable_text_runs(bytes) {
        if ranges.len() >= limits.states {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "EVM data text-run storage budget exceeded",
            ));
        }
        ranges.push((start + offset, start + offset + text.len()));
    }
    Ok(ranges)
}

impl DataInventory {
    pub(crate) fn discover(
        navigation: &Navigation,
        functions: &FunctionNavigation,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<Self> {
        let stage = perf.start_stage();
        let result = (|| {
            work(perf, navigation.carrier.bytecode.len(), limits.work)?;
            let metadata = perf
                .measure("analyze", || {
                    evmole::contract_info(
                        evmole::ContractInfoArgs::new(&navigation.carrier.bytecode).with_metadata(),
                    )
                })
                .metadata
                .map(|m| (m.bytecode_offset, navigation.carrier.bytecode.len()));
            let witnesses = functions.copy_witnesses(navigation, limits, perf)?;
            let seen: BTreeSet<_> = witnesses.iter().map(|w| w.operation.pc).collect();
            let mut sites = BTreeSet::new();
            for block in &navigation.blocks {
                for i in &navigation.instructions[block.instruction_start..block.instruction_end] {
                    work(perf, 1, limits.work)?;
                    if i.opcode == 0x39 {
                        sites.insert(i.decoded_offset);
                    }
                }
            }
            let unknown = witnesses
                .iter()
                .filter(|w| w.operation.source.is_none() || w.operation.length.is_none())
                .count();
            let unvisited = sites.difference(&seen).count();
            let unknown_payload_lower_bound = witnesses
                .iter()
                .filter(|w| w.operation.source.is_none() || w.operation.length.is_none())
                .fold(0usize, |total, w| {
                    let entry = if let Some(owner) = w.owner {
                        &functions.functions[owner].contexts[w.context.expect("function context")]
                            .entry
                    } else {
                        w.entry.as_ref().expect("observed ownerless entry")
                    };
                    total
                        .saturating_add(128)
                        .saturating_add(w.entry_blocks.len().saturating_mul(80))
                        .saturating_add(entry.stack.len().saturating_mul(24))
                        .saturating_add(entry.memory.len().saturating_mul(80))
                });
            if unknown_payload_lower_bound > limits.output_bytes {
                return Err(Error::new(
                    "RESOURCE_LIMIT",
                    "EVM copy coverage byte budget exceeded",
                ));
            }
            let mut regions = BTreeSet::new();
            let code_end = metadata.map_or(navigation.carrier.bytecode.len(), |m| m.0);
            let mut cursor = 0;
            for block in &navigation.blocks {
                if cursor < block.start.min(code_end) {
                    regions.insert(("unindexed", cursor, block.start.min(code_end)));
                }
                cursor = cursor.max(block.end.min(code_end));
            }
            if cursor < code_end {
                regions.insert(("unindexed", cursor, code_end));
            }
            if let Some((start, end)) = metadata {
                regions.insert(("metadata", start, end));
            }
            // A source slice can deliberately overlap instructions/PUSH operands. Label
            // this evidence as copied bytecode, rather than silently treating it as data.
            for witness in &witnesses {
                work(perf, 1 + regions.len(), limits.work)?;
                if let Some((start, end, _)) = Self::available(navigation, witness)
                    && start < end
                    && !regions.iter().any(|&(_, a, b)| a <= start && end <= b)
                {
                    let boundary = metadata.map_or(end, |m| m.0).clamp(start, end);
                    if start < boundary {
                        regions.insert(("copied", start, boundary));
                    }
                    if boundary < end {
                        regions.insert(("metadata", boundary, end));
                    }
                }
            }
            let mut literals = BTreeSet::new();
            for (kind, start, end) in regions {
                work(perf, end - start + 1, limits.work)?;
                let ranges = text_ranges(&navigation.carrier.bytecode[start..end], start, limits)?;
                let whole_text = end - start >= 4 && ranges == [(start, end)];
                literals.insert(DataLiteral {
                    kind,
                    start,
                    end,
                    text: whole_text,
                });
                for (a, b) in ranges {
                    if b - a >= 4 {
                        literals.insert(DataLiteral {
                            kind,
                            start: a,
                            end: b,
                            text: true,
                        });
                    }
                    if literals.len() > limits.states {
                        return Err(Error::new(
                            "RESOURCE_LIMIT",
                            "EVM data candidate storage budget exceeded",
                        ));
                    }
                }
                if literals.len() > limits.states {
                    return Err(Error::new(
                        "RESOURCE_LIMIT",
                        "EVM data candidate storage budget exceeded",
                    ));
                }
            }
            Ok(Self {
                literals: literals.into_iter().collect(),
                witnesses,
                metadata,
                unknown,
                unvisited,
            })
        })();
        perf.record("search", stage);
        result
    }

    fn id(&self, navigation: &Navigation, literal: &DataLiteral) -> String {
        format!(
            "rxd1:evm:{}:{}:literal:data:{}:{}:{}",
            navigation.carrier.source_id,
            navigation.carrier.encoding.as_str(),
            literal.kind,
            literal.start,
            literal.end
        )
    }

    fn select(&self, navigation: &Navigation, id: &str) -> Result<&DataLiteral> {
        let invalid = || {
            Error::new(
                "INVALID_ID",
                "expected a canonical content-bound EVM data literal ID",
            )
        };
        let fields: Vec<_> = id.split(':').collect();
        if fields.len() != 9
            || fields[0] != "rxd1"
            || fields[1] != "evm"
            || fields[4] != "literal"
            || fields[5] != "data"
            || !matches!(fields[6], "unindexed" | "metadata" | "copied")
            || fields[2].len() != 64
            || !fields[2]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !matches!(fields[3], "raw" | "hex")
        {
            return Err(invalid());
        }
        let start = fields[7].parse::<usize>().map_err(|_| invalid())?;
        let end = fields[8].parse::<usize>().map_err(|_| invalid())?;
        if start.to_string() != fields[7] || end.to_string() != fields[8] {
            return Err(invalid());
        }
        if fields[2] != navigation.carrier.source_id
            || fields[3] != navigation.carrier.encoding.as_str()
        {
            return Err(Error::new(
                "SOURCE_MISMATCH",
                "EVM data literal ID does not match the carrier and encoding",
            ));
        }
        self.literals
            .iter()
            .find(|l| l.kind == fields[6] && l.start == start && l.end == end)
            .ok_or_else(invalid)
    }

    // EVM CODECOPY zero-pads beyond code. U256 never drives an allocation, usize
    // addition, or a slice: cap available bytes against the immutable source first.
    fn available(navigation: &Navigation, witness: &CopyWitness) -> Option<(usize, usize, U256)> {
        let source = witness.operation.source?;
        let length = witness.operation.length?;
        let total = navigation.carrier.bytecode.len();
        let start = usize::try_from(source.min(U256::from(total))).ok()?;
        let available = length.min(U256::from(total - start));
        let size = usize::try_from(available).ok()?;
        Some((start, start + size, length - available))
    }

    fn routes(&self, navigation: &Navigation, literal: &DataLiteral) -> Vec<usize> {
        self.witnesses
            .iter()
            .enumerate()
            .filter_map(|(i, w)| {
                let (start, end, _) = Self::available(navigation, w)?;
                (start < literal.end && literal.start < end).then_some(i)
            })
            .collect()
    }

    fn entry<'a>(&'a self, functions: &'a FunctionNavigation, witness: usize) -> &'a AbstractState {
        let witness = &self.witnesses[witness];
        if let Some(owner) = witness.owner {
            &functions.functions[owner].contexts[witness.context.expect("function witness context")]
                .entry
        } else {
            witness
                .entry
                .as_ref()
                .expect("observed ownerless entry state")
        }
    }

    pub(crate) fn coverage(
        &self,
        navigation: &Navigation,
        functions: &FunctionNavigation,
    ) -> DataCoverage {
        DataCoverage {
            data_scope: "unindexed_and_contextually_copied_bytecode",
            metadata_range: self.metadata.map(|(decoded_start,decoded_end)| DataRange {decoded_start,decoded_end}),
            copy_witnesses: self.witnesses.len(), unknown_copy_witnesses: self.unknown,
            unvisited_copy_sites: self.unvisited, data_reference_coverage: "contextual_static_evidence",
            unknown_copy_evidence: self.witnesses.iter().enumerate().filter(|(_,w)| w.operation.source.is_none() || w.operation.length.is_none()).map(|(i,w)|json!({
                "instruction_offset":w.operation.pc,"block_id":navigation.block_id(&navigation.blocks[w.block]),"context_id":self.context(navigation,functions,Some(i)),"context_index":w.context,"context_entry_state":self.entry(functions,i).json(),"entry_evidence_block_ids":w.entry_blocks.iter().map(|&b|navigation.block_id(&navigation.blocks[b])).collect::<Vec<_>>(),
                "source_offset_hex":w.operation.source.map(|v|format!("{v:064x}")),"length_hex":w.operation.length.map(|v|format!("{v:064x}")),
                "destination_offset_hex":w.operation.destination.map(|v|format!("{v:064x}")),"reason":match (w.operation.source,w.operation.length){(None,None)=>"source_offset_and_length_unknown",(None,_)=>"source_offset_unknown",_=>"length_unknown"}
            })).collect(),
        }
    }

    pub(crate) fn search<'a>(
        &'a self,
        navigation: &Navigation,
        query: &LiteralQuery,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<(Vec<DataMatch<'a>>, usize)> {
        let mut found = Vec::new();
        let mut records = 0usize;
        let mut payload_lower_bound = 0usize;
        for literal in &self.literals {
            let bytes = &navigation.carrier.bytecode[literal.start..literal.end];
            work(perf, 1 + bytes.len(), limits.work)?;
            let mut matches = Vec::new();
            let matched = match query.kind {
                LiteralKind::Text => {
                    if !literal.text {
                        false
                    } else if query.normalized.is_none() {
                        true
                    } else {
                        matches =
                            query.text_matches(navigation, literal.start, bytes, limits, perf)?;
                        !matches.is_empty()
                    }
                }
                LiteralKind::Bytes => query.normalized.as_ref().is_none_or(|q| *q == hex(bytes)),
                LiteralKind::Number => {
                    bytes.len() <= 32
                        && query.number.is_none_or(|q| q == U256::from_be_slice(bytes))
                }
            };
            if !matched {
                continue;
            }
            work(perf, self.witnesses.len(), limits.work)?;
            let routes = self.routes(navigation, literal);
            let routes = if routes.is_empty() {
                vec![None]
            } else {
                routes.into_iter().map(Some).collect()
            };
            records = records.saturating_add(routes.len());
            Self::check_records(records, limits)?;
            payload_lower_bound = payload_lower_bound.saturating_add(
                (bytes.len().saturating_mul(2) + matches.len().saturating_mul(64))
                    .saturating_mul(routes.len()),
            );
            if payload_lower_bound > limits.output_bytes {
                return Err(Error::new(
                    "RESOURCE_LIMIT",
                    "EVM data evidence byte budget exceeded",
                ));
            }
            work(perf, routes.len(), limits.work)?;
            found.push(DataMatch {
                literal,
                matches,
                routes,
            });
        }
        Ok((found, records))
    }

    pub(crate) fn check_records(records: usize, limits: Limits) -> Result<()> {
        if records > limits.records {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "EVM result record budget exceeded",
            ));
        }
        if records.saturating_mul(128) > limits.output_bytes {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "EVM output byte budget exceeded",
            ));
        }
        Ok(())
    }

    fn context(
        &self,
        navigation: &Navigation,
        functions: &FunctionNavigation,
        witness: Option<usize>,
    ) -> Option<String> {
        witness.map(|i| {
            self.witnesses[i]
                .owner
                .map(|owner| functions.function_id(navigation, &functions.functions[owner]))
                .unwrap_or_else(|| navigation.block_id(&navigation.blocks[self.witnesses[i].block]))
        })
    }

    pub(crate) fn search_records<'a>(
        &'a self,
        navigation: &'a Navigation,
        functions: &'a FunctionNavigation,
        query: &'a LiteralQuery,
        found: &'a [DataMatch<'a>],
    ) -> impl Iterator<Item = Value> + 'a {
        found.iter().flat_map(move |item| item.routes.iter().map(move |&route| {
            let l = item.literal;
            let bytes = &navigation.carrier.bytecode[l.start..l.end];
            let (start, end) = navigation.carrier.carrier_range(l.start, l.end);
            let id = self.id(navigation, l);
            let context = self.context(navigation, functions, route);
            envelope::record("data_literal", RecordMeta { id: Some(&id), source_id: Some(&navigation.carrier.source_id),
                context_id: context.as_deref(), start: Some(start), end: Some(end), ..RecordMeta::default() },
                crate::navigation::object(json!({"literal_id":id,"literal_kind":"data_candidate","region_kind":l.kind,
                    "decoded_start":l.start,"decoded_end":l.end,"bytes_hex":hex(bytes),
                    "number_hex":(bytes.len() <= 32).then(|| format!("{:064x}",U256::from_be_slice(bytes))),
                    "query_kind":query.kind,"evidence_kind":match (l.kind, query.kind) {
                        ("metadata",LiteralKind::Text)=>"compiler_metadata_text_candidate",("copied",LiteralKind::Text)=>"copied_bytecode_text_candidate",
                        (_,LiteralKind::Text)=>"unindexed_data_text_candidate",("metadata",_)=>"compiler_metadata_bytes",("copied",_)=>"copied_bytecode_bytes",_=>"unindexed_data_bytes"},
                    "match_status":if query.normalized.is_some(){"matched"}else{"inventoried"},
                    "text_candidates":if l.text {vec![json!({"decoded_start":l.start,"decoded_end":l.end,"text":std::str::from_utf8(bytes).unwrap()})]}else{Vec::new()},
                    "matches":item.matches,"reference_status":if route.is_some(){"statically_copied"}else if self.unknown+self.unvisited>0{"unreferenced_or_unknown"}else{"unreferenced"},
                    "reference_count":if item.routes[0].is_some(){item.routes.len()}else{0},"context_kind":route.map(|i|if self.witnesses[i].owner.is_some(){"function"}else{"block"}),
                    "context_index":route.and_then(|i|self.witnesses[i].context),"runtime_string_proven":false})))
        }))
    }

    pub(crate) fn refs(
        &self,
        navigation: &Navigation,
        functions: &FunctionNavigation,
        id: &str,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<(DataLiteral, Vec<usize>, &'static str)> {
        work(
            perf,
            id.len() + self.literals.len() + self.witnesses.len(),
            limits.work,
        )?;
        let literal = self.select(navigation, id)?;
        let routes = self.routes(navigation, literal);
        Self::check_records(routes.len(), limits)?;
        let status = if !routes.is_empty() {
            "statically_copied"
        } else if self.unknown + self.unvisited > 0 {
            "unreferenced_or_unknown"
        } else {
            "unreferenced"
        };
        let mut payload_lower_bound = 0usize;
        for &route in &routes {
            let (start, end, _) = Self::available(navigation, &self.witnesses[route]).unwrap();
            let entry = self.entry(functions, route);
            payload_lower_bound = payload_lower_bound
                .saturating_add(self.witnesses[route].entry_blocks.len().saturating_mul(80))
                .saturating_add(entry.stack.len().saturating_mul(24))
                .saturating_add(entry.memory.len().saturating_mul(80));
            payload_lower_bound = payload_lower_bound.saturating_add(
                end.min(literal.end)
                    .saturating_sub(start.max(literal.start))
                    .saturating_mul(2),
            );
            work(perf, 1, limits.work)?;
        }
        if payload_lower_bound > limits.output_bytes {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "EVM copied evidence byte budget exceeded",
            ));
        }
        work(perf, routes.len(), limits.work)?;
        Ok((literal.clone(), routes, status))
    }

    pub(crate) fn reference_record(
        &self,
        navigation: &Navigation,
        functions: &FunctionNavigation,
        id: &str,
        literal: &DataLiteral,
        route: usize,
    ) -> Value {
        let witness = &self.witnesses[route];
        let operation = &witness.operation;
        let (available_start, available_end, padding) =
            Self::available(navigation, witness).unwrap();
        let copied_start = available_start.max(literal.start);
        let copied_end = available_end.min(literal.end);
        let (start, end) = navigation.carrier.carrier_range(copied_start, copied_end);
        let (instruction_start, instruction_end) = navigation
            .carrier
            .carrier_range(operation.pc, operation.pc + 1);
        let context = self.context(navigation, functions, Some(route)).unwrap();
        envelope::record(
            "data_reference",
            RecordMeta {
                source_id: Some(&navigation.carrier.source_id),
                context_id: Some(&context),
                start: Some(start),
                end: Some(end),
                ..RecordMeta::default()
            },
            crate::navigation::object(
                json!({"literal_id":id,"reference_kind":"codecopy","evidence_kind":"contextual_constant_codecopy",
                    "instruction_offset":operation.pc,"instruction_end":operation.pc+1,"instruction_carrier_start":instruction_start,"instruction_carrier_end":instruction_end,
                    "source_offset_hex":operation.source.map(|v|format!("{v:064x}")),"length_hex":operation.length.map(|v|format!("{v:064x}")),
                    "destination_offset_hex":operation.destination.map(|v|format!("{v:064x}")),"available_source_start":available_start,"available_source_end":available_end,
                    "zero_padding_hex":format!("{padding:064x}"),"copied_decoded_start":copied_start,"copied_decoded_end":copied_end,
                    "copied_bytes_hex":hex(&navigation.carrier.bytecode[copied_start..copied_end]),"block_id":navigation.block_id(&navigation.blocks[witness.block]),
                    "context_kind":if witness.owner.is_some(){"function"}else{"block"},"context_index":witness.context,"context_entry_state":self.entry(functions,route).json(),"entry_evidence_block_ids":witness.entry_blocks.iter().map(|&b|navigation.block_id(&navigation.blocks[b])).collect::<Vec<_>>(),"resolution":"resolved","runtime_string_proven":false}),
            ),
        )
    }
}
