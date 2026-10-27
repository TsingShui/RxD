use crate::{Abc, Decoded, Operand, reader::resource, structure::unsupported};
use rxd_core::Result;

pub struct AsyncPlan {
    pub enter: u32,
    pub save: u32,
    pub reject_return: u32,
}

// Only accept a complete compiler-generated async wrapper without await or user catch code.
pub fn simple_async(abc: &Abc<'_>, decoded: &Decoded<'_>) -> Result<Option<AsyncPlan>> {
    let instructions = &decoded.instructions;
    let Some(index) = instructions
        .iter()
        .position(|instruction| instruction.opcode.name == "asyncfunctionenter")
    else {
        return Ok(None);
    };
    let reject = || unsupported("async state machine requires additional analysis");
    if instructions[..index]
        .iter()
        .any(|instruction| !matches!(instruction.opcode.name, "mov" | "nop"))
        || instructions[index + 1..]
            .iter()
            .any(|instruction| instruction.opcode.name == "asyncfunctionenter")
    {
        return Err(reject());
    }
    let save = instructions.get(index + 1).ok_or_else(reject)?;
    if save.opcode.name != "sta" {
        return Err(reject());
    }
    let object = save.register(0)?;
    let [region] = decoded.code.tries.as_slice() else {
        return Err(reject());
    };
    let [handler] = region.catches.as_slice() else {
        return Err(reject());
    };
    if handler.type_index != 0
        || region.pc != save.end()
        || region.pc + region.length != handler.pc
        || handler.pc + handler.length != decoded.code.bytes.len() as u32
    {
        return Err(reject());
    }
    let handler_index = instructions
        .binary_search_by_key(&handler.pc, |instruction| instruction.pc)
        .map_err(|_| reject())?;
    let catch = &instructions[handler_index..];
    if catch.len() != 3
        || catch[0].opcode.name != "sta"
        || catch[1].opcode.name != "asyncfunctionreject"
        || catch[1].register(0)? != object
        || catch[2].opcode.name != "return"
    {
        return Err(reject());
    }
    abc.check_storage::<u32>(instructions.len())?;
    let mut branches = Vec::new();
    branches
        .try_reserve_exact(instructions.len())
        .map_err(|_| resource("ABC async branch allocation failed"))?;
    branches.extend(
        instructions
            .iter()
            .filter_map(|instruction| instruction.branch),
    );
    branches.sort_unstable();
    if branches.first().is_some_and(|target| *target <= save.pc) {
        return Err(reject());
    }
    for (offset, instruction) in instructions.iter().enumerate().skip(index + 2) {
        let name = instruction.opcode.name;
        if crate::instructions::register_range(instruction)?
            .is_some_and(|range| range.contains(&u64::from(object)))
        {
            return Err(reject());
        }
        if instruction
            .operands
            .iter()
            .any(|operand| matches!(operand, Operand::Register(register) if *register == object))
            && !matches!(name, "asyncfunctionresolve" | "asyncfunctionreject")
        {
            return Err(reject());
        }
        if matches!(name, "asyncfunctionresolve" | "asyncfunctionreject") {
            if instruction.register(0)? != object
                || instructions
                    .get(offset + 1)
                    .is_none_or(|next| next.opcode.name != "return")
            {
                return Err(reject());
            }
        } else if name == "return" {
            if offset == 0
                || !matches!(
                    instructions[offset - 1].opcode.name,
                    "asyncfunctionresolve" | "asyncfunctionreject"
                )
                || branches.binary_search(&instruction.pc).is_ok()
            {
                return Err(reject());
            }
        } else if name == "returnundefined"
            || name.starts_with("asyncfunction")
            || name.contains("generator")
            || name == "getresumemode"
        {
            return Err(reject());
        }
    }
    Ok(Some(AsyncPlan {
        enter: instructions[index].pc,
        save: save.pc,
        reject_return: catch[2].pc,
    }))
}
