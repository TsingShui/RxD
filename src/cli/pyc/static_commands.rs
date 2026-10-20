use super::Output;
use clap::Args;
use rxd_core::envelope;
use rxd_core::{Error, Result, perf::Perf};
use rxd_pyc::static_analysis::{Analysis, Limits};
use serde::Serialize;
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Args)]
pub struct Options {
    #[arg(short = 'o', long)]
    pub(crate) output: Option<PathBuf>,
    #[arg(long)]
    pub perf: bool,
    #[arg(long)]
    pub debug: bool,
}

impl Options {
    pub(crate) fn limits(&self) -> Limits {
        let budget = super::budget();
        Limits {
            input_bytes: budget.input_bytes,
            // No enumeration ceiling: the marshal reader's field stays for its own
            // arithmetic, not as a limit on how much a correct query may report.
            entities: usize::MAX,
            work: budget.work,
        }
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
    #[serde(serialize_with = "crate::cli::pyc::short_id")]
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
        at: envelope::at_of(&analysis.code_id(index)).into_owned(),
        source_id: None,
        parent_at: entry
            .parent
            .map(|parent| envelope::at_of(&analysis.code_id(parent)).into_owned()),
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
    serde_json::to_writer(&mut *writer, value)
        .map_err(|error| Error::new("RESOURCE_LIMIT", error.to_string()))?;
    std::io::Write::write_all(writer, b"\n")
        .map_err(|error| Error::new("RESOURCE_LIMIT", error.to_string()))
}

pub fn codes(
    input: PathBuf,
    version: Option<(u8, u8)>,
    filter: Option<rxd_core::filter::Contains>,
    name: Option<String>,
    index: Option<usize>,
    options: Options,
    perf: &mut Perf,
) -> Result<()> {
    super::validate_output(&input, options.output.as_deref())?;
    let limits = options.limits();
    let mut analysis = Analysis::load(&input, version, limits, perf)?;
    let selected = perf.measure("query", || {
        analysis.select_codes(filter.as_ref(), name.as_deref(), index)
    });
    perf.counters.work = Some(analysis.evidence.work);
    let selected = selected?;
    perf.counters.matches = Some(selected.len());
    perf.counters.records = Some(selected.len());
    let bytes = perf.measure("serialize", || -> Result<Vec<u8>> {
        let mut writer = Output { bytes: Vec::new() };
        append(&mut writer,&rxd_core::envelope::header("pyc","codes",Some(&analysis.source_id),Some(analysis.bytes.len()),
            json!({"filter":filter.as_ref().map(|filter| filter.text()),"name":name,"index":index,"version":version.map(|(major,minor)| format!("{major}.{minor}"))}),
            json!({"analysis":"metadata","python_version":analysis.version.display(),"code_count":analysis.codes.len(),"instruction_validation":"not_scanned"}).as_object().unwrap().clone()))?;
        for &index in &selected { append(&mut writer,&record(&analysis,index)?)?; }
        append(&mut writer,&rxd_core::envelope::summary(selected.len(),true,
            json!({"code_count":analysis.codes.len(),"analysis":"metadata","instruction_validation":"not_scanned"}).as_object().unwrap().clone()))?;
        Ok(writer.bytes)
    })?;
    perf.counters.output_bytes = Some(bytes.len());
    perf.measure("write", || {
        crate::cli::write_output(&bytes, options.output.as_deref())
    })
}
