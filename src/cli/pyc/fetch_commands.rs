use super::{BoundedOutput, static_commands};
use clap::Args;
use rxd_core::{Error, Result, perf::Perf};
use rxd_pyc::{
    code_view::{self, Evidence},
    static_analysis::Analysis,
};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Args)]
pub struct FetchOptions {
    #[command(flatten)]
    pub common: static_commands::Options,
    #[arg(long, default_value_t = 1_000_000, value_parser = crate::cli::positive)]
    max_instructions: usize,
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
    id: &'a str,
    source_id: &'a str,
    parent_id: Option<String>,
    context_id: Option<&'a str>,
    start: usize,
    end: usize,
    detail: Detail<'a>,
}
pub fn fetch(
    input: PathBuf,
    id: String,
    version: Option<(u8, u8)>,
    options: FetchOptions,
    perf: &mut Perf,
) -> Result<()> {
    super::validate_output(&input, options.common.output.as_deref())?;
    let limits = options.common.limits();
    let loaded = Analysis::load(&input, version, limits, perf);
    perf.resource_limits
        .as_mut()
        .unwrap()
        .insert("instructions", options.max_instructions);
    let mut analysis = loaded?;
    let selected = perf.measure("query", || analysis.parse_code_id(&id));
    perf.counters.work = Some(analysis.evidence.work);
    let selected = selected?;
    let result = perf.measure("decode", || {
        code_view::build(&mut analysis, selected, options.max_instructions)
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
            id: &id,
            source_id: &analysis.source_id,
            parent_id: entry.parent.map(|i| analysis.code_id(i)),
            context_id: None,
            start: span.start,
            end: span.end,
            detail: Detail {
                input_bytes: analysis.bytes.len(),
                python_version: analysis.version.display(),
                representation: "original_bytecode",
                complete: true,
                query: Query {
                    code_id: &id,
                    version: version.map(|(a, b)| format!("{a}.{b}")),
                },
                metadata: static_commands::record(&analysis, selected)?.detail,
                evidence,
            },
        };
        let mut out = BoundedOutput {
            bytes: Vec::new(),
            limit: limits.output_bytes,
        };
        serde_json::to_writer(&mut out, &row)
            .map_err(|e| Error::new("RESOURCE_LIMIT", e.to_string()))?;
        std::io::Write::write_all(&mut out, b"\n")
            .map_err(|e| Error::new("RESOURCE_LIMIT", e.to_string()))?;
        Ok(out.bytes)
    })?;
    perf.counters.output_bytes = Some(bytes.len());
    perf.measure("write", || {
        crate::cli::write_output(&bytes, options.common.output.as_deref())
    })
}
