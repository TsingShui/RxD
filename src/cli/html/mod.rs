mod args;
use super::{finish, write_output};
pub(crate) use args::Command;
use rxd_core::{Error, Limits, Result, perf::Perf};
use std::io::Write;

pub(super) fn execute(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    sink: &mut super::Output<'_>,
) -> Result<()> {
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
            perf.measure("write", || {
                write_output(&bytes, common.output.as_deref(), sink)
            })
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
            perf.measure("write", || {
                write_output(&bytes, common.output.as_deref(), sink)
            })
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
            let at = selector.resolve().map_err(super::classify)?;
            let source = rxd_html::Source::read(&common.input, limits, &mut perf)?;
            let response = source.extract(&at, content, destination, &mut perf)?;
            perf.measure("write", || {
                write_output(&response.bytes, Some(destination), sink)?;
                sink.write_all(&response.receipt).map_err(Error::from)
            })
        }
    })();
    finish(perf, emit_perf, result)
}
