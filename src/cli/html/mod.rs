mod args;
use super::{finish, write_output};
pub(crate) use args::Command;
use rxd_core::{Error, Limits, Result, perf::Perf};
use std::io::{self, Write};

pub(super) fn execute(command: Command) -> Result<()> {
    let (common, name) = match &command {
        Command::List { common, .. } | Command::Extract { common, .. } => (common, command.name()),
    };
    let limits = Limits {
        input_bytes: super::input_limit(common.max_input_bytes),
        records: common.max_records,
        output_bytes: common.max_output_bytes,
        ast_depth: common.max_depth,
        ..Limits::default()
    };
    let mut perf = Perf::new(common.debug || common.perf, "analysis", "html", name);
    perf.limits = Some(limits);
    let emit_perf = common.perf;
    let debug = common.debug;
    let result = (|| match command {
        Command::List {
            common,
            kind,
            tag,
            attr,
            within,
            depth,
        } => {
            let bytes = rxd_html::list(
                &common.input,
                rxd_html::ListQuery {
                    kind,
                    tag,
                    attr,
                    within,
                    depth,
                },
                limits,
                &mut perf,
            )?;
            perf.measure("write", || write_output(&bytes, common.output.as_deref()))
        }
        Command::Extract {
            common,
            id,
            content,
        } => {
            let destination = common
                .output
                .as_deref()
                .ok_or_else(|| Error::new("INVALID_QUERY", "extract requires --output"))?;
            let response =
                rxd_html::extract(&common.input, &id, content, destination, limits, &mut perf)?;
            perf.measure("write", || {
                write_output(&response.bytes, Some(destination))?;
                io::stdout()
                    .lock()
                    .write_all(&response.receipt)
                    .map_err(Error::from)
            })
        }
    })();
    finish(perf, emit_perf, debug, result)
}
