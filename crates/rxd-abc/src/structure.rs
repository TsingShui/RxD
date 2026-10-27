use crate::{
    Decoded, Instruction,
    reader::{invalid, resource},
};
use rxd_core::{Budget, Error, Result};

#[derive(Clone, Copy)]
pub struct Block {
    pub first: usize,
    pub end: usize,
}

pub struct Graph {
    pub blocks: Vec<Block>,
    pub reachable: Vec<bool>,
}

impl Graph {
    pub fn build(decoded: &Decoded<'_>, budget: Budget) -> Result<Self> {
        let instructions = &decoded.instructions;
        if instructions.is_empty() {
            return Err(invalid("ABC code item has no instructions"));
        }
        let handler_count = decoded
            .code
            .tries
            .iter()
            .try_fold(0usize, |count, block| {
                count.checked_add(block.catches.len())
            })
            .ok_or_else(|| resource("ABC handler count overflow"))?;
        let mut regions = Vec::new();
        regions
            .try_reserve_exact(decoded.code.tries.len())
            .map_err(|_| resource("ABC exception index allocation failed"))?;
        regions.extend(decoded.code.tries.iter());
        regions.sort_unstable_by_key(|region| region.pc);
        if regions
            .windows(2)
            .any(|pair| pair[0].pc + pair[0].length > pair[1].pc)
        {
            return Err(unsupported(
                "overlapping or nested exception ranges need additional analysis",
            ));
        }
        let capacity = instructions
            .len()
            .checked_mul(2)
            .and_then(|count| {
                decoded
                    .code
                    .tries
                    .len()
                    .checked_mul(2)
                    .and_then(|n| count.checked_add(n))
            })
            .and_then(|count| {
                handler_count
                    .checked_mul(2)
                    .and_then(|n| count.checked_add(n))
            })
            .and_then(|count| count.checked_add(1))
            .ok_or_else(|| resource("ABC control-flow size overflow"))?;
        let bytes = capacity
            .checked_mul(std::mem::size_of::<usize>())
            .and_then(|bytes| {
                instructions
                    .len()
                    .checked_mul(std::mem::size_of::<Block>() + 2)
                    .and_then(|n| bytes.checked_add(n))
            })
            .filter(|bytes| *bytes <= budget.metadata_bytes)
            .ok_or_else(|| resource("ABC control-flow graph exceeds metadata budget"))?;
        let _ = bytes;
        let mut leaders = Vec::new();
        leaders
            .try_reserve_exact(capacity)
            .map_err(|_| resource("ABC leaders allocation failed"))?;
        leaders.push(0);
        for (index, instruction) in instructions.iter().enumerate() {
            if let Some(target) = instruction.branch {
                leaders.push(
                    instructions
                        .binary_search_by_key(&target, |instruction| instruction.pc)
                        .map_err(|_| invalid("ABC branch target is not an instruction boundary"))?,
                );
            }
            if (instruction.branch.is_some()
                || instruction.opcode.is_return()
                || instruction.opcode.name == "throw")
                && index + 1 < instructions.len()
            {
                leaders.push(index + 1);
            }
        }
        let instruction_index = |pc: u32| -> Result<usize> {
            if pc as usize == decoded.code.bytes.len() {
                return Ok(instructions.len());
            }
            instructions
                .binary_search_by_key(&pc, |instruction| instruction.pc)
                .map_err(|_| invalid("ABC exception range is not an instruction boundary"))
        };
        for region in &decoded.code.tries {
            leaders.push(instruction_index(region.pc)?);
            leaders.push(instruction_index(region.pc + region.length)?);
            for handler in &region.catches {
                leaders.push(instruction_index(handler.pc)?);
                leaders.push(instruction_index(handler.pc + handler.length)?);
            }
        }
        leaders.push(instructions.len());
        leaders.sort_unstable();
        leaders.dedup();
        let mut blocks = Vec::new();
        blocks
            .try_reserve_exact(leaders.len() - 1)
            .map_err(|_| resource("ABC basic block allocation failed"))?;
        for pair in leaders.windows(2) {
            blocks.push(Block {
                first: pair[0],
                end: pair[1],
            });
        }
        let mut reachable = Vec::new();
        reachable
            .try_reserve_exact(blocks.len())
            .map_err(|_| resource("ABC reachability allocation failed"))?;
        reachable.resize(blocks.len(), false);
        let mut queue = Vec::new();
        queue
            .try_reserve_exact(blocks.len())
            .map_err(|_| resource("ABC reachability allocation failed"))?;
        queue.push(0);
        while let Some(index) = queue.pop() {
            if reachable[index] {
                continue;
            }
            reachable[index] = true;
            let instruction = &instructions[blocks[index].end - 1];
            let pc = instructions[blocks[index].first].pc;
            let region_index = regions.partition_point(|region| region.pc <= pc);
            if let Some(region) = region_index
                .checked_sub(1)
                .and_then(|index| regions.get(index))
                && pc < region.pc + region.length
            {
                for handler in &region.catches {
                    let target = Self::block_at(&blocks, instructions, handler.pc)?;
                    if !reachable[target] {
                        queue
                            .try_reserve(1)
                            .map_err(|_| resource("ABC reachability allocation failed"))?;
                        queue.push(target);
                    }
                }
            }
            if let Some(target) = instruction.branch {
                let target = Self::block_at(&blocks, instructions, target)?;
                if !reachable[target] {
                    queue
                        .try_reserve(1)
                        .map_err(|_| resource("ABC reachability allocation failed"))?;
                    queue.push(target);
                }
            }
            if (!instruction.opcode.is_jump() || instruction.opcode.is_conditional())
                && !instruction.opcode.is_return()
                && instruction.opcode.name != "throw"
            {
                if index + 1 >= blocks.len() {
                    return Err(invalid(
                        "reachable ABC control flow falls past the end of the method",
                    ));
                }
                if !reachable[index + 1] {
                    queue
                        .try_reserve(1)
                        .map_err(|_| resource("ABC reachability allocation failed"))?;
                    queue.push(index + 1);
                }
            }
        }
        Ok(Self { blocks, reachable })
    }

    pub fn target(&self, instructions: &[Instruction], pc: u32) -> Result<usize> {
        Self::block_at(&self.blocks, instructions, pc)
    }

    fn block_at(blocks: &[Block], instructions: &[Instruction], pc: u32) -> Result<usize> {
        blocks
            .binary_search_by_key(&pc, |block| instructions[block.first].pc)
            .map_err(|_| invalid("ABC branch does not target a basic block"))
    }
}

pub fn unsupported(message: impl Into<String>) -> Error {
    Error::new("DECOMPILE_FAILED", message)
}
