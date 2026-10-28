use crate::ir::*;

fn slot(slot: Slot) -> usize {
    match slot {
        Slot::Acc => 0,
        Slot::Resume => 1,
        Slot::Register(reg) => reg as usize + 2,
    }
}

// Class markers cannot appear on an ordinary object; heap mutations do not invalidate this fact.
pub(super) fn ordinary(function: &Function<'_>) -> Vec<bool> {
    let width = function.code.vregs as usize + function.code.args as usize + 2;
    let mut predecessors = vec![Vec::new(); function.blocks.len()];
    let mut handlers = vec![false; function.blocks.len()];
    for (from, block) in function.blocks.iter().enumerate() {
        if !function.graph.reachable[from] {
            continue;
        }
        for target in block.end.successors() {
            predecessors[target].push(from);
        }
        for handler in &block.handlers {
            handlers[*handler] = true;
        }
    }
    let mut outgoing = vec![vec![true; width]; function.blocks.len()];
    let mut facts = vec![true; function.values.len()];
    loop {
        let mut changed = false;
        for (index, block) in function.blocks.iter().enumerate() {
            if !function.graph.reachable[index] {
                continue;
            }
            let mut state =
                vec![index != 0 && !handlers[index] && !predecessors[index].is_empty(); width];
            if index != 0 && !handlers[index] {
                for predecessor in &predecessors[index] {
                    for (current, incoming) in state.iter_mut().zip(&outgoing[*predecessor]) {
                        *current &= *incoming;
                    }
                }
            }
            for event in &block.events {
                match event {
                    Event::Define(id) => {
                        let fact = match function.values[id.0].op {
                            Op::Array(_) | Op::Object(_) | Op::Closure { .. } => true,
                            Op::Read(source) => state[slot(source)],
                            Op::CopyData(target, _) => facts[target.0],
                            _ => false,
                        };
                        if fact != facts[id.0] {
                            facts[id.0] = fact;
                            changed = true;
                        }
                    }
                    Event::Action {
                        action: Action::Write(target, value),
                        ..
                    } => state[slot(*target)] = facts[value.0],
                    _ => {}
                }
            }
            if state != outgoing[index] {
                outgoing[index] = state;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    facts
}
