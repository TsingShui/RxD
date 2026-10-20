mod args;
mod recursive;
use super::{finish, write_output};
pub(crate) use args::Command;
use args::{Mode, SyntaxArg};
use rxd_core::perf::Perf;
use rxd_core::{Limits, Result};
use rxd_js::{SourceMode, Syntax};

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
        entities: budget.entities,
        records: budget.records,
        output_bytes: budget.output_bytes,
        ast_depth: budget.ast_depth,
        semantic_nodes: budget.semantic_nodes,
    };
    let scan = command.scan().map(|s| (s.recursive, s.threads));
    let mut perf = Perf::new(emit_perf, "analysis", "js", command.name());
    perf.limits = Some(limits);
    let query = command.into_query();
    let result = (|| {
        let bytes = if let Some((requested, threads)) = scan
            && (requested || input.symlink_metadata().is_ok_and(|m| m.is_dir()))
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
