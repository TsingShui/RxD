//! Explicit operand targets for the static reference scanner.
use crate::static_analysis::Analysis;
use pycdc::{
    bytecode::Instruction,
    object::PyObject,
    opcode::{Op, OpcodeTable},
};
use rxd_core::{Error, Result};
use serde::Serialize;

fn invalid(s: &str) -> Error {
    Error::new("INVALID_INPUT", s)
}

#[derive(Serialize)]
pub struct Target {
    pub table: &'static str,
    pub index: usize,

    pub id: String,
    pub value_id: String,
}

fn value_id(a: &Analysis, definition: usize) -> String {
    a.qualify_id(&format!("value:{definition}"))
}
fn table_id(a: &Analysis, owner: usize, table: &str, index: usize) -> String {
    a.qualify_id(&if table == "consts" {
        format!("const:{owner}:{index}")
    } else {
        format!("table:{owner}:{table}:{index}")
    })
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
    let value = a.evidence.occurrences[a.evidence.occurrences[child].definition]
        .object
        .as_deref();
    if table != "consts"
        && !matches!(value, Some(PyObject::Str(_)))
        && !(a.version.is_python2() && matches!(value, Some(PyObject::Bytes(_))))
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
/// Resolve and validate the table operands of one original instruction.
pub(crate) fn instruction_targets(
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
            Op::LOAD_GLOBAL if a.version.at_least(3, 11) => arg >> 1,
            Op::LOAD_ATTR if a.version.at_least(3, 12) => arg >> 1,
            Op::LOAD_SUPER_ATTR => arg >> 2,
            _ => arg,
        };
        targets.push(target(a, owner, "names", index as usize)?);
    }
    if table.takes_local(ins.opcode) || table.takes_free(ins.opcode) {
        if !a.version.at_least(3, 11) {
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
            let packed = a.version.at_least(3, 13)
                && matches!(
                    ins.op,
                    Op::LOAD_FAST_LOAD_FAST
                        | Op::LOAD_FAST_BORROW_LOAD_FAST_BORROW
                        | Op::STORE_FAST_LOAD_FAST
                        | Op::STORE_FAST_STORE_FAST
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
                // 3.13+ local loads, including borrowed loads, address full localsplus.
                let local_mask = if a.version.at_least(3, 13) {
                    0xe0
                } else {
                    0x20
                };
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
