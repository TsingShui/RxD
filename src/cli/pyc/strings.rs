use super::{BoundedOutput, static_commands::Options};
use rxd_core::{Error, Result, envelope, perf::Perf};
use rxd_pyc::{
    static_analysis::Analysis,
    string_constants::{self, Literal, Query},
};
use serde::Serialize;
use serde_json::json;
use std::path::PathBuf;

#[derive(Serialize)]
struct Alias<'a> {
    start: usize,
    end: usize,
    constant_path: &'a [usize],
}
#[derive(Serialize)]
struct Detail<'a> {
    code_index: usize,
    constant_slot: usize,
    constant_path: &'a [usize],
    kind: &'static str,
    value: Option<&'a str>,
    raw_bytes: String,
    definition_start: usize,
    definition_end: usize,
    payload_start: usize,
    payload_end: usize,
    occurrence_start: Option<usize>,
    occurrence_end: Option<usize>,
    reference: bool,
    aliases: Vec<Alias<'a>>,
}
#[derive(Serialize)]
struct Record<'a> {
    record: &'static str,
    at: String,
    source_id: Option<&'a str>,
    parent_at: String,
    context_at: String,
    start: Option<usize>,
    end: Option<usize>,
    detail: Detail<'a>,
}
fn record<'a>(
    analysis: &'a Analysis,
    literal: &'a Literal,
    remaining: usize,
) -> Result<Record<'a>> {
    let definition = &analysis.evidence.occurrences[literal.definition];
    let payload = definition.payload.unwrap();
    let bytes = analysis.payload(literal.definition).unwrap();
    let value = (literal.kind == "str")
        .then(|| std::str::from_utf8(bytes).ok())
        .flatten();
    let scalar_size = bytes
        .len()
        .saturating_mul(2)
        .saturating_add(value.map_or(0, str::len));
    fn path_bytes(path: &[usize]) -> usize {
        path.iter()
            .fold(path.len().saturating_sub(1), |size, &index| {
                size.saturating_add(if index == 0 {
                    1
                } else {
                    index.ilog10() as usize + 1
                })
            })
    }
    // A lower bound on the fixed envelope plus both serialized path representations.
    // Do not overestimate numeric paths: the exact total-output boundary must succeed.
    let path_size = path_bytes(&literal.path).saturating_mul(2);
    let alias_size = literal.aliases.iter().fold(0usize, |sum, alias| {
        sum.saturating_add(path_bytes(&alias.path))
            .saturating_add(38)
    });
    if scalar_size
        .saturating_add(path_size)
        .saturating_add(alias_size)
        .saturating_add(512)
        > remaining
    {
        return Err(Error::new("RESOURCE_LIMIT", "output byte budget exceeded"));
    }
    let occurrence = literal
        .occurrence
        .map(|id| &analysis.evidence.occurrences[id]);
    let actual_payload = occurrence.filter(|row| !row.is_reference).map(|_| payload);
    let mut raw_bytes = String::with_capacity(bytes.len() * 2);
    const HEX: &[u8] = b"0123456789abcdef";
    for &byte in bytes {
        raw_bytes.push(HEX[(byte >> 4) as usize] as char);
        raw_bytes.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(Record {
        record: "string",
        at: rxd_core::envelope::at_of(&string_constants::constant_id(
            analysis,
            literal.owner,
            &literal.path,
        ))
        .into_owned(),
        source_id: None,
        parent_at: rxd_core::envelope::at_of(&analysis.code_id(literal.owner)).into_owned(),
        context_at: rxd_core::envelope::at_of(&analysis.code_id(literal.owner)).into_owned(),
        start: actual_payload.map(|span| span.start),
        end: actual_payload.map(|span| span.end),
        detail: Detail {
            code_index: literal.owner,
            constant_slot: literal.path[0],
            constant_path: &literal.path,
            kind: literal.kind,
            value,
            raw_bytes,
            definition_start: definition.span.start,
            definition_end: definition.span.end,
            payload_start: payload.start,
            payload_end: payload.end,
            occurrence_start: occurrence.map(|row| row.span.start),
            occurrence_end: occurrence.map(|row| row.span.end),
            reference: literal.reference,
            aliases: literal
                .aliases
                .iter()
                .map(|alias| Alias {
                    start: alias.span.start,
                    end: alias.span.end,
                    constant_path: &alias.path,
                })
                .collect(),
        },
    })
}
fn append(writer: &mut BoundedOutput, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|error| Error::new("RESOURCE_LIMIT", error.to_string()))?;
    std::io::Write::write_all(writer, b"\n")
        .map_err(|error| Error::new("RESOURCE_LIMIT", error.to_string()))
}
pub(super) fn valid_hex(value: &str) -> std::result::Result<String, String> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("expected an even number of hexadecimal digits without separators".into());
    }
    Ok(value.into())
}
pub fn run(
    input: PathBuf,
    version: Option<(u8, u8)>,
    filter: Option<String>,
    filter_hex: Option<String>,
    options: Options,
    perf: &mut Perf,
) -> Result<()> {
    super::validate_output(&input, options.output.as_deref())?;
    let limits = options.limits();
    let mut analysis = Analysis::load(&input, version, limits, perf)?;
    let result = perf.measure("query", || -> Result<_> {
        let query_bytes = filter
            .as_ref()
            .or(filter_hex.as_ref())
            .map_or(0, String::len);
        analysis.charge_work(query_bytes)?;
        if query_bytes > limits.output_bytes {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "query output byte budget exceeded",
            ));
        }
        let hex = filter_hex.as_ref().map(|value| {
            value
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    fn nibble(byte: u8) -> u8 {
                        match byte {
                            b'0'..=b'9' => byte - b'0',
                            b'a'..=b'f' => byte - b'a' + 10,
                            _ => byte - b'A' + 10,
                        }
                    }
                    nibble(pair[0]) * 16 + nibble(pair[1])
                })
                .collect::<Vec<_>>()
        });
        let query = if let Some(text) = &filter {
            Query::Text(text)
        } else if let Some(bytes) = &hex {
            Query::Hex(bytes)
        } else {
            Query::All
        };
        string_constants::enumerate(&mut analysis, &query)
    });
    perf.counters.work = Some(analysis.evidence.work);
    let literals = result?;
    perf.counters.records = Some(literals.len());
    perf.counters.matches = Some(literals.len());
    let serialized = perf.measure("serialize", || -> Result<Vec<u8>> {
        let mut writer = BoundedOutput { bytes: Vec::new(), limit: limits.output_bytes };
        append(&mut writer, &envelope::header("pyc", "strings", Some(&analysis.source_id), Some(analysis.bytes.len()),
            json!({"filter":filter.as_deref(),"filter_hex":filter_hex.as_deref(),"version":version.map(|(major,minor)|format!("{major}.{minor}"))}),
            json!({"analysis":"constants","python_version":analysis.version.display(),"code_count":analysis.codes.len(),"instruction_validation":"not_scanned","containers":["tuple","frozenset"]}).as_object().unwrap().clone()))?;
        for literal in &literals {
            let raw_length = analysis.payload(literal.definition).unwrap().len();
            let alias_work = literal.aliases.iter().fold(0usize, |sum, alias| sum.saturating_add(alias.path.len()+1));
            // Charge raw hex conversion and each path/alias projection before allocating.
            analysis.charge_work(raw_length.saturating_add(literal.path.len()).saturating_add(alias_work))?;
            let row = record(&analysis, literal, writer.limit.saturating_sub(writer.bytes.len()))?;
            append(&mut writer, &row)?;
        }
        append(&mut writer, &envelope::summary(literals.len(), true, json!({"code_count":analysis.codes.len(),"analysis":"constants","instruction_validation":"not_scanned"}).as_object().unwrap().clone()))?;
        Ok(writer.bytes)
    });
    perf.counters.work = Some(analysis.evidence.work);
    let bytes = serialized?;
    perf.counters.output_bytes = Some(bytes.len());
    perf.measure("write", || {
        crate::cli::write_output(&bytes, options.output.as_deref())
    })
}
