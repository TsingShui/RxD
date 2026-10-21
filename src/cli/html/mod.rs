mod args;
use super::xref::Expect;
use super::{finish, write_output};
pub(crate) use args::Command;
use rxd_core::{Error, Limits, Result, perf::Perf};
use std::io::{self, Write};

pub(super) fn execute(command: Command, policy: rxd_core::BudgetPolicy) -> Result<()> {
    let (common, name) = match &command {
        Command::Entries { common, .. }
        | Command::Resources { common, .. }
        | Command::Extract { common, .. } => (common, command.name()),
    };
    let limits = Limits::from(policy.for_file(&common.input)?);
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
        Command::Resources {
            common,
            base_url,
            kind,
            location,
            filter,
            case,
            include_content,
        } => {
            let bytes = rxd_html::resources(
                &common.input,
                rxd_html::ResourcesQuery {
                    base_url,
                    kind,
                    location,
                    filter,
                    case_sensitive: case.sensitive(),
                    include_content,
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
            // Bind --from to the very buffer extraction will use, not merely its length
            // or a separate read that could change before the artifact is selected.
            let source = rxd_html::Source::read(&common.input, limits, &mut perf)?;
            let at = selector
                .resolve(
                    &["element", "text", "comment", "doctype", "resource"],
                    Expect::Content(source.source_id()),
                )
                .map_err(super::classify)?;
            let response = source.extract(&at, content, destination, &mut perf)?;
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
