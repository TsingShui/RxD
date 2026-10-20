mod args;
mod recursive;
use super::{finish, write_output};
pub(crate) use args::Command;
use args::{Mode, SyntaxArg};
use rxd_core::perf::Perf;
use rxd_core::{Limits, Result};
use rxd_js::{SourceMode, Syntax};

/// Resolve `--at`/`--from` into the name occurrence `xref` anchors on.
///
/// The stream is checked against this file's content id, and the content hash is computed
/// only on the `--from` path so a direct `--at` query still reads the file once.
fn resolve_xref_anchor(
    input: &std::path::Path,
    selector: &super::xref::Selector,
) -> Result<String> {
    use super::xref::Expect;
    let id = if selector.from.is_some() {
        let bytes = std::fs::read(input)
            .map_err(|error| rxd_core::Error::new("IO_ERROR", error.to_string()))?;
        Some(rxd_core::identity::content_id(input, &bytes))
    } else {
        None
    };
    let expected = match id.as_deref() {
        Some(id) => Expect::Content(id),
        None => Expect::Bytes(
            std::fs::metadata(input)
                .map_err(|error| rxd_core::Error::new("IO_ERROR", error.to_string()))?
                .len(),
        ),
    };
    selector
        .resolve(&["symbol", "identifier"], expected)
        // The selector already reports its class (`SOURCE_MISMATCH`, `INVALID_QUERY`);
        // re-wrapping would print two codes for one failure.
        .map_err(super::classify)
}

pub(super) fn execute(command: Command) -> Result<()> {
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
    let budget = rxd_core::Budget::default();
    let limits = Limits {
        input_bytes: budget.input_bytes,
        ast_depth: budget.ast_depth,
        semantic_nodes: budget.semantic_nodes,
    };
    let scan = command.scan().map(|s| s.threads);
    let mut perf = Perf::new(emit_perf, "analysis", "js", command.name());
    perf.limits = Some(limits);
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
                rxd_js::RefsTarget::Coordinate(resolve_xref_anchor(&input, selector)?)
            };
            rxd_js::Query::Refs { target }
        }
        Command::Slice { selector, json, .. } => {
            // The stream is checked against this file's content id. The hash is computed
            // only on the `--from` path, so a direct `--at` fetch still reads once.
            let id = if selector.from.is_some() {
                let bytes = std::fs::read(&input)
                    .map_err(|error| rxd_core::Error::new("IO_ERROR", error.to_string()))?;
                Some(rxd_core::identity::content_id(&input, &bytes))
            } else {
                None
            };
            let at = match id.as_deref() {
                Some(id) => selector.resolve(
                    &["entity", "symbol", "fragment"],
                    super::xref::Expect::Content(id),
                ),
                None => selector.resolve(
                    &["entity", "symbol", "fragment"],
                    super::xref::Expect::Bytes(
                        std::fs::metadata(&input)
                            .map_err(|error| rxd_core::Error::new("IO_ERROR", error.to_string()))?
                            .len(),
                    ),
                ),
            }
            // The selector already reports its class (`SOURCE_MISMATCH`, `INVALID_QUERY`);
            // re-wrapping would print two codes for one failure.
            .map_err(super::classify)?;
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
        let bytes = if let Some(threads) = scan
            && input.symlink_metadata().is_ok_and(|m| m.is_dir())
        {
            recursive::run(
                recursive::Request {
                    input: &input,
                    mode,
                    syntax,
                    query,
                    limits,
                    max_files: budget.entries,
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
