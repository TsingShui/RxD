use crate::{Abc, Decoded, Instruction, Operand, reader::resource, structure::Graph};
use rxd_core::Result;

fn get(bits: &[u64], index: usize) -> bool {
    bits[index / 64] & (1 << (index % 64)) != 0
}

fn set(bits: &mut [u64], index: usize, value: bool) {
    let bit = 1u64 << (index % 64);
    if value {
        bits[index / 64] |= bit;
    } else {
        bits[index / 64] &= !bit;
    }
}

fn register(instruction: &Instruction, index: usize) -> Result<usize> {
    Ok(instruction.register(index)? as usize)
}

/// Registers proven to hold the hole sentinel on every path reaching each request.
pub fn proven_hole(
    abc: &Abc<'_>,
    decoded: &Decoded<'_>,
    graph: &Graph,
    requests: &[(u32, u32)],
) -> Result<Vec<bool>> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(requests.len())
        .map_err(|_| resource("ABC hole proof allocation failed"))?;
    result.resize(requests.len(), false);
    if requests.is_empty() {
        return Ok(result);
    }
    let largest = decoded
        .instructions
        .iter()
        .flat_map(|instruction| instruction.operands.iter())
        .filter_map(|operand| match operand {
            Operand::Register(register) => Some(*register as usize),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .max(
            requests
                .iter()
                .map(|(_, register)| *register as usize)
                .max()
                .unwrap_or(0),
        );
    let words = (largest + 2).div_ceil(64);
    let total = graph
        .blocks
        .len()
        .checked_mul(words)
        .ok_or_else(|| resource("ABC hole proof overflow"))?;
    abc.check_storage::<u64>(
        total
            .checked_add(words)
            .ok_or_else(|| resource("ABC hole proof overflow"))?,
    )?;
    let mut states = Vec::new();
    states
        .try_reserve_exact(total)
        .map_err(|_| resource("ABC hole states allocation failed"))?;
    states.resize(total, 0u64);
    let mut initialized = Vec::new();
    initialized
        .try_reserve_exact(graph.blocks.len())
        .map_err(|_| resource("ABC hole states allocation failed"))?;
    initialized.resize(graph.blocks.len(), false);
    let mut current = Vec::new();
    current
        .try_reserve_exact(words)
        .map_err(|_| resource("ABC hole facts allocation failed"))?;
    current.resize(words, 0u64);
    let mut changed = true;
    let mut rounds = 0usize;
    while changed {
        changed = false;
        rounds += 1;
        if rounds > graph.blocks.len() + 1 {
            return Err(resource("ABC hole proof did not converge"));
        }
        initialized[0] = true;
        for (index, block) in graph.blocks.iter().enumerate() {
            if !graph.reachable[index] || !initialized[index] {
                continue;
            }
            if index == 0 {
                current.fill(0);
            } else {
                current.copy_from_slice(&states[index * words..(index + 1) * words]);
            }
            for pc in block.first..block.end {
                let instruction = &decoded.instructions[pc];
                for (position, (target, register)) in requests.iter().enumerate() {
                    if *target == instruction.pc {
                        result[position] = get(&current, *register as usize);
                    }
                }
                let name = instruction.opcode.name;
                match name {
                    "ldhole" | "ldundefined" => set(&mut current, 0, true),
                    "lda" => {
                        let value = get(&current, register(instruction, 0)?);
                        set(&mut current, 0, value);
                    }
                    "mov" => {
                        let value = get(&current, register(instruction, 1)?);
                        let dest = register(instruction, 0)?;
                        set(&mut current, dest, value);
                    }
                    "sta" => {
                        let value = get(&current, 0);
                        let dest = register(instruction, 0)?;
                        set(&mut current, dest, value);
                    }
                    _ if instruction.opcode.writes_acc() => set(&mut current, 0, false),
                    _ => {}
                }
            }
            let last = &decoded.instructions[block.end - 1];
            let mut successors = [None, None];
            if let Some(target) = last.branch {
                successors[0] = Some(graph.target(&decoded.instructions, target)?);
            }
            if (!last.opcode.is_jump() || last.opcode.is_conditional())
                && !last.opcode.is_return()
                && last.opcode.name != "throw"
            {
                successors[1] = (index + 1 < graph.blocks.len()).then_some(index + 1);
            }
            for target in successors.into_iter().flatten() {
                if target == 0 {
                    continue;
                }
                let state = &mut states[target * words..(target + 1) * words];
                if initialized[target] {
                    let mut merged = false;
                    for (value, incoming) in state.iter_mut().zip(&current) {
                        let next = *value & *incoming;
                        if next != *value {
                            *value = next;
                            merged = true;
                        }
                    }
                    changed |= merged;
                } else {
                    state.copy_from_slice(&current);
                    initialized[target] = true;
                    changed = true;
                }
            }
        }
    }
    Ok(result)
}

/// True when no instruction can observe the method's own function value.
pub fn function_value_unused(decoded: &Decoded<'_>) -> Result<bool> {
    let func = decoded.code.vregs;
    let mut tainted = Vec::new();
    tainted
        .try_reserve_exact(decoded.code.vregs as usize + 1)
        .map_err(|_| resource("ABC function taint allocation failed"))?;
    tainted.resize(decoded.code.vregs as usize + 1, false);
    let mut acc = false;
    for instruction in &decoded.instructions {
        let name = instruction.opcode.name;
        match name {
            "mov" => {
                let dest = instruction.register(0)? as usize;
                let source = instruction.register(1)?;
                let value =
                    source == func || (source < decoded.code.vregs && tainted[source as usize]);
                if dest < tainted.len() {
                    tainted[dest] = value;
                } else if value {
                    return Ok(false);
                }
                continue;
            }
            "lda" => {
                let source = instruction.register(0)?;
                acc = source == func || (source < decoded.code.vregs && tainted[source as usize]);
                continue;
            }
            "sta" => {
                let dest = instruction.register(0)? as usize;
                if dest < tainted.len() {
                    tainted[dest] = acc;
                } else if acc {
                    return Ok(false);
                }
                acc = false;
                continue;
            }
            "ldfunction" => return Ok(false),
            "supercallspread" | "callruntime.supercallforwardallargs" => {
                acc = false;
                continue;
            }
            _ => {}
        }
        if instruction.opcode.reads_acc() && acc {
            return Ok(false);
        }
        for operand in &instruction.operands {
            if let Operand::Register(register) = operand
                && (*register == func
                    || (*register < decoded.code.vregs && tainted[*register as usize]))
            {
                return Ok(false);
            }
        }
        if instruction.opcode.writes_acc() {
            acc = false;
        }
    }
    Ok(true)
}
