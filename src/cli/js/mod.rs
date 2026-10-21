mod args;
mod recursive;
use super::{finish, write_output};
pub(crate) use args::Command;
use args::{Mode, SyntaxArg};
use rxd_core::perf::Perf;
use rxd_core::{Limits, Result};
use rxd_js::{SourceMode, Syntax};

/// Resolve a query/read anchor using the same input buffer as the analysis.
///
/// The stream is checked against this file's content id, and the content hash is computed
/// on the `--from` path; direct coordinates retain the regular single read.
fn resolve_anchor(
    input: &std::path::Path,
    selector: &super::xref::Selector,
    units: &[&str],
    frozen: Option<&rxd_core::Source>,
) -> Result<String> {
    use super::xref::Expect;
    let expected = match frozen {
        Some(source) => Expect::Content(&source.id),
        None => Expect::Bytes(
            std::fs::metadata(input)
                .map_err(|error| rxd_core::Error::new("IO_ERROR", error.to_string()))?
                .len(),
        ),
    };
    selector
        .resolve(units, expected)
        // The selector already reports its class (`SOURCE_MISMATCH`, `INVALID_QUERY`);
        // re-wrapping would print two codes for one failure.
        .map_err(super::classify)
}

pub(super) fn execute(command: Command, policy: rxd_core::BudgetPolicy) -> Result<()> {
    let common = command.common();
    let input = common.input.clone();
    let destination = common.output.clone();
    let emit_perf = common.perf;
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
    let budget = if directory {
        // Discovery has its own depth/concurrency protection; each file gets its own policy.
        rxd_core::Budget::default()
    } else {
        policy.for_file(&input)?
    };
    let limits = Limits::from(budget);
    let mut perf = Perf::new(emit_perf, "analysis", "js", command.name());
    perf.limits = (!directory).then_some(limits);
    // Validate --from and analyze/extract from this exact buffer, never a second read.
    let frozen = if matches!(&command, Command::Slice { selector, .. } | Command::Xref { selector, .. } if selector.from.is_some())
    {
        Some(rxd_core::Source::read(&input, limits, &mut perf)?)
    } else {
        None
    };
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
                rxd_js::RefsTarget::Coordinate(resolve_anchor(
                    &input,
                    selector,
                    &["symbol", "identifier"],
                    frozen.as_ref(),
                )?)
            };
            rxd_js::Query::Refs { target }
        }
        Command::Slice { selector, json, .. } => {
            let at = resolve_anchor(
                &input,
                selector,
                &["entity", "symbol", "fragment"],
                frozen.as_ref(),
            )?;
            rxd_js::Query::Slice {
                id: at,
                json: *json,
            }
        }
        _ => command.into_query(),
    };
    let result = (|| {
        // A directory is a scan; a file is one input. There is no flag to choose,
        // because the same file must produce the same stream either way.
        let bytes = if let Some(source) = frozen {
            rxd_js::run_source(source, mode, syntax, query, limits, &mut perf)?
        } else if let Some(threads) = scan
            && input.symlink_metadata().is_ok_and(|m| m.is_dir())
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
        perf.measure("write", || write_output(&bytes, destination.as_deref()))
    })();
    finish(perf, emit_perf, result)
}
