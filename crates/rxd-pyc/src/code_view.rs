//! Single-code original evidence, built from the sole marshal parser and shared strict scanner.
use crate::static_analysis::Analysis;
use pycdc::{
    bytecode::{Instruction, decode_evidence_limited},
    object::PyObject,
    opcode::{Op, OpcodeTable, table_for},
};
use rxd_core::{Error, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
#[path = "code_locations.rs"]
mod locations;

fn invalid(s: &str) -> Error {
    Error::new("INVALID_INPUT", s)
}
pub fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 15) as usize] as char);
    }
    s
}
#[derive(Serialize)]
pub struct Prefix {
    pub offset: usize,
    pub opcode: u8,
    pub raw_operand: u32,
    pub raw_hex: String,
}
#[derive(Serialize)]
pub struct Target {
    pub table: &'static str,
    pub index: usize,
    /// Copy-only coordinate, the same form a row prints, so `--at` accepts it.
    pub id: String,
    pub value_id: String,
}

#[derive(Serialize)]
pub struct Unit {
    pub offset: usize,
    pub size: usize,
    pub start: usize,
    pub end: usize,
    pub opcode: u8,
    pub name: String,
    pub raw_operand: Option<u32>,
    pub operand: Option<u32>,
    pub raw_hex: String,
    pub role: &'static str,
    pub owner_offset: Option<usize>,
    pub logical_start: Option<usize>,
    pub logical_size: Option<usize>,
    pub prefixes: Vec<Prefix>,
    pub jump_target: Option<usize>,
    pub targets: Vec<Target>,
    pub status: &'static str,
    pub line: Option<u32>,
}
#[derive(Serialize)]
pub struct Entry {
    pub index: usize,
    pub id: String,
    pub value_id: String,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub alias_start: Option<usize>,
    pub alias_end: Option<usize>,
    pub reference: bool,
    pub kind: Option<u8>,
}
#[derive(Serialize)]
pub struct Table {
    pub present: bool,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub definition_start: Option<usize>,
    pub definition_end: Option<usize>,
    pub entries: Vec<Entry>,
}
#[derive(Serialize)]
pub struct ValueChild {
    pub index: usize,
    pub value_id: String,
    pub start: usize,
    pub end: usize,
    pub reference: bool,
}
#[derive(Serialize)]
pub struct Value {
    pub id: String,
    pub tag: u8,
    pub kind: &'static str,
    pub value: Option<String>,
    pub raw_hex: Option<String>,
    pub definition_start: usize,
    pub definition_end: usize,
    pub payload_start: Option<usize>,
    pub payload_end: Option<usize>,
    pub children: Vec<ValueChild>,
    pub code_id: Option<String>,
}
#[derive(Serialize)]
pub struct Evidence {
    pub bytecode_hex: String,
    pub instructions: Vec<Unit>,
    pub tables: BTreeMap<&'static str, Table>,
    pub values: Vec<Value>,
    pub line_table_hex: String,
    pub line_mapping: Vec<locations::Location>,
    pub exception_table_hex: String,
    pub exceptions: Vec<locations::Exception>,
}
fn value_id(_a: &Analysis, definition: usize) -> String {
    format!("value:{definition}")
}
fn table_id(_a: &Analysis, owner: usize, table: &str, index: usize) -> String {
    if table == "consts" {
        format!("const:{owner}:{index}")
    } else {
        format!("table:{owner}:{table}:{index}")
    }
}
fn target(a: &Analysis, owner: usize, table: &'static str, index: usize) -> Result<Target> {
    let field = if table == "localsplus" {
        "localsplus_names"
    } else {
        table
    };
    let definition = a
        .field_definition(owner, field)
        .ok_or_else(|| invalid("missing operand table"))?;
    let child = *a.evidence.occurrences[definition]
        .children
        .get(index)
        .ok_or_else(|| invalid("instruction operand table index is out of range"))?;
    if table != "consts"
        && !matches!(
            a.evidence.occurrences[a.evidence.occurrences[child].definition]
                .object
                .as_deref(),
            Some(PyObject::Str(_))
        )
    {
        return Err(invalid("name operand does not identify a string"));
    }
    Ok(Target {
        table,
        index,
        id: table_id(a, owner, table, index),
        value_id: value_id(a, a.evidence.occurrences[child].definition),
    })
}
/// Shared verified-final table operand resolution for FETCH and XREF.
pub fn instruction_targets(
    a: &Analysis,
    owner: usize,
    ins: &Instruction,
    table: &OpcodeTable,
) -> Result<Vec<Target>> {
    let arg = ins.arg;
    let mut targets = Vec::new();
    if table.takes_const(ins.opcode) {
        targets.push(target(a, owner, "consts", arg as usize)?);
    }
    if table.takes_name(ins.opcode) {
        let index = match ins.op {
            Op::LOAD_GLOBAL if a.version.minor >= 11 => arg >> 1,
            Op::LOAD_ATTR if a.version.minor >= 12 => arg >> 1,
            Op::LOAD_SUPER_ATTR => arg >> 2,
            _ => arg,
        };
        targets.push(target(a, owner, "names", index as usize)?);
    }
    if table.takes_local(ins.opcode) || table.takes_free(ins.opcode) {
        if a.version.minor <= 10 {
            if table.takes_local(ins.opcode) {
                targets.push(target(a, owner, "varnames", arg as usize)?);
            } else {
                let cells = a
                    .field_definition(owner, "cellvars")
                    .map_or(0, |d| a.evidence.occurrences[d].children.len());
                let index = arg as usize;
                targets.push(if index < cells {
                    target(a, owner, "cellvars", index)?
                } else {
                    target(a, owner, "freevars", index - cells)?
                });
            }
        } else {
            let kinds = a
                .payload(
                    a.field_definition(owner, "localsplus_kinds")
                        .ok_or_else(|| invalid("missing localsplus kinds"))?,
                )
                .ok_or_else(|| invalid("missing localsplus kinds payload"))?;
            let packed = a.version.minor == 13
                && matches!(
                    ins.op,
                    Op::LOAD_FAST_LOAD_FAST | Op::STORE_FAST_LOAD_FAST | Op::STORE_FAST_STORE_FAST
                );
            let indexes = if packed {
                vec![(arg >> 4) as usize, (arg & 15) as usize]
            } else {
                vec![arg as usize]
            };
            for index in indexes {
                let t = target(a, owner, "localsplus", index)?;
                let kind = *kinds
                    .get(index)
                    .ok_or_else(|| invalid("missing localsplus kind"))?;
                // 3.13 lowers LOAD_CLOSURE to LOAD_FAST, including cell-only
                // and free-only slots. GETLOCAL indexes the full localsplus.
                let local_mask = if a.version.minor == 13 { 0xe0 } else { 0x20 };
                if (table.takes_local(ins.opcode) && kind & local_mask == 0)
                    || (table.takes_free(ins.opcode) && kind & 0xc0 == 0)
                {
                    return Err(invalid("operand uses the wrong localsplus kind"));
                }
                targets.push(t);
            }
        }
    }
    if table.is_compare(ins.opcode)
        && pycdc::bytecode::compare_op_index(arg, a.version) >= table.cmp_op.len()
    {
        return Err(invalid("invalid comparison operand"));
    }
    if ins.op == Op::BINARY_OP && pycdc::bytecode::binary_op_name(arg, a.version).is_none() {
        return Err(invalid("invalid binary operator operand"));
    }
    Ok(targets)
}
fn selected_definitions(a: &mut Analysis, owner: usize) -> Result<Vec<usize>> {
    let mut pending = Vec::new();
    for field in [
        "consts",
        "names",
        "localsplus_names",
        "varnames",
        "freevars",
        "cellvars",
    ] {
        if let Some(d) = a.field_definition(owner, field) {
            a.charge_work(a.evidence.occurrences[d].children.len())?;
            pending.extend(a.evidence.occurrences[d].children.iter().rev().copied());
        }
    }
    let mut seen = HashSet::new();
    let mut values = Vec::new();
    while let Some(occurrence) = pending.pop() {
        a.charge_work(1)?;
        let d = a.evidence.occurrences[occurrence].definition;
        if !seen.insert(d) {
            continue;
        }
        values.push(d);
        if !matches!(
            a.evidence.occurrences[d].object.as_deref(),
            Some(PyObject::Code(_))
        ) {
            a.charge_work(a.evidence.occurrences[d].children.len())?;
            pending.extend(a.evidence.occurrences[d].children.iter().rev().copied());
        }
    }
    Ok(values)
}
fn table(a: &Analysis, owner: usize, name: &'static str, selection: Option<u8>) -> Result<Table> {
    let selection = if a.version.minor >= 11 {
        selection
    } else {
        None
    };
    let field = if selection.is_some() || name == "localsplus" {
        "localsplus_names"
    } else {
        name
    };
    let Some(site) = a.field_occurrence(owner, field) else {
        return Ok(Table {
            present: false,
            start: None,
            end: None,
            definition_start: None,
            definition_end: None,
            entries: Vec::new(),
        });
    };
    let site = &a.evidence.occurrences[site];
    let definition = &a.evidence.occurrences[site.definition];
    let kinds = a
        .field_definition(owner, "localsplus_kinds")
        .and_then(|d| a.payload(d));
    if field == "localsplus_names" && kinds.map(<[u8]>::len) != Some(definition.children.len()) {
        return Err(invalid("localsplus name/kind table lengths differ"));
    }
    let mut entries = Vec::new();
    for (index, &child) in definition.children.iter().enumerate() {
        let kind = (field == "localsplus_names").then(|| kinds.unwrap()[index]);
        if selection.is_some_and(|mask| kind.unwrap() & mask == 0) {
            continue;
        }
        let row = &a.evidence.occurrences[child];
        let span = (!site.is_reference).then_some(row.span);
        entries.push(Entry {
            index,
            id: table_id(a, owner, name, index),
            value_id: value_id(a, row.definition),
            start: span.map(|s| s.start),
            end: span.map(|s| s.end),
            alias_start: site.is_reference.then_some(site.span.start),
            alias_end: site.is_reference.then_some(site.span.end),
            reference: row.is_reference,
            kind,
        });
    }
    Ok(Table {
        present: true,
        start: Some(site.span.start),
        end: Some(site.span.end),
        definition_start: Some(definition.span.start),
        definition_end: Some(definition.span.end),
        entries,
    })
}
fn value(a: &Analysis, d: usize) -> Result<Value> {
    let row = &a.evidence.occurrences[d];
    let object = row
        .object
        .as_deref()
        .ok_or_else(|| invalid("unavailable value evidence"))?;
    let (kind, text) = match object {
        PyObject::None => ("none", None),
        PyObject::True => ("bool", Some("true".into())),
        PyObject::False => ("bool", Some("false".into())),
        PyObject::Ellipsis => ("ellipsis", None),
        PyObject::StopIteration => ("stop_iteration", None),
        PyObject::Int(n) => ("int", Some(n.to_string())),
        PyObject::Long(n) => ("int", Some(n.decimal.clone())),
        PyObject::Float(f, _) => ("float", Some(pycdc::bytecode::float_repr(*f))),
        PyObject::Complex(re, im) => (
            "complex",
            Some(format!(
                "{},{}",
                pycdc::bytecode::float_repr(*re),
                pycdc::bytecode::float_repr(*im)
            )),
        ),
        PyObject::Str(_) => (
            "str",
            a.payload(d)
                .and_then(|b| std::str::from_utf8(b).ok())
                .map(str::to_owned),
        ),
        PyObject::Bytes(_) => ("bytes", None),
        PyObject::Code(_) => ("code", None),
        PyObject::Tuple(_) => ("tuple", None),
        PyObject::List(_) => ("list", None),
        PyObject::Dict(_) => ("dict", None),
        PyObject::Set(_) => ("set", None),
        PyObject::FrozenSet(_) => ("frozenset", None),
        PyObject::Slice(..) => ("slice", None),
    };
    let raw = if row.children.is_empty() && !matches!(object, PyObject::Code(_)) {
        Some(hex(
            if matches!(object, PyObject::Str(_) | PyObject::Bytes(_)) {
                a.payload(d)
                    .ok_or_else(|| invalid("missing literal payload"))?
            } else {
                &a.bytes[row.span.start..row.span.end]
            },
        ))
    } else {
        None
    };
    Ok(Value {
        id: value_id(a, d),
        tag: a.bytes[row.span.start],
        kind,
        value: text,
        raw_hex: raw,
        definition_start: row.span.start,
        definition_end: row.span.end,
        payload_start: row.payload.map(|s| s.start),
        payload_end: row.payload.map(|s| s.end),
        children: row
            .children
            .iter()
            .enumerate()
            .map(|(index, &c)| {
                let child = &a.evidence.occurrences[c];
                ValueChild {
                    index,
                    value_id: value_id(a, child.definition),
                    start: child.span.start,
                    end: child.span.end,
                    reference: child.is_reference,
                }
            })
            .collect(),
        code_id: a.code_index(d).map(|i| a.code_id(i)),
    })
}
/// Caller owns complete bounded serialization. Allocation preflight is conservative and
/// additionally guards raw rows/tables/hex payload before those allocations.
pub fn build(a: &mut Analysis, owner: usize, max_instructions: usize) -> Result<Evidence> {
    if a.body_status(owner) == "unavailable" {
        return Err(invalid("selected code has no readable bytecode body"));
    }
    let code = a.codes[owner].code.clone();
    let n = code.code.len() / 2;
    if n > max_instructions {
        return Err(Error::new(
            "RESOURCE_LIMIT",
            "selected code instruction budget exceeded",
        ));
    }
    // Fixed-key JSON lower bounds (shortest scalar values, before dynamic IDs/hex).
    // They bound row allocation before complete bounded serialization.
    const UNIT_MIN_BYTES: usize = 239;
    const VALUE_MIN_BYTES: usize = 232;
    const CHILD_MIN_BYTES: usize = 140;
    const TABLE_ENTRY_MIN_BYTES: usize = 260;
    let mut minimum = n
        .saturating_mul(UNIT_MIN_BYTES)
        .saturating_add(code.code.len().saturating_mul(2))
        .saturating_add(code.line_table.len().saturating_mul(2))
        .saturating_add(code.exception_table.len().saturating_mul(2));
    let definitions = selected_definitions(a, owner)?;
    for &d in &definitions {
        let row = &a.evidence.occurrences[d];
        let length = row
            .payload
            .map_or(row.span.end - row.span.start, |s| s.end - s.start);
        minimum = minimum
            .saturating_add(VALUE_MIN_BYTES)
            .saturating_add(row.children.len().saturating_mul(CHILD_MIN_BYTES));
        if row.children.is_empty() && !matches!(row.object.as_deref(), Some(PyObject::Code(_))) {
            minimum = minimum.saturating_add(length.saturating_mul(2));
            match row.object.as_deref() {
                Some(PyObject::Str(_)) => {
                    minimum = minimum.saturating_add(
                        a.payload(d)
                            .and_then(|b| std::str::from_utf8(b).ok())
                            .map_or(0, str::len),
                    );
                }
                Some(PyObject::Long(n)) => {
                    minimum = minimum.saturating_add(n.decimal.len());
                }
                _ => {}
            }
        }
    }
    for field in [
        "consts",
        "names",
        "localsplus_names",
        "varnames",
        "freevars",
        "cellvars",
    ] {
        if let Some(d) = a.field_definition(owner, field) {
            let count = a.evidence.occurrences[d].children.len();
            a.charge_work(if field == "localsplus_names" {
                count.saturating_mul(4)
            } else {
                count
            })?;
            let mut rows = count;
            if field == "localsplus_names" {
                let kinds = a
                    .field_definition(owner, "localsplus_kinds")
                    .and_then(|d| a.payload(d))
                    .ok_or_else(|| invalid("missing localsplus kinds"))?;
                if kinds.len() != count {
                    return Err(invalid("localsplus name/kind table lengths differ"));
                }
                for &kind in kinds {
                    rows = rows.saturating_add(
                        [0x20, 0x40, 0x80]
                            .into_iter()
                            .filter(|mask| kind & mask != 0)
                            .count(),
                    );
                }
            }
            minimum = minimum.saturating_add(rows.saturating_mul(TABLE_ENTRY_MIN_BYTES));
        }
    }
    a.charge_work(
        code.line_table
            .len()
            .saturating_add(code.exception_table.len()),
    )?;
    let table = table_for(a.version).map_err(crate::input_error)?;
    let (logical, raw) = decode_evidence_limited(&code, table, a.version, |amount| {
        a.charge_work(amount)
            .map_err(|_| pycdc::PycError::ResourceLimit("static instruction work budget exceeded"))
    })
    .map_err(crate::input_error)?;
    let mut boundaries = HashSet::new();
    let mut by_offset = HashMap::new();
    for ins in &logical {
        boundaries.insert(ins.start);
        boundaries.insert(ins.offset);
        by_offset.insert(ins.offset, ins);
    }
    for ins in &logical {
        if table.is_jump(ins.opcode)
            && (ins
                .target
                .is_none_or(|t| t >= code.code.len() || !boundaries.contains(&t)))
        {
            return Err(invalid(
                "jump target is outside bytecode or not an instruction boundary",
            ));
        }
    }
    let line_mapping = locations::locations(
        &code.line_table,
        code.first_line,
        code.code.len(),
        a.version.minor,
    )?;
    let exceptions = locations::exceptions(
        &code.exception_table,
        code.code.len(),
        code.stack_size,
        &boundaries,
    )?;
    let span = a.evidence.occurrences[a.field_definition(owner, "code").unwrap()]
        .payload
        .ok_or_else(|| invalid("bytecode payload is unavailable"))?;
    let mut instructions = Vec::new();
    let mut location = 0;
    for raw in raw {
        while location < line_mapping.len() && line_mapping[location].end <= raw.offset {
            location += 1;
        }
        let line = line_mapping.get(location).and_then(|l| l.line);
        let ins = by_offset
            .get(&raw.offset)
            .copied()
            .filter(|_| raw.role == "instruction");
        let name = table.name(raw.opcode);
        let unsupported = raw.role == "instruction"
            && (table.op(raw.opcode) == Op::Unknown
                || name == "RESERVED"
                || name.starts_with("INSTRUMENTED_"));
        let targets = if let Some(ins) = ins {
            if unsupported {
                Vec::new()
            } else {
                instruction_targets(a, owner, ins, table)?
            }
        } else {
            Vec::new()
        };
        let mut prefixes = Vec::new();
        if let Some(ins) = ins {
            for off in (ins.start..ins.offset).step_by(2) {
                prefixes.push(Prefix {
                    offset: off,
                    opcode: code.code[off],
                    raw_operand: u32::from(code.code[off + 1]),
                    raw_hex: hex(&code.code[off..off + 2]),
                });
            }
        }
        instructions.push(Unit {
            offset: raw.offset,
            size: raw.size,
            start: span.start + raw.offset,
            end: span.start + raw.offset + raw.size,
            opcode: raw.opcode,
            name: name.into(),
            raw_operand: raw.raw_arg,
            operand: raw.effective_arg,
            raw_hex: hex(&code.code[raw.offset..raw.offset + raw.size]),
            role: raw.role,
            owner_offset: raw.owner_offset,
            logical_start: ins.map(|i| i.start),
            logical_size: ins.map(|i| i.size),
            prefixes,
            jump_target: raw.target,
            targets,
            status: if unsupported {
                "UNSUPPORTED_OPCODE"
            } else if raw.role == "cache" {
                "RAW_CACHE"
            } else {
                "DECODED"
            },
            line,
        });
    }
    let mut tables = BTreeMap::new();
    for (name, mask) in [
        ("consts", None),
        ("names", None),
        ("localsplus", None),
        ("varnames", Some(0x20)),
        ("cellvars", Some(0x40)),
        ("freevars", Some(0x80)),
    ] {
        tables.insert(name, self::table(a, owner, name, mask)?);
    }
    for &d in &definitions {
        let row = &a.evidence.occurrences[d];
        let length = row
            .payload
            .map_or(row.span.end - row.span.start, |s| s.end - s.start);
        if row.children.is_empty() && !matches!(row.object.as_deref(), Some(PyObject::Code(_))) {
            a.charge_work(length)?;
        }
    }
    let values = definitions
        .into_iter()
        .map(|d| value(a, d))
        .collect::<Result<_>>()?;
    Ok(Evidence {
        bytecode_hex: hex(&code.code),
        instructions,
        tables,
        values,
        line_table_hex: hex(&code.line_table),
        line_mapping,
        exception_table_hex: hex(&code.exception_table),
        exceptions,
    })
}
