use crate::{
    Abc, Decoded, Operand, instructions::register_range, reader::resource, structure::unsupported,
};
use rxd_core::Result;

pub struct ArrowPlan {
    skipped: Vec<u32>,
}

impl ArrowPlan {
    pub fn skips(&self, pc: u32) -> bool {
        self.skipped.binary_search(&pc).is_ok()
    }
}

// Only discard call-frame this copies when no other instruction can observe them.
pub fn arrow_plan(abc: &Abc<'_>, decoded: &Decoded<'_>, call_type: u8) -> Result<ArrowPlan> {
    let instructions = &decoded.instructions;
    let reject = || unsupported("arrow function needs unavailable invocation-frame semantics");
    let this = if call_type & 1 != 0 {
        Some(decoded.code.vregs + u32::from(call_type & 8 != 0) + u32::from(call_type & 2 != 0))
    } else {
        None
    };
    abc.check_storage::<u32>(
        instructions
            .len()
            .checked_mul(2)
            .ok_or_else(|| resource("ABC arrow analysis overflow"))?,
    )?;
    let mut skipped = Vec::new();
    let mut destinations = Vec::new();
    skipped
        .try_reserve_exact(instructions.len())
        .map_err(|_| resource("ABC arrow analysis allocation failed"))?;
    destinations
        .try_reserve_exact(instructions.len())
        .map_err(|_| resource("ABC arrow analysis allocation failed"))?;
    if let Some(this) = this {
        for instruction in instructions {
            if instruction.opcode.name == "mov" && instruction.register(1)? == this {
                let destination = instruction.register(0)?;
                if destination >= decoded.code.vregs {
                    return Err(reject());
                }
                skipped.push(instruction.pc);
                destinations.push(destination);
            }
        }
    }
    destinations.sort_unstable();
    destinations.dedup();
    for instruction in instructions {
        if skipped.binary_search(&instruction.pc).is_ok() {
            continue;
        }
        if matches!(
            instruction.opcode.name,
            "ldthis"
                | "ldthisbyname"
                | "stthisbyname"
                | "ldthisbyvalue"
                | "stthisbyvalue"
                | "getunmappedargs"
                | "copyrestargs"
                | "wide.copyrestargs"
        ) {
            return Err(reject());
        }
        for operand in &instruction.operands {
            if let Operand::Register(register) = operand
                && (Some(*register) == this || destinations.binary_search(register).is_ok())
            {
                return Err(reject());
            }
        }
        if let Some(range) = register_range(instruction)? {
            if this.is_some_and(|register| range.contains(&u64::from(register))) {
                return Err(reject());
            }
            let first = destinations.partition_point(|register| u64::from(*register) < range.start);
            if destinations
                .get(first)
                .is_some_and(|register| range.contains(&u64::from(*register)))
            {
                return Err(reject());
            }
        }
    }
    Ok(ArrowPlan { skipped })
}
