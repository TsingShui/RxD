//! Evidenced static control-flow references, never a recovered source call graph.
use rxd_core::{
    Error, Result,
    envelope::{self, RecordMeta},
    perf::Perf,
};
use serde_json::{Value, json};

use crate::{
    Edge, FunctionNavigation, Limits, Navigation, Output,
    functions::block_local_edges,
    navigation::{Listing, object, work, write_record},
    references::{self, HeaderDetail, SummaryDetail},
};

#[derive(Default)]
struct Coverage {
    resolved_edges: usize,
    unknown_jumps: usize,
    invalid_jumps: usize,
    analysis_problems: usize,
    scanned_contexts: usize,
    scanned_blocks: usize,
    external_call_sites: usize,
}

impl Coverage {
    fn edge(&mut self, edge: &Edge) {
        match edge.resolution {
            "resolved" => self.resolved_edges += 1,
            "unknown" => self.unknown_jumps += 1,
            "invalid" => self.invalid_jumps += 1,
            _ => unreachable!("validated native edge resolution"),
        }
    }
}

struct Hit {
    block: usize,
    function: Option<usize>,
    context: Option<usize>,
    edge: Edge,
    incoming_stack_words: Option<usize>,
}

struct References<'a> {
    id: &'a str,
    target_kind: &'static str,
    target: usize,
    target_validated: bool,
    hits: Vec<Hit>,
    coverage: Coverage,
}

impl<'a> References<'a> {
    fn scan(
        navigation: &Navigation,
        functions: &FunctionNavigation,
        id: &'a str,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<Self> {
        let (target_kind, target) = match id.split(':').nth(1) {
            Some("block") => ("block", navigation.select_block(id)?.start),
            Some("function") => (
                "function",
                functions.select(navigation, Some(id), None)?.entry,
            ),
            _ => {
                return Err(Error::new(
                    "INVALID_QUERY",
                    "expected an EVM block or function ID",
                ));
            }
        };
        let target_validated = target_kind == "block"
            || navigation
                .blocks
                .binary_search_by_key(&target, |b| b.start)
                .is_ok_and(|index| {
                    target == 0
                        || navigation.instructions[navigation.blocks[index].instruction_start]
                            .opcode
                            == 0x5b
                });
        let mut references = Self {
            id,
            target_kind,
            target,
            target_validated,
            hits: Vec::new(),
            coverage: Coverage::default(),
        };
        let mut witnessed = vec![false; navigation.blocks.len()];
        for (function_index, function) in functions.functions.iter().enumerate() {
            work(perf, 1 + function.unknown_successors.len(), limits.work)?;
            // Other problems are counted in the concrete context that produced them.
            references.coverage.analysis_problems += function
                .unknown_successors
                .iter()
                .filter(|p| p.kind == "entry")
                .count();
            for (context_index, context) in function.contexts.iter().enumerate() {
                work(perf, 1 + context.problems.len(), limits.work)?;
                witnessed[context.block_index] = true;
                references.coverage.scanned_contexts += 1;
                references.coverage.analysis_problems += context
                    .problems
                    .iter()
                    .filter(|p| {
                        !matches!(
                            p.reason,
                            "dynamic_jump_destination" | "invalid_jump_destination"
                        )
                    })
                    .count();
                for edge in &context.edges {
                    references.consider(
                        Hit {
                            block: context.block_index,
                            function: Some(function_index),
                            context: Some(context_index),
                            edge: edge.clone(),
                            incoming_stack_words: None,
                        },
                        limits,
                        perf,
                    )?;
                }
            }
        }
        for (index, seen) in witnessed.iter().enumerate() {
            work(perf, 1, limits.work)?;
            if *seen {
                continue;
            }
            references.coverage.scanned_blocks += 1;
            let (edges, problems, incoming_stack_words) =
                block_local_edges(navigation, index, limits, perf)?;
            references.coverage.analysis_problems += problems
                .iter()
                .filter(|p| {
                    !matches!(
                        p.reason,
                        "dynamic_jump_destination" | "invalid_jump_destination"
                    )
                })
                .count();
            for edge in edges {
                references.consider(
                    Hit {
                        block: index,
                        function: None,
                        context: None,
                        edge,
                        incoming_stack_words: Some(incoming_stack_words),
                    },
                    limits,
                    perf,
                )?;
            }
        }
        // Physical decoded opcode sites, not executed or resolved external ABI calls.
        for instruction in &navigation.instructions {
            work(perf, 1, limits.work)?;
            if matches!(instruction.opcode, 0xf1 | 0xf2 | 0xf4 | 0xfa) {
                references.coverage.external_call_sites += 1;
            }
        }
        perf.counters.records = Some(references.hits.len());
        perf.counters.matches = Some(references.hits.len());
        perf.counters.semantic_references = Some(
            references.coverage.resolved_edges
                + references.coverage.unknown_jumps
                + references.coverage.invalid_jumps,
        );
        Ok(references)
    }

    fn consider(&mut self, hit: Hit, limits: Limits, perf: &mut Perf) -> Result<()> {
        work(perf, 1, limits.work)?;
        self.coverage.edge(&hit.edge);
        if !self.target_validated
            || hit.edge.resolution != "resolved"
            || hit.edge.target != Some(self.target)
        {
            return Ok(());
        }
        self.hits.push(hit);
        Ok(())
    }

    fn record(&self, navigation: &Navigation, functions: &FunctionNavigation, hit: &Hit) -> Value {
        let instruction = &navigation.instructions[navigation
            .instructions
            .binary_search_by_key(&hit.edge.instruction_offset, |i| i.decoded_offset)
            .expect("validated original instruction")];
        let block_id = navigation.block_id(&navigation.blocks[hit.block]);
        let context_id = hit.function.map_or_else(
            || block_id.clone(),
            |i| functions.function_id(navigation, &functions.functions[i]),
        );
        let (start, end) = navigation
            .carrier
            .carrier_range(instruction.decoded_offset, instruction.decoded_end);
        envelope::record(
            "code_reference",
            RecordMeta {
                context_at: Some(&context_id),
                start: Some(start),
                end: Some(end),
                ..RecordMeta::default()
            },
            object(
                json!({"target":self.id,"target_kind":self.target_kind,"target_decoded_offset":self.target,
            "reference_kind":hit.edge.kind,"resolution":hit.edge.resolution,"instruction_offset":hit.edge.instruction_offset,
            "opcode":instruction.opcode,"operand_hex":instruction.operand_hex,"context_kind":if hit.function.is_some(){"function"}else{"block"},
            "referencing_block_id":block_id,"referencing_context_index":hit.context,"target_context_index":hit.edge.target_context,
            "incoming_stack_words":hit.incoming_stack_words,
            "evidence_kind":if hit.function.is_some(){"contextual_stack"}else{"block_local_stack"},
            "owner_ids":if hit.function.is_some(){vec![context_id.clone()]}else{vec![]},
            "shared":functions.owners[hit.block].len()>1,"ownership_status":if hit.function.is_some(){"candidate"}else{"unresolved"},
            "source_call_verified":false,"abi_call_verified":false}),
            ),
        )
    }

    fn json(
        &self,
        navigation: &Navigation,
        functions: &FunctionNavigation,
        listing: Listing<'_>,
    ) -> Result<Vec<u8>> {
        let needle = listing
            .filter
            .map(|needle| rxd_core::filter::Contains::new(needle, listing.case_sensitive));
        let header = references::header(
            navigation,
            self.id,
            listing,
            HeaderDetail {
                data: None,
                target_kind: self.target_kind,
                target_decoded_offset: self.target,
                target_semantics: if self.target_kind == "block" {
                    "exact_block_start"
                } else if self.target_validated {
                    "validated_candidate_entry"
                } else {
                    "unvalidated_candidate_entry"
                },
                coverage: "contextual_and_block_local",
                literal_scope: None,
                excluded_unindexed_pushes: None,
            },
        );
        let coverage = &self.coverage;
        let mut output = Output { bytes: Vec::new() };
        write_record(&mut output, &header)?;
        let mut emitted = 0;
        for hit in &self.hits {
            let record = self.record(navigation, functions, hit);
            if !references::keeps(&record, needle.as_ref()) {
                continue;
            }
            write_record(&mut output, &record)?;
            emitted += 1;
        }
        write_record(
            &mut output,
            &references::summary(
                navigation,
                SummaryDetail {
                    target_kind: self.target_kind,
                    target_decoded_offset: self.target,
                    references: emitted,
                    resolved_edges: Some(coverage.resolved_edges),
                    unknown_jumps: Some(coverage.unknown_jumps),
                    invalid_jumps: Some(coverage.invalid_jumps),
                    analysis_problems: Some(coverage.analysis_problems),
                    scanned_contexts: Some(coverage.scanned_contexts),
                    scanned_blocks: Some(coverage.scanned_blocks),
                    external_call_sites: Some(coverage.external_call_sites),
                    resolution_complete: Some(
                        coverage.unknown_jumps == 0
                            && coverage.invalid_jumps == 0
                            && coverage.analysis_problems == 0
                            && navigation.decoded_end == navigation.carrier.bytecode.len(),
                    ),
                    ..SummaryDetail::default()
                },
            ),
        )?;
        Ok(output.bytes)
    }
}

pub fn refs_json(
    navigation: &Navigation,
    functions: &FunctionNavigation,
    id: &str,
    listing: Listing<'_>,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Vec<u8>> {
    let stage = perf.start_stage();
    let result = References::scan(navigation, functions, id, limits, perf);
    perf.record("query", stage);
    let references = result?;
    perf.measure("serialize", || {
        references.json(navigation, functions, listing)
    })
}
