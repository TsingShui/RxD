//! JVM instructions -> the backend IR. Stack positions are virtual registers;
//! method arguments occupy the tail of the register file, which is where the
//! renderer expects parameters.
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use radguard::backend::{
    ArrayLoadExpression, ArrayStoreStatement, AssignKind, BinaryExpression, BinaryOp, BlockKind,
    CheckCastExpression, Constant as BackendConstant, ConstantValue, FieldReference,
    InstanceFieldExpression, InstancePutStatement, InvokeExpression, InvokeKind, LoweredBlock,
    LoweredMethod, MethodReference, NewArrayExpression, Statement, UnaryExpression, UnaryOp, Value,
    VariableId, VariableTable,
};

use crate::classfile::{
    Constant, Handler, Reader, constant, field_type, method_type, type_for_class,
};
use crate::types::{Primitive, Type};
use crate::{ClassFile, Limits, Method};

// Up to 12 moves implement a JVM stack permutation. Addresses remain ordered
// and unique; dividing by STRIDE recovers the original JVM byte offset.
const STRIDE: u32 = 32;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Instruction {
    pub byte_offset: u32,
    pub opcode: u8,
    pub mnemonic: &'static str,
    pub bytes: Vec<u8>,
    pub operands: Vec<i32>,
    pub targets: Vec<u32>,
    pub constant_index: Option<u16>,
    pub reference: Option<String>,
    #[serde(skip)]
    operation: u8,
}

pub fn bytecode(class: &ClassFile, method: &Method) -> Result<Vec<Instruction>> {
    let Some(bytes) = class.bytecode(method) else {
        return Ok(Vec::new());
    };
    let mut r = Reader::new(bytes);
    let mut result = Vec::new();
    let mut decoded_bytes = 0usize;
    while r.pos < bytes.len() {
        let offset = r.pos;
        let opcode = r.u1()?;
        ensure!(
            opcode < 0xca,
            "INVALID_INPUT: reserved JVM opcode 0x{opcode:02x} at {offset}"
        );
        let mut insn = Instruction {
            byte_offset: offset as u32,
            opcode,
            mnemonic: crate::opcodes::NAMES[opcode as usize],
            bytes: Vec::new(),
            operands: Vec::new(),
            targets: Vec::new(),
            constant_index: None,
            reference: None,
            operation: opcode,
        };
        match opcode {
            0x10 => insn.operands.push(i32::from(r.u1()? as i8)),
            0x11 => insn.operands.push(i32::from(r.u2()? as i16)),
            0x12 => insn.constant_index = Some(u16::from(r.u1()?)),
            0x13 | 0x14 | 0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                insn.constant_index = Some(r.u2()?)
            }
            0x15..=0x19 | 0x36..=0x3a | 0xa9 | 0xbc => insn.operands.push(i32::from(r.u1()?)),
            0x84 => {
                insn.operands.push(i32::from(r.u1()?));
                insn.operands.push(i32::from(r.u1()? as i8));
            }
            0x99..=0xa8 | 0xc6 | 0xc7 => {
                let delta = i32::from(r.u2()? as i16);
                insn.operands.push(delta);
                insn.targets.push(target(offset, delta, bytes.len())?);
            }
            0xc8 | 0xc9 => {
                let delta = r.u4()? as i32;
                insn.operands.push(delta);
                insn.targets.push(target(offset, delta, bytes.len())?);
            }
            0xaa | 0xab => {
                while !r.pos.is_multiple_of(4) {
                    r.u1()?; // JVMS specifies alignment, not padding values.
                }
                insn.targets
                    .push(target(offset, r.u4()? as i32, bytes.len())?);
                if opcode == 0xaa {
                    let low = r.u4()? as i32;
                    let high = r.u4()? as i32;
                    let count = i64::from(high) - i64::from(low) + 1;
                    ensure!(
                        count > 0 && count <= ((bytes.len() - r.pos) / 4) as i64,
                        "INVALID_INPUT: tableswitch bounds"
                    );
                    for key in i64::from(low)..=i64::from(high) {
                        insn.operands.push(key as i32);
                        insn.targets
                            .push(target(offset, r.u4()? as i32, bytes.len())?);
                    }
                } else {
                    let count = r.u4()? as i32;
                    ensure!(
                        count >= 0 && count as usize <= (bytes.len() - r.pos) / 8,
                        "INVALID_INPUT: lookupswitch size"
                    );
                    for _ in 0..count {
                        let key = r.u4()? as i32;
                        ensure!(
                            insn.operands.last().is_none_or(|old| *old < key),
                            "INVALID_INPUT: unsorted lookupswitch keys"
                        );
                        insn.operands.push(key);
                        insn.targets
                            .push(target(offset, r.u4()? as i32, bytes.len())?);
                    }
                }
            }
            0xb9 => {
                insn.constant_index = Some(r.u2()?);
                let count = r.u1()?;
                ensure!(
                    count > 0 && r.u1()? == 0,
                    "INVALID_INPUT: invokeinterface operands"
                );
                insn.operands.push(i32::from(count));
            }
            0xba => {
                insn.constant_index = Some(r.u2()?);
                ensure!(r.u2()? == 0, "INVALID_INPUT: invokedynamic operands");
            }
            0xc4 => {
                let op = r.u1()?;
                ensure!(
                    matches!(op, 0x15..=0x19 | 0x36..=0x3a | 0x84 | 0xa9),
                    "INVALID_INPUT: invalid wide opcode"
                );
                insn.operation = op;
                insn.operands.push(i32::from(r.u2()?));
                if op == 0x84 {
                    insn.operands.push(i32::from(r.u2()? as i16));
                }
            }
            0xc5 => {
                insn.constant_index = Some(r.u2()?);
                let dimensions = r.u1()?;
                ensure!(dimensions > 0, "INVALID_INPUT: zero array dimensions");
                insn.operands.push(i32::from(dimensions));
            }
            _ => {}
        }
        if let Some(index) = insn.constant_index {
            constant(&class.pool, index)?;
            insn.reference = reference(class, index)?;
        }
        insn.bytes = bytes[offset..r.pos].to_vec();
        decoded_bytes += 2 * std::mem::size_of::<Instruction>()
            + insn.bytes.capacity()
            + insn.operands.capacity() * 4
            + insn.targets.capacity() * 4
            + insn.reference.as_ref().map_or(0, String::capacity);
        ensure!(
            decoded_bytes <= 16 * 1024 * 1024,
            "RESOURCE_LIMIT: decoded instruction budget exceeded"
        );
        result.push(insn);
    }
    let boundaries: BTreeMap<u32, usize> = result
        .iter()
        .enumerate()
        .map(|(i, op)| (op.byte_offset, i))
        .collect();
    for op in &result {
        for target in &op.targets {
            ensure!(
                boundaries.contains_key(target),
                "INVALID_INPUT: branch target {target} is not an instruction boundary"
            );
        }
    }
    if let Some(code) = &method.code {
        for h in &code.handlers {
            ensure!(
                boundaries.contains_key(&u32::from(h.start))
                    && (usize::from(h.end) == bytes.len()
                        || boundaries.contains_key(&u32::from(h.end)))
                    && boundaries.contains_key(&u32::from(h.target)),
                "INVALID_INPUT: exception boundary is not an instruction boundary"
            );
        }
    }
    Ok(result)
}

fn target(offset: usize, delta: i32, length: usize) -> Result<u32> {
    let value = offset as i64 + i64::from(delta);
    ensure!(
        value >= 0 && value < length as i64,
        "INVALID_INPUT: branch target out of bounds at {offset}"
    );
    Ok(value as u32)
}

fn reference(class: &ClassFile, index: u16) -> Result<Option<String>> {
    Ok(match constant(&class.pool, index)? {
        Constant::Member { .. } => {
            let (_, owner, name, descriptor) = class.member(index)?;
            Some(format!("{owner}.{name}{descriptor}"))
        }
        Constant::Class(_) => Some(class.class_type(index)?.descriptor().into_owned()),
        Constant::Dynamic {
            tag,
            bootstrap,
            name_type,
        } => Some(format!(
            "dynamic:{tag}:bootstrap:{bootstrap}:name_type:{name_type}"
        )),
        Constant::Handle(kind, member) => {
            ensure!((1..=9).contains(kind), "INVALID_INPUT: method handle kind");
            constant(&class.pool, *member)?;
            Some(format!("handle:{kind}:{member}"))
        }
        Constant::MethodType(descriptor) | Constant::Module(descriptor) => {
            constant(&class.pool, *descriptor)?;
            Some(format!("constant:{descriptor}"))
        }
        _ => None,
    })
}

/// Complete selected-method decoding. Unsupported operations return an error;
/// no guessed Java or partially decoded CFG is returned.
pub fn decode_method(class: &ClassFile, method: &Method, limits: Limits) -> Result<LoweredMethod> {
    let code = method
        .code
        .as_ref()
        .context("INVALID_QUERY: method has no Code attribute")?;
    let instructions = bytecode(class, method)?;
    let descriptor = method_type(&method.descriptor)?;
    let words = descriptor.parameters.iter().map(Type::width).sum::<u32>()
        + u32::from(method.access & 8 == 0);
    ensure!(
        words <= 255,
        "INVALID_INPUT: method parameter word limit exceeded"
    );
    ensure!(
        words <= u32::from(code.max_locals),
        "INVALID_INPUT: parameters exceed max_locals"
    );
    let registers = u32::from(code.max_stack) + 6 + u32::from(code.max_locals) + words;
    let first_input = registers - words;
    // A handler is entered with the exception on the stack, and the same stack
    // slot can hold anything elsewhere in the method, so the exception gets a
    // register of its own: the catch clause renders its type, and clobbering a
    // shared slot would retype an unrelated value.
    let exception_register = registers;
    // Stack registers are keyed by `(depth, type)` rather than depth alone, so a
    // slot that holds an `int` at one point and an object at another gets two
    // registers and two declarations. Values of the same type at the same depth
    // still share a register, which is what keeps a value defined before a join
    // readable after it.
    let mut stack_registers: BTreeMap<(u32, &'static str), u32> = BTreeMap::new();
    let mut next_stack_register = exception_register + 1;
    let local = |slot: u32| {
        if slot < words {
            first_input + slot
        } else {
            u32::from(code.max_stack) + 6 + slot
        }
    };
    let offsets: BTreeMap<u32, usize> = instructions
        .iter()
        .enumerate()
        .map(|(i, op)| (op.byte_offset, i))
        .collect();
    // The register file is declared up front: the receiver and parameters occupy
    // the tail (declared parameters are what the header renders), everything else
    // is a stack slot or a local.
    let mut variables = VariableTable::default();
    let mut register = first_input;
    if method.access & 8 == 0 {
        let receiver = variables.register(register as u16);
        variables.set_declared(receiver, true);
        variables.set_is_this(receiver, true);
        variables.get_mut(receiver).expect("receiver").ty =
            Some(Cow::Owned(format!("L{};", class.name)));
        register += 1;
    }
    for (index, parameter) in descriptor.parameters.iter().enumerate() {
        let id = variables.register(register as u16);
        variables.set_declared(id, true);
        variables.get_mut(id).expect("parameter").ty =
            Some(Cow::Owned(parameter.descriptor().into_owned()));
        if let Some(Some(name)) = method
            .parameter_names
            .get(index)
            .filter(|name| name.as_ref().is_some_and(|name| !name.is_empty()))
        {
            variables.set_name(id, name);
        }
        register += parameter.width();
    }
    let mut states: Vec<Option<Vec<Type>>> = vec![None; instructions.len()];
    let mut bodies: Vec<Vec<Statement>> = vec![Vec::new(); instructions.len()];
    let mut terminators: Vec<Terminator> = vec![Terminator::FallThrough; instructions.len()];
    let mut queue = VecDeque::new();
    let mut remaining = limits.work;
    merge(&mut states[0], &[], &mut queue, 0, &mut remaining)?;
    for h in &code.handlers {
        let index = offsets[&u32::from(h.target)];
        let ty = h
            .catch_type
            .as_deref()
            .map(type_for_class)
            .transpose()?
            .unwrap_or_else(|| Type::object("java/lang/Throwable"));
        merge(&mut states[index], &[ty], &mut queue, index, &mut remaining)?;
    }
    while let Some(index) = queue.pop_front() {
        charge(
            &mut remaining,
            1 + states[index]
                .as_ref()
                .unwrap()
                .iter()
                .map(Type::cost)
                .sum::<usize>(),
        )?;
        let op = &instructions[index];
        let mut lift = Lift {
            class,
            stack: states[index].clone().unwrap(),
            statements: Vec::new(),
            terminator: Terminator::FallThrough,
            variables: &mut variables,
            max_stack: u32::from(code.max_stack),
            max_locals: u32::from(code.max_locals),
            local: &local,
            return_type: descriptor.return_type.clone(),
            stack_registers: &mut stack_registers,
            next_stack_register: &mut next_stack_register,
            exception_slot: code
                .handlers
                .iter()
                .any(|h| u32::from(h.target) == op.byte_offset)
                .then_some(exception_register),
        };
        if let Some(handler) = code
            .handlers
            .iter()
            .find(|h| u32::from(h.target) == op.byte_offset)
        {
            // A handler is entered with the exception on the stack; the backend
            // wants it bound by an explicit `move-exception`, and the stack slot
            // is register 0 because the seeded state has exactly one entry.
            let caught = handler
                .catch_type
                .as_deref()
                .map(type_for_class)
                .transpose()?
                .unwrap_or_else(|| Type::object("java/lang/Throwable"));
            let descriptor = caught.descriptor().into_owned();
            let lhs = lift.retyped(exception_register, &caught);
            lift.statements.push(Statement::MoveException {
                lhs,
                ty: Some(descriptor),
            });
        }
        lift.instruction(op).with_context(|| {
            format!(
                "{}.{}{} at JVM byte {}",
                class.name, method.name, method.descriptor, op.byte_offset
            )
        })?;
        if lift.statements.is_empty() && matches!(lift.terminator, Terminator::FallThrough) {
            lift.statements.push(Statement::Nop);
        }
        let Lift {
            mut statements,
            terminator,
            ..
        } = lift;
        if statements.is_empty() {
            statements.push(Statement::Nop);
        }
        bodies[index] = statements;
        terminators[index] = terminator;
        let terminal = matches!(op.operation, 0xac..=0xb1 | 0xbf | 0xa7 | 0xc8 | 0xaa | 0xab);
        if !terminal {
            ensure!(
                index + 1 < instructions.len(),
                "INVALID_INPUT: method falls through past Code"
            );
            merge(
                &mut states[index + 1],
                &lift.stack,
                &mut queue,
                index + 1,
                &mut remaining,
            )?;
        }
        for target in &op.targets {
            let next = offsets[target];
            merge(
                &mut states[next],
                &lift.stack,
                &mut queue,
                next,
                &mut remaining,
            )?;
        }
    }
    // Source names from the LocalVariableTable, so the rendering keeps the
    // identifiers the original source used. A slot reused by disjoint ranges
    // keeps the last name, which is what a reader expects to see.
    for local_variable in &code.locals {
        let register = local(u32::from(local_variable.slot));
        let id = variables.register(register as u16);
        // The table is the original source's own view of this slot, so it wins
        // over the type the stack model happens to carry.
        if let Ok(ty) = field_type(&local_variable.descriptor) {
            variables.get_mut(id).expect("registered variable").ty =
                Some(Cow::Owned(ty.descriptor().into_owned()));
        }
        if !local_variable.name.is_empty() {
            variables.set_name(id, &local_variable.name);
        }
    }
    fold_constructor_calls(&mut bodies);
    let blocks = build_blocks(&instructions, &bodies, &terminators, &code.handlers)?;
    Ok(LoweredMethod { variables, blocks })
}

/// Folds `new X` plus the `<init>` call that initializes it into one call.
///
/// The JVM creates an object with `new` and initializes it with a later
/// `invokespecial <init>`, usually after `dup`. The backend renders a constructor
/// call as an invoke whose base *is* the instance it builds, so the creation and
/// the copies made before the call are consumed by it:
/// `v = new X` + `w = v` + `v.<init>(args)` becomes `w = new X(args)`.
fn fold_constructor_calls(bodies: &mut [Vec<Statement>]) {
    let mut creations = Vec::new();
    for (instruction, statements) in bodies.iter().enumerate() {
        for (position, statement) in statements.iter().enumerate() {
            if let Statement::Assign {
                lhs: Some(lhs),
                rhs: Value::NewInstance { ty },
                ..
            } = statement
            {
                creations.push((instruction, position, *lhs, ty.clone()));
            }
        }
    }
    for (creation, position, source, ty) in creations {
        // Copies made before the call are the `dup`s of the uninitialized
        // reference; a copy after it is a use of the initialized object.
        let mut aliases = vec![source];
        let mut consumed = vec![(creation, position)];
        let mut call = None;
        'outer: for (instruction, statements) in bodies.iter().enumerate() {
            for (position, statement) in statements.iter().enumerate() {
                if (instruction, position) <= (creation, position) {
                    continue;
                }
                match statement {
                    Statement::Assign {
                        lhs: Some(lhs),
                        rhs: Value::Variable(alias),
                        kind: AssignKind::Move,
                    } if aliases.contains(alias) => {
                        aliases.push(*lhs);
                        consumed.push((instruction, position));
                    }
                    Statement::Assign {
                        rhs: Value::Invoke(invoke),
                        ..
                    } if invoke.method.name == "<init>"
                        && matches!(&invoke.base, Value::Variable(base) if aliases.contains(base)) =>
                    {
                        call = Some((instruction, position));
                        break 'outer;
                    }
                    _ => {}
                }
            }
        }
        let Some((instruction, position)) = call else {
            continue;
        };
        // `<init>` runs on the top of the stack, and the reference that survives
        // is the one still read afterwards - not necessarily the base itself.
        let live: std::collections::HashSet<_> = bodies[instruction..]
            .iter()
            .flat_map(|statements| statements.iter())
            .flat_map(Statement::used_variables)
            .collect();
        let survivor = aliases.iter().copied().find(|alias| live.contains(alias));
        let Statement::Assign { lhs, rhs, .. } = &mut bodies[instruction][position] else {
            continue;
        };
        let Value::Invoke(invoke) = rhs else { continue };
        invoke.base = Value::NewInstance { ty };
        *lhs = survivor;
        for (instruction, position) in consumed {
            bodies[instruction][position] = Statement::Nop;
        }
    }
}

/// What one instruction does to control flow, in JVM byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Terminator {
    FallThrough,
    Goto(u32),
    /// Conditional jump: the target is taken when the condition holds, the
    /// lexically next instruction otherwise.
    Branch(u32),
    /// The cases live in the `Statement::Switch`; this carries the edges.
    Switch {
        default: u32,
        cases: Vec<u32>,
    },
    Return,
    Throw,
}

/// Partitions the lowered instructions into blocks the backend can structure.
///
/// Leaders are branch targets, the instruction after a branch, handler and
/// handler-range boundaries, and the entry. Blocks carry the statements of every
/// instruction they cover, their edges, and the handler edges of the try ranges
/// they fall in.
fn build_blocks(
    instructions: &[Instruction],
    bodies: &[Vec<Statement>],
    terminators: &[Terminator],
    handlers: &[Handler],
) -> Result<Vec<LoweredBlock>> {
    let offsets: BTreeMap<u32, usize> = instructions
        .iter()
        .enumerate()
        .map(|(index, op)| (op.byte_offset, index))
        .collect();
    let index_of = |offset: u32| offsets.get(&offset).copied();

    let mut leaders = BTreeSet::new();
    leaders.insert(0usize);
    for (index, terminator) in terminators.iter().enumerate() {
        match terminator {
            Terminator::Goto(target) | Terminator::Branch(target) => {
                if let Some(target) = index_of(*target) {
                    leaders.insert(target);
                }
            }
            Terminator::Switch { default, cases } => {
                if let Some(target) = index_of(*default) {
                    leaders.insert(target);
                }
                for case in cases {
                    if let Some(target) = index_of(*case) {
                        leaders.insert(target);
                    }
                }
            }
            Terminator::FallThrough | Terminator::Return | Terminator::Throw => {}
        }
        if !matches!(terminator, Terminator::FallThrough) && index + 1 < instructions.len() {
            leaders.insert(index + 1);
        }
    }
    for handler in handlers {
        for offset in [
            u32::from(handler.start),
            u32::from(handler.end),
            u32::from(handler.target),
        ] {
            if let Some(index) = index_of(offset) {
                leaders.insert(index);
            }
        }
    }
    let leaders: Vec<usize> = leaders.into_iter().collect();
    let bound = |block: usize| {
        leaders
            .get(block + 1)
            .copied()
            .unwrap_or(instructions.len())
    };

    let mut block_of = vec![0usize; instructions.len()];
    for (block, &start) in leaders.iter().enumerate() {
        for slot in block_of.iter_mut().take(bound(block)).skip(start) {
            *slot = block;
        }
    }

    let mut blocks: Vec<LoweredBlock> = leaders
        .iter()
        .enumerate()
        .map(|(block, &start)| {
            let end = bound(block);
            let mut statements = Vec::new();
            for body in bodies.iter().take(end).skip(start) {
                statements.extend(body.iter().cloned());
            }
            LoweredBlock {
                start: instructions[start].byte_offset * STRIDE,
                end: instructions[end - 1].byte_offset * STRIDE + STRIDE,
                kind: BlockKind::Statement,
                statements,
                predecessors: Vec::new(),
                successors: Vec::new(),
                exception_successors: Vec::new(),
                exception_handlers: Vec::new(),
                branch_true: None,
                branch_false: None,
                exception: None,
            }
        })
        .collect();

    for (block, _) in leaders.iter().enumerate() {
        let end = bound(block);
        let next_block = (end < instructions.len()).then_some(block + 1);
        match terminators[end - 1].clone() {
            Terminator::FallThrough => {
                if let Some(next) = next_block {
                    blocks[block].successors.push(next);
                }
            }
            Terminator::Goto(target) => {
                if let Some(target) = index_of(target) {
                    blocks[block].successors.push(block_of[target]);
                }
            }
            Terminator::Branch(target) => {
                blocks[block].kind = BlockKind::Condition;
                let taken = index_of(target).map(|index| block_of[index]);
                // The renderer reads a condition block as
                // `[fallthrough, target]`, false then true.
                let false_block = next_block.or(taken);
                if let Some(false_block) = false_block {
                    blocks[block].successors.push(false_block);
                    blocks[block].branch_false = Some(false_block);
                }
                if let Some(taken) = taken {
                    blocks[block].successors.push(taken);
                    blocks[block].branch_true = Some(taken);
                }
            }
            Terminator::Switch { default, cases } => {
                blocks[block].kind = BlockKind::Switch;
                // The renderer zips the statement's keys with `successors[1..]`,
                // so the default has to come first.
                if let Some(default) = index_of(default) {
                    blocks[block].successors.push(block_of[default]);
                }
                for case in cases {
                    if let Some(case) = index_of(case) {
                        blocks[block].successors.push(block_of[case]);
                    }
                }
            }
            Terminator::Return => blocks[block].kind = BlockKind::Return,
            Terminator::Throw => blocks[block].kind = BlockKind::Throw,
        }

        for handler in handlers {
            let start = u32::from(handler.start);
            let end = u32::from(handler.end);
            let covered = instructions[leaders[block]..bound(block)]
                .iter()
                .any(|op| op.byte_offset >= start && op.byte_offset < end);
            let Some(target) = index_of(u32::from(handler.target)) else {
                continue;
            };
            if !covered {
                continue;
            }
            let target = block_of[target];
            if !blocks[block].exception_successors.contains(&target) {
                blocks[block].exception_successors.push(target);
            }
            let catch = handler
                .catch_type
                .as_deref()
                .map(|name| format!("L{name};"));
            blocks[block].exception_handlers.push((target, catch));
        }
    }
    Ok(blocks)
}

fn charge(remaining: &mut Option<usize>, cost: usize) -> Result<()> {
    if let Some(remaining) = remaining {
        *remaining = remaining
            .checked_sub(cost)
            .context("RESOURCE_LIMIT: JVM dataflow work budget exceeded")?;
    }
    Ok(())
}

fn merge(
    state: &mut Option<Vec<Type>>,
    incoming: &[Type],
    queue: &mut VecDeque<usize>,
    index: usize,
    remaining: &mut Option<usize>,
) -> Result<()> {
    charge(
        remaining,
        1 + incoming.iter().map(Type::cost).sum::<usize>(),
    )?;
    let Some(old) = state else {
        *state = Some(incoming.to_vec());
        queue.push_back(index);
        return Ok(());
    };
    ensure!(
        old.len() == incoming.len(),
        "INVALID_INPUT: inconsistent stack height at instruction {index}"
    );
    let mut changed = false;
    for (a, b) in old.iter_mut().zip(incoming) {
        if a == b {
            continue;
        }
        if a.integral() && b.integral() {
            if *a != Type::INT {
                *a = Type::INT;
                changed = true;
            }
            continue;
        }
        let reference_like = |t: &Type| t.is_reference() || *t == Type::unknown_object();
        ensure!(
            a.width() == b.width() && reference_like(a) && reference_like(b),
            "INVALID_INPUT: incompatible stack types at instruction {index}"
        );
        if *a != Type::unknown_object() {
            *a = Type::unknown_object();
            changed = true;
        }
    }
    if changed {
        queue.push_back(index);
    }
    Ok(())
}

struct Lift<'a> {
    class: &'a ClassFile,
    stack: Vec<Type>,
    statements: Vec<Statement>,
    terminator: Terminator,
    variables: &'a mut VariableTable,
    max_stack: u32,
    max_locals: u32,
    local: &'a dyn Fn(u32) -> u32,
    return_type: Type,
    /// The register family of each stack entry, shared by every instruction.
    stack_registers: &'a mut BTreeMap<(u32, &'static str), u32>,
    next_stack_register: &'a mut u32,
    /// Set when this instruction starts a handler: its stack entry is the
    /// exception, which lives in a register of its own rather than in the family
    /// map, so its catch type survives and no stack value shares it.
    exception_slot: Option<u32>,
}

impl Lift<'_> {
    /// The variable a register carries, typed on first use.
    fn typed(&mut self, register: u32, ty: &Type) -> VariableId {
        let id = self.variables.register(register as u16);
        let variable = self.variables.get_mut(id).expect("registered variable");
        // A local or parameter keeps the type its declaration gives it (the
        // LocalVariableTable, or the descriptor it arrived with).
        if variable.ty.is_none() {
            variable.ty = Some(Cow::Owned(ty.descriptor().into_owned()));
        }
        id
    }

    /// Sets a register's type even when it already has one, for positions where
    /// the producer is authoritative - a handler's exception slot is typed by its
    /// catch clause, not by whatever the same stack slot held earlier.
    fn retyped(&mut self, register: u32, ty: &Type) -> VariableId {
        let id = self.variables.register(register as u16);
        self.variables.get_mut(id).expect("registered variable").ty =
            Some(Cow::Owned(ty.descriptor().into_owned()));
        id
    }

    fn constant(&self, value: i64, ty: &Type) -> Value {
        Value::Constant(BackendConstant {
            value: ConstantValue::Integer {
                value,
                ty: static_descriptor(ty),
            },
        })
    }

    fn assign(&mut self, dest: VariableId, rhs: Value, kind: AssignKind) {
        self.statements.push(Statement::Assign {
            lhs: Some(dest),
            rhs,
            kind,
        });
    }

    /// The register a stack entry at `depth` with type `ty` lives in.
    ///
    /// The key is the type's *family*, not the exact type: the verifier merges
    /// the small integer types and unrelated references, so a value defined as
    /// `byte` on one path and `int` on another has to end up in one register or
    /// a use after the join would read the wrong one. The register carries the
    /// family's canonical type, so a declaration is always legal Java even when
    /// the paths disagree (`int x = (byte value);`).
    fn stack_slot(&mut self, depth: u32, ty: &Type) -> VariableId {
        let family = stack_family(ty);
        let key = (depth, family);
        let register = match self.stack_registers.get(&key) {
            Some(register) => *register,
            None => {
                let register = *self.next_stack_register;
                *self.next_stack_register += 1;
                self.stack_registers.insert(key, register);
                register
            }
        };
        let id = self.variables.register(register as u16);
        self.variables.get_mut(id).expect("registered variable").ty = Some(Cow::Borrowed(family));
        id
    }

    fn pop_typed(&mut self) -> Result<(Value, Type)> {
        let ty = self
            .stack
            .pop()
            .context("INVALID_INPUT: JVM stack underflow")?;
        if let Some(exception) = self.exception_slot
            && self.stack.is_empty()
        {
            let id = self.variables.register(exception as u16);
            return Ok((Value::Variable(id), ty));
        }
        let depth = self.stack.iter().map(Type::width).sum();
        let id = self.stack_slot(depth, &ty);
        Ok((Value::Variable(id), ty))
    }
    fn pop(&mut self) -> Result<Value> {
        Ok(self.pop_typed()?.0)
    }
    fn pop_width(&mut self, wide: bool) -> Result<Value> {
        let (value, ty) = self.pop_typed()?;
        ensure!(
            ty.is_wide() == wide,
            "INVALID_INPUT: JVM stack category mismatch"
        );
        Ok(value)
    }
    fn push(&mut self, ty: Type) -> Result<VariableId> {
        let depth = self.stack.iter().map(Type::width).sum::<u32>();
        ensure!(
            depth + ty.width() <= self.max_stack,
            "INVALID_INPUT: max_stack exceeded"
        );
        let id = self.stack_slot(depth, &ty);
        self.stack.push(ty);
        Ok(id)
    }
    fn slot(&self, slot: u32, ty: &Type) -> Result<u32> {
        ensure!(
            slot + ty.width() <= self.max_locals,
            "INVALID_INPUT: local slot out of bounds"
        );
        Ok((self.local)(slot))
    }
    fn const_value(&mut self, value: i64, ty: Type) -> Result<()> {
        let dest = self.push(ty.clone())?;
        let value = self.constant(value, &ty);
        self.assign(dest, value, AssignKind::Normal);
        Ok(())
    }
    fn load(&mut self, slot: u32, ty: Type) -> Result<()> {
        let source = self.slot(slot, &ty)?;
        let source = self.typed(source, &ty);
        let dest = self.push(ty)?;
        self.assign(dest, Value::Variable(source), AssignKind::Move);
        Ok(())
    }
    fn store(&mut self, slot: u32, wide: bool) -> Result<()> {
        let (value, ty) = self.pop_typed()?;
        ensure!(
            ty.is_wide() == wide,
            "INVALID_INPUT: JVM stack category mismatch"
        );
        let dest = self.slot(slot, &ty)?;
        let dest = self.typed(dest, &ty);
        // A local's own type (from the LocalVariableTable, or the first store)
        // decides how the value renders: `iconst_1` into a boolean is `true`.
        let target = self
            .variables
            .get(dest)
            .and_then(|variable| variable.ty.as_deref())
            .map(Type::from_descriptor)
            .unwrap_or(ty);
        self.assign(dest, coerce(value, &target), AssignKind::Move);
        Ok(())
    }
    fn binary(&mut self, operation: BinaryOp, ty: Type, shift: bool) -> Result<()> {
        let right = self.pop_width(!shift && ty.is_wide())?;
        let left = self.pop_width(ty.is_wide())?;
        let descriptor = static_descriptor(&ty);
        let dest = self.push(ty)?;
        self.assign(
            dest,
            Value::Binary(Box::new(BinaryExpression {
                op: operation,
                left,
                right,
                ty: descriptor,
            })),
            AssignKind::Normal,
        );
        Ok(())
    }
    fn unary(&mut self, operation: UnaryOp, ty: Type) -> Result<()> {
        let source = self.pop()?;
        let cast = match operation {
            UnaryOp::Cast => Some(primitive_cast(&ty)),
            UnaryOp::Negate | UnaryOp::Not => None,
        };
        let dest = self.push(ty.clone())?;
        self.assign(
            dest,
            Value::Unary(Box::new(UnaryExpression {
                op: operation,
                cast,
                value: source,
                ty: static_descriptor(&ty),
            })),
            AssignKind::Normal,
        );
        Ok(())
    }

    fn instruction(&mut self, op: &Instruction) -> Result<()> {
        let code = op.operation;
        let index = || {
            op.constant_index
                .context("INVALID_INPUT: missing constant index")
        };
        match code {
            0x00 => {}
            0x01 => self.const_value(0, Type::unknown_object())?,
            0x02..=0x08 => self.const_value(i64::from(code) - 3, Type::INT)?,
            0x09..=0x0a => self.const_value(i64::from(code - 9), Type::LONG)?,
            0x0b..=0x0d => {
                self.const_value(i64::from(f32::from(code - 0x0b).to_bits()), Type::FLOAT)?
            }
            0x0e..=0x0f => {
                self.const_value(f64::from(code - 0x0e).to_bits() as i64, Type::DOUBLE)?
            }
            0x10 | 0x11 => self.const_value(i64::from(op.operands[0]), Type::INT)?,
            0x12..=0x14 => self.load_constant(index()?, code == 0x14)?,
            0x15..=0x19 => self.load(op.operands[0] as u32, value_type(code - 0x15))?,
            0x1a..=0x2d => {
                self.load(u32::from((code - 0x1a) % 4), value_type((code - 0x1a) / 4))?
            }
            0x2e..=0x35 => {
                let index = self.pop_width(false)?;
                let array = self.pop_width(false)?;
                let ty = array_type(code - 0x2e);
                let dest = self.push(ty)?;
                self.assign(
                    dest,
                    Value::ArrayLoad(Box::new(ArrayLoadExpression {
                        array,
                        index,
                        ty: None,
                    })),
                    AssignKind::Normal,
                );
            }
            0x36..=0x3a => self.store(op.operands[0] as u32, matches!(code, 0x37 | 0x39))?,
            0x3b..=0x4e => self.store(
                u32::from((code - 0x3b) % 4),
                matches!((code - 0x3b) / 4, 1 | 3),
            )?,
            0x4f..=0x56 => {
                let value = self.pop_width(matches!(code, 0x50 | 0x52))?;
                let index = self.pop_width(false)?;
                let array = self.pop_width(false)?;
                self.statements
                    .push(Statement::ArrayStore(Box::new(ArrayStoreStatement {
                        value,
                        array,
                        index,
                        ty: None,
                    })));
            }
            0x57 => {
                self.pop_width(false)?;
            }
            0x58 => {
                let (_, ty) = self.pop_typed()?;
                if !ty.is_wide() {
                    self.pop_width(false)?;
                }
            }
            0x59..=0x5f => self.permute(code)?,
            0x60..=0x73 => {
                let ty = value_type((code - 0x60) % 4);
                let operation = [
                    BinaryOp::Add,
                    BinaryOp::Subtract,
                    BinaryOp::Multiply,
                    BinaryOp::Divide,
                    BinaryOp::Remainder,
                ][usize::from((code - 0x60) / 4)];
                self.binary(operation, ty, false)?;
            }
            0x74..=0x77 => self.unary(UnaryOp::Negate, value_type(code - 0x74))?,
            0x78..=0x83 => {
                let operation = [
                    BinaryOp::ShiftLeft,
                    BinaryOp::ShiftRight,
                    BinaryOp::UnsignedShiftRight,
                    BinaryOp::And,
                    BinaryOp::Or,
                    BinaryOp::Xor,
                ][usize::from((code - 0x78) / 2)];
                self.binary(
                    operation,
                    if code.is_multiple_of(2) {
                        Type::INT
                    } else {
                        Type::LONG
                    },
                    code <= 0x7d,
                )?;
            }
            0x84 => {
                let local = self.slot(op.operands[0] as u32, &Type::INT)?;
                let local = self.typed(local, &Type::INT);
                let left = Value::Variable(local);
                let right = self.constant(i64::from(op.operands[1]), &Type::INT);
                self.assign(
                    local,
                    Value::Binary(Box::new(BinaryExpression {
                        op: BinaryOp::Add,
                        left,
                        right,
                        ty: "I",
                    })),
                    AssignKind::Normal,
                );
            }
            0x85..=0x93 => {
                let ty = [
                    Type::LONG,
                    Type::FLOAT,
                    Type::DOUBLE,
                    Type::INT,
                    Type::FLOAT,
                    Type::DOUBLE,
                    Type::INT,
                    Type::LONG,
                    Type::DOUBLE,
                    Type::INT,
                    Type::LONG,
                    Type::FLOAT,
                    Type::BYTE,
                    Type::CHAR,
                    Type::SHORT,
                ][usize::from(code - 0x85)]
                .clone();
                self.unary(UnaryOp::Cast, ty)?;
            }
            0x94..=0x98 => {
                let right = self.pop_width(matches!(code, 0x94 | 0x97 | 0x98))?;
                let left = self.pop_width(matches!(code, 0x94 | 0x97 | 0x98))?;
                let dest = self.push(Type::INT)?;
                // `cmpl`/`cmpg` differ only in the NaN direction, which the
                // compare-then-branch folding below does not model.
                let descriptor = match code {
                    0x94 => "J",
                    0x95 | 0x96 => "F",
                    _ => "D",
                };
                self.assign(
                    dest,
                    Value::Binary(Box::new(BinaryExpression {
                        op: BinaryOp::Compare,
                        left,
                        right,
                        ty: descriptor,
                    })),
                    AssignKind::Normal,
                );
            }
            0x99..=0x9e | 0xc6 | 0xc7 => {
                let value = self.pop_width(false)?;
                let cmp = if code >= 0xc6 {
                    if code == 0xc6 {
                        BinaryOp::Equal
                    } else {
                        BinaryOp::NotEqual
                    }
                } else {
                    condition(code - 0x99)
                };
                // A missing right operand renders as `0` for a primitive and as
                // `null` for a reference, which is exactly if-eqz versus ifnull.
                self.statements.push(Statement::Condition {
                    op: cmp,
                    left: value,
                    right: None,
                });
                self.terminator = Terminator::Branch(op.targets[0]);
            }
            0x9f..=0xa6 => {
                let right = self.pop_width(false)?;
                let left = self.pop_width(false)?;
                self.statements.push(Statement::Condition {
                    op: condition((code - 0x9f) % 6),
                    left,
                    right: Some(right),
                });
                self.terminator = Terminator::Branch(op.targets[0]);
            }
            0xa7 | 0xc8 => self.terminator = Terminator::Goto(op.targets[0]),
            0xaa | 0xab => {
                let value = self.pop_width(false)?;
                self.statements.push(Statement::Switch {
                    value,
                    payload_offset: 0,
                    values: op.operands.clone(),
                });
                self.terminator = Terminator::Switch {
                    default: op.targets[0],
                    cases: op.targets[1..].to_vec(),
                };
            }
            0xac..=0xb0 => {
                let value = self.pop_width(matches!(code, 0xad | 0xaf))?;
                let value = coerce(value, &self.return_type.clone());
                self.stack.clear();
                self.statements.push(Statement::Return(Some(value)));
                self.terminator = Terminator::Return;
            }
            0xb1 => {
                self.stack.clear();
                self.statements.push(Statement::Return(None));
                self.terminator = Terminator::Return;
            }
            0xb2..=0xb5 => self.field(code, index()?)?,
            0xb6..=0xb9 => self.invoke(code, index()?, op)?,
            0xbb => {
                let ty = self.class.class_type(index()?)?;
                ensure!(ty.is_object(), "INVALID_INPUT: new requires object type");
                let dest = self.push(ty.clone())?;
                self.assign(
                    dest,
                    Value::NewInstance {
                        ty: ty.descriptor().into_owned(),
                    },
                    AssignKind::Normal,
                );
            }
            0xbc | 0xbd => {
                let size = self.pop_width(false)?;
                let element = if code == 0xbc {
                    primitive_array(op.operands[0])?
                } else {
                    self.class.class_type(index()?)?
                };
                self.new_array(Type::array(element), vec![size])?;
            }
            0xbe => {
                let array = self.pop_width(false)?;
                let dest = self.push(Type::INT)?;
                self.assign(
                    dest,
                    Value::ArrayLength(Box::new(array)),
                    AssignKind::Normal,
                );
            }
            0xbf => {
                let value = self.pop_width(false)?;
                self.stack.clear();
                self.statements.push(Statement::Throw(value));
                self.terminator = Terminator::Throw;
            }
            0xc0 | 0xc1 => {
                let ty = self.class.class_type(index()?)?;
                let source = self.pop_width(false)?;
                let dest = self.push(if code == 0xc0 {
                    ty.clone()
                } else {
                    Type::BOOLEAN
                })?;
                let value = if code == 0xc0 {
                    Value::CheckCast(Box::new(CheckCastExpression {
                        value: source,
                        ty: ty.java_name(),
                        descriptor: ty.descriptor().into_owned(),
                    }))
                } else {
                    Value::Binary(Box::new(BinaryExpression {
                        op: BinaryOp::InstanceOf,
                        left: source,
                        right: Value::BaseClass {
                            name: ty.java_name(),
                        },
                        ty: "Z",
                    }))
                };
                self.assign(dest, value, AssignKind::Normal);
            }
            0xc2 | 0xc3 => {
                let value = self.pop_width(false)?;
                self.statements.push(if code == 0xc2 {
                    Statement::MonitorEnter(value)
                } else {
                    Statement::MonitorExit(value)
                });
            }
            0xc5 => bail!("UNSUPPORTED_BYTECODE: multianewarray"),
            0xba => bail!("UNSUPPORTED_BYTECODE: invokedynamic"),
            0xa8 | 0xa9 | 0xc9 => bail!("UNSUPPORTED_BYTECODE: jsr/ret subroutine"),
            _ => bail!("UNSUPPORTED_BYTECODE: {}", op.mnemonic),
        }
        Ok(())
    }

    fn load_constant(&mut self, index: u16, wide: bool) -> Result<()> {
        match constant(&self.class.pool, index)? {
            Constant::Integer(value) => {
                ensure!(!wide, "INVALID_INPUT: ldc2_w category");
                self.const_value(i64::from(*value), Type::INT)?;
            }
            Constant::Float(value) => {
                ensure!(!wide, "INVALID_INPUT: ldc2_w category");
                self.const_value(i64::from(*value), Type::FLOAT)?;
            }
            Constant::Long(value) => {
                ensure!(wide, "INVALID_INPUT: ldc category");
                self.const_value(*value, Type::LONG)?;
            }
            Constant::Double(value) => {
                ensure!(wide, "INVALID_INPUT: ldc category");
                self.const_value(*value as i64, Type::DOUBLE)?;
            }
            Constant::String(_) => {
                ensure!(!wide, "INVALID_INPUT: ldc2_w category");
                let value = self.class.string(index)?;
                let dest = self.push(Type::string())?;
                let text = value.as_str().to_owned();
                self.assign(
                    dest,
                    Value::Constant(BackendConstant {
                        value: ConstantValue::String(text),
                    }),
                    AssignKind::Normal,
                );
            }
            Constant::Class(_) => {
                ensure!(!wide, "INVALID_INPUT: ldc2_w category");
                let ty = self.class.class_type(index)?;
                let dest = self.push(Type::object("java/lang/Class"))?;
                self.assign(
                    dest,
                    Value::BaseClass {
                        name: format!("{}.class", ty.java_name()),
                    },
                    AssignKind::Normal,
                );
            }
            Constant::Dynamic { .. } | Constant::Handle(..) | Constant::MethodType(_) => {
                bail!("UNSUPPORTED_BYTECODE: dynamic/method-handle constant")
            }
            _ => bail!("INVALID_INPUT: invalid ldc constant"),
        }
        Ok(())
    }

    fn field(&mut self, opcode: u8, index: u16) -> Result<()> {
        let (tag, owner, name, descriptor) = self.class.member(index)?;
        ensure!(tag == 9, "INVALID_INPUT: field operand tag");
        let ty = field_type(&descriptor)?;
        let static_field = opcode <= 0xb3;
        let get = opcode.is_multiple_of(2);
        let descriptor = ty.descriptor().into_owned();
        let field = FieldReference {
            class: Arc::from(format!("L{owner};").as_str()),
            ty: Arc::from(descriptor.as_str()),
            name: Arc::from(name.as_str()),
        };
        match (static_field, get) {
            (true, true) => {
                let dest = self.push(ty)?;
                self.assign(
                    dest,
                    Value::StaticField(Box::new(field)),
                    AssignKind::Normal,
                );
            }
            (false, true) => {
                let object = self.pop_width(false)?;
                let dest = self.push(ty)?;
                self.assign(
                    dest,
                    Value::InstanceField(Box::new(InstanceFieldExpression { object, field })),
                    AssignKind::Normal,
                );
            }
            (true, false) => {
                let value = coerce(self.pop_width(ty.is_wide())?, &ty);
                self.statements.push(Statement::StaticPut { value, field });
            }
            (false, false) => {
                let value = coerce(self.pop_width(ty.is_wide())?, &ty);
                let object = self.pop_width(false)?;
                self.statements
                    .push(Statement::InstancePut(Box::new(InstancePutStatement {
                        value,
                        object,
                        field,
                    })));
            }
        }
        Ok(())
    }

    fn invoke(&mut self, opcode: u8, index: u16, op: &Instruction) -> Result<()> {
        let (tag, owner, name, descriptor) = self.class.member(index)?;
        ensure!(
            matches!(tag, 10 | 11) && (opcode != 0xb9 || tag == 11),
            "INVALID_INPUT: invoke operand tag"
        );
        ensure!(
            opcode != 0xb7 || tag != 11 || owner == self.class.name,
            "UNSUPPORTED_BYTECODE: qualified interface-super invocation"
        );
        let proto = method_type(&descriptor)?;
        let mut args = Vec::new();
        for ty in proto.parameters.iter().rev() {
            args.push(coerce(self.pop_width(ty.is_wide())?, ty));
        }
        if opcode != 0xb8 {
            args.push(self.pop_width(false)?);
        }
        args.reverse();
        if opcode == 0xb9 {
            ensure!(
                op.operands[0] as u32 == proto.parameters.iter().map(Type::width).sum::<u32>() + 1,
                "INVALID_INPUT: invokeinterface count"
            );
        }
        let kind = match opcode {
            0xb8 => InvokeKind::Static,
            0xb9 => InvokeKind::Interface,
            0xb7 if name == "<init>" || owner == self.class.name => InvokeKind::Direct,
            0xb7 => InvokeKind::Super,
            _ => InvokeKind::Virtual,
        };
        let (base, arguments) = if opcode == 0xb8 {
            (
                Value::BaseClass {
                    name: Type::object(&owner).java_name(),
                },
                args,
            )
        } else {
            let mut arguments = args;
            let base = arguments.remove(0);
            (base, arguments)
        };
        let parameters = proto
            .parameters
            .iter()
            .map(|ty| ty.descriptor().into_owned())
            .collect();
        let return_type = proto.return_type.descriptor().into_owned();
        let result = if proto.return_type.is_void() {
            None
        } else {
            Some(self.push(proto.return_type.clone())?)
        };
        let value = Value::Invoke(Box::new(InvokeExpression {
            kind,
            method: MethodReference {
                class: format!("L{owner};"),
                name,
                parameters,
                return_type,
            },
            base,
            arguments,
        }));
        match result {
            Some(dest) => self.assign(dest, value, AssignKind::Normal),
            None => self.statements.push(Statement::Assign {
                lhs: None,
                rhs: value,
                kind: AssignKind::Normal,
            }),
        }
        Ok(())
    }

    fn new_array(&mut self, ty: Type, dimensions: Vec<Value>) -> Result<()> {
        ensure!(
            dimensions.len() == 1,
            "UNSUPPORTED_BYTECODE: multi-dimensional array creation"
        );
        let size = dimensions.into_iter().next().expect("one dimension");
        let descriptor = ty.descriptor().into_owned();
        let dest = self.push(ty)?;
        self.assign(
            dest,
            Value::NewArray(Box::new(NewArrayExpression {
                size,
                ty: descriptor,
            })),
            AssignKind::Normal,
        );
        Ok(())
    }

    fn permute(&mut self, opcode: u8) -> Result<()> {
        let original = self.stack.clone();
        let n = original.len();
        let narrow = |index: usize| -> Result<()> {
            ensure!(
                !original
                    .get(index)
                    .context("INVALID_INPUT: stack permutation underflow")?
                    .is_wide(),
                "INVALID_INPUT: stack permutation category"
            );
            Ok(())
        };
        let sub = |amount: usize| {
            n.checked_sub(amount)
                .context("INVALID_INPUT: stack permutation underflow")
        };
        let (start, order): (usize, Vec<usize>) = match opcode {
            0x59 => {
                let a = sub(1)?;
                narrow(a)?;
                (a, vec![a, a])
            }
            0x5a => {
                let a = sub(1)?;
                let b = sub(2)?;
                narrow(a)?;
                narrow(b)?;
                (b, vec![a, b, a])
            }
            0x5b => {
                let a = sub(1)?;
                let b = sub(2)?;
                narrow(a)?;
                if original[b].is_wide() {
                    (b, vec![a, b, a])
                } else {
                    let c = sub(3)?;
                    narrow(c)?;
                    (c, vec![a, c, b, a])
                }
            }
            0x5c => {
                let a = sub(1)?;
                if original[a].is_wide() {
                    (a, vec![a, a])
                } else {
                    let b = sub(2)?;
                    narrow(b)?;
                    (b, vec![b, a, b, a])
                }
            }
            0x5d => {
                let a = sub(1)?;
                let b = sub(2)?;
                if original[a].is_wide() {
                    narrow(b)?;
                    (b, vec![a, b, a])
                } else {
                    let c = sub(3)?;
                    narrow(b)?;
                    narrow(c)?;
                    (c, vec![b, a, c, b, a])
                }
            }
            0x5e => {
                let a = sub(1)?;
                let b = sub(2)?;
                if original[a].is_wide() {
                    if original[b].is_wide() {
                        (b, vec![a, b, a])
                    } else {
                        let c = sub(3)?;
                        narrow(c)?;
                        (c, vec![a, c, b, a])
                    }
                } else {
                    narrow(b)?;
                    let c = sub(3)?;
                    if original[c].is_wide() {
                        (c, vec![b, a, c, b, a])
                    } else {
                        let d = sub(4)?;
                        narrow(d)?;
                        (d, vec![b, a, d, c, b, a])
                    }
                }
            }
            0x5f => {
                let a = sub(1)?;
                let b = sub(2)?;
                narrow(a)?;
                narrow(b)?;
                (b, vec![a, b])
            }
            _ => unreachable!(),
        };
        let mut depths = Vec::new();
        let mut depth = 0;
        for ty in &original {
            depths.push(depth);
            depth += ty.width();
        }
        let mut temporaries = BTreeMap::new();
        for &index in &order {
            if temporaries.contains_key(&index) {
                continue;
            }
            let ty = original[index].clone();
            let source = self.stack_slot(depths[index], &ty);
            let dest = self.variables.temporary();
            self.variables.get_mut(dest).expect("temporary").ty =
                Some(Cow::Owned(ty.descriptor().into_owned()));
            self.assign(dest, Value::Variable(source), AssignKind::Move);
            temporaries.insert(index, dest);
        }
        self.stack.truncate(start);
        for index in order {
            let ty = original[index].clone();
            let dest = self.push(ty.clone())?;
            let source = temporaries[&index];
            self.assign(dest, Value::Variable(source), AssignKind::Move);
        }
        Ok(())
    }
}

/// Retypes an integer constant used where a boolean belongs.
///
/// The JVM stack only knows `int`: `iconst_1` stored into a `boolean` field is
/// the same value, and the backend renders booleans from the constant's type, so
/// the type has to come from the use site rather than from the push.
fn coerce(value: Value, target: &Type) -> Value {
    match (value, target) {
        (
            Value::Constant(BackendConstant {
                value: ConstantValue::Integer { value, .. },
            }),
            Type::Primitive(Primitive::Boolean),
        ) => Value::Constant(BackendConstant {
            value: ConstantValue::Integer {
                value: i64::from(value != 0),
                ty: "Z",
            },
        }),
        (value, _) => value,
    }
}

/// The register family of a stack value, and the descriptor a register of that
/// family declares. Values in one family can occupy the same stack slot in
/// different paths, which is what the JVM's own verification merges.
fn stack_family(ty: &Type) -> &'static str {
    match ty {
        Type::Primitive(
            Primitive::Boolean
            | Primitive::Byte
            | Primitive::Short
            | Primitive::Char
            | Primitive::Int,
        ) => "I",
        Type::Primitive(Primitive::Float) => "F",
        Type::Primitive(Primitive::Long) => "J",
        Type::Primitive(Primitive::Double) => "D",
        Type::Void | Type::Object(_) | Type::Array(_) | Type::Unknown(_) => "Ljava/lang/Object;",
    }
}

/// The descriptor the IR records for a type, which is static for primitives.
fn static_descriptor(ty: &Type) -> &'static str {
    match ty {
        Type::Primitive(Primitive::Boolean) => "Z",
        Type::Primitive(Primitive::Byte) => "B",
        Type::Primitive(Primitive::Short) => "S",
        Type::Primitive(Primitive::Char) => "C",
        Type::Primitive(Primitive::Int) => "I",
        Type::Primitive(Primitive::Float) => "F",
        Type::Primitive(Primitive::Long) => "J",
        Type::Primitive(Primitive::Double) => "D",
        Type::Void => "V",
        Type::Object(_) | Type::Array(_) | Type::Unknown(_) => "Ljava/lang/Object;",
    }
}

/// A type's Java source name, as `instanceof` and casts render it.
/// The Java type name a primitive conversion renders with, e.g. `(long) scale`.
fn primitive_cast(ty: &Type) -> &'static str {
    match ty {
        Type::Primitive(primitive) => primitive.java_name(),
        Type::Void | Type::Object(_) | Type::Array(_) | Type::Unknown(_) => "Object",
    }
}

fn value_type(index: u8) -> Type {
    [
        Type::INT,
        Type::LONG,
        Type::FLOAT,
        Type::DOUBLE,
        Type::unknown_object(),
    ][usize::from(index)]
    .clone()
}
fn array_type(index: u8) -> Type {
    [
        Type::INT,
        Type::LONG,
        Type::FLOAT,
        Type::DOUBLE,
        Type::unknown_object(),
        Type::Unknown(vec![Primitive::Boolean, Primitive::Byte]),
        Type::CHAR,
        Type::SHORT,
    ][usize::from(index)]
    .clone()
}
fn primitive_array(code: i32) -> Result<Type> {
    Ok(match code {
        4 => Type::BOOLEAN,
        5 => Type::CHAR,
        6 => Type::FLOAT,
        7 => Type::DOUBLE,
        8 => Type::BYTE,
        9 => Type::SHORT,
        10 => Type::INT,
        11 => Type::LONG,
        _ => bail!("INVALID_INPUT: newarray type"),
    })
}
fn condition(index: u8) -> BinaryOp {
    [
        BinaryOp::Equal,
        BinaryOp::NotEqual,
        BinaryOp::Less,
        BinaryOp::GreaterEqual,
        BinaryOp::Greater,
        BinaryOp::LessEqual,
    ][usize::from(index)]
}
