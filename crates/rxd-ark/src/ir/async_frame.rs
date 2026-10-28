use crate::{Decoded, Operand, instructions::register_range, ir::lift::unsupported};
use rxd_core::Result;
use std::collections::HashSet;

pub(super) struct AsyncFrame {
    pub skip: HashSet<u32>,
}

// Normalize only a complete promise frame and the explicit compiler await/resume sequence.
pub(super) fn normalize(decoded: &mut Decoded<'_>) -> Result<AsyncFrame> {
    let instructions = &decoded.instructions;
    let enter = instructions
        .iter()
        .position(|instruction| instruction.opcode.name == "asyncfunctionenter")
        .ok_or_else(|| unsupported(0, "async function lacks promise frame"))?;
    if let Some(instruction) = instructions[..enter].iter().find(|instruction| {
        !matches!(
            instruction.opcode.name,
            "mov"
                | "lda"
                | "sta"
                | "nop"
                | "newlexenv"
                | "wide.newlexenv"
                | "newlexenvwithname"
                | "wide.newlexenvwithname"
                | "stlexvar"
                | "wide.stlexvar"
                | "ldlexvar"
                | "wide.ldlexvar"
        )
    }) {
        return Err(unsupported(
            instruction.pc,
            "effectful prefix before promise frame",
        ));
    }
    let save = instructions
        .get(enter + 1)
        .filter(|instruction| instruction.opcode.name == "sta")
        .ok_or_else(|| unsupported(instructions[enter].pc, "async promise is not saved"))?;
    let object = save.register(0)?;
    let count = instructions.len();
    if count < 3 {
        return Err(unsupported(save.pc, "truncated async frame"));
    }
    let handler = &instructions[count - 3..];
    if handler[0].opcode.name != "sta"
        || handler[1].opcode.name != "asyncfunctionreject"
        || handler[1].register(0)? != object
        || handler[2].opcode.name != "return"
    {
        return Err(unsupported(save.pc, "async rejection frame shape"));
    }
    let region = decoded
        .code
        .tries
        .iter()
        .position(|region| {
            region.pc == save.end()
                && region.pc + region.length == handler[0].pc
                && region.catches.len() == 1
                && region.catches[0].pc == handler[0].pc
                && region.catches[0].type_index == 0
                && region.catches[0].length == handler[2].end() - handler[0].pc
        })
        .ok_or_else(|| unsupported(save.pc, "async rejection region is not complete"))?;
    let mut skip = HashSet::from([instructions[enter].pc, save.pc]);
    let mut interior = HashSet::new();
    for (index, instruction) in instructions[..count - 3].iter().enumerate() {
        let name = instruction.opcode.name;
        if name == "asyncfunctionenter" && index != enter {
            return Err(unsupported(instruction.pc, "multiple async frames"));
        }
        if instruction
            .branch
            .is_some_and(|target| target <= save.pc || target >= handler[0].pc)
        {
            return Err(unsupported(instruction.pc, "branch crosses async frame"));
        }
        if name == "asyncfunctionawaituncaught" {
            let shape = [
                "suspendgenerator",
                "lda",
                "resumegenerator",
                "sta",
                "lda",
                "getresumemode",
            ];
            if instruction.register(0)? != object
                || shape.iter().enumerate().any(|(offset, name)| {
                    instructions
                        .get(index + 1 + offset)
                        .is_none_or(|next| next.opcode.name != *name)
                })
            {
                return Err(unsupported(instruction.pc, "await/resume protocol"));
            }
            let dispatch = ["sta", "ldai", "eq", "jeqz", "lda", "throw"];
            if dispatch.iter().enumerate().any(|(offset, name)| {
                instructions
                    .get(index + 7 + offset)
                    .is_none_or(|next| next.opcode.name != *name)
            }) || !matches!(
                instructions[index + 8].operands.first(),
                Some(Operand::Signed(1))
            ) || instructions[index + 9].register(1)? != instructions[index + 7].register(0)?
                || instructions[index + 11].register(0)? != instructions[index + 4].register(0)?
                || instructions
                    .get(index + 13)
                    .is_none_or(|next| instructions[index + 10].branch != Some(next.pc))
            {
                return Err(unsupported(
                    instruction.pc,
                    "async rejection is not a verified throw dispatch",
                ));
            }
            for member in &instructions[index + 1..=index + 12] {
                interior.insert(member.pc);
                if decoded.code.tries.iter().any(|region| {
                    let contains = |pc| region.pc <= pc && pc < region.pc + region.length;
                    contains(instruction.pc) != contains(member.pc)
                }) {
                    return Err(unsupported(
                        member.pc,
                        "await dispatch crosses an exception region",
                    ));
                }
            }
            for offset in [1, 2, 5] {
                let member = &instructions[index + offset];
                if member.register(0)? != object {
                    return Err(unsupported(member.pc, "await switches promise objects"));
                }
                skip.insert(member.pc);
            }
        }
        if register_range(instruction)?.is_some_and(|range| range.contains(&u64::from(object))) {
            return Err(unsupported(
                instruction.pc,
                "promise escapes through argument range",
            ));
        }
        if index == enter + 1 || skip.contains(&instruction.pc) {
            continue;
        }
        if instruction
            .operands
            .iter()
            .any(|operand| matches!(operand, Operand::Register(register) if *register == object))
            && !matches!(name, "asyncfunctionresolve" | "asyncfunctionawaituncaught")
        {
            return Err(unsupported(
                instruction.pc,
                "promise escapes its async frame",
            ));
        }
        if name == "asyncfunctionresolve" {
            if instruction.register(0)? != object
                || instructions
                    .get(index + 1)
                    .is_none_or(|next| next.opcode.name != "return")
            {
                return Err(unsupported(
                    instruction.pc,
                    "async resolution is not terminal",
                ));
            }
            let return_pc = instructions[index + 1].pc;
            interior.insert(return_pc);
            if decoded.code.tries.iter().any(|region| {
                let contains = |pc| region.pc <= pc && pc < region.pc + region.length;
                contains(instruction.pc) != contains(return_pc)
            }) {
                return Err(unsupported(return_pc, "async return crosses a handler"));
            }
            skip.insert(instruction.pc);
        }
        if matches!(name, "return" | "returnundefined")
            && (name != "return"
                || index == 0
                || instructions[index - 1].opcode.name != "asyncfunctionresolve")
        {
            return Err(unsupported(
                instruction.pc,
                "async return bypasses resolution",
            ));
        }
        if matches!(name, "resumegenerator" | "getresumemode") {
            let expected = if name == "resumegenerator" { 3 } else { 6 };
            if index < expected
                || instructions[index - expected].opcode.name != "asyncfunctionawaituncaught"
            {
                return Err(unsupported(instruction.pc, "resume outside verified await"));
            }
        }
    }
    if let Some(branch) = instructions.iter().find(|instruction| {
        instruction
            .branch
            .is_some_and(|target| interior.contains(&target))
    }) {
        return Err(unsupported(
            branch.pc,
            "branch enters async protocol interior",
        ));
    }
    if let Some(catch) = decoded
        .code
        .tries
        .iter()
        .flat_map(|region| &region.catches)
        .find(|catch| interior.contains(&catch.pc))
    {
        return Err(unsupported(
            catch.pc,
            "handler enters async protocol interior",
        ));
    }
    decoded.code.tries.remove(region);
    decoded.instructions.truncate(count - 3);
    Ok(AsyncFrame { skip })
}
