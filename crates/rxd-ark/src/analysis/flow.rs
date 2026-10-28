use crate::{
    ir::{Action, Constant, Event, Function, Op, Slot, Terminator, ValueId},
    reader::resource,
};
use rxd_core::{Budget, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fact {
    Unknown,
    Boolean,
    Bool(bool),
    Number(u64),
    Null,
    Undefined,
    Input(u32),
}

impl Fact {
    fn boolean(self) -> bool {
        matches!(self, Self::Boolean | Self::Bool(_))
    }
    fn merge(self, other: Self) -> Self {
        if self == other {
            self
        } else if self.boolean() && other.boolean() {
            Self::Boolean
        } else {
            Self::Unknown
        }
    }
    fn constant(self) -> Option<Constant> {
        match self {
            Self::Bool(value) => Some(Constant::Bool(value)),
            Self::Number(value) => Some(Constant::Number(f64::from_bits(value))),
            Self::Null => Some(Constant::Null),
            Self::Undefined => Some(Constant::Undefined),
            _ => None,
        }
    }
}

fn index(slot: Slot) -> usize {
    match slot {
        Slot::Acc => 0,
        Slot::Resume => 1,
        Slot::Register(reg) => reg as usize + 2,
    }
}

fn value_fact(function: &Function<'_>, value: ValueId, state: &[Fact]) -> Fact {
    match &function.values[value.0].op {
        Op::Read(slot) => state[index(*slot)],
        Op::Constant(Constant::Bool(value)) => Fact::Bool(*value),
        Op::Constant(Constant::Number(value)) => Fact::Number(value.to_bits()),
        Op::Constant(Constant::Null) => Fact::Null,
        Op::Constant(Constant::Undefined) => Fact::Undefined,
        Op::This => function.frame.this_reg.map_or(Fact::Unknown, Fact::Input),
        Op::NewTarget => function.frame.new_target.map_or(Fact::Unknown, Fact::Input),
        Op::Function => function.frame.func.map_or(Fact::Unknown, Fact::Input),
        _ if function.boolean(value) => Fact::Boolean,
        _ => Fact::Unknown,
    }
}

struct Analysis<'f, 'a> {
    function: &'f Function<'a>,
    incoming: Vec<Option<Vec<Fact>>>,
    queued: Vec<bool>,
    pending: Vec<usize>,
    facts: Vec<Fact>,
}

impl Analysis<'_, '_> {
    fn merge(&mut self, block: usize, state: &[Fact], exceptional: bool) -> Result<()> {
        let Some(existing) = &mut self.incoming[block] else {
            let mut facts = Vec::new();
            facts
                .try_reserve_exact(state.len())
                .map_err(|_| resource("Ark value-flow state allocation failed"))?;
            facts.extend_from_slice(state);
            if exceptional {
                facts[0] = Fact::Unknown;
            }
            self.incoming[block] = Some(facts);
            if !self.queued[block] {
                self.queued[block] = true;
                self.pending.push(block);
            }
            return Ok(());
        };
        let mut changed = false;
        for (slot, (old, next)) in existing.iter_mut().zip(state).enumerate() {
            let next = if exceptional && slot == 0 {
                Fact::Unknown
            } else {
                *next
            };
            let joined = old.merge(next);
            if joined != *old {
                *old = joined;
                changed = true;
            }
        }
        if changed && !self.queued[block] {
            self.queued[block] = true;
            self.pending.push(block);
        }
        Ok(())
    }

    fn exceptional(&mut self, block: usize, state: &[Fact]) -> Result<()> {
        for handler in &self.function.blocks[block].handlers {
            self.merge(*handler, state, true)?;
        }
        Ok(())
    }

    fn run(&mut self) -> Result<()> {
        while let Some(block) = self.pending.pop() {
            self.queued[block] = false;
            let Some(incoming) = &self.incoming[block] else {
                continue;
            };
            let mut state = Vec::new();
            state
                .try_reserve_exact(incoming.len())
                .map_err(|_| resource("Ark value-flow working allocation failed"))?;
            state.extend_from_slice(incoming);
            for event in &self.function.blocks[block].events {
                match event {
                    Event::Define(id) => {
                        if self.function.values[id.0].effects.may_throw {
                            self.exceptional(block, &state)?;
                        }
                        self.facts[id.0] = value_fact(self.function, *id, &state);
                    }
                    Event::Action { action, .. } => {
                        if action.effects().may_throw {
                            self.exceptional(block, &state)?;
                        }
                        if let Action::Write(slot, value) = action {
                            state[index(*slot)] = self.facts[value.0];
                        }
                    }
                }
            }
            if matches!(self.function.blocks[block].end, Terminator::Throw(_)) {
                self.exceptional(block, &state)?;
            }
            for successor in self.function.blocks[block].end.successors() {
                self.merge(successor, &state, false)?;
            }
        }
        Ok(())
    }
}

pub(super) fn propagate(function: &mut Function<'_>, budget: Budget) -> Result<Vec<bool>> {
    let count = function.blocks.len();
    let width = (function.code.vregs as usize)
        .checked_add(function.code.args as usize)
        .and_then(|width| width.checked_add(2))
        .ok_or_else(|| resource("Ark value-flow frame overflow"))?;
    let storage = count
        .checked_mul(width)
        .and_then(|n| n.checked_mul(std::mem::size_of::<Fact>()))
        .and_then(|n| {
            function
                .values
                .len()
                .checked_mul(std::mem::size_of::<Fact>())
                .and_then(|v| n.checked_add(v))
        })
        .ok_or_else(|| resource("Ark value-flow admission overflow"))?;
    if storage > budget.metadata_bytes {
        return Err(resource("Ark value-flow exceeds metadata admission"));
    }
    let mut initial = Vec::new();
    initial
        .try_reserve_exact(width)
        .map_err(|_| resource("Ark value-flow entry allocation failed"))?;
    initial.resize(width, Fact::Unknown);
    for reg in function.code.vregs..function.code.vregs + function.code.args {
        initial[index(Slot::Register(reg))] = Fact::Input(reg);
    }
    let mut pending = Vec::new();
    pending
        .try_reserve_exact(count)
        .map_err(|_| resource("Ark value-flow worklist allocation failed"))?;
    let mut incoming = Vec::new();
    incoming
        .try_reserve_exact(count)
        .map_err(|_| resource("Ark value-flow graph allocation failed"))?;
    incoming.resize_with(count, || None);
    let mut queued = Vec::new();
    queued
        .try_reserve_exact(count)
        .map_err(|_| resource("Ark value-flow worklist allocation failed"))?;
    queued.resize(count, false);
    let mut facts = Vec::new();
    facts
        .try_reserve_exact(function.values.len())
        .map_err(|_| resource("Ark value-flow facts allocation failed"))?;
    facts.resize(function.values.len(), Fact::Unknown);
    let mut analysis = Analysis {
        function,
        incoming,
        queued,
        pending,
        facts,
    };
    analysis.merge(0, &initial, false)?;
    analysis.run()?;
    let mut replacements = Vec::new();
    let mut boolean = Vec::new();
    boolean
        .try_reserve_exact(function.values.len())
        .map_err(|_| resource("Ark value-flow boolean allocation failed"))?;
    boolean.resize(function.values.len(), false);
    for (block, incoming) in analysis.incoming.iter().enumerate() {
        let Some(incoming) = incoming else {
            continue;
        };
        let mut state = Vec::new();
        state
            .try_reserve_exact(incoming.len())
            .map_err(|_| resource("Ark value-flow working allocation failed"))?;
        state.extend_from_slice(incoming);
        for event in &analysis.function.blocks[block].events {
            match event {
                Event::Define(id) => {
                    let fact = value_fact(analysis.function, *id, &state);
                    analysis.facts[id.0] = fact;
                    boolean[id.0] = fact.boolean();
                    if let Op::Read(original) = analysis.function.values[id.0].op {
                        let replacement = fact.constant().map(Op::Constant).or_else(|| {
                            if let Fact::Input(reg) = fact {
                                let source = Slot::Register(reg);
                                if source != original && state[index(source)] == fact {
                                    return Some(Op::Read(source));
                                }
                            }
                            None
                        });
                        if let Some(op) = replacement {
                            replacements.try_reserve(1).map_err(|_| {
                                resource("Ark value-flow replacement allocation failed")
                            })?;
                            replacements.push((*id, op));
                        }
                    }
                }
                Event::Action {
                    action: Action::Write(slot, value),
                    ..
                } => state[index(*slot)] = analysis.facts[value.0],
                _ => {}
            }
        }
    }
    for (id, op) in replacements {
        function.values[id.0].op = op;
    }
    Ok(boolean)
}
