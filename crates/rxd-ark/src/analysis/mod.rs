pub(crate) mod dominance;
mod flow;
mod receivers;
mod values;

use crate::{ir::Function, reader::resource};
use rxd_core::Result;
pub(crate) use values::Values;

pub(crate) struct Control {
    pub successors: Vec<Vec<usize>>,
    pub predecessors: Vec<Vec<usize>>,
    pub postdominators: dominance::Dominance,
    pub loops: Vec<Option<Loop>>,
}

#[derive(Clone)]
pub(crate) struct Loop {
    pub header: usize,
    pub members: Vec<bool>,
    pub exit: Option<usize>,
}

pub(crate) fn control(function: &Function<'_>) -> Result<Control> {
    let count = function.blocks.len();
    let successors: Vec<_> = function
        .blocks
        .iter()
        .map(|block| block.end.successors())
        .collect();
    let mut predecessors = vec![Vec::new(); count];
    for (from, targets) in successors.iter().enumerate() {
        for target in targets {
            if *target >= count {
                return Err(resource("Ark CFG edge exceeds graph"));
            }
            predecessors[*target].push(from);
        }
    }
    let dominators = dominance::compute(&successors, &predecessors, 0)?;
    let mut reverse = predecessors.clone();
    reverse.push(Vec::new());
    for (node, targets) in successors.iter().enumerate() {
        if targets.is_empty() {
            reverse[count].push(node);
        }
    }
    let mut reverse_pred = successors.clone();
    reverse_pred.push(Vec::new());
    for node in &reverse[count] {
        reverse_pred[*node].push(count);
    }
    let postdominators = dominance::compute(&reverse, &reverse_pred, count)?;
    let mut loops: Vec<Option<Loop>> = vec![None; count];
    for (latch, targets) in successors.iter().enumerate() {
        for header in targets {
            if !dominates(&dominators.idom, *header, latch) {
                continue;
            }
            let natural = loops[*header].get_or_insert_with(|| Loop {
                header: *header,
                members: vec![false; count],
                exit: None,
            });
            natural.members[*header] = true;
            let mut stack = vec![latch];
            while let Some(node) = stack.pop() {
                if natural.members[node] {
                    continue;
                }
                natural.members[node] = true;
                stack.extend(predecessors[node].iter().copied());
            }
        }
    }
    for natural in loops.iter_mut().flatten() {
        let mut exits = Vec::new();
        for (node, inside) in natural.members.iter().enumerate() {
            if *inside {
                exits.extend(
                    successors[node]
                        .iter()
                        .filter(|next| !natural.members[**next])
                        .copied(),
                );
            }
        }
        exits.sort_unstable();
        exits.dedup();
        if exits.len() == 1 {
            natural.exit = Some(exits[0]);
        }
    }
    Ok(Control {
        successors,
        predecessors,
        postdominators,
        loops,
    })
}

pub(crate) fn dominates(idom: &[Option<usize>], ancestor: usize, mut node: usize) -> bool {
    for _ in 0..idom.len() {
        if node == ancestor {
            return true;
        }
        let Some(parent) = idom.get(node).copied().flatten() else {
            return false;
        };
        if parent == node {
            return false;
        }
        node = parent;
    }
    false
}
