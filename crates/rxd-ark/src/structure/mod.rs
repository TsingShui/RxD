mod conditions;

use crate::{
    analysis::{Control, Values},
    ir::{Function, Terminator, ValueId},
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

pub(crate) enum Node {
    Block(usize),
    If {
        condition: Condition,
        yes: Vec<Node>,
        no: Vec<Node>,
    },
    Loop {
        header: usize,
        body: Vec<Node>,
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
}

struct Builder<'f, 'a> {
    function: &'f Function<'a>,
    control: &'f Control,
    values: &'f Values,
    owned: Vec<bool>,
    depth: usize,
    budget: Budget,
    residual_blocks: usize,
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
        owned: vec![false; function.blocks.len()],
        depth: 0,
        budget,
        residual_blocks: 0,
    };
    let nodes = if function
        .blocks
        .iter()
        .any(|block| !block.handlers.is_empty())
    {
        vec![builder.residual(0, None, None)?]
    } else {
        builder.region(0, None, None, false)?
    };
    if function
        .graph
        .reachable
        .iter()
        .zip(&builder.owned)
        .any(|(reachable, owned)| *reachable && !owned)
    {
        return Err(resource(
            "Ark structural plan did not account for a reachable block",
        ));
    }
    Ok(Body {
        nodes,
        owned: builder.owned,
        residual_blocks: builder.residual_blocks,
    })
}

impl Builder<'_, '_> {
    fn region(
        &mut self,
        start: usize,
        stop: Option<usize>,
        active: Option<LoopContext>,
        loop_entry: bool,
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
        self.depth += 1;
        let structured = if self.depth < self.budget.ast_depth / 4 {
            self.walk(start, stop, active)
        } else {
            Err(resource("Ark structure nesting requires residual plan"))
        };
        self.depth -= 1;
        match structured {
            Ok(nodes) => Ok(nodes),
            Err(_) => {
                self.owned = before;
                self.residual_blocks = residual_before;
                Ok(vec![self.residual(start, stop, active)?])
            }
        }
    }

    fn walk(
        &mut self,
        mut current: usize,
        stop: Option<usize>,
        active: Option<LoopContext>,
    ) -> Result<Vec<Node>> {
        let mut nodes = Vec::new();
        loop {
            if Some(current) == stop {
                break;
            }
            if current >= self.owned.len() || self.owned[current] {
                return Err(resource("Ark region has a shared entry"));
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
                let body = self.region(current, next, Some(context), true)?;
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
                        self.condition(*value, *nonzero, *yes, *no, join, active);
                    let yes_nodes = self.region(yes, join, active, false)?;
                    let no_nodes = self.region(no, join, active, false)?;
                    nodes.push(Node::If {
                        condition,
                        yes: yes_nodes,
                        no: no_nodes,
                    });
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
            members[node] = true;
            stack.extend(self.control.successors[node].iter().copied());
            stack.extend(self.function.blocks[node].handlers.iter().copied());
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
        })
    }
}
