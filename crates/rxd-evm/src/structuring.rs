//! Bounded acyclic and simple natural-loop structuring over lowered EVM values and contextual edges.
//! No opcode semantics or source-level calls are inferred here. Postdominators
//! are a linear-size tree; work, plan growth and pending tasks are charged.
use std::collections::BTreeMap;

use rxd_core::{Error, Result};

use crate::{
    Limits,
    reconstruction::{ReconstructedGraph, ReconstructionCounts, StackEdge, Terminator, ValueId},
};

#[derive(Clone, Copy, Debug)]
pub enum EdgeLocation {
    Entry,
    Jump(usize),
    True(usize),
    False(usize),
}

impl EdgeLocation {
    pub fn edge(self, graph: &ReconstructedGraph) -> Option<&StackEdge> {
        match self {
            Self::Entry => Some(&graph.entry),
            Self::Jump(c) => match &graph.blocks[c].terminator {
                Terminator::Jump(edge) => Some(edge),
                _ => None,
            },
            Self::True(c) => match &graph.blocks[c].terminator {
                Terminator::Branch { when_true, .. } => when_true.as_ref(),
                _ => None,
            },
            Self::False(c) => match &graph.blocks[c].terminator {
                Terminator::Branch { when_false, .. } => when_false.as_ref(),
                _ => None,
            },
        }
    }

    pub fn source(self) -> Option<usize> {
        match self {
            Self::Entry => None,
            Self::Jump(c) | Self::True(c) | Self::False(c) => Some(c),
        }
    }
}

/// Linear rendering plan. A Block emits its ordered statements and only an
/// explicit terminal; Transfer performs a simultaneous assignment to the target
/// parameters. If/Else/EndIf retain the original nonzero/zero JUMPI polarity.
#[derive(Clone, Copy, Debug)]
pub enum StructuredStep {
    Block(usize),
    Transfer(EdgeLocation),
    If { context: usize, condition: ValueId },
    Else,
    EndIf,
    Stop,
    Loop,
    EndLoop,
    Break,
    Continue,
}

pub struct StructuringOutcome {
    pub steps: Option<Vec<StructuredStep>>,
    /// A complete contextual-goto graph remains available when not structured.
    pub reason: Option<&'static str>,
}

struct Budget<'a> {
    limits: Limits,
    counts: &'a mut ReconstructionCounts,
}

impl Budget<'_> {
    fn charge(&mut self, amount: usize) -> Result<()> {
        self.counts.structuring_work = self.counts.structuring_work.saturating_add(amount);
        self.counts.work = self.counts.work.saturating_add(amount);
        if self.counts.work > self.limits.reconstruction_work {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "EVM control-flow structuring work budget exceeded",
            ));
        }
        Ok(())
    }
}

fn successors(terminator: &Terminator) -> [Option<usize>; 2] {
    match terminator {
        Terminator::Jump(edge) => [Some(edge.target_context), None],
        Terminator::Branch {
            when_true,
            when_false,
            ..
        } => [
            when_true.as_ref().map(|e| e.target_context),
            when_false.as_ref().map(|e| e.target_context),
        ],
        _ => [None, None],
    }
}

fn common_ancestor(
    mut a: usize,
    mut b: usize,
    parent: &[usize],
    depth: &[usize],
    budget: &mut Budget<'_>,
) -> Result<usize> {
    while a != b {
        budget.charge(1)?;
        if depth[a] >= depth[b] {
            a = parent[a];
        } else {
            b = parent[b];
        }
    }
    Ok(a)
}

#[derive(Clone, Copy)]
enum Task {
    Walk {
        context: usize,
        stop: usize,
        owner: Option<usize>,
    },
    Edge {
        location: EdgeLocation,
        stop: usize,
        owner: Option<usize>,
    },
    Emit(StructuredStep),
}

#[derive(Default)]
struct LoopRegion {
    header: usize,
    exit: Option<usize>,
    terminal_exit: bool,
    backedges: usize,
}

fn unavailable(reason: &'static str) -> StructuringOutcome {
    StructuringOutcome {
        steps: None,
        reason: Some(reason),
    }
}

fn is_terminal(graph: &ReconstructedGraph, context: usize) -> bool {
    matches!(
        graph.blocks[context].terminator,
        Terminator::Stop | Terminator::Invalid | Terminator::Return { .. }
    )
}

/// Kahn order and immediate postdominators for a bounded DAG view. A missing
/// branch arm reaches virtual exit n. Storage is linear in selected contexts.
fn postdominators(
    next: &[[Option<usize>; 2]],
    branch: &[bool],
    active: &[bool],
    budget: &mut Budget<'_>,
) -> Result<Option<Vec<usize>>> {
    let n = next.len();
    budget.charge((n + 1).saturating_mul(4))?;
    let mut indegree = vec![0usize; n];
    for (c, edges) in next.iter().enumerate() {
        budget.charge(1)?;
        if active[c] {
            for t in edges.iter().flatten() {
                budget.charge(1)?;
                indegree[*t] += 1;
            }
        }
    }
    let mut order: Vec<_> = (0..n).filter(|&c| active[c] && indegree[c] == 0).collect();
    let mut cursor = 0;
    while cursor < order.len() {
        let c = order[cursor];
        cursor += 1;
        budget.charge(1)?;
        for t in next[c].iter().flatten() {
            budget.charge(1)?;
            indegree[*t] -= 1;
            if indegree[*t] == 0 {
                order.push(*t);
            }
        }
    }
    budget.charge(n)?;
    if order.len() != active.iter().filter(|&&v| v).count() {
        return Ok(None);
    }
    let mut parent = vec![n; n + 1];
    let mut depth = vec![0usize; n + 1];
    for &c in order.iter().rev() {
        budget.charge(1)?;
        let join = if branch[c] {
            common_ancestor(
                next[c][0].unwrap_or(n),
                next[c][1].unwrap_or(n),
                &parent,
                &depth,
                budget,
            )?
        } else {
            next[c][0].unwrap_or(n)
        };
        parent[c] = join;
        depth[c] = depth[join] + 1;
    }
    Ok(Some(parent))
}

fn intersect(
    mut a: usize,
    mut b: usize,
    parent: &[usize],
    rank: &[usize],
    budget: &mut Budget<'_>,
) -> Result<usize> {
    while a != b {
        budget.charge(1)?;
        if rank[a] > rank[b] {
            a = parent[a];
        } else {
            b = parent[b];
        }
    }
    Ok(a)
}

/// Structure only the already selected contextual graph. Natural loops require
/// one SCC entry, dominated header backedges, an acyclic iteration after cutting
/// those backedges, and one ordinary continuation. Distinct terminal exits stay
/// inside the loop. Nested/irreducible flow retains the entire contextual graph.
/// No recursive walk, dense dominance matrix, opcode semantics or text parsing.
pub fn structure_acyclic(
    graph: &ReconstructedGraph,
    limits: Limits,
    counts: &mut ReconstructionCounts,
) -> Result<StructuringOutcome> {
    let mut budget = Budget { limits, counts };
    let n = graph.blocks.len();
    budget.charge(n.saturating_add(1).saturating_mul(20))?;
    let entry = graph.entry.target_context;
    if entry >= n {
        return Ok(unavailable("invalid_entry_context"));
    }
    let mut heights = BTreeMap::new();
    let mut next = vec![[None; 2]; n];
    let mut reverse = vec![Vec::new(); n];
    if graph.entry.arguments.len() != graph.blocks[entry].parameters.len() {
        return Ok(unavailable("incompatible_edge_stack_height"));
    }
    for (c, block) in graph.blocks.iter().enumerate() {
        budget.charge(1)?;
        if block.context != c {
            return Ok(unavailable("invalid_context_order"));
        }
        if let Some(height) = heights.insert(block.block_index, block.parameters.len())
            && height != block.parameters.len()
        {
            return Ok(unavailable("incompatible_join_stack_heights"));
        }
        for location in [
            EdgeLocation::Jump(c),
            EdgeLocation::True(c),
            EdgeLocation::False(c),
        ] {
            if let Some(edge) = location.edge(graph) {
                budget.charge(1)?;
                if edge.target_context >= n {
                    return Ok(unavailable("invalid_context_edge"));
                }
                if edge.arguments.len() != graph.blocks[edge.target_context].parameters.len() {
                    return Ok(unavailable("incompatible_edge_stack_height"));
                }
                reverse[edge.target_context].push(c);
            }
        }
        next[c] = successors(&block.terminator);
    }
    // Iterative DFS finishing order, reused for SCCs and reverse-postorder idom.
    let mut seen = vec![false; n];
    let mut finish = Vec::new();
    let mut pending = vec![(entry, false)];
    budget.charge(1)?;
    while let Some((c, complete)) = pending.pop() {
        budget.charge(1)?;
        if complete {
            finish.push(c);
            continue;
        }
        if seen[c] {
            continue;
        }
        seen[c] = true;
        budget.charge(3)?;
        pending.push((c, true));
        for t in next[c].iter().rev().flatten() {
            pending.push((*t, false));
        }
    }
    if finish.len() != n {
        return Ok(unavailable("unreachable_context"));
    }
    let mut component = vec![n; n];
    let mut sizes = Vec::new();
    for &root in finish.iter().rev() {
        budget.charge(1)?;
        if component[root] != n {
            continue;
        }
        let id = sizes.len();
        budget.charge(2)?;
        sizes.push(0usize);
        let mut stack = vec![root];
        component[root] = id;
        while let Some(c) = stack.pop() {
            budget.charge(1)?;
            sizes[id] += 1;
            for &p in &reverse[c] {
                budget.charge(1)?;
                if component[p] == n {
                    budget.charge(1)?;
                    component[p] = id;
                    stack.push(p);
                }
            }
        }
    }
    let mut cyclic = vec![false; sizes.len()];
    for c in 0..n {
        budget.charge(1)?;
        cyclic[component[c]] |= sizes[component[c]] > 1 || next[c].contains(&Some(c));
    }
    let mut regions: Vec<Option<LoopRegion>> = (0..sizes.len()).map(|_| None).collect();
    for c in 0..n {
        budget.charge(1)?;
        let id = component[c];
        if !cyclic[id] {
            continue;
        }
        budget.charge(reverse[c].len())?;
        let incoming = c == entry || reverse[c].iter().any(|&p| component[p] != id);
        if incoming {
            if regions[id].is_some() {
                return Ok(unavailable("multiple_entry_loop"));
            }
            regions[id] = Some(LoopRegion {
                header: c,
                ..LoopRegion::default()
            });
        }
    }
    // Cooper idom over contextual nodes, linear storage and charged fixed points.
    let mut rank = vec![0usize; n];
    for (i, &c) in finish.iter().rev().enumerate() {
        budget.charge(1)?;
        rank[c] = i;
    }
    let mut idom = vec![n; n];
    idom[entry] = entry;
    let mut changed = cyclic.iter().any(|&v| v);
    while changed {
        changed = false;
        for &c in finish.iter().rev().skip(1) {
            budget.charge(1)?;
            let mut parent = None;
            for &p in &reverse[c] {
                budget.charge(1)?;
                if idom[p] != n {
                    parent = Some(match parent {
                        None => p,
                        Some(a) => intersect(a, p, &idom, &rank, &mut budget)?,
                    });
                }
            }
            if let Some(p) = parent
                && idom[c] != p
            {
                idom[c] = p;
                changed = true;
            }
        }
    }
    // Header outward edge is the normal pre-test exit (including a return block).
    // Otherwise choose a nonterminal continuation, or one terminal latch exit.
    for region in regions.iter_mut().flatten() {
        budget.charge(1)?;
        region.exit = next[region.header]
            .iter()
            .flatten()
            .copied()
            .find(|&t| component[t] != component[region.header]);
    }
    for c in 0..n {
        let id = component[c];
        if let Some(region) = regions[id].as_mut() {
            for &t in next[c].iter().flatten() {
                budget.charge(1)?;
                if component[t] != id && !is_terminal(graph, t) {
                    if let Some(exit) = region.exit
                        && exit != t
                    {
                        return Ok(unavailable("unsupported_loop_exits"));
                    }
                    region.exit = Some(t);
                }
            }
        }
    }
    for c in 0..n {
        let id = component[c];
        if let Some(region) = regions[id].as_mut() {
            for &t in next[c].iter().flatten() {
                budget.charge(1)?;
                if t == region.header {
                    let mut ancestor = c;
                    while ancestor != region.header && ancestor != entry {
                        budget.charge(1)?;
                        ancestor = idom[ancestor];
                    }
                    if ancestor != region.header {
                        return Ok(unavailable("undominated_loop_backedge"));
                    }
                    region.backedges += 1;
                } else if component[t] != id {
                    if region.exit.is_none() {
                        region.exit = Some(t);
                    }
                    if region.exit != Some(t) {
                        if !is_terminal(graph, t) {
                            return Ok(unavailable("unsupported_loop_exits"));
                        }
                        region.terminal_exit = true;
                    }
                }
            }
        }
    }
    let mut inner = vec![[None; 2]; n];
    let mut outer = next.clone();
    let mut inner_active = vec![false; n];
    let mut outer_active = vec![true; n];
    let mut branch: Vec<_> = graph
        .blocks
        .iter()
        .map(|b| matches!(b.terminator, Terminator::Branch { .. }))
        .collect();
    for c in 0..n {
        budget.charge(1)?;
        if let Some(region) = &regions[component[c]] {
            inner_active[c] = true;
            outer_active[c] = c == region.header;
            for (slot, &t) in next[c].iter().enumerate() {
                if let Some(t) = t
                    && component[t] == component[c]
                    && t != region.header
                {
                    inner[c][slot] = Some(t);
                }
            }
            outer[c] = [region.exit, None];
        }
    }
    let Some(inner_parent) = postdominators(&inner, &branch, &inner_active, &mut budget)? else {
        return Ok(unavailable("unsupported_nested_loop"));
    };
    for region in regions.iter().flatten() {
        branch[region.header] = region.terminal_exit;
    }
    let Some(outer_parent) = postdominators(&outer, &branch, &outer_active, &mut budget)? else {
        return Ok(unavailable("cyclic_loop_quotient"));
    };
    seen.fill(false);
    let mut steps = Vec::new();
    let mut tasks = vec![Task::Walk {
        context: entry,
        stop: n,
        owner: None,
    }];
    budget.charge(2)?;
    steps.push(StructuredStep::Transfer(EdgeLocation::Entry));
    budget.counts.structured_edge_count += 1;
    while let Some(task) = tasks.pop() {
        budget.charge(1)?;
        match task {
            Task::Emit(step) => {
                budget.charge(1)?;
                if matches!(step, StructuredStep::Transfer(_)) {
                    budget.counts.structured_edge_count += 1;
                }
                steps.push(step);
            }
            Task::Edge {
                location,
                stop,
                owner,
            } => {
                let target = location.edge(graph).expect("validated edge").target_context;
                budget.charge(2)?;
                let following = if let Some(id) = owner {
                    let region = regions[id].as_ref().expect("loop owner");
                    if target == region.header {
                        Task::Emit(StructuredStep::Continue)
                    } else if Some(target) == region.exit {
                        Task::Emit(StructuredStep::Break)
                    } else {
                        Task::Walk {
                            context: target,
                            stop,
                            owner,
                        }
                    }
                } else {
                    Task::Walk {
                        context: target,
                        stop,
                        owner,
                    }
                };
                tasks.push(following);
                tasks.push(Task::Emit(StructuredStep::Transfer(location)));
            }
            Task::Walk {
                context: c,
                stop,
                owner,
            } => {
                if c == stop || c == n {
                    continue;
                }
                if owner.is_none()
                    && let Some(region) = &regions[component[c]]
                {
                    budget.charge(4)?;
                    tasks.push(Task::Walk {
                        context: region.exit.unwrap_or(n),
                        stop,
                        owner: None,
                    });
                    tasks.push(Task::Emit(StructuredStep::EndLoop));
                    tasks.push(Task::Walk {
                        context: c,
                        stop: n,
                        owner: Some(component[c]),
                    });
                    tasks.push(Task::Emit(StructuredStep::Loop));
                    continue;
                }
                budget.charge(1)?;
                steps.push(StructuredStep::Block(c));
                if !seen[c] {
                    seen[c] = true;
                    budget.counts.structured_context_count += 1;
                }
                match &graph.blocks[c].terminator {
                    Terminator::Jump(_) => {
                        budget.charge(1)?;
                        tasks.push(Task::Edge {
                            location: EdgeLocation::Jump(c),
                            stop,
                            owner,
                        });
                    }
                    Terminator::Branch {
                        condition,
                        when_true,
                        when_false,
                    } => {
                        let join = if owner.is_some() {
                            inner_parent[c]
                        } else {
                            outer_parent[c]
                        };
                        budget.charge(8)?;
                        steps.push(StructuredStep::If {
                            context: c,
                            condition: *condition,
                        });
                        tasks.push(Task::Walk {
                            context: join,
                            stop,
                            owner,
                        });
                        tasks.push(Task::Emit(StructuredStep::EndIf));
                        for (edge, location) in [
                            (when_false, EdgeLocation::False(c)),
                            (when_true, EdgeLocation::True(c)),
                        ] {
                            if edge.is_some() {
                                tasks.push(Task::Edge {
                                    location,
                                    stop: join,
                                    owner,
                                });
                            } else {
                                tasks.push(Task::Emit(StructuredStep::Stop));
                            }
                            if matches!(location, EdgeLocation::False(_)) {
                                tasks.push(Task::Emit(StructuredStep::Else));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if seen.iter().any(|&v| !v) {
        return Ok(unavailable("unreachable_structured_context"));
    }
    budget.counts.structured_loop_count = regions.iter().flatten().count();
    budget.counts.structured_backedge_count = regions.iter().flatten().map(|r| r.backedges).sum();
    Ok(StructuringOutcome {
        steps: Some(steps),
        reason: None,
    })
}
