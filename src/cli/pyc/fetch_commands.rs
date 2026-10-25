use super::{Output, static_commands};
use clap::Args;
use rxd_core::{Result, perf::Perf};
use rxd_pyc::code_view::{self, Evidence};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Args)]
pub struct FetchOptions {
    #[command(flatten)]
    pub common: static_commands::Options,
}
#[derive(Serialize)]
struct Detail<'a> {
    input_bytes: usize,
    python_version: String,
    representation: &'static str,
    complete: bool,
    query: Query<'a>,
    metadata: static_commands::Detail<'a>,
    #[serde(flatten)]
    evidence: Evidence,
}
#[derive(Serialize)]
struct Query<'a> {
    code_id: &'a str,
    version: Option<String>,
}
#[derive(Serialize)]
struct View<'a> {
    schema_version: u8,
    backend: &'static str,
    command: &'static str,
    record: &'static str,
    at: String,
    source_id: Option<&'a str>,
    parent_at: Option<String>,
    context_at: Option<&'a str>,
    start: usize,
    end: usize,
    detail: Detail<'a>,
}
pub fn fetch(
    input: PathBuf,
    selector: super::super::xref::Selector,
    version: Option<(u8, u8)>,
    options: FetchOptions,
    perf: &mut Perf,
) -> Result<()> {
    super::validate_output(&input, options.common.output.as_deref())?;
    let loaded = options.common.load(&input, version, perf);
    if let Some(instructions) = options.common.budget.instructions {
        perf.resource_limits
            .as_mut()
            .unwrap()
            .insert("instructions", instructions);
    }
    let mut analysis = loaded?;
    let at = selector.resolve().map_err(super::super::classify)?;
    let selected = perf.measure("query", || analysis.parse_code_id(&at));
    perf.counters.work = Some(analysis.evidence.work);
    let selected = selected?;
    let result = perf.measure("decode", || {
        code_view::build(&mut analysis, selected, options.common.budget.instructions)
    });
    perf.counters.work = Some(analysis.evidence.work);
    let evidence = result?;
    perf.counters.instructions = Some(evidence.instructions.len());
    perf.counters.records = Some(1);
    let bytes = perf.measure("serialize", || -> Result<_> {
        let entry = &analysis.codes[selected];
        let span = analysis.evidence.occurrences[entry.definition].span;
        let row = View {
            schema_version: 2,
            backend: "pyc",
            command: "get-code",
            record: "code_view",
            at: analysis.code_id(selected),
            source_id: None,
            parent_at: entry.parent.map(|i| analysis.code_id(i)),
            context_at: None,
            start: span.start,
            end: span.end,
            detail: Detail {
                input_bytes: analysis.bytes.len(),
                python_version: analysis.version.display(),
                representation: "original_bytecode",
                complete: true,
                query: Query {
                    code_id: &at,
                    version: version.map(|(a, b)| format!("{a}.{b}")),
                },
                metadata: static_commands::record(&analysis, selected)?.detail,
                evidence,
            },
        };
        let mut out = Output::new(options.common.budget.output_bytes);
        out.json_line(&row)?;
        Ok(out.into_bytes())
    })?;
    perf.counters.output_bytes = Some(bytes.len());
    perf.measure("write", || {
        crate::cli::write_output(&bytes, options.common.output.as_deref())
    })
}
