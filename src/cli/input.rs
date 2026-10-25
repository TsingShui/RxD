//! Selected container bytes and admission shared by code-backend adapters.
use std::path::Path;

use rxd_core::{Budget, BudgetPolicy, Limits, Origin, Result, perf::Perf};

pub(super) struct Entry {
    pub bytes: Vec<u8>,
    pub budget: Budget,
    pub origin: Origin,
}

pub(super) fn asar_entry(
    input: &Path,
    at: &str,
    output: Option<&Path>,
    policy: BudgetPolicy,
    perf: &mut Perf,
) -> Result<Entry> {
    let container_budget = policy.for_file(input)?;
    let mut limits = Limits::from(container_budget);
    if matches!(policy, BudgetPolicy::InputScaled) {
        limits.input_bytes = rxd_core::MAX_SPAN_BYTES;
    }
    let bytes = rxd_asar::read_entry(
        at,
        rxd_asar::Options {
            input,
            output,
            limits,
            max_entries: container_budget.entries,
            max_header_bytes: container_budget.metadata_bytes,
        },
        perf,
    )?;
    let budget = match policy {
        BudgetPolicy::InputScaled => Budget::for_input_bytes(bytes.len())?,
        BudgetPolicy::Fixed(budget) => budget,
    };
    Ok(Entry {
        bytes,
        budget,
        origin: Origin {
            container: input.to_string_lossy().into_owned(),
            format: "asar",
            entry: at.to_owned(),
        },
    })
}
