use super::Output;
use clap::Args;
use rxd_core::{Result, perf::Perf};
use rxd_pyc::static_analysis::{Analysis, Limits};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Args)]
pub struct Options {
    #[arg(short = 'o', long)]
    pub(crate) output: Option<PathBuf>,
    #[arg(long)]
    pub perf: bool,
    #[arg(long)]
    pub debug: bool,
    #[arg(skip)]
    pub(crate) budget: rxd_core::Budget,
}

impl Options {
    pub(crate) fn limits(&self) -> Limits {
        let budget = self.budget;
        Limits {
            input_bytes: budget.input_bytes,
            // Every evidence occurrence consumes the selected work allowance;
            // there is no separate CLI entity-count knob.
            entities: budget.work,
            metadata_bytes: budget.metadata_bytes,
            marshal: rxd_pyc::marshal_limits(budget),
            work: budget.work,
        }
    }

    pub(super) fn load(
        &self,
        input: &Path,
        version: Option<(u8, u8)>,
        perf: &mut Perf,
    ) -> Result<Analysis> {
        let loaded = Analysis::load(input, version, self.limits(), perf);
        // Analysis owns parser limits; complete serialization is a CLI constraint.
        if let Some(limits) = &mut perf.resource_limits {
            limits.insert("output_bytes", self.budget.output_bytes);
        }
        loaded
    }
}

#[derive(Serialize)]
struct Field {
    start: usize,
    end: usize,
    definition_start: Option<usize>,
    definition_end: Option<usize>,
    payload_start: Option<usize>,
    payload_end: Option<usize>,
    reference: bool,
    tag: Option<u8>,
}
#[derive(Serialize)]
struct Incoming<'a> {
    owner_id: String,
    constant_path: &'a [usize],
    start: Option<usize>,
    end: Option<usize>,
    alias_start: Option<usize>,
    alias_end: Option<usize>,
    reference: bool,
}
#[derive(Serialize)]
pub(crate) struct Detail<'a> {
    index: usize,
    name: Option<&'a str>,
    qualname: Option<&'a str>,
    filename: Option<&'a str>,
    name_bytes: Option<String>,
    qualname_bytes: Option<String>,
    filename_bytes: Option<String>,
    kind: &'static str,
    arg_count: u32,
    posonly_arg_count: u32,
    kwonly_arg_count: u32,
    n_locals: u32,
    stack_size: u32,
    flags: u32,
    first_line: u32,
    body_status: &'static str,
    bytecode_bytes: Option<usize>,
    consts_count: usize,
    names_count: usize,
    varnames_count: usize,
    freevars_count: usize,
    cellvars_count: usize,
    localsplus_count: Option<usize>,
    line_table_bytes: usize,
    exception_table_bytes: usize,
    fields: BTreeMap<&'static str, Field>,
    incoming: Vec<Incoming<'a>>,
    incoming_complete: bool,
    ownership: &'static str,
}
#[derive(Serialize)]
pub(crate) struct Record<'a> {
    record: &'static str,
    at: String,
    source_id: Option<&'a str>,
    parent_at: Option<String>,
    context_at: Option<&'a str>,
    start: usize,
    end: usize,
    pub(crate) detail: Detail<'a>,
}

fn raw_text(analysis: &Analysis, index: usize, name: &str) -> Option<String> {
    if analysis.text_field(index, name).is_some() {
        return None;
    }
    analysis
        .payload(analysis.field_definition(index, name)?)
        .map(|bytes| bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub(crate) fn record(analysis: &Analysis, index: usize) -> Result<Record<'_>> {
    let entry = &analysis.codes[index];
    let definition = &analysis.evidence.occurrences[entry.definition];
    // Check unavoidable scalar payload size before allocating JSON detail or hex evidence.
    let mut scalar_bytes = 0usize;
    for name in ["name", "qualname", "filename"] {
        if let Some(field) = analysis.field_definition(index, name) {
            let length = analysis.payload(field).map_or(0, <[u8]>::len);
            scalar_bytes =
                scalar_bytes.saturating_add(if analysis.text_field(index, name).is_some() {
                    length
                } else {
                    length.saturating_mul(2)
                });
        }
    }
    let fields = definition
        .fields
        .iter()
        .map(|(&name, field)| {
            let occurrence = field
                .occurrence
                .map(|id| &analysis.evidence.occurrences[id]);
            let definition = occurrence.map(|row| &analysis.evidence.occurrences[row.definition]);
            (
                name,
                Field {
                    start: field.span.start,
                    end: field.span.end,
                    definition_start: definition.map(|row| row.span.start),
                    definition_end: definition.map(|row| row.span.end),
                    payload_start: definition.and_then(|row| row.payload.map(|span| span.start)),
                    payload_end: definition.and_then(|row| row.payload.map(|span| span.end)),
                    reference: occurrence.is_some_and(|row| row.is_reference),
                    tag: occurrence.map(|row| row.tag),
                },
            )
        })
        .collect();
    let incoming = entry
        .incoming
        .iter()
        .map(|edge| Incoming {
            owner_id: analysis.code_id(edge.owner),
            constant_path: &edge.path,
            start: edge.span.map(|span| span.start),
            end: edge.span.map(|span| span.end),
            alias_start: edge.alias.map(|span| span.start),
            alias_end: edge.alias.map(|span| span.end),
            reference: edge.is_reference,
        })
        .collect();
    let code = &entry.code;
    let body_status = analysis.body_status(index);
    Ok(Record {
        record: "code",
        at: analysis.code_id(index),
        source_id: None,
        parent_at: entry.parent.map(|parent| analysis.code_id(parent)),
        context_at: None,
        start: definition.span.start,
        end: definition.span.end,
        detail: Detail {
            index,
            name: analysis.text_field(index, "name"),
            qualname: analysis.text_field(index, "qualname"),
            filename: analysis.text_field(index, "filename"),
            name_bytes: raw_text(analysis, index, "name"),
            qualname_bytes: raw_text(analysis, index, "qualname"),
            filename_bytes: raw_text(analysis, index, "filename"),
            kind: "unknown",
            arg_count: code.arg_count,
            posonly_arg_count: code.posonly_arg_count,
            kwonly_arg_count: code.kwonly_arg_count,
            n_locals: code.n_locals,
            stack_size: code.stack_size,
            flags: code.flags,
            first_line: code.first_line,
            body_status,
            bytecode_bytes: (body_status != "unavailable").then_some(code.code.len()),
            consts_count: code.consts.len(),
            names_count: code.names.len(),
            varnames_count: code.varnames.len(),
            freevars_count: code.freevars.len(),
            cellvars_count: code.cellvars.len(),
            localsplus_count: analysis
                .field_definition(index, "localsplus_names")
                .and_then(|id| {
                    analysis.evidence.occurrences[id]
                        .object
                        .as_deref()?
                        .as_tuple()
                        .map(<[_]>::len)
                }),
            line_table_bytes: code.line_table.len(),
            exception_table_bytes: code.exception_table.len(),
            fields,
            incoming,
            incoming_complete: false,
            ownership: "first_containment",
        },
    })
}

fn append(writer: &mut Output, value: &impl Serialize) -> Result<()> {
    writer.json_line(value)
}

pub struct CodeQuery {
    pub filter: Option<String>,
    pub case_sensitive: bool,
    pub name: Option<String>,
    pub selector: super::super::xref::Selector,
}

pub fn codes(
    input: PathBuf,
    version: Option<(u8, u8)>,
    query: CodeQuery,
    options: Options,
    perf: &mut Perf,
) -> Result<()> {
    super::validate_output(&input, options.output.as_deref())?;
    let mut analysis = options.load(&input, version, perf)?;
    let at = if query.selector.at.is_some() {
        Some(query.selector.resolve().map_err(super::super::classify)?)
    } else {
        None
    };
    let filter = query
        .filter
        .as_deref()
        .map(|f| rxd_core::filter::Contains::new(f, query.case_sensitive));
    let selected = perf.measure("query", || {
        let index = at
            .as_deref()
            .map(|at| analysis.parse_code_id(at))
            .transpose()?;
        analysis.select_codes(filter.as_ref(), query.name.as_deref(), index)
    });
    perf.counters.work = Some(analysis.evidence.work);
    let selected = selected?;
    perf.counters.matches = Some(selected.len());
    perf.counters.records = Some(selected.len());
    let bytes = perf.measure("serialize", || -> Result<Vec<u8>> {
        let mut writer = Output::new(options.budget.output_bytes);
        append(&mut writer,&rxd_core::envelope::header("pyc","codes",Some(&analysis.source_id),Some(analysis.bytes.len()),
            json!({"filter":query.filter,"case":if query.case_sensitive {"sensitive"} else {"insensitive"},"name":query.name,"at":at,"version":version.map(|(major,minor)| format!("{major}.{minor}"))}),
            json!({"analysis":"metadata","python_version":analysis.version.display(),"code_count":analysis.codes.len(),"instruction_validation":"not_scanned"}).as_object().unwrap().clone()))?;
        for &index in &selected { append(&mut writer,&record(&analysis,index)?)?; }
        append(&mut writer,&rxd_core::envelope::summary(selected.len(),true,
            json!({"code_count":analysis.codes.len(),"analysis":"metadata","instruction_validation":"not_scanned"}).as_object().unwrap().clone()))?;
        Ok(writer.into_bytes())
    })?;
    perf.counters.output_bytes = Some(bytes.len());
    perf.measure("write", || {
        crate::cli::write_output(&bytes, options.output.as_deref())
    })
}
