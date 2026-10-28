mod conditions;
mod exceptions;

use exceptions::{Exceptions, Protected};

use crate::{
    analysis::{Control, Values},
    ir::{Event, Function, Slot, Terminator, ValueId},
    reader::resource,
};
use rxd_core::{Budget, Result};

pub(crate) enum Condition {
    Value(ValueId, bool),
    Not(Box<Condition>),
    ShortCircuit {
        all: bool,
        left: Box<Condition>,
        right: Box<Condition>,
    },
}

pub(crate) enum Choice {
    Value(ValueId),
    Existing,
}

pub(crate) enum Node {
    Block(usize),
    If {
        condition: Condition,
        yes: Vec<Node>,
        no: Vec<Node>,
    },
    Select {
        condition: Condition,
        slot: Slot,
        yes: Choice,
        no: Choice,
    },
    Loop {
        header: usize,
        body: Vec<Node>,
    },
    Try {
        id: usize,
        body: Vec<Node>,
        handler: Vec<Node>,
    },
    Break(usize),
    Continue(usize),
    Return(Option<ValueId>),
    Throw(ValueId),
    Residual {
        entry: usize,
        blocks: Vec<usize>,
        stop: Option<usize>,
        active: Option<LoopContext>,
        caught_by: Option<usize>,
    },
}

#[derive(Clone, Copy)]
pub(crate) struct LoopContext {
    pub header: usize,
    pub exit: Option<usize>,
}

pub(crate) struct Body {
    pub nodes: Vec<Node>,
    pub owned: Vec<bool>,
    pub residual_blocks: usize,
    pub structured_tries: usize,
}

struct Builder<'f, 'a> {
    function: &'f Function<'a>,
    control: &'f Control,
    values: &'f Values,
    exceptions: Exceptions,
    owned: Vec<bool>,
    depth: usize,
    budget: Budget,
    residual_blocks: usize,
    structured_tries: usize,
}

pub(crate) fn structure(
    function: &Function<'_>,
    control: &Control,
    values: &Values,
    budget: Budget,
) -> Result<Body> {
    let mut builder = Builder {
        function,
        control,
        values,
        exceptions: Exceptions::build(function, control, budget)?,
        owned: vec![false; function.blocks.len()],
        depth: 0,
        budget,
        residual_blocks: 0,
        structured_tries: 0,
    };
    let mut nodes = builder.region(0, None, None, false, None)?;
    if function
        .graph
        .reachable
        .iter()
        .zip(&builder.owned)
        .any(|(reachable, owned)| *reachable && !owned)
    {
        builder.owned.fill(false);
        builder.residual_blocks = 0;
        builder.structured_tries = 0;
        nodes = vec![builder.residual(0, None, None, None)?];
    }
    Ok(Body {
        nodes,
        owned: builder.owned,
        residual_blocks: builder.residual_blocks,
        structured_tries: builder.structured_tries,
    })
}

impl Builder<'_, '_> {
    fn statement_free(&self, block: usize) -> bool {
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

    fn region(
        &mut self,
        start: usize,
        stop: Option<usize>,
        active: Option<LoopContext>,
        loop_entry: bool,
        scope: Option<&Protected>,
    ) -> Result<Vec<Node>> {
        if !loop_entry && let Some(active) = active {
            if start == active.header {
                return Ok(vec![Node::Continue(active.header)]);
            }
            if Some(start) == active.exit {
                return Ok(vec![Node::Break(active.header)]);
            }
        }
        if Some(start) == stop {
            return Ok(Vec::new());
        }
        let before = self.owned.clone();
        let residual_before = self.residual_blocks;
        let tries_before = self.structured_tries;
        self.depth += 1;
        let structured = if self.depth < self.budget.ast_depth / 4 {
            self.walk(start, stop, active, scope)
        } else {
            Err(resource("Ark structure nesting requires residual plan"))
        };
        self.depth -= 1;
        match structured {
            Ok(nodes) => self.merge_boolean_joins(nodes),
            Err(_) => {
                self.owned = before;
                self.residual_blocks = residual_before;
                self.structured_tries = tries_before;
                Ok(vec![self.residual(start, stop, active, scope)?])
            }
        }
    }

    fn walk(
        &mut self,
        mut current: usize,
        stop: Option<usize>,
        active: Option<LoopContext>,
        scope: Option<&Protected>,
    ) -> Result<Vec<Node>> {
        let mut nodes = Vec::new();
        loop {
            if Some(current) == stop {
                break;
            }
            if current >= self.owned.len() || self.owned[current] {
                return Err(resource("Ark region has a shared entry"));
            }
            if !self.in_scope(current, scope) {
                return Err(resource("Ark region crosses its exception scope"));
            }
            let exception = self.exception_candidate(current, scope);
            let loop_first = self.control.loops[current].as_ref().is_some_and(|natural| {
                active.is_none_or(|active| active.header != current)
                    && exception.is_some_and(|id| !self.try_contains_loop(id, &natural.members))
            });
            if !loop_first && let Some(id) = exception {
                let (node, continuation) = self.exception(id, stop, active, scope)?;
                nodes.push(node);
                if let Some(next) = continuation {
                    if let Some(active) = active {
                        if next == active.header {
                            nodes.push(Node::Continue(active.header));
                            break;
                        }
                        if Some(next) == active.exit {
                            nodes.push(Node::Break(active.header));
                            break;
                        }
                    }
                    current = next;
                    continue;
                }
                break;
            }
            if let Some(natural) = &self.control.loops[current]
                && active.is_none_or(|active| active.header != current)
            {
                if natural.exit.is_none()
                    && natural.members.iter().enumerate().any(|(node, inside)| {
                        *inside
                            && self.control.successors[node]
                                .iter()
                                .any(|next| !natural.members[*next])
                    })
                {
                    return Err(resource("Ark multi-exit loop needs a residual region"));
                }
                let next = natural.exit;
                let context = LoopContext {
                    header: natural.header,
                    exit: next,
                };
                let body = self.region(current, next, Some(context), true, scope)?;
                nodes.push(Node::Loop {
                    header: current,
                    body,
                });
                if let Some(next) = next {
                    current = next;
                    continue;
                } else {
                    break;
                }
            }
            if !scope.is_some_and(|scope| scope.bridge(current))
                && self.function.blocks[current]
                    .handlers
                    .first()
                    .is_some_and(|handler| scope.is_none_or(|scope| *handler != scope.handler))
            {
                return Err(resource("Ark protected block needs an exception region"));
            }
            self.owned[current] = true;
            nodes.push(Node::Block(current));
            match &self.function.blocks[current].end {
                Terminator::Return(value) => {
                    nodes.push(Node::Return(*value));
                    break;
                }
                Terminator::Throw(value) => {
                    nodes.push(Node::Throw(*value));
                    break;
                }
                Terminator::Jump(next) => {
                    if let Some(active) = active {
                        if *next == active.header {
                            nodes.push(Node::Continue(active.header));
                            break;
                        }
                        if Some(*next) == active.exit {
                            nodes.push(Node::Break(active.header));
                            break;
                        }
                    }
                    current = *next;
                }
                Terminator::Branch {
                    value,
                    nonzero,
                    yes,
                    no,
                } => {
                    let join = self.control.postdominators.idom[current]
                        .filter(|join| *join < self.owned.len() && *join != current);
                    let join = if join == active.map(|active| active.header) {
                        stop
                    } else {
                        join.or(stop)
                    };
                    let (condition, yes, no) =
                        self.condition(*value, *nonzero, (*yes, *no), join, active, scope);
                    let yes_nodes = self.region(yes, join, active, false, scope)?;
                    let no_nodes = self.region(no, join, active, false, scope)?;
                    if let Some((slot, yes, no)) = self.select(&yes_nodes, &no_nodes) {
                        nodes.push(Node::Select {
                            condition,
                            slot,
                            yes,
                            no,
                        });
                    } else {
                        nodes.push(Node::If {
                            condition,
                            yes: yes_nodes,
                            no: no_nodes,
                        });
                    }
                    if let Some(join) = join {
                        if let Some(active) = active
                            && Some(join) == active.exit
                        {
                            nodes.push(Node::Break(active.header));
                            break;
                        }
                        current = join;
                    } else {
                        break;
                    }
                }
            }
        }
        Ok(nodes)
    }

    fn residual(
        &mut self,
        entry: usize,
        stop: Option<usize>,
        active: Option<LoopContext>,
        scope: Option<&Protected>,
    ) -> Result<Node> {
        let mut stack = vec![entry];
        let mut members = vec![false; self.owned.len()];
        while let Some(node) = stack.pop() {
            if Some(node) == stop
                || active.is_some_and(|active| {
                    (node == active.header && node != entry) || Some(node) == active.exit
                })
            {
                continue;
            }
            if members[node] {
                continue;
            }
            if self.owned[node] {
                return Err(resource("Ark residual region crosses an owned block"));
            }
            if !self.in_scope(node, scope) {
                return Err(resource("Ark residual crosses its exception scope"));
            }
            members[node] = true;
            stack.extend(self.control.successors[node].iter().copied());
            let handlers = &self.function.blocks[node].handlers;
            let local = scope.map_or(handlers.len(), |scope| {
                handlers
                    .iter()
                    .position(|handler| *handler == scope.handler)
                    .unwrap_or(0)
            });
            stack.extend(handlers[..local].iter().copied());
        }
        let blocks: Vec<_> = members
            .iter()
            .enumerate()
            .filter_map(|(index, inside)| inside.then_some(index))
            .collect();
        for block in &blocks {
            self.owned[*block] = true;
        }
        self.residual_blocks += blocks.len();
        Ok(Node::Residual {
            entry,
            blocks,
            stop,
            active,
            caught_by: scope.map(|scope| scope.handler),
        })
    }
}
