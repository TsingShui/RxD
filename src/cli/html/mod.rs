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
    #[cfg(feature = "backend-asar")]
    let entry = common.entry.clone();
    #[cfg(feature = "backend-asar")]
    let input = common.input.clone();
    #[cfg(feature = "backend-asar")]
    let output = common.output.clone();
    let result = (|| {
        let (source, limits) = (None::<rxd_html::Source>, limits);
        #[cfg(feature = "backend-asar")]
        let (source, limits) = if let Some(entry) = entry {
            let selected =
                super::input::asar_entry(&input, &entry, output.as_deref(), policy, &mut perf)?;
            let limits = Limits::from(selected.budget);
            perf.limits = Some(limits);
            let mut source =
                rxd_html::Source::from_bytes(entry, selected.bytes, limits, &mut perf)?;
            source.container_origin = Some(selected.origin);
            (Some(source), limits)
        } else {
            (source, limits)
        };
        match command {
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
                let query = rxd_html::ListQuery {
                    kind,
                    tag,
                    attr,
                    within,
                    depth,
                    contains: filter,
                    case_sensitive: case.sensitive(),
                };
                let bytes = match source {
                    Some(source) => source.list(query, &mut perf),
                    None => rxd_html::list(&common.input, query, limits, &mut perf),
                }?;
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
                let query = rxd_html::ResourcesQuery {
                    base_url,
                    kind,
                    location,
                    filter,
                    case_sensitive: case.sensitive(),
                    include_content,
                };
                let bytes = match source {
                    Some(source) => source.resources(query, &mut perf),
                    None => rxd_html::resources(&common.input, query, limits, &mut perf),
                }?;
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
                let source = match source {
                    Some(source) => source,
                    None => rxd_html::Source::read(&common.input, limits, &mut perf)?,
                };
                let response = source.extract(&at, content, destination, &mut perf)?;
                perf.measure("write", || {
                    write_output(&response.bytes, Some(destination), sink)?;
                    sink.write_all(&response.receipt).map_err(Error::from)
                })
            }
        }
    })();
    finish(perf, emit_perf, result)
}
