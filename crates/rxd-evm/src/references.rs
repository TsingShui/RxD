//! Typed reference dispatch and one fixed JSONL stream contract for EVM references.
use rxd_core::{Error, Result, envelope, perf::Perf};
use serde_json::{Value, json};

use crate::{
    FunctionNavigation, Limits, LiteralInventory, Navigation,
    navigation::{Listing, header_detail, object, summary_detail},
};

#[derive(Default)]
pub(crate) struct HeaderDetail {
    pub target_kind: &'static str,
    pub target_decoded_offset: usize,
    pub target_semantics: &'static str,
    pub coverage: &'static str,
    pub data: Option<crate::data::DataCoverage>,
    pub literal_scope: Option<&'static str>,
    pub excluded_unindexed_pushes: Option<usize>,
}

#[derive(Default)]
pub(crate) struct SummaryDetail {
    pub target_kind: &'static str,
    pub target_decoded_offset: usize,
    pub references: usize,
    pub literals: Option<usize>,
    pub data_literals: Option<usize>,
    pub reference_status: Option<&'static str>,
    pub excluded_unindexed_pushes: Option<usize>,
    pub resolved_edges: Option<usize>,
    pub unknown_jumps: Option<usize>,
    pub invalid_jumps: Option<usize>,
    pub analysis_problems: Option<usize>,
    pub scanned_contexts: Option<usize>,
    pub scanned_blocks: Option<usize>,
    pub external_call_sites: Option<usize>,
    pub resolution_complete: Option<bool>,
}

pub(crate) fn header(
    navigation: &Navigation,
    id: &str,
    listing: Listing<'_>,
    detail: HeaderDetail,
) -> Value {
    envelope::header(
        "evm",
        "xref",
        Some(&navigation.carrier.source_id),
        Some(navigation.carrier.bytes.len()),
        json!({"encoding":navigation.carrier.encoding,"at":id,"filter":listing.filter,
            "reference_scope":if detail.target_kind=="literal"{"exact_literal_occurrence"}else{"static_control_flow"}}),
        header_detail(object(json!({
            "carrier":navigation.carrier_detail(),"bytecode":navigation.bytecode_detail(listing.include_bytecode),
            "offset_unit":"decoded_bytecode_byte","target_kind":detail.target_kind,
            "target_decoded_offset":detail.target_decoded_offset,"target_semantics":detail.target_semantics,
            "coverage":detail.coverage,"literal_scope":detail.literal_scope,
            "excluded_unindexed_pushes":detail.excluded_unindexed_pushes,"discovery_complete":null,
            "disassembly_complete":navigation.decoded_end==navigation.carrier.bytecode.len(),
            "undecoded_tail":navigation.undecoded_tail(),"source_call_graph_complete":false,"abi_verified":false,
            "data_scope":detail.data.as_ref().map(|d|d.data_scope), "metadata_range":detail.data.as_ref().and_then(|d|d.metadata_range.as_ref()),
            "copy_witnesses":detail.data.as_ref().map(|d|d.copy_witnesses),"unknown_copy_witnesses":detail.data.as_ref().map(|d|d.unknown_copy_witnesses),
            "unvisited_copy_sites":detail.data.as_ref().map(|d|d.unvisited_copy_sites),"data_reference_coverage":detail.data.as_ref().map(|d|d.data_reference_coverage),
            "unknown_copy_evidence":detail.data.as_ref().map(|d|&d.unknown_copy_evidence)}))),
    )
}

pub(crate) fn summary(navigation: &Navigation, detail: SummaryDetail) -> Value {
    envelope::summary(
        detail.references,
        true,
        summary_detail(object(
            json!({"target_kind":detail.target_kind,"target_decoded_offset":detail.target_decoded_offset,
            "references":detail.references,"literals":detail.literals,"data_literals":detail.data_literals,"reference_status":detail.reference_status,
            "excluded_unindexed_pushes":detail.excluded_unindexed_pushes,"resolved_edges":detail.resolved_edges,
            "unknown_jumps":detail.unknown_jumps,"invalid_jumps":detail.invalid_jumps,
            "analysis_problems":detail.analysis_problems,"scanned_contexts":detail.scanned_contexts,
            "scanned_blocks":detail.scanned_blocks,"external_call_sites":detail.external_call_sites,
            "resolution_complete":detail.resolution_complete,"discovery_complete":null,
            "disassembly_complete":navigation.decoded_end==navigation.carrier.bytecode.len(),
            "source_call_graph_complete":false}),
        )),
    )
}

/// A filter narrows a reference listing to rows whose caller coordinate matches.
pub(crate) fn keeps(record: &Value, needle: Option<&rxd_core::filter::Contains>) -> bool {
    needle.is_none_or(|needle| {
        record["context_at"]
            .as_str()
            .is_some_and(|context| needle.matches(context))
    })
}

pub fn refs_json(
    navigation: &Navigation,
    functions: &FunctionNavigation,
    at: &str,
    listing: Listing<'_>,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Vec<u8>> {
    let id = at.to_string();
    match id.split(':').nth(1) {
        Some("literal") => LiteralInventory::discover(navigation, limits, perf)?
            .refs_json(navigation, functions, &id, listing, limits, perf),
        Some("function" | "block") => {
            crate::code_references::refs_json(navigation, functions, &id, listing, limits, perf)
        }
        _ => Err(Error::new(
            "INVALID_QUERY",
            "expected an EVM literal, block or function ID",
        )),
    }
}
