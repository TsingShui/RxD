//! Materialize values at original PCs; DUP shares IDs without expanding expressions.
//! Edges pass bottom-to-top stack arguments simultaneously.
use std::collections::BTreeSet;

use alloy_primitives::U256;
use rxd_core::{Error, Result, perf::Perf};
use serde::Serialize;

use crate::{AbstractValue, FunctionCandidate, Limits, MemoryWord, Navigation, functions::effect};

pub type ValueId = usize;

#[derive(Debug)]
pub enum Value {
    Constant(U256),
    Incoming(usize),
    CalldataWord0,
    Parameter {
        context: usize,
        slot: usize,
    },
    Operation {
        context: usize,
        pc: usize,
        name: &'static str,
        arguments: Vec<ValueId>,
    },
}

#[derive(Debug)]
pub enum Statement {
    Assign(ValueId),
    MemoryStore {
        offset: ValueId,
        value: ValueId,
        bytes: usize,
    },
    Copy {
        name: &'static str,
        destination: ValueId,
        source: ValueId,
        length: ValueId,
    },
    StorageStore {
        slot: ValueId,
        value: ValueId,
        transient: bool,
    },
}

#[derive(Debug)]
pub struct StackEdge {
    pub target_context: usize,
    /// Positional, bottom-to-top arguments, evaluated before any parameter update.
    pub arguments: Vec<ValueId>,
}

#[derive(Debug)]
pub enum Terminator {
    Stop,
    Invalid,
    Return {
        offset: ValueId,
        size: ValueId,
        revert: bool,
    },
    Jump(StackEdge),
    Branch {
        condition: ValueId,
        when_true: Option<StackEdge>,
        when_false: Option<StackEdge>,
    },
}

#[derive(Debug)]
pub struct ReconstructedBlock {
    pub context: usize,
    pub block_index: usize,
    pub parameters: Vec<ValueId>,
    pub statements: Vec<Statement>,
    pub terminator: Terminator,
}

/// EVM contextual IR with original block/context identities.
#[derive(Debug)]
pub struct ReconstructedGraph {
    pub entry: StackEdge,
    pub prelude_memory: Vec<(usize, MemoryWord)>,
    pub values: Vec<Value>,
    pub blocks: Vec<ReconstructedBlock>,
}

#[derive(Default, Clone, Copy, Serialize)]
pub struct ReconstructionCounts {
    pub reconstructed_instruction_count: usize,
    pub reconstructed_block_count: usize,
    pub reconstructed_context_count: usize,
    pub value_count: usize,
    pub work: usize,
    pub structured_context_count: usize,
    pub structured_edge_count: usize,
    pub structuring_work: usize,
    pub structured_loop_count: usize,
    pub structured_backedge_count: usize,
}

#[derive(Serialize)]
pub struct Reconstruction {
    pub engine: Engine,
    pub language: &'static str,
    pub status: &'static str,
    pub reason: Option<&'static str>,
    pub failure_offset: Option<usize>,
    pub complete: bool,
    pub text: Option<String>,
    pub control_flow_status: &'static str,
    pub structuring_reason: Option<&'static str>,
    #[serde(flatten)]
    pub counts: ReconstructionCounts,
}

#[derive(Serialize)]
pub struct Engine {
    pub name: &'static str,
    pub version: &'static str,
}

impl Reconstruction {
    pub fn not_requested() -> Self {
        Self {
            engine: Engine {
                name: "rxd-native",
                version: "0.1.0",
            },
            language: "evm-pseudocode",
            status: "not_requested",
            reason: None,
            failure_offset: None,
            complete: false,
            text: None,
            control_flow_status: "not_requested",
            structuring_reason: None,
            counts: ReconstructionCounts::default(),
        }
    }
}

enum Failure {
    Unsupported(&'static str, Option<usize>),
    Resource(Error),
}

impl From<Error> for Failure {
    fn from(value: Error) -> Self {
        Self::Resource(value)
    }
}

type BuildResult<T> = std::result::Result<T, Failure>;

struct Builder<'a> {
    navigation: &'a Navigation,
    function: &'a FunctionCandidate,
    limits: Limits,
    counts: ReconstructionCounts,
    values: Vec<Value>,
    visited_blocks: BTreeSet<usize>,
}

impl Builder<'_> {
    fn charge(&mut self, amount: usize) -> BuildResult<()> {
        self.counts.work = self.counts.work.saturating_add(amount);
        if self
            .limits
            .reconstruction_work
            .is_some_and(|limit| self.counts.work > limit)
        {
            return Err(Failure::Resource(Error::new(
                "RESOURCE_LIMIT",
                "EVM reconstruction work budget exceeded",
            )));
        }
        Ok(())
    }

    fn value(&mut self, value: Value) -> BuildResult<ValueId> {
        self.charge(1)?;
        if self.values.len() >= self.limits.values {
            return Err(Failure::Resource(Error::new(
                "RESOURCE_LIMIT",
                "EVM reconstruction value arena budget exceeded",
            )));
        }
        let id = self.values.len();
        self.values.push(value);
        self.counts.value_count = self.values.len();
        Ok(id)
    }

    fn edge(
        &mut self,
        target: Option<usize>,
        stack: &[ValueId],
        pc: usize,
    ) -> BuildResult<Option<StackEdge>> {
        let Some(target_context) = target else {
            return Ok(None);
        };
        let Some(context) = self.function.contexts.get(target_context) else {
            return Err(Failure::Unsupported("invalid_context_edge", Some(pc)));
        };
        if context.entry.stack.len() != stack.len() {
            return Err(Failure::Unsupported("inconsistent_stack_height", Some(pc)));
        }
        self.charge(1 + stack.len())?;
        Ok(Some(StackEdge {
            target_context,
            arguments: stack.to_vec(),
        }))
    }

    fn graph(&mut self) -> BuildResult<ReconstructedGraph> {
        if !self.function.boundary_complete() {
            let evidence = self.function.unknown_successors.first();
            return Err(Failure::Unsupported(
                evidence.map_or("incomplete_function_region", |e| e.reason),
                evidence.map(|e| e.decoded_offset),
            ));
        }
        self.charge(self.function.contexts.len())?;
        let entries: Vec<_> = self
            .function
            .contexts
            .iter()
            .enumerate()
            .filter(|(_, c)| self.navigation.blocks[c.block_index].start == self.function.entry)
            .map(|(i, _)| i)
            .collect();
        if entries.len() != 1 || self.function.entry_states.len() != 1 {
            return Err(Failure::Unsupported(
                "multiple_or_unknown_entry_states",
                Some(self.function.entry),
            ));
        }
        let entry_context = entries[0];
        let state = &self.function.entry_states[0];
        let mut entry_arguments = Vec::new();
        for (slot, value) in state.stack.iter().enumerate() {
            let value = match value {
                AbstractValue::Constant(value) => Value::Constant(*value),
                AbstractValue::CalldataWord0 => Value::CalldataWord0,
                _ => Value::Incoming(slot),
            };
            entry_arguments.push(self.value(value)?);
        }
        let entry = self
            .edge(Some(entry_context), &entry_arguments, self.function.entry)?
            .unwrap();
        self.charge(state.memory.len())?;
        let prelude_memory = state
            .memory
            .iter()
            .map(|(&offset, word)| (offset, word.clone()))
            .collect();
        let mut blocks = Vec::new();
        for context in 0..self.function.contexts.len() {
            blocks.push(self.block(context)?);
        }
        Ok(ReconstructedGraph {
            entry,
            prelude_memory,
            values: std::mem::take(&mut self.values),
            blocks,
        })
    }

    fn block(&mut self, context: usize) -> BuildResult<ReconstructedBlock> {
        let native_context = &self.function.contexts[context];
        let block_index = native_context.block_index;
        let native = &self.navigation.blocks[block_index];
        self.charge(1)?;
        self.counts.reconstructed_context_count += 1;
        self.visited_blocks.insert(block_index);
        self.counts.reconstructed_block_count = self.visited_blocks.len();
        let mut stack = Vec::new();
        for slot in 0..native_context.entry.stack.len() {
            stack.push(self.value(Value::Parameter { context, slot })?);
        }
        let parameters = stack.clone();
        let mut statements = Vec::new();
        let mut terminator = None;
        for instruction in
            &self.navigation.instructions[native.instruction_start..native.instruction_end]
        {
            let pc = instruction.decoded_offset;
            let op = instruction.opcode;
            self.charge(1)?;
            self.counts.reconstructed_instruction_count += 1;
            if (0x5f..=0x7f).contains(&op) {
                let value = U256::from_be_slice(
                    &self.navigation.carrier.bytecode[pc + 1..instruction.decoded_end],
                );
                stack.push(self.value(Value::Constant(value))?);
                continue;
            }
            if (0x80..=0x8f).contains(&op) {
                let depth = usize::from(op - 0x7f);
                if stack.len() < depth {
                    return Err(Failure::Unsupported("stack_underflow", Some(pc)));
                }
                stack.push(stack[stack.len() - depth]);
                continue;
            }
            if (0x90..=0x9f).contains(&op) {
                let depth = usize::from(op - 0x8f);
                let len = stack.len();
                if len <= depth {
                    return Err(Failure::Unsupported("stack_underflow", Some(pc)));
                }
                stack.swap(len - 1, len - 1 - depth);
                continue;
            }
            let Some((pops, _)) = effect(op) else {
                return Err(Failure::Unsupported("unsupported_opcode", Some(pc)));
            };
            if stack.len() < pops {
                return Err(Failure::Unsupported("stack_underflow", Some(pc)));
            }
            self.charge(pops)?;
            let arguments: Vec<_> = (0..pops).map(|_| stack.pop().unwrap()).collect();
            let operation = match op {
                0x00 => {
                    terminator = Some(Terminator::Stop);
                    None
                }
                0xfe => {
                    terminator = Some(Terminator::Invalid);
                    None
                }
                0x50 | 0x5b => None,
                0x52 | 0x53 => {
                    statements.push(Statement::MemoryStore {
                        offset: arguments[0],
                        value: arguments[1],
                        bytes: if op == 0x52 { 32 } else { 1 },
                    });
                    None
                }
                0x37 | 0x39 | 0x3e | 0x5e => {
                    let name = match op {
                        0x37 => "calldata_copy",
                        0x39 => "code_copy",
                        0x3e => "returndata_copy",
                        _ => "memory_copy",
                    };
                    statements.push(Statement::Copy {
                        name,
                        destination: arguments[0],
                        source: arguments[1],
                        length: arguments[2],
                    });
                    None
                }
                0x55 | 0x5d => {
                    statements.push(Statement::StorageStore {
                        slot: arguments[0],
                        value: arguments[1],
                        transient: op == 0x5d,
                    });
                    None
                }
                0xf3 | 0xfd => {
                    terminator = Some(Terminator::Return {
                        offset: arguments[0],
                        size: arguments[1],
                        revert: op == 0xfd,
                    });
                    None
                }
                0x56 => {
                    let edge = native_context.edges.iter().find(|e| e.kind == "jump");
                    let target = edge.and_then(|e| e.target_context);
                    let edge = self
                        .edge(target, &stack, pc)?
                        .ok_or(Failure::Unsupported("unresolved_jump", Some(pc)))?;
                    terminator = Some(Terminator::Jump(edge));
                    None
                }
                0x57 => {
                    let target = |kind| {
                        native_context
                            .edges
                            .iter()
                            .find(|e| e.kind == kind)
                            .and_then(|e| e.target_context)
                    };
                    let when_true = self.edge(target("branch_true"), &stack, pc)?;
                    let when_false = self.edge(target("branch_false"), &stack, pc)?;
                    terminator = Some(Terminator::Branch {
                        condition: arguments[1],
                        when_true,
                        when_false,
                    });
                    None
                }
                0x01 => Some("u256_add"),
                0x02 => Some("u256_mul"),
                0x03 => Some("u256_sub"),
                0x04 => Some("u256_div"),
                0x05 => Some("i256_div"),
                0x06 => Some("u256_mod"),
                0x07 => Some("i256_mod"),
                0x08 => Some("u256_addmod"),
                0x09 => Some("u256_mulmod"),
                0x0a => Some("u256_exp"),
                0x0b => Some("u256_signextend"),
                0x10 => Some("u256_lt"),
                0x11 => Some("u256_gt"),
                0x12 => Some("i256_lt"),
                0x13 => Some("i256_gt"),
                0x14 => Some("u256_eq"),
                0x15 => Some("u256_iszero"),
                0x16 => Some("u256_and"),
                0x17 => Some("u256_or"),
                0x18 => Some("u256_xor"),
                0x19 => Some("u256_not"),
                0x1a => Some("u256_byte"),
                0x1b => Some("u256_shl"),
                0x1c => Some("u256_shr"),
                0x1d => Some("i256_sar"),
                0x34 => Some("callvalue"),
                0x35 => Some("calldata_word"),
                0x36 => Some("calldata_size"),
                0x3d => Some("returndata_size"),
                0x5a => Some("gas_remaining"),
                0x51 => Some("memory_word"),
                0x54 => Some("persistent_storage"),
                0x5c => Some("transient_storage"),
                _ => return Err(Failure::Unsupported("unsupported_opcode", Some(pc))),
            };
            if let Some(name) = operation {
                let id = self.value(Value::Operation {
                    context,
                    pc,
                    name,
                    arguments,
                })?;
                statements.push(Statement::Assign(id));
                stack.push(id);
            }
            if terminator.is_some() {
                break;
            }
        }
        if terminator.is_none() {
            let edge = native_context
                .edges
                .iter()
                .find(|e| e.kind == "fallthrough");
            terminator = Some(
                if let Some(edge) =
                    self.edge(edge.and_then(|e| e.target_context), &stack, native.end)?
                {
                    Terminator::Jump(edge)
                } else {
                    Terminator::Stop
                },
            );
        }
        if stack.len() != native_context.exit.stack.len() {
            return Err(Failure::Unsupported(
                "inconsistent_exit_stack",
                Some(native.last_opcode_start),
            ));
        }
        Ok(ReconstructedBlock {
            context,
            block_index,
            parameters,
            statements,
            terminator: terminator.unwrap(),
        })
    }
}

struct Text<'a, 'b> {
    bytes: String,
    builder: &'a mut Builder<'b>,
}

impl Text<'_, '_> {
    fn append(&mut self, text: &str) -> BuildResult<()> {
        self.builder.charge(text.len())?;
        self.bytes.push_str(text);
        Ok(())
    }
}

pub fn value_name(value: &Value) -> String {
    match value {
        Value::Constant(value) => format!("0x{value:x}"),
        Value::Incoming(slot) => format!("incoming_stack[{slot}]"),
        Value::CalldataWord0 => "calldata_word(0x0)".to_owned(),
        Value::Parameter { context, slot } => format!("s_{context}_{slot}"),
        Value::Operation { context, pc, .. } => format!("v_{pc}_c{context}"),
    }
}

fn edge_text(graph: &ReconstructedGraph, edge: &StackEdge) -> String {
    let args = edge
        .arguments
        .iter()
        .map(|&id| value_name(&graph.values[id]))
        .collect::<Vec<_>>()
        .join(", ");
    format!("goto c_{}({args});", edge.target_context)
}

fn statement_text(graph: &ReconstructedGraph, statement: &Statement) -> String {
    let name = |id| value_name(&graph.values[id]);
    match statement {
        Statement::Assign(id) => {
            let Value::Operation {
                name: operation,
                arguments,
                ..
            } = &graph.values[*id]
            else {
                unreachable!("assign is a materialized operation");
            };
            let args = arguments
                .iter()
                .map(|&id| name(id))
                .collect::<Vec<_>>()
                .join(", ");
            let expression = if matches!(
                *operation,
                "memory_word" | "persistent_storage" | "transient_storage"
            ) {
                format!("{operation}[{args}]")
            } else {
                format!("{operation}({args})")
            };
            format!("let {} = {expression};", name(*id))
        }
        Statement::MemoryStore {
            offset,
            value,
            bytes,
        } => {
            if *bytes == 32 {
                format!("memory_word[{}] = {};", name(*offset), name(*value))
            } else {
                format!(
                    "memory_byte[{}] = low_byte({});",
                    name(*offset),
                    name(*value)
                )
            }
        }
        Statement::Copy {
            name: operation,
            destination,
            source,
            length,
        } => format!(
            "{operation}(destination={}, source={}, length={});",
            name(*destination),
            name(*source),
            name(*length)
        ),
        Statement::StorageStore {
            slot,
            value,
            transient,
        } => format!(
            "{}_storage[{}] = {};",
            if *transient {
                "transient"
            } else {
                "persistent"
            },
            name(*slot),
            name(*value)
        ),
    }
}

fn terminal_text(graph: &ReconstructedGraph, terminal: &Terminator) -> Option<String> {
    let name = |id| value_name(&graph.values[id]);
    match terminal {
        Terminator::Stop => Some("stop;".to_owned()),
        Terminator::Invalid => Some("invalid;".to_owned()),
        Terminator::Return {
            offset,
            size,
            revert,
        } => Some(format!(
            "{} memory_slice({}, {});",
            if *revert { "revert" } else { "return" },
            name(*offset),
            name(*size)
        )),
        _ => None,
    }
}

fn render_structured(
    builder: &mut Builder<'_>,
    graph: &ReconstructedGraph,
    steps: &[crate::structuring::StructuredStep],
) -> BuildResult<String> {
    use crate::structuring::StructuredStep;
    let selector = builder.function.selector.clone();
    let mut text = Text {
        bytes: String::new(),
        builder,
    };
    text.append(&format!("evm_function(0x{selector}) {{\n"))?;
    for (offset, word) in &graph.prelude_memory {
        text.append(&format!(
            "  assume_incoming_memory_word(0x{offset:x}, 0x{:x}); // dispatcher pc {}\n",
            word.value, word.write_pc
        ))?;
    }
    for block in &graph.blocks {
        if !block.parameters.is_empty() {
            let names = block
                .parameters
                .iter()
                .map(|&id| value_name(&graph.values[id]))
                .collect::<Vec<_>>()
                .join(", ");
            text.append(&format!("  word {names};\n"))?;
        }
    }
    let mut indentation = 1usize;
    for step in steps {
        let indent = "  ".repeat(indentation);
        match *step {
            StructuredStep::Block(context) => {
                let block = &graph.blocks[context];
                let pc = text.builder.navigation.blocks[block.block_index].start;
                text.append(&format!("{indent}// c_{context}: pc {pc}\n"))?;
                for statement in &block.statements {
                    text.append(&format!("{indent}{}\n", statement_text(graph, statement)))?;
                }
                if let Some(terminal) = terminal_text(graph, &block.terminator) {
                    text.append(&format!("{indent}{terminal}\n"))?;
                }
            }
            StructuredStep::Transfer(location) => {
                let edge = location.edge(graph).expect("structured plan edge");
                let target = edge.target_context;
                let source = location
                    .source()
                    .map_or("entry".to_owned(), |c| format!("c_{c}"));
                let comment = format!("simultaneous edge {source} -> c_{target}");
                if edge.arguments.is_empty() {
                    text.append(&format!("{indent}// {comment}\n"))?;
                } else {
                    let parameters = graph.blocks[target]
                        .parameters
                        .iter()
                        .map(|&id| value_name(&graph.values[id]))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let arguments = edge
                        .arguments
                        .iter()
                        .map(|&id| value_name(&graph.values[id]))
                        .collect::<Vec<_>>()
                        .join(", ");
                    text.append(&format!(
                        "{indent}({parameters}) = ({arguments}); // {comment}\n"
                    ))?;
                }
            }
            StructuredStep::If { condition, .. } => {
                text.append(&format!(
                    "{indent}if ({} != 0) {{\n",
                    value_name(&graph.values[condition])
                ))?;
                indentation += 1;
            }
            StructuredStep::Else => {
                indentation -= 1;
                text.append(&format!("{}}} else {{\n", "  ".repeat(indentation)))?;
                indentation += 1;
            }
            StructuredStep::EndIf => {
                indentation -= 1;
                text.append(&format!("{}}}\n", "  ".repeat(indentation)))?;
            }
            StructuredStep::Loop => {
                text.append(&format!("{indent}while (true) {{\n"))?;
                indentation += 1;
            }
            StructuredStep::EndLoop => {
                indentation -= 1;
                text.append(&format!("{}}}\n", "  ".repeat(indentation)))?;
            }
            StructuredStep::Break => text.append(&format!("{indent}break;\n"))?,
            StructuredStep::Continue => text.append(&format!("{indent}continue;\n"))?,
            StructuredStep::Stop => text.append(&format!("{indent}stop;\n"))?,
        }
    }
    text.append("}\n")?;
    Ok(text.bytes)
}

fn render(builder: &mut Builder<'_>, graph: &ReconstructedGraph) -> BuildResult<String> {
    let selector = builder.function.selector.clone();
    let mut text = Text {
        bytes: String::new(),
        builder,
    };
    text.append(&format!("evm_function(0x{selector}) {{\n"))?;
    for (offset, word) in &graph.prelude_memory {
        text.append(&format!(
            "  assume_incoming_memory_word(0x{offset:x}, 0x{:x}); // dispatcher pc {}\n",
            word.value, word.write_pc
        ))?;
    }
    text.append(&format!("  {}\n", edge_text(graph, &graph.entry)))?;
    for block in &graph.blocks {
        let parameters = block
            .parameters
            .iter()
            .map(|&id| value_name(&graph.values[id]))
            .collect::<Vec<_>>()
            .join(", ");
        let pc = text.builder.navigation.blocks[block.block_index].start;
        text.append(&format!("c_{}({parameters}): // pc {pc}\n", block.context))?;
        for statement in &block.statements {
            let line = statement_text(graph, statement);
            text.append(&format!("  {line}\n"))?;
        }
        let name = |id| value_name(&graph.values[id]);
        let terminal = match &block.terminator {
            terminal @ (Terminator::Stop | Terminator::Invalid | Terminator::Return { .. }) => {
                terminal_text(graph, terminal).expect("terminal without outgoing edges")
            }
            Terminator::Jump(edge) => edge_text(graph, edge),
            Terminator::Branch {
                condition,
                when_true,
                when_false,
            } => {
                let edge = |edge: &Option<StackEdge>| {
                    edge.as_ref()
                        .map_or("stop;".to_owned(), |e| edge_text(graph, e))
                };
                format!(
                    "if ({} != 0) {{ {} }} else {{ {} }}",
                    name(*condition),
                    edge(when_true),
                    edge(when_false)
                )
            }
        };
        text.append(&format!("  {terminal}\n"))?;
    }
    text.append("}\n")?;
    Ok(text.bytes)
}

/// Selected-context lowering; unsupported graphs retain counts but no partial IR.
pub struct LoweringOutcome {
    pub graph: Option<ReconstructedGraph>,
    pub reason: Option<&'static str>,
    pub failure_offset: Option<usize>,
    pub counts: ReconstructionCounts,
}

fn record_counts(perf: &mut Perf, counts: ReconstructionCounts) {
    perf.counters.reconstructed_instructions = Some(counts.reconstructed_instruction_count);
    perf.counters.reconstructed_blocks = Some(counts.reconstructed_block_count);
    perf.counters.reconstructed_contexts = Some(counts.reconstructed_context_count);
    perf.counters.reconstruction_values = Some(counts.value_count);
    perf.counters.reconstruction_work = Some(counts.work);
    perf.counters.structured_contexts = Some(counts.structured_context_count);
    perf.counters.structured_edges = Some(counts.structured_edge_count);
    perf.counters.structuring_work = Some(counts.structuring_work);
    perf.counters.structured_loops = Some(counts.structured_loop_count);
    perf.counters.structured_backedges = Some(counts.structured_backedge_count);
}

pub fn lower_selected(
    navigation: &Navigation,
    function: &FunctionCandidate,
    limits: Limits,
    perf: &mut Perf,
) -> Result<LoweringOutcome> {
    let stage = perf.start_stage();
    let mut builder = Builder {
        navigation,
        function,
        limits,
        counts: ReconstructionCounts::default(),
        values: Vec::new(),
        visited_blocks: BTreeSet::new(),
    };
    let result = builder.graph();
    perf.record("reconstruct", stage);
    record_counts(perf, builder.counts);
    let mut outcome = LoweringOutcome {
        graph: None,
        reason: None,
        failure_offset: None,
        counts: builder.counts,
    };
    match result {
        Ok(graph) => outcome.graph = Some(graph),
        Err(Failure::Unsupported(reason, offset)) => {
            outcome.reason = Some(reason);
            outcome.failure_offset = offset;
        }
        Err(Failure::Resource(error)) => return Err(error),
    }
    Ok(outcome)
}

pub fn reconstruct(
    navigation: &Navigation,
    function: &FunctionCandidate,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Reconstruction> {
    let lowered = lower_selected(navigation, function, limits, perf)?;
    let mut output = Reconstruction::not_requested();
    output.counts = lowered.counts;
    if let Some(graph) = lowered.graph {
        let stage = perf.start_stage();
        let mut builder = Builder {
            navigation,
            function,
            limits,
            counts: lowered.counts,
            values: Vec::new(),
            visited_blocks: BTreeSet::new(),
        };
        let structured_result =
            crate::structuring::structure_acyclic(&graph, limits, &mut builder.counts);
        perf.record("structure", stage);
        record_counts(perf, builder.counts);
        let structured = structured_result?;
        let stage = perf.start_stage();
        let control_flow_status =
            if structured.steps.is_some() && builder.counts.structured_loop_count > 0 {
                "structured_loops"
            } else if structured.steps.is_some() {
                "structured_acyclic"
            } else {
                "contextual_gotos"
            };
        output.structuring_reason = structured.reason;
        let result = if let Some(steps) = &structured.steps {
            render_structured(&mut builder, &graph, steps)
        } else {
            render(&mut builder, &graph)
        };
        perf.record("reconstruct", stage);
        record_counts(perf, builder.counts);
        output.counts = builder.counts;
        match result {
            Ok(text) => {
                output.status = "success";
                output.complete = true;
                output.text = Some(text);
                output.control_flow_status = control_flow_status;
            }
            Err(Failure::Resource(error)) => return Err(error),
            Err(Failure::Unsupported(reason, offset)) => {
                output.status = "raw_fallback";
                output.reason = Some(reason);
                output.failure_offset = offset;
                output.control_flow_status = "unavailable";
            }
        }
    } else {
        output.status = "raw_fallback";
        output.reason = lowered.reason;
        output.failure_offset = lowered.failure_offset;
        output.control_flow_status = "unavailable";
    }
    Ok(output)
}
