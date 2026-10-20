mod args;
use super::{finish, write_output};
pub(crate) use args::Command;
use rxd_core::perf::Perf;
use rxd_core::{Budget, Error, Limits, Result};
use std::io::{self, Write};

pub(super) fn execute(command: Command) -> Result<()> {
    let common = command.common();
    let input = common.input.clone();
    let destination = common.output.clone();
    let debug = common.debug;
    let emit_perf = common.perf;
    let budget = Budget::default();
    let limits = Limits {
        input_bytes: budget.input_bytes,
        records: budget.records,
        output_bytes: budget.output_bytes,
        ..Limits::default()
    };
    let max_entries = budget.entries;
    let max_header_bytes = budget.metadata_bytes;
    let mut perf = Perf::new(debug || emit_perf, "analysis", "asar", command.name());
    perf.limits = Some(limits);
    let query = match command {
        Command::Entries { filter, case, .. } => rxd_asar::Query::Entries {
            contains: filter.clone(),
            case_sensitive: case.sensitive(),
        },
        Command::Extract { at, .. } => rxd_asar::Query::Extract { at: at.clone() },
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
    finish(perf, emit_perf, debug, result)
}
