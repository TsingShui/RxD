use crate::{ir::*, reader::resource};
use oxc_syntax::operator::{BinaryOperator as B, UnaryOperator as U};
use rxd_core::Result;

pub(crate) struct Values {
    pub needed: Vec<bool>,
    pub keep: Vec<Vec<bool>>,
    pub materialize: Vec<bool>,
    pub uses: Vec<usize>,
    pub ordinary: Vec<bool>,
}

fn slot_index(slot: Slot) -> usize {
    match slot {
        Slot::Acc => 0,
        Slot::Resume => 1,
        Slot::Register(reg) => reg as usize + 2,
    }
}
fn merge(into: &mut [bool], from: &[bool], exceptional: bool) {
    for (index, (into, from)) in into.iter_mut().zip(from).enumerate() {
        if index != 0 || !exceptional {
            *into |= *from;
        }
    }
}

impl Values {
    pub fn may_throw(&self, function: &Function<'_>, block: usize) -> bool {
        let block_ir = &function.blocks[block];
        matches!(block_ir.end, Terminator::Throw(_))
            || block_ir
                .events
                .iter()
                .enumerate()
                .any(|(position, event)| match event {
                    Event::Define(id) => {
                        self.needed[id.0] && function.values[id.0].effects.may_throw
                    }
                    Event::Action { action, .. } => {
                        self.keep[block][position] && action.effects().may_throw
                    }
                })
    }

    pub fn analyze(function: &mut Function<'_>, budget: rxd_core::Budget) -> Result<Self> {
        fold(function);
        function.graph.reachable.fill(false);
        let mut pending = vec![0];
        while let Some(block) = pending.pop() {
            if function.graph.reachable[block] {
                continue;
            }
            function.graph.reachable[block] = true;
            pending.extend(function.blocks[block].end.successors());
            pending.extend(function.blocks[block].handlers.iter().copied());
        }
        let width = function.code.vregs as usize + function.code.args as usize + 2;
        let storage = function
            .blocks
            .len()
            .checked_mul(width)
            .and_then(|n| n.checked_mul(3))
            .ok_or_else(|| resource("Ark liveness allocation overflow"))?;
        if storage > budget.metadata_bytes {
            return Err(resource("Ark liveness exceeds metadata admission"));
        }
        let mut live_in = vec![vec![false; width]; function.blocks.len()];
        let mut keep: Vec<_> = function
            .blocks
            .iter()
            .map(|block| vec![true; block.events.len()])
            .collect();
        loop {
            let mut changed = false;
            for index in (0..function.blocks.len()).rev() {
                let block = &function.blocks[index];
                let mut live = vec![false; width];
                for next in block.end.successors() {
                    merge(&mut live, &live_in[next], false);
                }
                for (event_index, event) in block.events.iter().enumerate().rev() {
                    match event {
                        Event::Define(value) => {
                            if function.values[value.0].effects.may_throw {
                                for handler in &block.handlers {
                                    merge(&mut live, &live_in[*handler], true);
                                }
                            }
                            if let Op::Read(slot) = function.values[value.0].op {
                                live[slot_index(slot)] = true;
                            }
                        }
                        Event::Action {
                            action: Action::Write(slot, _),
                            ..
                        } => {
                            keep[index][event_index] = live[slot_index(*slot)];
                            live[slot_index(*slot)] = false;
                        }
                        Event::Action { action, .. } if action.effects().may_throw => {
                            for handler in &block.handlers {
                                merge(&mut live, &live_in[*handler], true);
                            }
                        }
                        _ => {}
                    }
                }
                if live != live_in[index] {
                    live_in[index] = live;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let mut needed = vec![false; function.values.len()];
        let mut stack = Vec::new();
        for (index, block) in function.blocks.iter().enumerate() {
            if !function.graph.reachable[index] {
                continue;
            }
            stack.extend(block.end.inputs());
            for (event_index, event) in block.events.iter().enumerate() {
                match event {
                    Event::Define(value) if function.values[value.0].effects.ordered() => {
                        stack.push(*value)
                    }
                    Event::Action { action, .. } if keep[index][event_index] => {
                        stack.extend(action.inputs())
                    }
                    _ => {}
                }
            }
        }
        while let Some(id) = stack.pop() {
            if needed[id.0] {
                continue;
            }
            needed[id.0] = true;
            stack.extend(function.values[id.0].op.inputs());
        }
        let mut uses = vec![0usize; function.values.len()];
        let mut def_positions = vec![0; function.values.len()];
        let mut last_use = vec![0; function.values.len()];
        for (index, block) in function.blocks.iter().enumerate() {
            for (position, event) in block.events.iter().enumerate() {
                let inputs = match event {
                    Event::Define(id) => {
                        def_positions[id.0] = position;
                        if needed[id.0] {
                            function.values[id.0].op.inputs()
                        } else {
                            Vec::new()
                        }
                    }
                    Event::Action { action, .. } if keep[index][position] => action.inputs(),
                    _ => Vec::new(),
                };
                for input in inputs {
                    uses[input.0] += 1;
                    last_use[input.0] = position;
                }
            }
            for input in block.end.inputs() {
                uses[input.0] +=
                    if matches!(block.end, Terminator::Branch { .. }) && !function.boolean(input) {
                        3
                    } else {
                        1
                    };
                last_use[input.0] = block.events.len();
            }
        }
        let mut materialize: Vec<_> = function
            .values
            .iter()
            .enumerate()
            .map(|(id, value)| {
                uses[id] > 1
                    && !matches!(
                        value.op,
                        Op::Constant(_) | Op::Read(_) | Op::This | Op::NewTarget | Op::Function
                    )
            })
            .collect();
        for index in (0..function.values.len()).rev() {
            if !materialize[index] {
                for input in function.values[index].op.inputs() {
                    last_use[input.0] = last_use[input.0].max(last_use[index]);
                }
            }
        }
        for (index, value) in function.values.iter().enumerate() {
            if !needed[index] || uses[index] == 0 {
                continue;
            }
            let block = &function.blocks[value.block];
            let first = def_positions[index];
            let last = last_use[index];
            materialize[index] = match &value.op {
                Op::Constant(_) | Op::This | Op::NewTarget | Op::Function => false,
                Op::Read(slot) => block.events[first + 1..last.min(block.events.len())].iter().any(|event| {
                    matches!(event, Event::Action { action: Action::Write(target, _), .. } if target == slot)
                }),
                _ if uses[index] > 1 => true,
                _ => value.effects.ordered() && block.events[first + 1..last.min(block.events.len())].iter().enumerate().any(|(offset, event)| {
                    match event {
                        Event::Define(id) => needed[id.0] && function.values[id.0].effects.ordered(),
                        Event::Action { .. } => keep[value.block][first + 1 + offset],
                    }
                }),
            };
        }
        let ordinary = super::receivers::ordinary(function);
        Ok(Self {
            needed,
            keep,
            materialize,
            uses,
            ordinary,
        })
    }
}

fn fold(function: &mut Function<'_>) {
    for index in 0..function.values.len() {
        let constant = match &function.values[index].op {
            Op::Unary(U::LogicalNot, value) => {
                truth(&function.values[value.0].op).map(|value| Constant::Bool(!value))
            }
            Op::Binary(operator, left, right) => {
                match (&function.values[left.0].op, &function.values[right.0].op) {
                    (
                        Op::Constant(Constant::Number(left)),
                        Op::Constant(Constant::Number(right)),
                    ) => match operator {
                        B::Addition => Some(Constant::Number(left + right)),
                        B::Subtraction => Some(Constant::Number(left - right)),
                        B::Multiplication => Some(Constant::Number(left * right)),
                        B::Division => Some(Constant::Number(left / right)),
                        B::StrictEquality | B::Equality => Some(Constant::Bool(left == right)),
                        B::StrictInequality | B::Inequality => Some(Constant::Bool(left != right)),
                        B::LessThan => Some(Constant::Bool(left < right)),
                        B::GreaterThan => Some(Constant::Bool(left > right)),
                        B::LessEqualThan => Some(Constant::Bool(left <= right)),
                        B::GreaterEqualThan => Some(Constant::Bool(left >= right)),
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(value) = constant {
            function.values[index].op = Op::Constant(value);
            function.values[index].effects = Effects::PURE;
        }
    }
    for block in &mut function.blocks {
        if let Terminator::Branch {
            value,
            nonzero,
            yes,
            no,
        } = block.end
            && let Op::Constant(constant) = &function.values[value.0].op
        {
            let taken = match constant {
                Constant::Bool(value) => *value == nonzero,
                Constant::Number(value) => (*value != 0.0) == nonzero,
                _ => false,
            };
            block.end = Terminator::Jump(if taken { yes } else { no });
        }
    }
}

fn truth(op: &Op) -> Option<bool> {
    match op {
        Op::Constant(Constant::Bool(value)) => Some(*value),
        Op::Constant(Constant::Number(value)) => Some(*value != 0.0 && !value.is_nan()),
        Op::Constant(Constant::Undefined | Constant::Null) => Some(false),
        _ => None,
    }
}
