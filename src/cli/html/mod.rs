mod args;
use super::xref::Expect;
use super::{finish, write_output};
pub(crate) use args::Command;
use rxd_core::{Budget, Error, Limits, Result, perf::Perf};
use std::io::{self, Write};

pub(super) fn execute(command: Command) -> Result<()> {
    let (common, name) = match &command {
        Command::Entries { common, .. } | Command::Extract { common, .. } => {
            (common, command.name())
        }
    };
    // One budget table for every backend; the CLI exposes no knobs for it.
    let budget = Budget::default();
    let limits = Limits {
        input_bytes: budget.input_bytes,
        records: budget.records,
        output_bytes: budget.output_bytes,
        ast_depth: 512,
        ..Limits::default()
    };
    let mut perf = Perf::new(common.perf, "analysis", "html", name);
    perf.limits = Some(limits);
    let emit_perf = common.perf;
    let result = (|| match command {
        Command::Entries {
            common,
            kind,
            tag,
            attr,
            within,
            depth,
            filter,
            case,
        } => {
            let bytes = rxd_html::list(
                &common.input,
                rxd_html::ListQuery {
                    kind,
                    tag,
                    attr,
                    within,
                    depth,
                    contains: filter,
                    case_sensitive: case.sensitive(),
                },
                limits,
                &mut perf,
            )?;
            perf.measure("write", || write_output(&bytes, common.output.as_deref()))
        }
        Command::Extract {
            common,
            selector,
            content,
        } => {
            let destination = common
                .output
                .as_deref()
                .ok_or_else(|| Error::new("INVALID_QUERY", "extract requires --output"))?;
            // HTML hashes the file inside the query, so the stream is checked against the
            // input size here; that is a mismatch guard, not a content identity.
            let bytes = std::fs::metadata(&common.input)
                .map_err(|error| Error::new("IO_ERROR", error.to_string()))?
                .len();
            let at = selector
                .resolve(
                    &["element", "text", "comment", "doctype"],
                    Expect::Bytes(bytes),
                )
                .map_err(|error| Error::new("INVALID_QUERY", format!("{error:#}")))?;
            let response =
                rxd_html::extract(&common.input, &at, content, destination, limits, &mut perf)?;
            perf.measure("write", || {
                write_output(&response.bytes, Some(destination))?;
                io::stdout()
                    .lock()
                    .write_all(&response.receipt)
                    .map_err(Error::from)
            })
        }
    })();
    finish(perf, emit_perf, result)
}
