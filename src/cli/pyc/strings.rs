use super::{Output, static_commands::Options};
use rxd_core::{Result, perf::Perf};
use rxd_pyc::string_constants::{self, Query};
use std::{io::Write as _, path::PathBuf};

pub(super) fn valid_hex(value: &str) -> std::result::Result<String, String> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("expected an even number of hexadecimal digits without separators".into());
    }
    Ok(value.into())
}

pub struct Filter {
    pub text: Option<String>,
    pub case_sensitive: bool,
    pub hex: Option<String>,
}

pub fn run(
    input: PathBuf,
    version: Option<(u8, u8)>,
    filter: Filter,
    options: Options,
    perf: &mut Perf,
    sink: &mut crate::cli::Output<'_>,
) -> Result<()> {
    super::validate_output(&input, options.output.as_deref())?;
    let source = options.load(&input, version, perf)?;
    let hex = filter.hex.as_ref().map(|value| {
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
    let query = match (&filter.text, &hex) {
        (Some(text), _) => Query::Text {
            text,
            case_sensitive: filter.case_sensitive,
        },
        (_, Some(bytes)) => Query::Hex(bytes),
        _ => Query::All,
    };
    let mut writer = Output::new(options.budget.output_bytes);
    let mut records = 0usize;
    source.visit(None, perf, |analysis, perf| {
        let literals = perf.measure("query", || string_constants::enumerate(analysis, &query))?;
        records = records
            .checked_add(literals.len())
            .ok_or_else(|| rxd_core::Error::new("RESOURCE_LIMIT", "PYC record counter overflow"))?;
        perf.measure("serialize", || -> Result<()> {
            for literal in literals {
                let length = analysis.payload(literal.definition).unwrap().len();
                analysis.charge_work(length.saturating_add(literal.path.len()))?;
                let value = if literal.kind == "str" {
                    std::str::from_utf8(analysis.payload(literal.definition).unwrap()).ok()
                } else {
                    None
                };
                let at = string_constants::constant_id(analysis, literal.owner, &literal.path);
                use crate::cli::table::{self, Cell};
                let line = table::line(&[Cell::Atom(&at), value.map_or(Cell::Null, Cell::Text)])
                    .map_err(super::super::classify)?;
                writer.write_all(line.as_bytes())?;
            }
            Ok(())
        })
    })?;
    perf.counters.records = Some(records);
    perf.counters.matches = Some(records);
    let bytes = writer.into_bytes();
    perf.counters.output_bytes = Some(bytes.len());
    perf.measure("write", || {
        crate::cli::write_output(&bytes, options.output.as_deref(), sink)
    })
}
