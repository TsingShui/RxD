//! Explicit instruction references. No AST, source recovery or runtime dispatch inference.
use crate::{
    code_view::{Target, instruction_targets},
    static_analysis::Analysis,
    string_constants::SelectedConstant,
};
use pycdc::{
    bytecode::decode_evidence_limited,
    object::PyObject,
    opcode::{Op, table_for},
};
use rxd_core::{Error, Result};
use serde::Serialize;
use std::collections::HashSet;

#[derive(Serialize)]
pub struct Hit {
    pub code_index: usize,
    pub offset: usize,
    pub opcode: u8,
    pub name: String,
    pub raw_operand: Option<u32>,
    pub operand: Option<u32>,
    pub target: Target,
    pub target_name: Option<String>,
    pub operation: &'static str,
    pub constant_id: Option<String>,
    pub constant_slot: Option<usize>,
    pub constant_path: Option<Vec<usize>>,
    pub literal_kind: Option<&'static str>,
    pub relation: Option<&'static str>,
    #[serde(skip)]
    pub start: usize,
    #[serde(skip)]
    pub end: usize,
}
#[derive(Serialize)]
pub struct Unsupported {
    pub code_index: usize,
    pub context_at: String,
    pub offset: usize,
    pub start: usize,
    pub end: usize,
    pub opcode: u8,
    pub name: String,
    pub status: &'static str,
}
#[derive(Default)]
pub struct Scan {
    pub hits: Vec<Hit>,
    pub unsupported: Vec<Unsupported>,
    pub instructions: usize,
    pub scanned_codes: usize,
    retained_output_minimum: usize,
}

fn operation(op: Op, position: usize) -> Option<&'static str> {
    Some(match op {
        Op::LOAD_NAME => "name_read",
        Op::STORE_NAME => "name_write",
        Op::DELETE_NAME => "name_delete",
        Op::LOAD_GLOBAL => "global_read",
        Op::STORE_GLOBAL => "global_write",
        Op::DELETE_GLOBAL => "global_delete",
        Op::LOAD_FAST | Op::LOAD_FAST_CHECK | Op::LOAD_FAST_LOAD_FAST => "local_read",
        Op::STORE_FAST_LOAD_FAST if position == 0 => "local_write",
        Op::STORE_FAST_LOAD_FAST => "local_read",
        Op::STORE_FAST | Op::STORE_FAST_STORE_FAST => "local_write",
        Op::DELETE_FAST => "local_delete",
        Op::LOAD_FAST_AND_CLEAR => "local_read_clear",
        Op::MAKE_CELL => "cell_create",
        Op::LOAD_CLOSURE => "closure_capture",
        Op::LOAD_DEREF => "closure_read",
        Op::LOAD_CLASSDEREF => "class_closure_read",
        Op::STORE_DEREF => "closure_write",
        Op::DELETE_DEREF => "closure_delete",
        Op::LOAD_ATTR | Op::LOAD_SUPER_ATTR | Op::LOAD_METHOD => "attribute_read",
        Op::STORE_ATTR => "attribute_write",
        Op::DELETE_ATTR => "attribute_delete",
        Op::IMPORT_NAME => "import_name",
        Op::IMPORT_FROM => "import_from",
        Op::LOAD_FROM_DICT_OR_GLOBALS => "dict_or_global_read",
        Op::LOAD_FROM_DICT_OR_DEREF => "dict_or_closure_read",
        _ => return None,
    })
}
fn invalid(message: &str) -> Error {
    Error::new("INVALID_INPUT", message)
}
fn reserve(a: &mut Analysis, scan: &mut Scan, minimum: usize) -> Result<()> {
    a.charge_work(1)?;
    scan.retained_output_minimum = scan.retained_output_minimum.saturating_add(minimum);
    if scan.retained_output_minimum > rxd_core::Budget::default().metadata_bytes {
        return Err(Error::failure(
            rxd_core::Failure::Resource,
            "reference metadata bytes",
        ));
    }
    Ok(())
}
pub enum Query<'a> {
    Name(&'a str),
    NameFilter(&'a rxd_core::filter::Contains),
    Constant {
        id: &'a str,
        selected: &'a SelectedConstant,
    },
}
/// Scan name scope or only the selected constant owner, once per indexed code.
/// `scan` retains actual counters even when a cumulative or structural limit fails.
pub fn scan(
    a: &mut Analysis,
    query: &Query<'_>,
    within: Option<usize>,
    max_instructions: usize,
    scan: &mut Scan,
) -> Result<()> {
    let table = table_for(a.version).map_err(crate::input_error)?;
    let scope = match query {
        Query::Name(_) | Query::NameFilter(_) => within.map_or(0..a.codes.len(), |i| i..i + 1),
        Query::Constant { selected, .. } if within.is_none_or(|i| i == selected.owner) => {
            selected.owner..selected.owner + 1
        }
        Query::Constant { .. } => 0..0,
    };
    for owner in scope {
        if a.body_status(owner) == "unavailable" {
            return Err(invalid("scanned code has no readable bytecode body"));
        }
        let code = a.codes[owner].code.clone();
        let units = code.code.len() / 2;
        if units > max_instructions {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "scanned code instruction budget exceeded",
            ));
        }
        let span = a
            .field_definition(owner, "code")
            .and_then(|d| a.evidence.occurrences[d].payload)
            .ok_or_else(|| invalid("bytecode payload is unavailable"))?;
        // All retained decoder state is bounded before allocation by the per-code cap
        // and invocation work; decoding does not construct unused code-view JSON.
        let mut decoded = 0usize;
        let result = decode_evidence_limited(&code, table, a.version, |amount| {
            a.charge_work(amount).map_err(|_| {
                pycdc::PycError::ResourceLimit("static instruction work budget exceeded")
            })?;
            decoded = decoded.saturating_add(amount);
            Ok(())
        });
        // The last successful charge is raw projection, counted as work but not a
        // second instruction scan. A failed decode has only its actual first pass.
        let (logical, raw) = match result {
            Ok(evidence) => {
                scan.instructions += evidence.1.len();
                evidence
            }
            Err(error) => {
                scan.instructions += decoded.min(units);
                return Err(crate::input_error(error));
            }
        };
        a.charge_work(logical.len().saturating_mul(2))?;
        let boundaries: HashSet<_> = logical
            .iter()
            .flat_map(|ins| [ins.start, ins.offset])
            .collect();
        for ins in &logical {
            a.charge_work(1)?;
            if table.is_jump(ins.opcode)
                && ins
                    .target
                    .is_none_or(|t| t >= code.code.len() || !boundaries.contains(&t))
            {
                return Err(invalid(
                    "jump target is outside bytecode or not an instruction boundary",
                ));
            }
        }
        let local_count = a
            .field_definition(owner, "localsplus_names")
            .map_or(0, |d| a.evidence.occurrences[d].children.len());
        let kinds = a
            .field_definition(owner, "localsplus_kinds")
            .and_then(|d| a.payload(d));
        if a.version.minor >= 11 && kinds.is_none_or(|k| k.len() != local_count) {
            return Err(invalid("localsplus name/kind table lengths differ"));
        }
        for ins in &logical {
            a.charge_work(1)?;
            let opname = table.name(ins.opcode);
            if ins.op == Op::Unknown || opname == "RESERVED" || opname.starts_with("INSTRUMENTED_")
            {
                reserve(a, scan, 192)?;
                scan.unsupported.push(Unsupported {
                    code_index: owner,
                    context_at: rxd_core::envelope::at_of(&a.code_id(owner)).into_owned(),
                    offset: ins.offset,
                    start: span.start + ins.offset,
                    end: span.start + ins.offset + 2,
                    opcode: ins.opcode,
                    name: opname.into(),
                    status: "UNSUPPORTED_OPCODE",
                });
                continue;
            }
            // Resolve/validate every supported operand even for a zero-hit query.
            let packed = a.version.minor == 13
                && matches!(
                    ins.op,
                    Op::LOAD_FAST_LOAD_FAST | Op::STORE_FAST_LOAD_FAST | Op::STORE_FAST_STORE_FAST
                );
            let resolutions = usize::from(table.takes_const(ins.opcode))
                + usize::from(table.takes_name(ins.opcode))
                + if packed {
                    2
                } else {
                    usize::from(table.takes_local(ins.opcode) || table.takes_free(ins.opcode))
                };
            a.charge_work(resolutions)?;
            let targets = instruction_targets(a, owner, ins, table)?;
            for (position, target) in targets.into_iter().enumerate() {
                let (
                    operation,
                    constant_id,
                    constant_slot,
                    constant_path,
                    literal_kind,
                    relation,
                    target_name,
                ) = match query {
                    Query::Constant { id, selected } => {
                        a.charge_work(1)?;
                        if target.table != "consts" || target.index != selected.path[0] {
                            continue;
                        }
                        let operation = match ins.op {
                            Op::LOAD_CONST => "constant_load",
                            Op::RETURN_CONST => "constant_return",
                            Op::KW_NAMES => "keyword_names",
                            _ => continue,
                        };
                        a.charge_work(id.len().saturating_add(selected.path.len()))?;
                        let path_bytes = selected.path.iter().fold(
                            selected.path.len().saturating_sub(1),
                            |n, &i| {
                                n.saturating_add(if i == 0 { 1 } else { i.ilog10() as usize + 1 })
                            },
                        );
                        reserve(
                            a,
                            scan,
                            256usize.saturating_add(id.len()).saturating_add(path_bytes),
                        )?;
                        (
                            operation,
                            Some((*id).to_owned()),
                            Some(selected.path[0]),
                            Some(selected.path.clone()),
                            Some(selected.kind),
                            Some(if selected.path.len() == 1 {
                                "direct_constant"
                            } else {
                                "containing_constant"
                            }),
                            None,
                        )
                    }
                    Query::Name(_) | Query::NameFilter(_) => {
                        let Some(operation) = operation(ins.op, position) else {
                            continue;
                        };
                        let field = if target.table == "localsplus" {
                            "localsplus_names"
                        } else {
                            target.table
                        };
                        let definition = a
                            .field_definition(owner, field)
                            .ok_or_else(|| invalid("missing name operand table"))?;
                        let child = a.evidence.occurrences[definition].children[target.index];
                        let definition = a.evidence.occurrences[child].definition;
                        if !matches!(
                            a.evidence.occurrences[definition].object.as_deref(),
                            Some(PyObject::Str(_))
                        ) {
                            return Err(invalid("name operand does not identify a string"));
                        }
                        let length = a
                            .payload(definition)
                            .ok_or_else(|| invalid("name operand has no original payload"))?
                            .len();
                        let matched = match query {
                            Query::Name(name) => {
                                let mut equal = length == name.len();
                                if equal {
                                    for (i, byte) in name.bytes().enumerate() {
                                        a.charge_work(1)?;
                                        if a.payload(definition).unwrap()[i] != byte {
                                            equal = false;
                                            break;
                                        }
                                    }
                                }
                                equal
                            }
                            Query::NameFilter(filter) => {
                                a.charge_work(length)?;
                                // Surrogatepass payload is not valid UTF-8: never match a lossy replacement.
                                std::str::from_utf8(a.payload(definition).unwrap())
                                    .is_ok_and(|s| filter.matches(s))
                            }
                            Query::Constant { .. } => unreachable!(),
                        };
                        if !matched {
                            continue;
                        }
                        a.charge_work(length)?;
                        reserve(a, scan, 256usize.saturating_add(length))?;
                        let target_name = std::str::from_utf8(a.payload(definition).unwrap())
                            .ok()
                            .map(str::to_owned);
                        (operation, None, None, None, None, None, target_name)
                    }
                };
                scan.hits.push(Hit {
                    code_index: owner,
                    offset: ins.offset,
                    opcode: ins.opcode,
                    name: opname.into(),
                    raw_operand: raw[ins.offset / 2].raw_arg,
                    operand: ins.has_arg.then_some(ins.arg),
                    target,
                    target_name,
                    operation,
                    constant_id,
                    constant_slot,
                    constant_path,
                    literal_kind,
                    relation,
                    start: span.start + ins.offset,
                    end: span.start + ins.offset + 2,
                });
            }
        }
        scan.scanned_codes += 1;
    }
    Ok(())
}
