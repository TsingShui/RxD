//! Recover comprehension filters from their actual keep/skip branches.
use std::collections::HashMap;

use crate::{
    ast::{BoolOpKind, Expr, ExprRef, UnaryOp},
    budget,
    bytecode::Instruction,
    object::PyObject,
    opcode::Op,
};

#[derive(Debug, Clone)]
pub(crate) struct Guard {
    pub offset: usize,
    pub next: usize,
    pub target: usize,
    pub condition: ExprRef,
    pub jump_if_true: bool,
}

struct Graph<'a> {
    instructions: &'a [Instruction],
    indexes: &'a crate::decompiler::OffsetIndex,
    guards: HashMap<usize, usize>,
    destinations: HashMap<usize, usize>,
    loop_offset: usize,
    end: usize,
    last_guard: usize,
    keep: usize,
    skip: usize,
}

impl Graph<'_> {
    fn destination(&mut self, mut offset: usize) -> Option<usize> {
        let mut trail = Vec::new();
        let result = loop {
            budget::charge(1);
            if offset == self.loop_offset {
                break self.skip;
            }
            if offset <= self.loop_offset || offset >= self.end {
                return None;
            }
            if let Some(&destination) = self.destinations.get(&offset) {
                break destination;
            }
            if let Some(&guard) = self.guards.get(&offset) {
                break guard;
            }
            let instruction = self.instructions.get(*self.indexes.get(&offset)?)?;
            if instruction.start != offset && instruction.offset != offset {
                return None;
            }
            if let Some(&guard) = self.guards.get(&instruction.offset) {
                break guard;
            }
            trail.push(offset);
            if matches!(instruction.op, Op::NOP | Op::CACHE | Op::NOT_TAKEN) {
                offset = instruction.end();
            } else if matches!(
                instruction.op,
                Op::JUMP_FORWARD
                    | Op::JUMP_ABSOLUTE
                    | Op::JUMP_BACKWARD
                    | Op::JUMP_BACKWARD_NO_INTERRUPT
                    | Op::JUMP
            ) {
                let target = instruction.target?;
                if target != self.loop_offset && target <= instruction.offset {
                    return None;
                }
                offset = target;
            } else if instruction.offset > self.last_guard {
                break self.keep;
            } else if instruction.target.is_some() {
                return None;
            } else {
                offset = instruction.end();
            }
        };
        for offset in trail {
            budget::charge(1);
            self.destinations.insert(offset, result);
        }
        Some(result)
    }
}

fn boolean(op: BoolOpKind, left: ExprRef, right: ExprRef) -> ExprRef {
    budget::rc(Expr::BoolOp {
        op,
        values: vec![left, right],
    })
}

fn not(condition: ExprRef) -> ExprRef {
    budget::rc(Expr::Unary {
        op: UnaryOp::Not,
        operand: condition,
    })
}

fn shared_suffix(expression: &ExprRef, suffix: &ExprRef) -> Option<(BoolOpKind, ExprRef)> {
    let Expr::BoolOp { op, values } = &**expression else {
        return None;
    };
    let [prefix, tail] = values.as_slice() else {
        return None;
    };
    ExprRef::ptr_eq(tail, suffix).then(|| (*op, crate::budget_clone!(prefix)))
}

// Factor shared suffixes instead of expanding the condition DAG into a tree.
fn reconverge(condition: ExprRef, yes: ExprRef, no: ExprRef) -> Option<ExprRef> {
    if ExprRef::ptr_eq(&yes, &no) {
        let yes_value = budget::rc(Expr::Const(budget::rc(PyObject::True)));
        let probe = budget::rc(Expr::Ternary {
            cond: condition,
            then_expr: crate::budget_clone!(yes_value),
            else_expr: yes_value,
        });
        return Some(boolean(BoolOpKind::And, probe, yes));
    }
    if let Some((op, prefix)) = shared_suffix(&yes, &no) {
        let (test, inner) = match op {
            BoolOpKind::Or => (condition, BoolOpKind::And),
            BoolOpKind::And => (not(condition), BoolOpKind::Or),
        };
        return Some(boolean(op, boolean(inner, test, prefix), no));
    }
    if let Some((op, prefix)) = shared_suffix(&no, &yes) {
        let (test, inner) = match op {
            BoolOpKind::Or => (not(condition), BoolOpKind::And),
            BoolOpKind::And => (condition, BoolOpKind::Or),
        };
        return Some(boolean(op, boolean(inner, test, prefix), yes));
    }
    None
}

pub(crate) fn reconstruct(
    guards: &[Guard],
    instructions: &[Instruction],
    indexes: &crate::decompiler::OffsetIndex,
    loop_offset: usize,
    end: usize,
) -> Option<ExprRef> {
    let _frame = budget::enter();
    let last_guard = guards.last()?.offset;
    let keep = guards.len();
    let skip = keep.checked_add(1)?;
    let mut graph = Graph {
        instructions,
        indexes,
        guards: HashMap::new(),
        destinations: HashMap::new(),
        loop_offset,
        end,
        last_guard,
        keep,
        skip,
    };
    for (index, guard) in guards.iter().enumerate() {
        budget::charge(1);
        if index > 0 && guard.offset <= guards[index - 1].offset {
            return None;
        }
        graph.guards.insert(guard.offset, index);
    }
    let mut expressions = budget::filled(None, skip.checked_add(1)?);
    expressions[keep] = Some(budget::rc(Expr::Const(budget::rc(PyObject::True))));
    expressions[skip] = Some(budget::rc(Expr::Const(budget::rc(PyObject::False))));
    for (index, guard) in guards.iter().enumerate().rev() {
        budget::charge(1);
        let jumped = graph.destination(guard.target)?;
        let continued = graph.destination(guard.next)?;
        let (yes, no) = if guard.jump_if_true {
            (jumped, continued)
        } else {
            (continued, jumped)
        };
        if yes <= index || no <= index {
            return None;
        }
        let condition = crate::budget_clone!(guard.condition);
        let then_expr = crate::budget_clone!(expressions.get(yes)?.as_ref()?);
        let else_expr = crate::budget_clone!(expressions.get(no)?.as_ref()?);
        expressions[index] = Some(match (yes, no) {
            (t, f) if t == keep && f == skip => condition,
            (t, f) if t == skip && f == keep => not(condition),
            (t, f) if t == keep && f != keep => boolean(BoolOpKind::Or, condition, else_expr),
            (t, f) if f == skip && t != skip => boolean(BoolOpKind::And, condition, then_expr),
            (t, f) if t == skip && f != skip => boolean(BoolOpKind::And, not(condition), else_expr),
            (t, f) if f == keep && t != keep => boolean(BoolOpKind::Or, not(condition), then_expr),
            (t, f) if t == f && (t == keep || t == skip) => budget::rc(Expr::Ternary {
                cond: condition,
                then_expr,
                else_expr,
            }),
            _ => reconverge(condition, then_expr, else_expr)?,
        });
    }
    expressions.into_iter().next().flatten()
}
