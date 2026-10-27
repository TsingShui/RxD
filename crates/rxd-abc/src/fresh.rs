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
    Ok(instruction.register(index)? as usize + 1)
}

/// Prove ordinary literal receivers on all forward paths; loop entries start unknown.
pub fn ordinary_receivers(
    abc: &Abc<'_>,
    decoded: &Decoded<'_>,
    graph: &Graph,
) -> Result<Vec<bool>> {
    let instructions = &decoded.instructions;
    let mut result = Vec::new();
    result
        .try_reserve_exact(instructions.len())
        .map_err(|_| resource("ABC receiver facts allocation failed"))?;
    result.resize(instructions.len(), false);
    if !decoded.code.tries.is_empty()
        || !instructions.iter().any(|instruction| {
            matches!(
                instruction.opcode.name,
                "stownbyindex" | "wide.stownbyindex" | "stownbyname" | "stownbyvalue"
            )
        })
    {
        return Ok(result);
    }
    let largest = instructions
        .iter()
        .flat_map(|instruction| instruction.operands.iter())
        .filter_map(|operand| match operand {
            Operand::Register(register) => Some(*register as usize),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let words = (largest + 2).div_ceil(64);
    let total = graph
        .blocks
        .len()
        .checked_mul(words)
        .ok_or_else(|| resource("ABC receiver facts overflow"))?;
    abc.check_storage::<u64>(
        total
            .checked_add(words)
            .ok_or_else(|| resource("ABC receiver facts overflow"))?,
    )?;
    let mut states = Vec::new();
    states
        .try_reserve_exact(total)
        .map_err(|_| resource("ABC receiver states allocation failed"))?;
    states.resize(total, 0u64);
    let mut initialized = Vec::new();
    initialized
        .try_reserve_exact(graph.blocks.len())
        .map_err(|_| resource("ABC receiver states allocation failed"))?;
    initialized.resize(graph.blocks.len(), false);
    initialized[0] = true;
    let mut loop_header = Vec::new();
    loop_header
        .try_reserve_exact(graph.blocks.len())
        .map_err(|_| resource("ABC receiver states allocation failed"))?;
    loop_header.resize(graph.blocks.len(), false);
    for (index, block) in graph.blocks.iter().enumerate() {
        if let Some(pc) = instructions[block.end - 1].branch {
            let target = graph.target(instructions, pc)?;
            if target <= index {
                loop_header[target] = true;
            }
        }
    }
    let mut current = Vec::new();
    current
        .try_reserve_exact(words)
        .map_err(|_| resource("ABC receiver facts allocation failed"))?;
    current.resize(words, 0u64);
    for (index, block) in graph.blocks.iter().enumerate() {
        if !graph.reachable[index] {
            continue;
        }
        if !initialized[index] && !loop_header[index] {
            continue;
        }
        if loop_header[index] {
            current.fill(0);
        } else {
            current.copy_from_slice(&states[index * words..(index + 1) * words]);
        }
        for pc in block.first..block.end {
            let instruction = &instructions[pc];
            let name = instruction.opcode.name;
            match name {
                "stownbyindex" | "stownbyname" | "stownbyvalue" => {
                    result[pc] = get(
                        &current,
                        register(instruction, if name == "stownbyname" { 2 } else { 1 })?,
                    )
                }
                "wide.stownbyindex" => result[pc] = get(&current, register(instruction, 0)?),
                _ => {}
            }
            match name {
                "lda" => {
                    let value = get(&current, register(instruction, 0)?);
                    set(&mut current, 0, value);
                }
                "sta" => {
                    let value = get(&current, 0);
                    set(&mut current, register(instruction, 0)?, value);
                }
                "mov" => {
                    let value = get(&current, register(instruction, 1)?);
                    set(&mut current, register(instruction, 0)?, value);
                }
                "createemptyarray"
                | "createarraywithbuffer"
                | "createemptyobject"
                | "createobjectwithbuffer" => set(&mut current, 0, true),
                _ if instruction.opcode.writes_acc() => set(&mut current, 0, false),
                _ => {}
            }
        }
        let last = &instructions[block.end - 1];
        let mut successors = [None, None];
        if let Some(target) = last.branch {
            successors[0] = Some(graph.target(instructions, target)?);
        }
        if (!last.opcode.is_jump() || last.opcode.is_conditional())
            && !last.opcode.is_return()
            && last.opcode.name != "throw"
        {
            successors[1] = (index + 1 < graph.blocks.len()).then_some(index + 1);
        }
        for target in successors
            .into_iter()
            .flatten()
            .filter(|target| *target > index)
        {
            let state = &mut states[target * words..(target + 1) * words];
            if initialized[target] {
                for (value, incoming) in state.iter_mut().zip(&current) {
                    *value &= incoming;
                }
            } else {
                state.copy_from_slice(&current);
                initialized[target] = true;
            }
        }
    }
    Ok(result)
}
