use crate::reader::resource;
use rxd_core::Result;

/// Immediate dominators from the Cooper-Harvey-Kennedy algorithm.
pub struct Dominance {
    pub idom: Vec<Option<usize>>,
}

fn predecessors(succ: &[Vec<usize>], abc: &crate::Abc<'_>) -> Result<Vec<Vec<usize>>> {
    let mut pred = Vec::new();
    pred.try_reserve_exact(succ.len())
        .map_err(|_| resource("ABC predecessor allocation failed"))?;
    pred.resize_with(succ.len(), Vec::new);
    for (node, targets) in succ.iter().enumerate() {
        for target in targets {
            abc.check_storage::<usize>(pred[*target].len() + 1)?;
            pred[*target]
                .try_reserve(1)
                .map_err(|_| resource("ABC predecessor allocation failed"))?;
            pred[*target].push(node);
        }
    }
    Ok(pred)
}

fn reverse_postorder(succ: &[Vec<usize>], root: usize) -> Vec<usize> {
    let mut post = Vec::with_capacity(succ.len());
    let mut seen = vec![false; succ.len()];
    if root >= succ.len() {
        return post;
    }
    let mut stack = vec![(root, 0usize)];
    seen[root] = true;
    while let Some((node, next)) = stack.pop() {
        if next < succ[node].len() {
            stack.push((node, next + 1));
            let target = succ[node][next];
            if !seen[target] {
                seen[target] = true;
                stack.push((target, 0));
            }
        } else {
            post.push(node);
        }
    }
    post
}

fn intersect(mut a: usize, mut b: usize, idom: &[Option<usize>], order: &[usize]) -> usize {
    while a != b {
        while order[a] > order[b] {
            a = idom[a].expect("processed dominator");
        }
        while order[b] > order[a] {
            b = idom[b].expect("processed dominator");
        }
    }
    a
}

pub fn compute(succ: &[Vec<usize>], pred: &[Vec<usize>], root: usize) -> Result<Dominance> {
    let post = reverse_postorder(succ, root);
    let mut order = vec![usize::MAX; succ.len()];
    for (index, node) in post.iter().rev().enumerate() {
        order[*node] = index;
    }
    let mut idom = vec![None; succ.len()];
    idom[root] = Some(root);
    let mut changed = true;
    while changed {
        changed = false;
        for node in post.iter().rev().copied() {
            if node == root {
                continue;
            }
            let mut candidate = None;
            for predecessor in &pred[node] {
                if order[*predecessor] == usize::MAX || idom[*predecessor].is_none() {
                    continue;
                }
                candidate = Some(match candidate {
                    None => *predecessor,
                    Some(current) => intersect(*predecessor, current, &idom, &order),
                });
            }
            if candidate != idom[node] {
                idom[node] = candidate;
                changed = true;
            }
        }
    }
    Ok(Dominance { idom })
}

/// Immediate dominators for a CFG whose root is `root`.
pub fn compute_for(abc: &crate::Abc<'_>, succ: &[Vec<usize>], root: usize) -> Result<Dominance> {
    let pred = predecessors(succ, abc)?;
    compute(succ, &pred, root)
}
