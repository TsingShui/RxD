mod args;
mod recursive;
use super::{finish, write_output};
pub(crate) use args::Command;
use args::{Mode, SyntaxArg};
use rxd_core::perf::Perf;
use rxd_core::{Limits, Result};
use rxd_js::{SourceMode, Syntax};

pub(super) fn execute(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    sink: &mut super::Output<'_>,
) -> Result<()> {
    let common = command.common();
    let input = common.input.clone();
    let destination = common.output.clone();
    let emit_perf = common.perf;
    #[cfg(feature = "backend-asar")]
    let entry = common.entry.clone();
    let mode = match common.source_type {
        Mode::Auto => SourceMode::Auto,
        Mode::Script => SourceMode::Script,
        Mode::Module => SourceMode::Module,
    };
    let syntax = match common.syntax {
        SyntaxArg::Auto => Syntax::Auto,
        SyntaxArg::Js => Syntax::Js,
        SyntaxArg::Jsx => Syntax::Jsx,
        SyntaxArg::Ts => Syntax::Ts,
        SyntaxArg::Tsx => Syntax::Tsx,
    };
    let scan = command.scan().map(|s| s.threads);
    let directory = scan.is_some() && input.symlink_metadata().is_ok_and(|m| m.is_dir());
    #[cfg(feature = "backend-asar")]
    let directory = directory && entry.is_none();
    let budget = if directory {
        // Discovery has its own depth/concurrency protection; each file gets its own policy.
        rxd_core::Budget::default()
    } else {
        policy.for_file(&input)?
    };
    let limits = Limits::from(budget);
    let mut perf = Perf::new(emit_perf, "analysis", "js", command.name());
    perf.limits = (!directory).then_some(limits);
    let query = match &command {
        Command::Xref {
            selector,
            byte,
            filter,
            kind,
            case,
            ..
        } => {
            if let Some(kind) = kind
                && kind != "symbol"
            {
                return Err(rxd_core::Error::new(
                    "INVALID_QUERY",
                    format!("JS has one reference kind, `symbol`; got `{kind}`"),
                ));
            }
            let target = if let Some(byte) = byte {
                rxd_js::RefsTarget::Byte(*byte)
            } else if let Some(pattern) = filter {
                rxd_js::RefsTarget::Filter {
                    pattern: pattern.clone(),
                    case_sensitive: case.sensitive(),
                }
            } else {
                rxd_js::RefsTarget::Coordinate(selector.resolve().map_err(super::classify)?)
            };
            rxd_js::Query::Refs { target }
        }
        Command::Slice { selector, json, .. } => {
            let at = selector.resolve().map_err(super::classify)?;
            rxd_js::Query::Slice {
                id: at,
                json: *json,
            }
        }
        _ => command.into_query(),
    };
    let result = (|| {
        let (source, limits) = (None::<rxd_core::Source>, limits);
        #[cfg(feature = "backend-asar")]
        let (source, limits) = if let Some(entry) = entry {
            let selected = super::input::asar_entry(
                &input,
                &entry,
                destination.as_deref(),
                policy,
                &mut perf,
            )?;
            let limits = Limits::from(selected.budget);
            perf.limits = Some(limits);
            let mut source =
                rxd_core::Source::from_bytes(entry, selected.bytes, limits, &mut perf)?;
            source.origin = Some(selected.origin);
            (Some(source), limits)
        } else {
            (source, limits)
        };
        perf.limits = (!directory).then_some(limits);
        let bytes = if let Some(source) = source {
            rxd_js::run_source(source, mode, syntax, query, limits, &mut perf)?
        } else if let Some(threads) = scan
            && directory
        {
            recursive::run(
                recursive::Request {
                    input: &input,
                    mode,
                    syntax,
                    query,
                    policy,
                    max_files: match policy {
                        rxd_core::BudgetPolicy::Fixed(budget) => Some(budget.entries),
                        rxd_core::BudgetPolicy::InputScaled => None,
                    },
                    threads,
                },
                &mut perf,
            )?
        } else {
            rxd_js::run(&input, mode, syntax, query, limits, &mut perf)?
        };
        perf.measure("write", || {
            write_output(&bytes, destination.as_deref(), sink)
        })
    })();
    finish(perf, emit_perf, result)
}
