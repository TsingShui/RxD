use super::{Output, static_commands};
use clap::Args;
use rxd_core::{Error, Result, envelope, perf::Perf};
use rxd_pyc::{
    references::{self, Scan},
    string_constants,
};
use serde::Serialize;
use serde_json::json;
use std::path::PathBuf;

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
#[derive(Serialize)]
struct Record<'a> {
    record: &'static str,
    at: Option<&'a str>,
    source_id: Option<&'a str>,
    parent_at: Option<&'a str>,
    context_at: String,
    start: usize,
    end: usize,
    detail: &'a references::Hit,
}
fn append(out: &mut Output, value: &impl Serialize) -> Result<()> {
    out.json_line(value)
}
pub fn run(
    input: PathBuf,
    target: Target,
    within: Option<String>,
    version: Option<(u8, u8)>,
    options: RefsOptions,
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
    let at = if target.filter.is_none() {
        Some(target.selector.resolve().map_err(super::super::classify)?)
    } else {
        None
    };
    let kind = target.kind.as_deref().unwrap_or("constant");
    if !matches!(kind, "name" | "constant") {
        return Err(Error::failure(
            rxd_core::Failure::Query,
            format!("xref supports name or constant targets, got {kind}"),
        ));
    }
    if target.filter.is_some() && kind != "name" {
        return Err(Error::failure(
            rxd_core::Failure::Query,
            "--match requires --kind name; select a constant with strings then --at",
        ));
    }
    let filter = target
        .filter
        .as_deref()
        .map(|s| rxd_core::filter::Contains::new(s, target.case.sensitive()));
    let mut scan = Scan::default();
    let result = perf.measure("query", || -> Result<()> {
        let selected = within
            .as_deref()
            .map(|id| analysis.parse_code_id(id))
            .transpose()?;
        let constant = if kind == "constant" {
            let value = at.as_deref().unwrap();
            let coordinate_kind = value.split(':').next().unwrap_or("");
            if coordinate_kind != "const" {
                return Err(Error::failure(rxd_core::Failure::Query, format!("expected constant coordinate (const), got {coordinate_kind}; use --kind name for literal names")));
            }
            Some(string_constants::resolve_id(
                &mut analysis,
                at.as_deref().unwrap(),
            )?)
        } else {
            None
        };
        let query = if let Some(filter) = &filter {
            analysis.charge_work(filter.text().len())?;
            references::Query::NameFilter(filter)
        } else if let Some(selected) = &constant {
            references::Query::Constant {
                id: at.as_deref().unwrap(),
                selected,
            }
        } else {
            let name = at.as_deref().unwrap();
            analysis.charge_work(name.len())?;
            references::Query::Name(name)
        };
        references::scan(
            &mut analysis,
            &query,
            selected,
            options.common.budget.instructions,
            &mut scan,
        )
    });
    perf.counters.work = Some(analysis.evidence.work);
    perf.counters.instructions = Some(scan.instructions);
    perf.counters.records = Some(scan.hits.len());
    perf.counters.matches = Some(scan.hits.len());
    result?;
    let bytes = perf.measure("serialize", || -> Result<_> {
        let mut out = Output::new(options.common.budget.output_bytes);
        let mut query = json!({"kind": kind});
        if let Some(at) = at.as_deref() {
            query["at"] = json!(at);
        }
        if let Some(filter) = &target.filter {
            query["match"] = json!(filter);
            query["case"] = json!(if target.case.sensitive() {
                "sensitive"
            } else {
                "insensitive"
            });
        }
        if let Some(within) = within.as_deref() {
            query["within"] = json!(within);
        }
        if let Some((major, minor)) = version {
            query["version"] = json!(format!("{major}.{minor}"));
        }
        for hit in &scan.hits {
            analysis.charge_work(
                1usize
                    .saturating_add(hit.constant_id.as_ref().map_or(0, String::len))
                    .saturating_add(hit.constant_path.as_ref().map_or(0, Vec::len)),
            )?;
            append(
                &mut out,
                &Record {
                    record: envelope::REFERENCE,
                    at: None,
                    source_id: None,
                    parent_at: None,
                    context_at: analysis.code_id(hit.code_index),
                    start: hit.start,
                    end: hit.end,
                    detail: hit,
                },
            )?;
        }
        analysis.charge_work(scan.unsupported.len())?;
        Ok(out.into_bytes())
    });
    perf.counters.work = Some(analysis.evidence.work);
    let bytes = bytes?;
    perf.counters.output_bytes = Some(bytes.len());
    perf.measure("write", || {
        crate::cli::write_output(&bytes, options.common.output.as_deref())
    })
}
