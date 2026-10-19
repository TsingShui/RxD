//! Bounded, contextual native stack evidence for selector-dispatched function candidates.
//!
//! Membership is reachable decoded blocks, not adjacent entry intervals or a union of
//! producer CFG return targets. Contexts retain valid constant jump destinations in
//! their original stack slots; ordinary changing values widen at joins.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use alloy_primitives::U256;
use rxd_core::{
    Error, Result,
    envelope::{self, RecordMeta},
    perf::Perf,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    BoundedOutput, Limits, Navigation, hex,
    navigation::{object, work, write_record},
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AbstractValue {
    Unknown,
    Constant(U256),
    /// A word with the selected four-byte prefix and unknown remaining calldata bytes.
    CalldataWord0,
    /// Generic dispatcher provenance, never a guessed calldata word.
    SelectorWord,
    SelectorEquals(u32),
}

impl AbstractValue {
    pub fn constant(&self) -> Option<U256> {
        if let Self::Constant(value) = self {
            Some(*value)
        } else {
            None
        }
    }

    pub fn json(&self) -> Value {
        match self {
            Self::Unknown => json!({"kind": "unknown", "hex": null}),
            Self::Constant(value) => json!({"kind": "constant", "hex": format!("{value:064x}")}),
            Self::CalldataWord0 => json!({"kind": "calldata_word_0", "hex": null}),
            Self::SelectorWord => json!({"kind":"selector_word","hex":null}),
            Self::SelectorEquals(selector) => {
                json!({"kind":"selector_equals","hex":format!("{selector:08x}")})
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryWord {
    pub value: U256,
    pub write_pc: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AbstractState {
    /// Bottom to top. Unknown values remain incoming preconditions, never guessed zero.
    pub stack: Vec<AbstractValue>,
    /// Exact observed MSTORE words; overlapping/unknown writes and calls invalidate facts.
    pub memory: BTreeMap<usize, MemoryWord>,
}

impl AbstractState {
    pub fn json(&self) -> Value {
        json!({"stack": self.stack.iter().map(AbstractValue::json).collect::<Vec<_>>(),
            "memory_words": self.memory.iter().map(|(offset, word)| json!({"offset": offset,
                "value_hex": format!("{:064x}",word.value), "write_pc": word.write_pc})).collect::<Vec<_>>()})
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Evidence {
    pub decoded_offset: usize,
    pub kind: &'static str,
    pub destination_hex: Option<String>,
    pub reason: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Edge {
    pub instruction_offset: usize,
    pub kind: &'static str,
    /// Native validated bytecode offset; never an upstream CFG ID.
    pub target: Option<usize>,
    pub target_context: Option<usize>,
    pub resolution: &'static str,
}

#[derive(Clone, Debug)]
pub struct Context {
    pub block_index: usize,
    pub entry: AbstractState,
    pub exit: AbstractState,
    pub edges: Vec<Edge>,
    pub problems: Vec<Evidence>,
}

#[derive(Debug)]
pub struct FunctionCandidate {
    pub selector: String,
    pub dispatch: &'static str,
    pub entry: usize,
    pub block_indices: Vec<usize>,
    pub entry_states: Vec<AbstractState>,
    pub contexts: Vec<Context>,
    pub unknown_successors: Vec<Evidence>,
    pub cross_entries: Vec<usize>,
}

impl FunctionCandidate {
    pub fn boundary_complete(&self) -> bool {
        !self.entry_states.is_empty()
            && self.unknown_successors.is_empty()
            && self.cross_entries.is_empty()
    }
}

pub enum FunctionSelection<'a> {
    Id(&'a str),
    Selector(&'a str),
}

pub struct FunctionNavigation {
    pub functions: Vec<FunctionCandidate>,
    /// Supported function indices for each native block, in stable function order.
    pub owners: Vec<Vec<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct StateKey {
    block: usize,
    // Retain constants only when they can control a native jump. Every other value
    // merges monotonically to unknown. A loop counter does not create endless states.
    controls: Vec<Option<U256>>,
}

#[derive(Default)]
struct GraphBudget {
    states: usize,
    stack_cells: usize,
}

struct Walker<'a> {
    navigation: &'a Navigation,
    selector: Option<U256>,
    discovered: BTreeSet<(String, usize)>,
    entries: &'a BTreeSet<usize>,
    limits: Limits,
    budget: &'a mut GraphBudget,
    perf: &'a mut Perf,
}

#[derive(Clone, Debug)]
pub(crate) struct CopyOperation {
    pub pc: usize,
    pub destination: Option<U256>,
    pub source: Option<U256>,
    pub length: Option<U256>,
}

pub(crate) struct CopyWitness {
    pub block: usize,
    pub owner: Option<usize>,
    pub context: Option<usize>,
    pub operation: CopyOperation,
    pub entry: Option<AbstractState>,
    pub entry_blocks: Vec<usize>,
}

struct Transfer {
    state: AbstractState,
    edges: Vec<Edge>,
    problems: Vec<Evidence>,
}

fn usize_word(value: U256) -> Option<usize> {
    usize::try_from(value).ok()
}

impl<'a> Walker<'a> {
    fn block(&self, offset: usize) -> Option<usize> {
        self.navigation
            .blocks
            .binary_search_by_key(&offset, |b| b.start)
            .ok()
    }

    fn jumpdest(&self, value: U256) -> Option<usize> {
        let pc = usize_word(value)?;
        let block = self.block(pc)?;
        (self.navigation.instructions[self.navigation.blocks[block].instruction_start].opcode
            == 0x5b)
            .then_some(pc)
    }

    fn key(&self, block: usize, state: &AbstractState) -> StateKey {
        StateKey {
            block,
            controls: state
                .stack
                .iter()
                .map(|value| {
                    value
                        .constant()
                        .filter(|value| self.jumpdest(*value).is_some())
                })
                .collect(),
        }
    }

    fn reserve_storage(&mut self, cells: usize) -> Result<()> {
        self.budget.stack_cells = self.budget.stack_cells.saturating_add(cells);
        if self.budget.stack_cells > self.limits.stack_cells {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "EVM contextual stack storage budget exceeded",
            ));
        }
        Ok(())
    }

    fn charge_state(&mut self, state: &AbstractState) -> Result<()> {
        work(
            self.perf,
            1 + state.stack.len() + state.memory.len(),
            self.limits.work,
        )
    }

    fn transfer(
        &mut self,
        block: usize,
        mut state: AbstractState,
        mut copies: Option<&mut Vec<CopyOperation>>,
    ) -> Result<Transfer> {
        let native = &self.navigation.blocks[block];
        let mut edges = Vec::new();
        let mut problems = Vec::new();
        let mut terminated = false;
        for i in native.instruction_start..native.instruction_end {
            let instruction = &self.navigation.instructions[i];
            let pc = instruction.decoded_offset;
            let op = instruction.opcode;
            work(self.perf, 1, self.limits.work)?;
            if (0x80..=0x8f).contains(&op) {
                let depth = usize::from(op - 0x7f);
                if state.stack.len() < depth {
                    problems.push(problem(pc, "stack", None, "stack_underflow"));
                    break;
                }
                state
                    .stack
                    .push(state.stack[state.stack.len() - depth].clone());
            } else if (0x90..=0x9f).contains(&op) {
                let depth = usize::from(op - 0x8f);
                let len = state.stack.len();
                if len <= depth {
                    problems.push(problem(pc, "stack", None, "stack_underflow"));
                    break;
                }
                state.stack.swap(len - 1, len - 1 - depth);
            } else if (0x5f..=0x7f).contains(&op) {
                let value = U256::from_be_slice(
                    &self.navigation.carrier.bytecode[pc + 1..instruction.decoded_end],
                );
                state.stack.push(AbstractValue::Constant(value));
            } else {
                let Some((pops, pushes)) = effect(op) else {
                    problems.push(problem(pc, "opcode", None, "unsupported_opcode"));
                    break;
                };
                if state.stack.len() < pops {
                    problems.push(problem(pc, "stack", None, "stack_underflow"));
                    break;
                }
                let mut args = Vec::with_capacity(pops);
                for _ in 0..pops {
                    args.push(state.stack.pop().unwrap());
                }
                if op == 0x39
                    && let Some(copies) = copies.as_deref_mut()
                {
                    work(self.perf, 1, self.limits.work)?;
                    if copies.len() >= self.limits.states {
                        return Err(Error::new(
                            "RESOURCE_LIMIT",
                            "EVM temporary copy witness storage budget exceeded",
                        ));
                    }
                    copies.push(CopyOperation {
                        pc,
                        destination: args[0].constant(),
                        source: args[1].constant(),
                        length: args[2].constant(),
                    });
                }
                match op {
                    0x00 | 0xf3 | 0xfd | 0xfe | 0xff => {
                        terminated = true;
                        break;
                    }
                    0x56 | 0x57 => {
                        let dispatch_selector = if op == 0x57 && self.selector.is_none() {
                            if let AbstractValue::SelectorEquals(value) = args[1] {
                                Some(value)
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        if let Some(selector) = dispatch_selector
                            && let Some(target) = args[0].constant().and_then(|v| self.jumpdest(v))
                        {
                            self.discovered.insert((format!("{selector:08x}"), target));
                        }
                        let condition = if op == 0x57 { args[1].constant() } else { None };
                        if op == 0x56 || condition.is_none_or(|v| v != U256::ZERO) {
                            let destination = args[0].constant();
                            let target = destination.and_then(|v| self.jumpdest(v));
                            let resolution = if target.is_some() {
                                "resolved"
                            } else if destination.is_some() {
                                "invalid"
                            } else {
                                "unknown"
                            };
                            let kind = if dispatch_selector.is_some() {
                                "selector_dispatch"
                            } else if op == 0x56 {
                                "jump"
                            } else {
                                "branch_true"
                            };
                            edges.push(Edge {
                                instruction_offset: pc,
                                kind,
                                target,
                                target_context: None,
                                resolution,
                            });
                            if target.is_none() {
                                problems.push(problem(
                                    pc,
                                    kind,
                                    destination,
                                    if destination.is_some() {
                                        "invalid_jump_destination"
                                    } else {
                                        "dynamic_jump_destination"
                                    },
                                ));
                            }
                        }
                        if op == 0x57 && condition.is_none_or(|v| v == U256::ZERO) {
                            let target = self
                                .block(instruction.decoded_end)
                                .map(|_| instruction.decoded_end);
                            // Falling off the bytecode is an EVM stop; a truncated/omitted prefix is not.
                            if target.is_some() {
                                edges.push(Edge {
                                    instruction_offset: pc,
                                    kind: "branch_false",
                                    target,
                                    target_context: None,
                                    resolution: "resolved",
                                });
                            } else if instruction.decoded_end
                                < self.navigation.carrier.bytecode.len()
                            {
                                problems.push(problem(
                                    pc,
                                    "branch_false",
                                    None,
                                    "undecoded_or_unindexed_successor",
                                ));
                            }
                        }
                        terminated = true;
                        break;
                    }
                    0x35 if args[0].constant() == Some(U256::ZERO) => {
                        state.stack.push(AbstractValue::CalldataWord0)
                    }
                    0x51 => {
                        let value = args[0]
                            .constant()
                            .and_then(usize_word)
                            .and_then(|offset| state.memory.get(&offset))
                            .map(|word| word.value);
                        state
                            .stack
                            .push(value.map_or(AbstractValue::Unknown, AbstractValue::Constant));
                    }
                    0x52 => {
                        work(self.perf, state.memory.len(), self.limits.work)?;
                        let offset = args[0].constant().and_then(usize_word);
                        if let Some(offset) = offset {
                            state
                                .memory
                                .retain(|start, _| !overlap(*start, 32, offset, 32));
                            if let Some(value) = args[1].constant() {
                                work(self.perf, 1, self.limits.work)?;
                                if state.memory.len() >= self.limits.stack_cells {
                                    return Err(Error::new(
                                        "RESOURCE_LIMIT",
                                        "EVM temporary memory-fact storage budget exceeded",
                                    ));
                                }
                                state.memory.insert(
                                    offset,
                                    MemoryWord {
                                        value,
                                        write_pc: pc,
                                    },
                                );
                            }
                        } else {
                            state.memory.clear();
                        }
                    }
                    0x53 => {
                        work(self.perf, state.memory.len(), self.limits.work)?;
                        if let Some(offset) = args[0].constant().and_then(usize_word) {
                            state
                                .memory
                                .retain(|start, _| !overlap(*start, 32, offset, 1));
                        } else {
                            state.memory.clear();
                        }
                    }
                    0x37 | 0x39 | 0x3c | 0x3e | 0x5e | 0xf0 | 0xf1 | 0xf2 | 0xf4 | 0xf5 | 0xfa => {
                        work(self.perf, state.memory.len(), self.limits.work)?;
                        // Copies/calls can alias known prelude words. No unbounded memory simulation.
                        state.memory.clear();
                        for _ in 0..pushes {
                            state.stack.push(AbstractValue::Unknown);
                        }
                    }
                    0x58 => state.stack.push(AbstractValue::Constant(U256::from(pc))),
                    0x38 => state.stack.push(AbstractValue::Constant(U256::from(
                        self.navigation.carrier.bytecode.len(),
                    ))),
                    _ if pushes == 1 => state.stack.push(self.calculate(op, &args)),
                    _ => {
                        for _ in 0..pushes {
                            state.stack.push(AbstractValue::Unknown);
                        }
                    }
                }
            }
            if state.stack.len() > 1024 {
                problems.push(problem(pc, "stack", None, "stack_overflow"));
                break;
            }
        }
        if !terminated && problems.is_empty() {
            if let Some(target) = self.block(native.end).map(|_| native.end) {
                edges.push(Edge {
                    instruction_offset: native.last_opcode_start,
                    kind: "fallthrough",
                    target: Some(target),
                    target_context: None,
                    resolution: "resolved",
                });
            } else if native.end < self.navigation.carrier.bytecode.len() {
                problems.push(problem(
                    native.end,
                    "fallthrough",
                    None,
                    "undecoded_or_unindexed_successor",
                ));
            }
        }
        Ok(Transfer {
            state,
            edges,
            problems,
        })
    }

    fn calculate(&self, op: u8, args: &[AbstractValue]) -> AbstractValue {
        // Exact selector-prefix extraction; the rest of calldata word 0 stays unknown.
        if matches!(op, 0x1c | 0x04) && args.len() == 2 {
            if op == 0x1c
                && args[0].constant() == Some(U256::from(224))
                && args[1] == AbstractValue::CalldataWord0
            {
                return self
                    .selector
                    .map_or(AbstractValue::SelectorWord, AbstractValue::Constant);
            }
            if op == 0x04
                && args[0] == AbstractValue::CalldataWord0
                && args[1].constant() == Some(U256::from(1) << 224)
            {
                return self
                    .selector
                    .map_or(AbstractValue::SelectorWord, AbstractValue::Constant);
            }
        }
        if op == 0x14 && args.len() == 2 {
            let constant = if args[0] == AbstractValue::SelectorWord {
                args[1].constant()
            } else if args[1] == AbstractValue::SelectorWord {
                args[0].constant()
            } else {
                None
            };
            if let Some(selector) = constant.and_then(|value| u32::try_from(value).ok()) {
                return AbstractValue::SelectorEquals(selector);
            }
        }
        let Some(a) = args.first().and_then(AbstractValue::constant) else {
            return AbstractValue::Unknown;
        };
        let b = args.get(1).and_then(AbstractValue::constant);
        let value = match (op, b) {
            (0x01, Some(b)) => a.wrapping_add(b),
            (0x02, Some(b)) => a.wrapping_mul(b),
            (0x03, Some(b)) => a.wrapping_sub(b),
            (0x04, Some(b)) => {
                if b == U256::ZERO {
                    U256::ZERO
                } else {
                    a / b
                }
            }
            (0x06, Some(b)) => {
                if b == U256::ZERO {
                    U256::ZERO
                } else {
                    a % b
                }
            }
            (0x10, Some(b)) => U256::from(u8::from(a < b)),
            (0x11, Some(b)) => U256::from(u8::from(a > b)),
            (0x14, Some(b)) => U256::from(u8::from(a == b)),
            (0x15, _) => U256::from(u8::from(a == U256::ZERO)),
            (0x16, Some(b)) => a & b,
            (0x17, Some(b)) => a | b,
            (0x18, Some(b)) => a ^ b,
            (0x19, _) => !a,
            (0x1b, Some(b)) => {
                if a >= U256::from(256) {
                    U256::ZERO
                } else {
                    b << usize_word(a).unwrap()
                }
            }
            (0x1c, Some(b)) => {
                if a >= U256::from(256) {
                    U256::ZERO
                } else {
                    b >> usize_word(a).unwrap()
                }
            }
            _ => return AbstractValue::Unknown,
        };
        AbstractValue::Constant(value)
    }

    fn walk(
        &mut self,
        entry: usize,
        starts: Vec<AbstractState>,
        dispatcher: bool,
    ) -> Result<(Vec<Context>, Vec<AbstractState>)> {
        let Some(block) = self.block(if dispatcher { 0 } else { entry }) else {
            return Ok((Vec::new(), Vec::new()));
        };
        let mut contexts: Vec<Context> = Vec::new();
        let mut keys = BTreeMap::new();
        let mut queue = VecDeque::new();
        let mut arrivals = Vec::new();
        for state in starts {
            self.enqueue(block, state, &mut contexts, &mut keys, &mut queue)?;
        }
        while let Some(index) = queue.pop_front() {
            let block = contexts[index].block_index;
            let pc = self.navigation.blocks[block].start;
            if dispatcher && pc == entry {
                self.charge_state(&contexts[index].entry)?;
                arrivals.push(contexts[index].entry.clone());
                continue;
            }
            if dispatcher && pc != 0 && self.entries.contains(&pc) {
                continue;
            }
            self.charge_state(&contexts[index].entry)?;
            let transfer = self.transfer(block, contexts[index].entry.clone(), None)?;
            let mut edges = transfer.edges;
            for edge in &mut edges {
                if edge.kind == "selector_dispatch" {
                    continue;
                }
                if let Some(target) = edge.target {
                    let target_block = self.block(target).unwrap();
                    let target_context = self.enqueue(
                        target_block,
                        transfer.state.clone(),
                        &mut contexts,
                        &mut keys,
                        &mut queue,
                    )?;
                    edge.target_context = Some(target_context);
                }
            }
            let previous_slots =
                contexts[index].exit.stack.len() + contexts[index].exit.memory.len();
            let next_slots = transfer.state.stack.len() + transfer.state.memory.len();
            self.reserve_storage(next_slots.saturating_sub(previous_slots))?;
            contexts[index].exit = transfer.state;
            contexts[index].edges = edges;
            contexts[index].problems = transfer.problems;
        }
        arrivals.sort_by_key(|state| format!("{}", state.json()));
        arrivals.dedup();
        Ok((contexts, arrivals))
    }

    fn enqueue(
        &mut self,
        block: usize,
        state: AbstractState,
        contexts: &mut Vec<Context>,
        keys: &mut BTreeMap<StateKey, usize>,
        queue: &mut VecDeque<usize>,
    ) -> Result<usize> {
        self.charge_state(&state)?;
        let key = self.key(block, &state);
        if let Some(&index) = keys.get(&key) {
            let previous = &mut contexts[index].entry;
            let mut changed = false;
            for (left, right) in previous.stack.iter_mut().zip(&state.stack) {
                if *left != *right && *left != AbstractValue::Unknown {
                    *left = AbstractValue::Unknown;
                    changed = true;
                }
            }
            let count = previous.memory.len();
            previous
                .memory
                .retain(|offset, word| state.memory.get(offset) == Some(word));
            changed |= previous.memory.len() != count;
            if changed {
                queue.push_back(index);
            }
            return Ok(index);
        }
        self.budget.states = self.budget.states.saturating_add(1);
        // Entry/exit stacks and the control-value key are three stored slot arrays.
        self.reserve_storage(
            state.stack.len().saturating_mul(3) + state.memory.len().saturating_mul(2),
        )?;
        if self.budget.states > self.limits.states {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "EVM contextual state budget exceeded",
            ));
        }
        let index = contexts.len();
        keys.insert(key, index);
        contexts.push(Context {
            block_index: block,
            entry: state.clone(),
            exit: state,
            edges: Vec::new(),
            problems: Vec::new(),
        });
        queue.push_back(index);
        Ok(index)
    }
}

fn overlap(a: usize, a_len: usize, b: usize, b_len: usize) -> bool {
    a < b.saturating_add(b_len) && b < a.saturating_add(a_len)
}

fn problem(pc: usize, kind: &'static str, value: Option<U256>, reason: &'static str) -> Evidence {
    Evidence {
        decoded_offset: pc,
        kind,
        destination_hex: value.map(|v| format!("{v:064x}")),
        reason,
    }
}

// Stack shape contracts for native legacy instructions, without copying upstream VM code.
pub(crate) fn effect(op: u8) -> Option<(usize, usize)> {
    Some(match op {
        0x00 | 0x5b | 0xfe => (0, 0),
        0x01..=0x07 | 0x0a..=0x0b | 0x10..=0x14 | 0x16..=0x18 | 0x1a..=0x1d | 0x20 => (2, 1),
        0x08..=0x09 => (3, 1),
        0x15 | 0x19 | 0x1e | 0x31 | 0x35 | 0x3b | 0x3f | 0x40 | 0x49 | 0x51 | 0x54 | 0x5c => (1, 1),
        0x30 | 0x32..=0x34 | 0x36 | 0x38 | 0x3a | 0x3d | 0x41..=0x48 | 0x4a | 0x58..=0x5a => (0, 1),
        0x37 | 0x39 | 0x3e | 0x5e => (3, 0),
        0x3c => (4, 0),
        0x50 | 0x56 | 0xff => (1, 0),
        0x52..=0x53 | 0x55 | 0x57 | 0x5d | 0xf3 | 0xfd => (2, 0),
        0xa0..=0xa4 => (usize::from(op - 0xa0) + 2, 0),
        0xf0 => (3, 1),
        0xf1..=0xf2 => (7, 1),
        0xf4 | 0xfa => (6, 1),
        0xf5 => (4, 1),
        _ => return None,
    })
}

/// Evidence local to one raw block, with unknown incoming stack preconditions.
/// This does not establish reachability or guess a caller's saved return address.
pub(crate) fn block_local_edges(
    navigation: &Navigation,
    block: usize,
    limits: Limits,
    perf: &mut Perf,
) -> Result<(Vec<Edge>, Vec<Evidence>, usize)> {
    let native = &navigation.blocks[block];
    let mut height: isize = 0;
    let mut incoming = 0;
    let mut peak = 0;
    for instruction in &navigation.instructions[native.instruction_start..native.instruction_end] {
        work(perf, 1, limits.work)?;
        let op = instruction.opcode;
        let (required, pops, pushes) = if (0x80..=0x8f).contains(&op) {
            (usize::from(op - 0x7f), 0, 1)
        } else if (0x90..=0x9f).contains(&op) {
            (usize::from(op - 0x8f) + 1, 0, 0)
        } else if (0x5f..=0x7f).contains(&op) {
            (0, 0, 1)
        } else if let Some((pops, pushes)) = effect(op) {
            (pops, pops, pushes)
        } else {
            break;
        };
        incoming = incoming.max(required as isize - height);
        height += pushes as isize - pops as isize;
        peak = peak.max(height);
    }
    let incoming = if native.start == 0 {
        0
    } else {
        incoming as usize
    };
    if incoming > 1024 {
        return Ok((
            Vec::new(),
            vec![problem(native.start, "stack", None, "stack_overflow")],
            incoming,
        ));
    }
    if incoming.saturating_add(peak as usize) > limits.stack_cells {
        return Err(Error::new(
            "RESOURCE_LIMIT",
            "EVM block-local stack storage budget exceeded",
        ));
    }
    let entries = BTreeSet::new();
    let mut budget = GraphBudget::default();
    let mut walker = Walker {
        navigation,
        selector: None,
        discovered: BTreeSet::new(),
        entries: &entries,
        limits,
        budget: &mut budget,
        perf,
    };
    let transfer = walker.transfer(
        block,
        AbstractState {
            stack: vec![AbstractValue::Unknown; incoming],
            memory: BTreeMap::new(),
        },
        None,
    )?;
    Ok((transfer.edges, transfer.problems, incoming))
}

impl FunctionNavigation {
    pub fn discover(navigation: &Navigation, limits: Limits, perf: &mut Perf) -> Result<Self> {
        let producer = perf.measure("analyze", || {
            evmole::contract_info(
                evmole::ContractInfoArgs::new(&navigation.carrier.bytecode).with_selectors(),
            )
        });
        let stage = perf.start_stage();
        let result = (|| {
            let mut candidates: BTreeSet<(String, &'static str, usize)> = producer
                .functions
                .unwrap_or_default()
                .into_iter()
                .map(|f| (hex(&f.selector), f.dispatch.as_str(), f.bytecode_offset))
                .collect();
            let producer_candidates = candidates.clone();
            let mut budget = GraphBudget::default();
            // Producer selector maps collapse duplicate selectors. Preserve independently
            // witnessed EQ dispatcher entries so selector lookup cannot choose arbitrarily.
            if !candidates.is_empty() {
                let entries = candidates.iter().map(|f| f.2).collect();
                let mut walker = Walker {
                    navigation,
                    selector: None,
                    discovered: BTreeSet::new(),
                    entries: &entries,
                    limits,
                    budget: &mut budget,
                    perf,
                };
                walker.walk(usize::MAX, vec![AbstractState::default()], true)?;
                for (selector, entry) in walker.discovered {
                    if !candidates.iter().any(|f| f.0 == selector && f.2 == entry) {
                        candidates.insert((selector, "abi", entry));
                    }
                }
            }
            let entries: BTreeSet<_> = candidates.iter().map(|f| f.2).collect();
            let mut functions = Vec::new();
            for (selector, dispatch, entry) in candidates {
                work(perf, 1, limits.work)?;
                let selector_word =
                    U256::from_str_radix(&selector, 16).expect("native four-byte selector");
                let mut walker = Walker {
                    navigation,
                    selector: Some(selector_word),
                    discovered: BTreeSet::new(),
                    entries: &entries,
                    limits,
                    budget: &mut budget,
                    perf,
                };
                let valid_entry = walker.block(entry).is_some()
                    && (entry == 0 || walker.jumpdest(U256::from(entry)).is_some());
                let (_, entry_states) = if valid_entry {
                    walker.walk(entry, vec![AbstractState::default()], true)?
                } else {
                    (Vec::new(), Vec::new())
                };
                let (contexts, _) = walker.walk(entry, entry_states.clone(), false)?;
                if entry_states.is_empty()
                    && !producer_candidates.contains(&(selector.clone(), dispatch, entry))
                {
                    continue;
                }
                let mut block_indices: Vec<_> = contexts
                    .iter()
                    .map(|c| c.block_index)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                // Even unresolved entry-state evidence retains an exact block FETCH route.
                if block_indices.is_empty() && valid_entry {
                    block_indices.push(walker.block(entry).unwrap());
                }
                let mut unknown: BTreeSet<_> = contexts
                    .iter()
                    .flat_map(|c| c.problems.iter().cloned())
                    .collect();
                if entry_states.is_empty() {
                    unknown.insert(problem(
                        entry,
                        "entry",
                        None,
                        if valid_entry {
                            "entry_state_unresolved"
                        } else {
                            "entry_not_validated"
                        },
                    ));
                }
                let cross_entries = block_indices
                    .iter()
                    .map(|&i| navigation.blocks[i].start)
                    .filter(|&pc| pc != entry && entries.contains(&pc))
                    .collect();
                functions.push(FunctionCandidate {
                    selector,
                    dispatch,
                    entry,
                    block_indices,
                    entry_states,
                    contexts,
                    unknown_successors: unknown.into_iter().collect(),
                    cross_entries,
                });
            }
            functions.sort_by(|a, b| {
                (&a.selector, a.dispatch, a.entry).cmp(&(&b.selector, b.dispatch, b.entry))
            });
            perf.counters.entities = Some(functions.len());
            let mut owners = vec![Vec::new(); navigation.blocks.len()];
            for (index, function) in functions.iter().enumerate() {
                for &block in &function.block_indices {
                    work(perf, 1, limits.work)?;
                    owners[block].push(index);
                }
            }
            Ok(Self { functions, owners })
        })();
        perf.record("navigate", stage);
        result
    }

    // Replay the native transfer with the exact context entry, never a global owner union.
    // The replay allocates no EVM memory and captures the pre-pop CODECOPY operands.
    pub(crate) fn copy_witnesses(
        &self,
        navigation: &Navigation,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<Vec<CopyWitness>> {
        let mut copy_blocks = BTreeSet::new();
        for (block, native) in navigation.blocks.iter().enumerate() {
            work(
                perf,
                native.instruction_end - native.instruction_start,
                limits.work,
            )?;
            if navigation.instructions[native.instruction_start..native.instruction_end]
                .iter()
                .any(|i| i.opcode == 0x39)
            {
                copy_blocks.insert(block);
            }
        }
        if copy_blocks.is_empty() {
            return Ok(Vec::new());
        }
        let entries = self.functions.iter().map(|f| f.entry).collect();
        let mut budget = GraphBudget::default();
        let mut witnesses = Vec::new();
        for (owner, function) in self.functions.iter().enumerate() {
            let mut walker = Walker {
                navigation,
                selector: Some(
                    U256::from_str_radix(&function.selector, 16).expect("native selector"),
                ),
                discovered: BTreeSet::new(),
                entries: &entries,
                limits,
                budget: &mut budget,
                perf,
            };
            for (context, state) in function.contexts.iter().enumerate() {
                if !copy_blocks.contains(&state.block_index) {
                    continue;
                }
                walker.charge_state(&state.entry)?;
                let mut copies = Vec::new();
                walker.transfer(state.block_index, state.entry.clone(), Some(&mut copies))?;
                for operation in copies {
                    work(walker.perf, 1, limits.work)?;
                    if witnesses.len() >= limits.states {
                        return Err(Error::new(
                            "RESOURCE_LIMIT",
                            "EVM copy witness storage budget exceeded",
                        ));
                    }
                    walker.charge_state(&state.entry)?;
                    witnesses.push(CopyWitness {
                        entry: None,
                        entry_blocks: Vec::new(),
                        block: state.block_index,
                        owner: Some(owner),
                        context: Some(context),
                        operation,
                    });
                }
            }
        }
        // Ownerless entry paths still have an exact block FETCH route. An unvisited
        // block receives no guessed empty stack or manufactured copy association.
        if copy_blocks.iter().any(|&b| self.owners[b].is_empty()) {
            let mut walker = Walker {
                navigation,
                selector: None,
                discovered: BTreeSet::new(),
                entries: &entries,
                limits,
                budget: &mut budget,
                perf,
            };
            let (contexts, _) = walker.walk(0, vec![AbstractState::default()], false)?;
            walker.reserve_storage(contexts.len().saturating_mul(2))?;
            work(walker.perf, contexts.len().saturating_mul(2), limits.work)?;
            let mut parents = vec![None; contexts.len()];
            let mut seen = vec![false; contexts.len()];
            let mut queue = VecDeque::new();
            if !contexts.is_empty() {
                seen[0] = true;
                queue.push_back(0);
            }
            while let Some(from) = queue.pop_front() {
                for edge in &contexts[from].edges {
                    work(walker.perf, 1, limits.work)?;
                    if let Some(to) = edge.target_context
                        && !seen[to]
                    {
                        seen[to] = true;
                        parents[to] = Some(from);
                        queue.push_back(to);
                    }
                }
            }
            for (context, state) in contexts.iter().enumerate() {
                if !copy_blocks.contains(&state.block_index)
                    || !self.owners[state.block_index].is_empty()
                {
                    continue;
                }
                walker.charge_state(&state.entry)?;
                let mut copies = Vec::new();
                walker.transfer(state.block_index, state.entry.clone(), Some(&mut copies))?;
                for operation in copies {
                    work(walker.perf, 1, limits.work)?;
                    if witnesses.len() >= limits.states {
                        return Err(Error::new(
                            "RESOURCE_LIMIT",
                            "EVM copy witness storage budget exceeded",
                        ));
                    }
                    walker.charge_state(&state.entry)?;
                    let mut entry_blocks = vec![state.block_index];
                    let mut prior = context;
                    while let Some(parent) = parents[prior] {
                        work(walker.perf, 1, limits.work)?;
                        entry_blocks.push(contexts[parent].block_index);
                        prior = parent;
                    }
                    entry_blocks.reverse();
                    walker.reserve_storage(
                        state.entry.stack.len() + state.entry.memory.len() + entry_blocks.len(),
                    )?;
                    witnesses.push(CopyWitness {
                        entry_blocks,
                        entry: Some(state.entry.clone()),
                        block: state.block_index,
                        owner: None,
                        context: None,
                        operation,
                    });
                }
            }
        }
        Ok(witnesses)
    }

    pub fn function_id(&self, navigation: &Navigation, function: &FunctionCandidate) -> String {
        format!(
            "rxd1:evm:{}:{}:function:{}:{}:{}",
            navigation.carrier.source_id,
            navigation.carrier.encoding.as_str(),
            function.selector,
            function.dispatch,
            function.entry
        )
    }

    pub fn owner_ids(&self, navigation: &Navigation, block: usize) -> Vec<String> {
        self.owners[block]
            .iter()
            .map(|&index| self.function_id(navigation, &self.functions[index]))
            .collect()
    }

    pub fn select<'a>(
        &'a self,
        navigation: &Navigation,
        id: Option<&str>,
        selector: Option<&str>,
    ) -> Result<&'a FunctionCandidate> {
        if let Some(selector) = selector {
            let selector = selector
                .strip_prefix("0x")
                .or_else(|| selector.strip_prefix("0X"))
                .unwrap_or(selector)
                .to_ascii_lowercase();
            if selector.len() != 8 || !selector.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::new(
                    "INVALID_QUERY",
                    "selector must contain exactly four hex bytes",
                ));
            }
            let mut matches = self.functions.iter().filter(|f| f.selector == selector);
            let selected = matches.next().ok_or_else(|| {
                Error::new(
                    "NOT_FOUND",
                    "EVM selector has no discovered function candidate",
                )
            })?;
            if matches.next().is_some() {
                return Err(Error::new(
                    "AMBIGUOUS_SELECTOR",
                    "EVM selector identifies multiple candidates; select a function ID",
                ));
            }
            return Ok(selected);
        }
        let id =
            id.ok_or_else(|| Error::new("INVALID_QUERY", "select a function ID or --selector"))?;
        let fields: Vec<_> = id.split(':').collect();
        let invalid = || {
            Error::new(
                "INVALID_ID",
                "expected a canonical content-bound EVM function ID",
            )
        };
        if fields.len() != 8
            || fields[0] != "rxd1"
            || fields[1] != "evm"
            || fields[4] != "function"
            || fields[2].len() != 64
            || !fields[2]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !matches!(fields[3], "raw" | "hex")
            || fields[5].len() != 8
            || !fields[5]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !matches!(fields[6], "abi" | "fallback")
        {
            return Err(invalid());
        }
        let entry = fields[7].parse::<usize>().map_err(|_| invalid())?;
        if entry.to_string() != fields[7] {
            return Err(invalid());
        }
        if fields[2] != navigation.carrier.source_id
            || fields[3] != navigation.carrier.encoding.as_str()
        {
            return Err(Error::new(
                "SOURCE_MISMATCH",
                "EVM function ID does not match carrier and encoding",
            ));
        }
        self.functions
            .iter()
            .find(|f| f.selector == fields[5] && f.dispatch == fields[6] && f.entry == entry)
            .ok_or_else(invalid)
    }

    pub fn function_record(
        &self,
        navigation: &Navigation,
        function: &FunctionCandidate,
        raw: Option<bool>,
    ) -> Value {
        let mut intervals: Vec<(usize, usize)> = Vec::new();
        for &block in &function.block_indices {
            let b = &navigation.blocks[block];
            if let Some(last) = intervals.last_mut()
                && last.1 == b.start
            {
                last.1 = b.end;
            } else {
                intervals.push((b.start, b.end));
            }
        }
        let range = if intervals.len() == 1 {
            Some(
                navigation
                    .carrier
                    .carrier_range(intervals[0].0, intervals[0].1),
            )
        } else {
            None
        };
        let fetch = raw.is_some();
        let segments: Vec<_> = intervals.iter().map(|&(start,end)| {
            let (carrier_start,carrier_end) = navigation.carrier.carrier_range(start,end);
            json!({"decoded_start":start,"decoded_end":end,"carrier_start":carrier_start,"carrier_end":carrier_end,
                "raw_hex":fetch.then(||hex(&navigation.carrier.bytecode[start..end]))})
        }).collect();
        let blocks: Vec<_> = function
            .block_indices
            .iter()
            .map(|&index| {
                let mut record = navigation.block_record(&navigation.blocks[index], false);
                let owners = self.owner_ids(navigation, index);
                record["detail"]["shared"] = json!(owners.len() > 1);
                record["detail"]["owner_ids"] = json!(owners);
                record
            })
            .collect();
        let id = self.function_id(navigation, function);
        let instruction_count: usize = function
            .block_indices
            .iter()
            .map(|&i| navigation.blocks[i].instruction_end - navigation.blocks[i].instruction_start)
            .sum();
        let mut detail = object(
            json!({"unit_kind":"function","selector":function.selector,"dispatch":function.dispatch,"decoded_entry":function.entry,
            "entry_evidence":"selector_dispatch_candidate","entry_state_status":if function.entry_states.is_empty(){"unknown"}else{"dispatcher_derived"},
            "entry_states":function.entry_states.iter().map(AbstractState::json).collect::<Vec<_>>(),
            "included_block_ids":function.block_indices.iter().map(|&i|navigation.block_id(&navigation.blocks[i])).collect::<Vec<_>>(),
            "segments":segments,"blocks":blocks,"instruction_count":instruction_count,"context_count":function.contexts.len(),
            "boundary_status":if function.boundary_complete(){"static_reachable"}else{"incomplete"},"boundary_complete":function.boundary_complete(),
            "ownership_status":if function.cross_entries.is_empty(){"candidate"}else{"cross_entry_unknown"},"cross_entries":function.cross_entries,
            "unknown_successors":function.unknown_successors,"discovery_complete":null}),
        );
        if raw.is_some() {
            let instructions: Vec<_> = function
                .block_indices
                .iter()
                .flat_map(|&i| {
                    let b = &navigation.blocks[i];
                    navigation.instructions[b.instruction_start..b.instruction_end].iter()
                })
                .collect();
            detail.extend(object(json!({"carrier":navigation.carrier_detail(),"bytecode":navigation.bytecode_detail(false),
                "instructions":instructions,"unresolved_evidence":function.unknown_successors.iter().map(|e| {
                    let start = e.decoded_offset.min(navigation.carrier.bytecode.len());
                    let width = navigation.carrier.bytecode.get(start).map_or(0, |op| 1 + if (0x60..=0x7f).contains(op) { usize::from(*op - 0x5f) } else { 0 });
                    let end = start.saturating_add(width).min(navigation.carrier.bytecode.len());
                    let (carrier_start, carrier_end) = navigation.carrier.carrier_range(start, end);
                    json!({"decoded_start":start,"decoded_end":end,"carrier_start":carrier_start,"carrier_end":carrier_end,
                        "raw_hex":hex(&navigation.carrier.bytecode[start..end]),"reason":e.reason})
                }).collect::<Vec<_>>(),"control_flow":function.contexts.iter().enumerate().map(|(index,c)|json!({"context":index,
                    "block_id":navigation.block_id(&navigation.blocks[c.block_index]),"entry_state":c.entry.json(),"exit_state":c.exit.json(),
                    "edges":c.edges,"problems":c.problems})).collect::<Vec<_>>(),
                "reconstruction":crate::reconstruction::Reconstruction::not_requested(),
                "complete":true})));
        }
        let mut record = envelope::record(
            if fetch { "function_code" } else { "function" },
            RecordMeta {
                at: Some(&id),
                source_id: Some(&navigation.carrier.source_id),
                start: range.map(|r| r.0),
                end: range.map(|r| r.1),
                ..RecordMeta::default()
            },
            detail,
        );
        if fetch {
            record.as_object_mut().unwrap().extend(object(json!({"backend":"evm","command":"get-function","schema_version":envelope::SCHEMA_VERSION})));
        }
        record
    }

    pub(crate) fn prepare_record(
        &self,
        navigation: &Navigation,
        function: &FunctionCandidate,
        fetch: bool,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<()> {
        // Check a guaranteed lower bound on encoded payload BEFORE constructing nested
        // JSON arrays; a huge shared-owner record must not allocate its entire text first.
        let mut bytes = 0usize;
        let mut operations = 0usize;
        for &index in &function.block_indices {
            let block = &navigation.blocks[index];
            operations = operations.saturating_add(1 + self.owners[index].len());
            bytes = bytes.saturating_add(navigation.block_id(block).len() * 2);
            for &owner in &self.owners[index] {
                bytes = bytes
                    .saturating_add(self.function_id(navigation, &self.functions[owner]).len());
            }
            if fetch {
                bytes = bytes.saturating_add((block.end - block.start) * 2);
                for instruction in
                    &navigation.instructions[block.instruction_start..block.instruction_end]
                {
                    operations = operations.saturating_add(1);
                    bytes = bytes
                        .saturating_add(instruction.text.len() + instruction.operand_hex.len());
                }
            }
        }
        for state in &function.entry_states {
            operations = operations.saturating_add(state.stack.len() + state.memory.len());
            bytes = bytes.saturating_add(
                state
                    .stack
                    .iter()
                    .filter(|v| v.constant().is_some())
                    .count()
                    * 64
                    + state.memory.len() * 64,
            );
        }
        if fetch {
            operations = operations.saturating_add(function.unknown_successors.len());
            // Every evidence point carries at most a complete PUSH operand (33 bytes).
            for evidence in &function.unknown_successors {
                let start = evidence
                    .decoded_offset
                    .min(navigation.carrier.bytecode.len());
                let width = navigation.carrier.bytecode.get(start).map_or(0, |op| {
                    1 + if (0x60..=0x7f).contains(op) {
                        usize::from(*op - 0x5f)
                    } else {
                        0
                    }
                });
                bytes =
                    bytes.saturating_add(width.min(navigation.carrier.bytecode.len() - start) * 2);
            }
            for context in &function.contexts {
                operations = operations.saturating_add(
                    context.entry.stack.len()
                        + context.exit.stack.len()
                        + context.entry.memory.len()
                        + context.exit.memory.len()
                        + context.edges.len(),
                );
                for state in [&context.entry, &context.exit] {
                    bytes = bytes.saturating_add(
                        state
                            .stack
                            .iter()
                            .filter(|v| v.constant().is_some())
                            .count()
                            * 64
                            + state.memory.len() * 64,
                    );
                }
            }
        }
        work(perf, operations, limits.work)?;
        if bytes > limits.output_bytes {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "EVM output byte budget exceeded",
            ));
        }
        Ok(())
    }

    pub fn function_json(
        &self,
        navigation: &Navigation,
        selection: FunctionSelection<'_>,
        raw: bool,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<Vec<u8>> {
        let query = perf.start_stage();
        let preparation: Result<&FunctionCandidate> = (|| {
            let function = match selection {
                FunctionSelection::Id(id) => self.select(navigation, Some(id), None)?,
                FunctionSelection::Selector(selector) => {
                    self.select(navigation, None, Some(selector))?
                }
            };
            self.prepare_record(navigation, function, true, limits, perf)?;
            Ok(function)
        })();
        perf.record("query", query);
        let function = preparation?;
        perf.counters.records = Some(1);
        // Distinguish full-input decoding from selected raw target in existing sidecar fields.
        perf.counters.semantic_nodes = Some(function.contexts.len());
        perf.counters.matches = Some(
            function
                .block_indices
                .iter()
                .map(|&i| {
                    navigation.blocks[i].instruction_end - navigation.blocks[i].instruction_start
                })
                .sum(),
        );
        if limits.records == 0 {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "EVM result record budget exceeded",
            ));
        }
        let reconstruction = if raw {
            crate::reconstruction::Reconstruction::not_requested()
        } else {
            crate::reconstruction::reconstruct(navigation, function, limits, perf)?
        };
        let serialization = perf.start_stage();
        let result = (|| {
            let mut record = self.function_record(navigation, function, Some(raw));
            record["detail"]["reconstruction"] = serde_json::to_value(reconstruction)
                .map_err(|e| Error::new("FORMAT_ERROR", e.to_string()))?;
            let mut output = BoundedOutput {
                bytes: Vec::new(),
                limit: limits.output_bytes,
                exceeded: false,
            };
            write_record(&mut output, &record)?;
            Ok(output.bytes)
        })();
        perf.record("serialize", serialization);
        result
    }
}
