use super::{Builder, LoopContext, Node};
use crate::{
    analysis::Control,
    ir::{Function, Terminator},
    reader::{invalid, resource},
};
use rxd_core::{Budget, Result};
use std::collections::HashSet;

#[derive(Clone, Copy)]
struct Region {
    first: usize,
    end: usize,
    handler: usize,
    // Handler size is a continuation hint, not an assumed catch-body boundary.
    handler_end: usize,
}

pub(super) struct Exceptions {
    regions: Vec<Region>,
    entries: Vec<Vec<usize>>,
}

pub(super) struct Protected {
    first: usize,
    end: usize,
    pub handler: usize,
    bridges: HashSet<usize>,
}

impl Protected {
    pub fn contains(&self, block: usize) -> bool {
        (self.first <= block && block < self.end) || self.bridges.contains(&block)
    }
    pub fn bridge(&self, block: usize) -> bool {
        self.bridges.contains(&block)
    }
}

impl Exceptions {
    pub fn build(function: &Function<'_>, control: &Control, budget: Budget) -> Result<Self> {
        if function.code.tries.is_empty() {
            return Ok(Self {
                regions: Vec::new(),
                entries: Vec::new(),
            });
        }
        let count = function.blocks.len();
        let storage = count
            .checked_mul(std::mem::size_of::<Vec<usize>>())
            .and_then(|bytes| {
                function
                    .code
                    .tries
                    .len()
                    .checked_mul(std::mem::size_of::<Region>() + std::mem::size_of::<usize>())
                    .and_then(|extra| bytes.checked_add(extra))
            })
            .ok_or_else(|| resource("Ark exception plan size overflow"))?;
        if storage > budget.metadata_bytes {
            return Err(resource("Ark exception plan exceeds metadata admission"));
        }
        let mut regions = Vec::new();
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|_| resource("Ark exception entry allocation failed"))?;
        entries.resize_with(count, Vec::new);
        for protected in &function.code.tries {
            if protected.length == 0 || protected.catches.len() != 1 {
                continue;
            }
            let catch = &protected.catches[0];
            let first = function
                .blocks
                .binary_search_by_key(&protected.pc, |block| block.pc)
                .map_err(|_| invalid("Ark try entry is not a block boundary"))?;
            let end = function
                .blocks
                .partition_point(|block| block.pc < protected.pc + protected.length);
            let handler = function
                .blocks
                .binary_search_by_key(&catch.pc, |block| block.pc)
                .map_err(|_| invalid("Ark handler is not a block boundary"))?;
            if first <= handler && handler < end {
                continue;
            }
            if (first + 1..end).any(|block| {
                control.predecessors[block]
                    .iter()
                    .any(|from| function.graph.reachable[*from] && (*from < first || *from >= end))
            }) {
                continue;
            }
            if function.blocks.iter().enumerate().any(|(from, block)| {
                function.graph.reachable[from]
                    && (from < first || from >= end)
                    && block
                        .handlers
                        .iter()
                        .any(|target| first < *target && *target < end)
            }) {
                continue;
            }
            let handler_end = function
                .blocks
                .partition_point(|block| block.pc < catch.pc + catch.length);
            regions
                .try_reserve(1)
                .map_err(|_| resource("Ark exception region allocation failed"))?;
            entries[first]
                .try_reserve(1)
                .map_err(|_| resource("Ark exception entry allocation failed"))?;
            entries[first].push(regions.len());
            regions.push(Region {
                first,
                end,
                handler,
                handler_end,
            });
        }
        for entry in &mut entries {
            entry.sort_by_key(|id| std::cmp::Reverse(regions[*id].end));
        }
        Ok(Self { regions, entries })
    }

    fn candidate(
        &self,
        function: &Function<'_>,
        block: usize,
        scope: Option<&Protected>,
    ) -> Option<usize> {
        let entries = self.entries.get(block)?;
        let handlers = &function.blocks[block].handlers;
        entries.iter().copied().find(|id| {
            let region = self.regions[*id];
            let Some(position) = handlers
                .iter()
                .position(|handler| *handler == region.handler)
            else {
                return false;
            };
            // Earlier handlers must be nested inside this try, not shadowed enclosing tries.
            let nested_priority = handlers[..position].iter().all(|handler| {
                entries.iter().any(|other| {
                    let earlier = self.regions[*other];
                    earlier.handler == *handler && earlier.end < region.end
                })
            });
            nested_priority
                && scope.is_none_or(|scope| {
                    region.handler != scope.handler
                        && scope.first <= region.first
                        && region.end <= scope.end
                        && (scope.first != region.first || scope.end != region.end)
                })
        })
    }
}

impl Builder<'_, '_> {
    pub(super) fn in_scope(&self, block: usize, scope: Option<&Protected>) -> bool {
        scope.is_none_or(|scope| scope.contains(block))
    }

    pub(super) fn exception_candidate(
        &self,
        block: usize,
        scope: Option<&Protected>,
    ) -> Option<usize> {
        self.exceptions.candidate(self.function, block, scope)
    }

    pub(super) fn try_contains_loop(&self, id: usize, members: &[bool]) -> bool {
        let region = self.exceptions.regions[id];
        members
            .iter()
            .enumerate()
            .all(|(block, member)| !member || (region.first <= block && block < region.end))
    }

    // A bridge may retain slot writes, but extending try coverage must not add a throw site.
    fn nonthrowing_bridge(&self, block: usize) -> bool {
        matches!(self.function.blocks[block].end, Terminator::Jump(_))
            && !self.values.may_throw(self.function, block)
    }

    fn canonical_exit(
        &self,
        mut block: usize,
        handler: usize,
        active: Option<LoopContext>,
        bridges: &mut HashSet<usize>,
    ) -> Result<usize> {
        let mut seen = HashSet::new();
        for _ in 0..self.function.blocks.len() {
            seen.try_reserve(1)
                .map_err(|_| resource("Ark exception bridge allocation failed"))?;
            if !seen.insert(block) {
                break;
            }
            if block == handler
                || self
                    .exceptions
                    .entries
                    .get(block)
                    .is_some_and(|entries| !entries.is_empty())
                || active.is_some_and(|active| block == active.header || Some(block) == active.exit)
                || !self.nonthrowing_bridge(block)
            {
                break;
            }
            let Terminator::Jump(next) = self.function.blocks[block].end else {
                break;
            };
            if next == block {
                break;
            }
            bridges
                .try_reserve(1)
                .map_err(|_| resource("Ark exception bridge allocation failed"))?;
            bridges.insert(block);
            block = next;
        }
        Ok(block)
    }

    fn layout(
        &self,
        id: usize,
        stop: Option<usize>,
        active: Option<LoopContext>,
    ) -> Result<Option<(Protected, Option<usize>)>> {
        let region = self.exceptions.regions[id];
        let mut bridges = HashSet::new();
        let mut exit = None;
        for block in region.first..region.end {
            if !self.function.graph.reachable[block] {
                continue;
            }
            for next in &self.control.successors[block] {
                if region.first <= *next && *next < region.end {
                    continue;
                }
                let next = self.canonical_exit(*next, region.handler, active, &mut bridges)?;
                if active.is_some_and(|active| next == active.header || Some(next) == active.exit) {
                    continue;
                }
                if exit.is_some_and(|exit| exit != next) {
                    return Ok(None);
                }
                exit = Some(next);
            }
        }
        let continuation = match exit {
            Some(next) => Some(next),
            None => {
                let end = region.handler_end;
                if end > region.handler
                    && end < self.function.blocks.len()
                    && self.control.predecessors[end].iter().any(|from| {
                        region.handler <= *from
                            && *from < end
                            && self.function.graph.reachable[*from]
                    })
                {
                    Some(end)
                } else {
                    stop
                }
            }
        };
        if continuation.is_some_and(|next| region.first <= next && next < region.end) {
            return Ok(None);
        }
        Ok(Some((
            Protected {
                first: region.first,
                end: region.end,
                handler: region.handler,
                bridges,
            },
            continuation,
        )))
    }

    pub(super) fn exception(
        &mut self,
        id: usize,
        stop: Option<usize>,
        active: Option<LoopContext>,
        parent: Option<&Protected>,
    ) -> Result<(Node, Option<usize>)> {
        let region = self.exceptions.regions[id];
        let Some((scope, continuation)) = self.layout(id, stop, active)? else {
            return Ok((self.residual(region.first, stop, active, parent)?, stop));
        };
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(self.owned.len())
            .map_err(|_| resource("Ark exception ownership allocation failed"))?;
        owned.extend_from_slice(&self.owned);
        let residual = self.residual_blocks;
        let tries = self.structured_tries;
        let attempt: Result<(Vec<Node>, Vec<Node>)> = (|| {
            let body = self.region(region.first, continuation, active, true, Some(&scope))?;
            let handler = self.region(region.handler, continuation, active, false, parent)?;
            Ok((body, handler))
        })();
        match attempt {
            Ok((body, handler)) => {
                self.structured_tries += 1;
                Ok((Node::Try { id, body, handler }, continuation))
            }
            Err(_) => {
                // Roll back both arms before fallback; every block has one emission owner.
                self.owned = owned;
                self.residual_blocks = residual;
                self.structured_tries = tries;
                Ok((
                    self.residual(region.first, continuation, active, parent)?,
                    continuation,
                ))
            }
        }
    }
}
