mod args;
use super::{finish, write_output};
pub(crate) use args::Command;
use rxd_core::perf::Perf;
use rxd_core::{Error, Limits, Result};
use std::io::{self, Write};

pub(super) fn execute(command: Command, policy: rxd_core::BudgetPolicy) -> Result<()> {
    let common = command.common();
    let input = common.input.clone();
    let destination = common.output.clone();
    let emit_perf = common.perf;
    let budget = policy.for_file(&input)?;
    let mut limits = Limits::from(budget);
    // Unpacked children are not counted by the archive carrier. Their actual length
    // is checked on the opened child handle; retain the content-span guard.
    if matches!(policy, rxd_core::BudgetPolicy::InputScaled) {
        limits.input_bytes = rxd_core::MAX_SPAN_BYTES;
    }
    let max_entries = budget.entries;
    let max_header_bytes = budget.metadata_bytes;
    let mut perf = Perf::new(emit_perf, "analysis", "asar", command.name());
    perf.limits = Some(limits);
    let query = match command {
        Command::Entries { filter, case, .. } => rxd_asar::Query::Entries {
            contains: filter.clone(),
            case_sensitive: case.sensitive(),
        },
        Command::Extract { selector, .. } => rxd_asar::Query::Extract {
            at: selector.resolve().map_err(super::classify)?,
        },
    };
    let result = (|| {
        let response = rxd_asar::run(
            query,
            rxd_asar::Options {
                input: &input,
                output: destination.as_deref(),
                limits,
                max_entries,
                max_header_bytes,
            },
            &mut perf,
        )?;
        perf.measure("write", || match response {
            rxd_asar::Response::List(bytes) => write_output(&bytes, destination.as_deref()),
            rxd_asar::Response::Extract { bytes, receipt } => {
                write_output(&bytes, destination.as_deref())?;
                io::stdout().lock().write_all(&receipt).map_err(Error::from)
            }
        })
    })();
    finish(perf, emit_perf, result)
}
