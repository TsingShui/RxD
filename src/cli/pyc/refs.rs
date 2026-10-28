use super::{Output, static_commands};
use clap::Args;
use rxd_core::{Error, Result, perf::Perf};
use rxd_pyc::{
    references::{self, Scan},
    string_constants,
};
use std::{io::Write as _, path::PathBuf};

#[derive(Args)]
#[command(group(clap::ArgGroup::new("anchor").required(true).args(["at", "filter"])))]
pub struct Target {
    #[command(flatten)]
    selector: super::super::xref::Selector,
    /// Substring over referenced names; requires --kind name.
    #[arg(short = 'm', long = "match", requires = "kind", conflicts_with = "at")]
    filter: Option<String>,
    /// name for an exact literal name or content search; constant is inferred from --at.
    #[arg(long, value_parser = ["name", "constant"])]
    kind: Option<String>,
    #[command(flatten)]
    case: super::super::filter::CaseArgs,
}

#[derive(Args)]
pub struct RefsOptions {
    #[command(flatten)]
    pub common: static_commands::Options,
}
pub fn run(
    input: PathBuf,
    target: Target,
    within: Option<String>,
    version: Option<(u8, u8)>,
    options: RefsOptions,
    perf: &mut Perf,
    sink: &mut crate::cli::Output<'_>,
) -> Result<()> {
    super::validate_output(&input, options.common.output.as_deref())?;
    let at = if target.filter.is_none() {
        Some(target.selector.resolve().map_err(super::super::classify)?)
    } else {
        None
    };
    let kind = target.kind.as_deref().unwrap_or("constant");
    if target.filter.is_some() && kind != "name" {
        return Err(Error::new(
            "INVALID_QUERY",
            "--match requires --kind name; select constants with strings then --at",
        ));
    }
    let source = options.common.load(&input, version, perf)?;
    if let Some(instructions) = options.common.budget.instructions {
        perf.resource_limits
            .as_mut()
            .unwrap()
            .insert("instructions", instructions);
    }
    let constant_scope = if kind == "constant" {
        source.split(at.as_deref().unwrap())?.0
    } else {
        None
    };
    let within_scope = within
        .as_deref()
        .map(|at| source.split(at))
        .transpose()?
        .and_then(|(name, _)| name);
    if constant_scope
        .zip(within_scope)
        .is_some_and(|(left, right)| left != right)
    {
        return Err(Error::new(
            "INVALID_QUERY",
            "constant coordinate and --within select different PYZ modules",
        ));
    }
    let filter = target
        .filter
        .as_deref()
        .map(|text| rxd_core::filter::Contains::new(text, target.case.sensitive()));
    let mut out = Output::new(options.common.budget.output_bytes);
    out.write_all(crate::cli::table::header(&["entry", "context", "match"]).as_bytes())?;
    let mut records = 0usize;
    let mut instructions = 0usize;
    source.visit(constant_scope.or(within_scope), perf, |analysis, perf| {
        let mut scan = Scan::default();
        let result = perf.measure("query", || -> Result<()> {
            let selected = within.as_deref()
                .filter(|at| analysis.namespace.as_deref() != Some(*at))
                .map(|id| analysis.parse_code_id(id)).transpose()?;
            let constant = if kind == "constant" {
                let id = at.as_deref().unwrap();
                if !analysis.local_id(id)?.starts_with("const:") {
                    return Err(Error::new("INVALID_QUERY", "expected a strings.at constant coordinate; literal names require --kind name"));
                }
                Some(string_constants::resolve_id(analysis, id)?)
            } else { None };
            let query = if let Some(filter) = &filter {
                analysis.charge_work(filter.text().len())?;
                references::Query::NameFilter(filter)
            } else if let Some(selected) = &constant {
                references::Query::Constant { id: at.as_deref().unwrap(), selected }
            } else {
                let name = at.as_deref().unwrap();
                analysis.charge_work(name.len())?;
                references::Query::Name(name)
            };
            references::scan(analysis, &query, selected, options.common.budget.instructions, &mut scan)?;
            if let Some(unsupported) = scan.unsupported.first() {
                return Err(Error::new("UNSUPPORTED_INPUT", format!(
                    "unsupported opcode {} at {} offset {}", unsupported.name, unsupported.context_at, unsupported.offset,
                )));
            }
            Ok(())
        });
        instructions = instructions.checked_add(scan.instructions)
            .ok_or_else(|| Error::new("RESOURCE_LIMIT", "PYC instruction counter overflow"))?;
        perf.counters.instructions = Some(instructions);
        result?;
        records = records.checked_add(scan.hits.len())
            .ok_or_else(|| Error::new("RESOURCE_LIMIT", "PYC record counter overflow"))?;
        perf.measure("serialize", || -> Result<()> {
            for hit in &scan.hits {
                analysis.charge_work(1usize
                    .saturating_add(hit.constant_id.as_ref().map_or(0, String::len))
                    .saturating_add(hit.constant_path.as_ref().map_or(0, Vec::len)))?;
                use crate::cli::table::{self, Cell};
                let matched = hit.target_name.as_deref().or(hit.constant_id.as_deref())
                    .ok_or_else(|| Error::new("INVALID_INPUT", "reference has no lossless target"))?;
                let context = analysis.code_id(hit.code_index);
                let row = table::line(&[
                    analysis.namespace.as_deref().map_or(Cell::Null, Cell::Text),
                    Cell::Atom(&context),
                    Cell::Text(matched),
                ]).map_err(super::super::classify)?;
                out.write_all(row.as_bytes())?;
            }
            Ok(())
        })
    })?;
    perf.counters.records = Some(records);
    perf.counters.matches = Some(records);
    let bytes = out.into_bytes();
    perf.counters.output_bytes = Some(bytes.len());
    perf.measure("write", || {
        crate::cli::write_output(&bytes, options.common.output.as_deref(), sink)
    })
}
