use super::{Builder, Condition, LoopContext};
use crate::ir::{Event, Terminator, ValueId};

impl Builder<'_, '_> {
    pub(super) fn condition(
        &mut self,
        value: ValueId,
        nonzero: bool,
        mut yes: usize,
        mut no: usize,
        stop: Option<usize>,
        active: Option<LoopContext>,
    ) -> (Condition, usize, usize) {
        let mut condition = Condition::Value(value, nonzero);
        for _ in 0..self.budget.ast_depth / 8 {
            let mut folded = false;
            for (candidate, all, shared) in [(no, false, yes), (yes, true, no)] {
                if self.owned[candidate]
                    || Some(candidate) == stop
                    || self.control.predecessors[candidate].len() != 1
                    || active.is_some_and(|active| {
                        candidate == active.header || Some(candidate) == active.exit
                    })
                    || !self.expression_only(candidate)
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

    // Only consume blocks whose surviving definitions can be evaluated inside the branch expression.
    fn expression_only(&self, block: usize) -> bool {
        self.function.blocks[block]
            .events
            .iter()
            .enumerate()
            .all(|(position, event)| match event {
                Event::Action { .. } => !self.values.keep[block][position],
                Event::Define(id) => {
                    !self.values.needed[id.0]
                        || (!self.values.materialize[id.0]
                            && (!self.function.values[id.0].effects.ordered()
                                || self.values.uses[id.0] != 0))
                }
            })
    }
}
