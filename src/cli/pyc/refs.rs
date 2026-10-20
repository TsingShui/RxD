use super::{BoundedOutput, static_commands};
use clap::Args;
use rxd_core::{Error, Result, envelope, perf::Perf};
use rxd_pyc::{
    references::{self, Scan},
    static_analysis::Analysis,
    string_constants,
};
use serde::Serialize;
use serde_json::json;
use std::path::PathBuf;

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
fn append(out: &mut BoundedOutput, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *out, value)
        .map_err(|e| Error::new("RESOURCE_LIMIT", e.to_string()))?;
    std::io::Write::write_all(out, b"\n").map_err(|e| Error::new("RESOURCE_LIMIT", e.to_string()))
}
pub fn run(
    input: PathBuf,
    name: Option<String>,
    at: Option<String>,
    within: Option<String>,
    version: Option<(u8, u8)>,
    options: RefsOptions,
    perf: &mut Perf,
) -> Result<()> {
    super::validate_output(&input, options.common.output.as_deref())?;
    let limits = options.common.limits();
    let loaded = Analysis::load(&input, version, limits, perf);
    perf.resource_limits
        .as_mut()
        .unwrap()
        .insert("instructions", super::budget().instructions);
    let mut analysis = loaded?;
    let mut scan = Scan::default();
    let result = perf.measure("query", || -> Result<()> {
        let selector_bytes = name.as_ref().or(at.as_ref()).map_or(0, String::len);
        if selector_bytes > limits.output_bytes {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "query output byte budget exceeded",
            ));
        }
        let selected = within
            .as_deref()
            .map(|id| analysis.parse_code_id(id))
            .transpose()?;
        let constant = at
            .as_deref()
            .map(|id| string_constants::resolve_id(&mut analysis, id))
            .transpose()?;
        let query = match (name.as_deref(), at.as_deref(), constant.as_ref()) {
            (Some(name), None, None) => {
                analysis.charge_work(name.len())?;
                references::Query::Name(name)
            }
            (None, Some(id), Some(selected)) => references::Query::Constant { id, selected },
            _ => {
                return Err(Error::new(
                    "INVALID_QUERY",
                    "exactly one of --name or --at is required",
                ));
            }
        };
        references::scan(
            &mut analysis,
            &query,
            selected,
            super::budget().instructions,
            &mut scan,
        )
    });
    perf.counters.work = Some(analysis.evidence.work);
    perf.counters.instructions = Some(scan.instructions);
    perf.counters.records = Some(scan.hits.len());
    perf.counters.matches = Some(scan.hits.len());
    result?;
    let analysis_kind = if at.is_some() {
        "explicit_constant_references"
    } else {
        "explicit_name_references"
    };
    let scope = if within.is_some() {
        "selected_code"
    } else if at.is_some() {
        "constant_owner"
    } else {
        "all_codes"
    };
    let bytes = perf.measure("serialize", || -> Result<_> {
        let mut out = BoundedOutput { bytes: Vec::new(), limit: limits.output_bytes };
        append(&mut out, &envelope::header("pyc", "xref", Some(&analysis.source_id), Some(analysis.bytes.len()),
            json!({"name":name,"at":at,"within":within,"version":version.map(|(a,b)|format!("{a}.{b}"))}),
            json!({"analysis":analysis_kind,"python_version":analysis.version.display(),"code_count":analysis.codes.len(),"scope":scope,"instruction_validation":"scanned_scope"}).as_object().unwrap().clone()))?;
        for hit in &scan.hits {
            analysis.charge_work(1usize.saturating_add(hit.constant_id.as_ref().map_or(0, String::len)).saturating_add(hit.constant_path.as_ref().map_or(0, Vec::len)))?;
            append(&mut out, &Record {record:envelope::REFERENCE, at:None, source_id:None, parent_at:None,
                context_at:envelope::at_of(&analysis.code_id(hit.code_index)).into_owned(), start:hit.start, end:hit.end, detail:hit})?;
        }
        analysis.charge_work(scan.unsupported.len())?;
        append(&mut out, &envelope::summary(scan.hits.len(), scan.unsupported.is_empty(),
            json!({"analysis":analysis_kind,"code_count":analysis.codes.len(),"scanned_code_count":scan.scanned_codes,"instructions":scan.instructions,"unsupported_count":scan.unsupported.len(),"unsupported":scan.unsupported,"instruction_validation":"scanned_scope"}).as_object().unwrap().clone()))?;
        Ok(out.bytes)
    });
    perf.counters.work = Some(analysis.evidence.work);
    let bytes = bytes?;
    perf.counters.output_bytes = Some(bytes.len());
    perf.measure("write", || {
        crate::cli::write_output(&bytes, options.common.output.as_deref())
    })
}
