mod args;
use super::{finish, write_output};
pub(crate) use args::Command;
use rxd_core::perf::Perf;
use rxd_core::{Error, Limits, Result};
use std::io::{self, Write};

pub(super) fn execute(command: Command) -> Result<()> {
    let common = command.common();
    let input = common.input.clone();
    let destination = common.output.clone();
    let debug = common.debug;
    let emit_perf = common.perf;
    let limits = Limits {
        input_bytes: super::input_limit(common.max_input_bytes),
        records: common.max_records,
        output_bytes: common.max_output_bytes,
        ..Limits::default()
    };
    let max_entries = common.max_entries;
    let max_header_bytes = common.max_header_bytes;
    let mut perf = Perf::new(debug || emit_perf, "analysis", "asar", command.name());
    perf.limits = Some(limits);
    let query = match command {
        Command::List { .. } => rxd_asar::Query::List,
        Command::Extract { file, .. } => rxd_asar::Query::Extract { file },
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
