use super::{Builder, Condition, LoopContext, Protected};
use crate::ir::{Terminator, ValueId};

impl Builder<'_, '_> {
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
