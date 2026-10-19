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
    let debug = common.debug;
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
    let limits = Limits {
        input_bytes: super::input_limit(common.max_input_bytes),
        entities: common.max_entities,
        records: common.max_records,
        output_bytes: common.max_output_bytes,
        ast_depth: common.max_ast_depth,
        semantic_nodes: command.semantic_nodes(),
    };
    let scan = command
        .scan()
        .map(|s| (s.recursive, s.max_files, s.threads));
    let mut perf = Perf::new(debug || emit_perf, "analysis", "js", command.name());
    perf.limits = Some(limits);
    let query = command.into_query();
    let result = (|| {
        let bytes = if let Some((requested, max_files, threads)) = scan
            && (requested || input.symlink_metadata().is_ok_and(|m| m.is_dir()))
        {
            recursive::run(
                recursive::Request {
                    input: &input,
                    mode,
                    syntax,
                    query,
                    limits,
                    max_files,
                    threads,
                },
                &mut perf,
            )?
        } else {
            rxd_js::run(&input, mode, syntax, query, limits, &mut perf)?
        };
        perf.measure("write", || write_output(&bytes, destination.as_deref()))
    })();
    finish(perf, emit_perf, debug, result)
}
