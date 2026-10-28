use super::{Builder, Choice, Condition, LoopContext, Node, Protected};
use crate::{
    ir::{Action, Constant, Event, Op, Slot, Terminator, ValueId},
    reader::resource,
};
use oxc_syntax::operator::UnaryOperator;
use rxd_core::Result;

impl Builder<'_, '_> {
    fn slot_predicate(&self, condition: &Condition, slot: Slot) -> Option<bool> {
        let Condition::Value(mut value, nonzero) = *condition else {
            return None;
        };
        if !self.values.boolean[value.0] {
            return None;
        }
        let mut inverted = !nonzero;
        for _ in 0..self.budget.ast_depth {
            match self.function.values[value.0].op {
                Op::Read(read) if read == slot => return Some(inverted),
                Op::Unary(UnaryOperator::LogicalNot, input) => {
                    value = input;
                    inverted = !inverted;
                }
                _ => return None,
            }
        }
        None
    }

    fn bool_choice(&self, choice: &Choice) -> Option<bool> {
        let Choice::Value(value) = choice else {
            return None;
        };
        match self.function.values[value.0].op {
            Op::Constant(Constant::Bool(value)) => Some(value),
            _ => None,
        }
    }

    // Collapse a boolean phi only when its slot is dead on both outgoing edges.
    pub(super) fn merge_boolean_joins(&self, nodes: Vec<Node>) -> Result<Vec<Node>> {
        let mut result = Vec::new();
        result
            .try_reserve_exact(nodes.len())
            .map_err(|_| resource("Ark condition plan allocation failed"))?;
        for node in nodes {
            result.push(node);
            if result.len() < 3 {
                continue;
            }
            let tail = &result[result.len() - 3..];
            let (Node::Select { slot, yes, no, .. }, Node::Block(join), Node::If { condition, .. }) =
                (&tail[0], &tail[1], &tail[2])
            else {
                continue;
            };
            let (Some(yes), Some(no)) = (self.bool_choice(yes), self.bool_choice(no)) else {
                continue;
            };
            if yes == no || !self.statement_free(*join) {
                continue;
            }
            let Terminator::Branch {
                yes: yes_block,
                no: no_block,
                ..
            } = self.function.blocks[*join].end
            else {
                continue;
            };
            if self.values.live_in(yes_block, *slot) || self.values.live_in(no_block, *slot) {
                continue;
            }
            let Some(invert_test) = self.slot_predicate(condition, *slot) else {
                continue;
            };
            let invert = !yes ^ invert_test;
            let Node::If { yes, no, .. } = result.pop().unwrap() else {
                unreachable!()
            };
            result.pop();
            let Node::Select { condition, .. } = result.pop().unwrap() else {
                unreachable!()
            };
            let condition = if invert {
                Condition::Not(Box::new(condition))
            } else {
                condition
            };
            result.push(Node::If { condition, yes, no });
        }
        Ok(result)
    }

    // A join can become an expression only when each arm emits at most one slot write.
    fn select_arm(&self, nodes: &[Node]) -> Option<Option<(Slot, ValueId)>> {
        let mut assignment = None;
        for node in nodes {
            let Node::Block(block) = node else {
                return None;
            };
            for (position, event) in self.function.blocks[*block].events.iter().enumerate() {
                match event {
                    Event::Define(id) if self.values.needed[id.0] => {
                        if self.values.materialize[id.0]
                            || (self.function.values[id.0].effects.ordered()
                                && self.values.uses[id.0] == 0)
                        {
                            return None;
                        }
                    }
                    Event::Action { action, .. } if self.values.keep[*block][position] => {
                        let Action::Write(slot, value) = action else {
                            return None;
                        };
                        if assignment.replace((*slot, *value)).is_some() {
                            return None;
                        }
                    }
                    _ => {}
                }
            }
        }
        Some(assignment)
    }

    pub(super) fn select(&self, yes: &[Node], no: &[Node]) -> Option<(Slot, Choice, Choice)> {
        let yes = self.select_arm(yes)?;
        let no = self.select_arm(no)?;
        let slot = yes.or(no)?.0;
        if yes.is_some_and(|(target, _)| target != slot)
            || no.is_some_and(|(target, _)| target != slot)
        {
            return None;
        }
        if matches!(slot, Slot::Register(reg) if Some(reg) == self.function.frame.this_reg || Some(reg) == self.function.frame.new_target || Some(reg) == self.function.frame.func)
        {
            return None;
        }
        Some((
            slot,
            yes.map_or(Choice::Existing, |(_, value)| Choice::Value(value)),
            no.map_or(Choice::Existing, |(_, value)| Choice::Value(value)),
        ))
    }

    pub(super) fn condition(
        &mut self,
        value: ValueId,
        nonzero: bool,
        targets: (usize, usize),
        stop: Option<usize>,
        active: Option<LoopContext>,
        scope: Option<&Protected>,
    ) -> (Condition, usize, usize) {
        let (mut yes, mut no) = targets;
        let mut condition = Condition::Value(value, nonzero);
        for _ in 0..self.budget.ast_depth / 8 {
            let mut folded = false;
            for (candidate, all, shared) in [(no, false, yes), (yes, true, no)] {
                if self.owned[candidate]
                    || !self.in_scope(candidate, scope)
                    || self.function.blocks[candidate].handlers.first()
                        != self.function.blocks[self.function.values[value.0].block]
                            .handlers
                            .first()
                    || Some(candidate) == stop
                    || self.control.predecessors[candidate].len() != 1
                    || active.is_some_and(|active| {
                        candidate == active.header || Some(candidate) == active.exit
                    })
                    || !self.statement_free(candidate)
                {
                    continue;
                }
                let Terminator::Branch {
                    value,
                    nonzero,
                    yes: next_yes,
                    no: next_no,
                } = self.function.blocks[candidate].end
                else {
                    continue;
                };
                let (negate, next) = if next_yes == shared {
                    (all, next_no)
                } else if next_no == shared {
                    (!all, next_yes)
                } else {
                    continue;
                };
                let mut right = Condition::Value(value, nonzero);
                if negate {
                    right = Condition::Not(Box::new(right));
                }
                condition = Condition::ShortCircuit {
                    all,
                    left: Box::new(condition),
                    right: Box::new(right),
                };
                self.owned[candidate] = true;
                if all {
                    yes = next;
                } else {
                    no = next;
                }
                folded = true;
                break;
            }
            if !folded {
                break;
            }
        }
        (condition, yes, no)
    }
}
