use super::Output;
use clap::Args;
use rxd_core::{Result, perf::Perf};
use rxd_pyc::{input::Input, static_analysis::Analysis};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct Options {
    /// Atomically write the result to FILE instead of stdout; never overwrite input.
    #[arg(short = 'o', long)]
    pub(crate) output: Option<PathBuf>,
    /// Write timing/resource JSONL to stderr without changing the result payload.
    #[arg(long)]
    pub perf: bool,
    /// Enable backend diagnostic details on stderr.
    #[arg(long)]
    pub debug: bool,
    /// Input-scaled analysis admission supplied by dispatch.
    #[arg(skip)]
    pub(crate) budget: rxd_core::Budget,
}

impl Options {
    pub(super) fn load(
        &self,
        input: &Path,
        version: Option<(u8, u8)>,
        perf: &mut Perf,
    ) -> Result<Input> {
        Input::load(input, version, self.budget, perf)
    }
}

pub fn modules(
    input: PathBuf,
    version: Option<(u8, u8)>,
    filter: Option<String>,
    case_sensitive: bool,
    options: Options,
    perf: &mut Perf,
    sink: &mut crate::cli::Output<'_>,
) -> Result<()> {
    use crate::cli::table::{self, Cell};
    use std::io::Write as _;

    super::validate_output(&input, options.output.as_deref())?;
    let source = options.load(&input, version, perf)?;
    let filter = filter
        .as_deref()
        .map(|text| rxd_core::filter::Contains::new(text, case_sensitive));
    let mut writer = Output::new(options.budget.output_bytes);
    let mut records = 0usize;
    source.modules(filter.as_ref(), perf, |at, kind| {
        let line =
            table::line(&[Cell::Atom(at), Cell::Text(kind)]).map_err(super::super::classify)?;
        writer.write_all(line.as_bytes())?;
        records = records
            .checked_add(1)
            .ok_or_else(|| rxd_core::Error::new("RESOURCE_LIMIT", "PYC record counter overflow"))?;
        Ok(())
    })?;
    perf.counters.matches = Some(records);
    perf.counters.records = Some(records);
    let bytes = writer.into_bytes();
    perf.counters.output_bytes = Some(bytes.len());
    perf.measure("write", || {
        crate::cli::write_output(&bytes, options.output.as_deref(), sink)
    })
}

fn append_code(writer: &mut Output, analysis: &Analysis, index: usize) -> Result<()> {
    use std::io::Write as _;
    let at = analysis.code_id(index);
    let qualname = analysis
        .text_field(index, "qualname")
        .or_else(|| analysis.text_field(index, "name"))
        .unwrap_or_default();
    use crate::cli::table::{self, Cell};
    let line =
        table::line(&[Cell::Atom(&at), Cell::Text(qualname)]).map_err(super::super::classify)?;
    writer.write_all(line.as_bytes())?;
    Ok(())
}

pub struct CodeQuery {
    pub filter: Option<String>,
    pub case_sensitive: bool,
    pub name: Option<String>,
    pub selector: super::super::xref::Selector,
    pub within: Option<String>,
}

pub fn codes(
    input: PathBuf,
    version: Option<(u8, u8)>,
    query: CodeQuery,
    options: Options,
    perf: &mut Perf,
    sink: &mut crate::cli::Output<'_>,
) -> Result<()> {
    super::validate_output(&input, options.output.as_deref())?;
    let input_source = options.load(&input, version, perf)?;
    let at = query
        .selector
        .at
        .as_ref()
        .map(|_| query.selector.resolve().map_err(super::super::classify))
        .transpose()?;
    let scope = if let Some(within) = query.within.as_deref() {
        input_source.module_scope(within)?
    } else {
        at.as_deref()
            .map(|at| input_source.split(at))
            .transpose()?
            .and_then(|(namespace, _)| namespace)
    };
    let filter = query
        .filter
        .as_deref()
        .map(|f| rxd_core::filter::Contains::new(f, query.case_sensitive));
    let mut writer = Output::new(options.budget.output_bytes);
    let mut records = 0usize;
    input_source.visit(scope, perf, |analysis, perf| {
        let selected = perf.measure("query", || {
            let index = at
                .as_deref()
                .map(|at| analysis.parse_code_id(at))
                .transpose()?;
            analysis.select_codes(filter.as_ref(), query.name.as_deref(), index)
        })?;
        records = records
            .checked_add(selected.len())
            .ok_or_else(|| rxd_core::Error::new("RESOURCE_LIMIT", "PYC record counter overflow"))?;
        perf.measure("serialize", || -> Result<()> {
            for index in selected {
                append_code(&mut writer, analysis, index)?;
            }
            Ok(())
        })
    })?;
    perf.counters.matches = Some(records);
    perf.counters.records = Some(records);
    let bytes = writer.into_bytes();
    perf.counters.output_bytes = Some(bytes.len());
    perf.measure("write", || {
        crate::cli::write_output(&bytes, options.output.as_deref(), sink)
    })
}
