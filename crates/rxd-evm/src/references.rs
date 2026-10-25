//! Typed reference dispatch and one fixed JSONL stream contract for EVM references.
use rxd_core::{Error, Result, perf::Perf};
use serde_json::{Value, json};

use crate::{
    FunctionNavigation, Limits, LiteralInventory, Navigation,
    navigation::{Listing, analysis_detail, analysis_record, object},
};

#[derive(Default)]
pub(crate) struct AnalysisDetail {
    pub target_kind: &'static str,
    pub target_decoded_offset: usize,
    pub target_semantics: &'static str,
    pub coverage: &'static str,
    pub data: Option<crate::data::DataCoverage>,
    pub literal_scope: Option<&'static str>,
    pub excluded_unindexed_pushes: Option<usize>,
}

pub(crate) fn analysis(
    navigation: &Navigation,
    listing: Listing<'_>,
    detail: AnalysisDetail,
) -> Value {
    analysis_record(analysis_detail(object(json!({
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
            "unknown_copy_evidence":detail.data.as_ref().map(|d|&d.unknown_copy_evidence)}))))
}

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
