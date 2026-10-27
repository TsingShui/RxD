use crate::{
    Abc, Code, Method,
    reader::{invalid, resource},
};
use rxd_core::{Error, Result};
use std::io::Write;

fn output_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::OutOfMemory {
        resource(error.to_string())
    } else {
        error.into()
    }
}

#[path = "opcodes.rs"]
mod opcodes;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    String,
    Method,
    Literal,
    Type,
    Field,
}

#[derive(Debug, Clone, Copy)]
pub enum OperandType {
    Register,
    Unsigned,
    Signed,
    Float,
    Id(EntityKind),
}

pub struct Parameter {
    pub kind: OperandType,
    pub bits: u8,
}

pub struct Opcode {
    pub code: u16,
    pub name: &'static str,
    pub format: &'static str,
    pub size: u8,
    pub flags: u8,
    pub parameters: &'static [Parameter],
    pub properties: &'static [&'static str],
}

impl Opcode {
    pub fn is_jump(&self) -> bool {
        self.flags & 1 != 0
    }
    pub fn is_conditional(&self) -> bool {
        self.flags & 2 != 0
    }
    pub fn is_return(&self) -> bool {
        self.flags & 4 != 0
    }
    pub fn reads_acc(&self) -> bool {
        self.flags & 8 != 0
    }
    pub fn writes_acc(&self) -> bool {
        self.flags & 16 != 0
    }
}

#[derive(Debug, Clone)]
pub enum Operand {
    Register(u32),
    Unsigned(u64),
    Signed(i64),
    Float(u64),
    Id {
        kind: EntityKind,
        index: u32,
        offset: u32,
    },
}

pub struct Instruction {
    pub pc: u32,
    pub opcode: &'static Opcode,
    pub operands: Vec<Operand>,
    pub branch: Option<u32>,
}

impl Instruction {
    pub fn end(&self) -> u32 {
        self.pc + u32::from(self.opcode.size)
    }

    pub fn unsigned(&self, index: usize) -> Result<u32> {
        match self.operands.get(index) {
            Some(Operand::Unsigned(value)) => {
                u32::try_from(*value).map_err(|_| invalid("ABC immediate exceeds u32"))
            }
            _ => Err(invalid(format!(
                "{} operand {index} is not unsigned",
                self.opcode.name
            ))),
        }
    }

    pub fn register(&self, index: usize) -> Result<u32> {
        match self.operands.get(index) {
            Some(Operand::Register(value)) => Ok(*value),
            _ => Err(invalid(format!(
                "{} operand {index} is not a register",
                self.opcode.name
            ))),
        }
    }

    pub fn entity(&self, index: usize, expected: EntityKind) -> Result<u32> {
        match self.operands.get(index) {
            Some(Operand::Id { kind, offset, .. }) if *kind == expected => Ok(*offset),
            _ => Err(invalid(format!(
                "{} operand {index} has the wrong entity kind",
                self.opcode.name
            ))),
        }
    }
}

pub struct Decoded<'a> {
    pub method: u32,
    pub code: Code<'a>,
    pub instructions: Vec<Instruction>,
}

fn read_bits(bytes: &[u8], at: usize, width: u8) -> u64 {
    if width == 4 {
        return u64::from((bytes[at / 8] >> (at % 8)) & 15);
    }
    let mut value = 0u64;
    for index in 0..usize::from(width / 8) {
        value |= u64::from(bytes[at / 8 + index]) << (index * 8);
    }
    value
}

fn signed(value: u64, width: u8) -> i64 {
    let shift = 64 - width;
    ((value << shift) as i64) >> shift
}

fn input_reference(error: Error) -> Error {
    if matches!(error.code, "NOT_FOUND" | "INVALID_QUERY") {
        invalid(format!(
            "ABC operand names no declared entity: {}",
            error.message
        ))
    } else {
        error
    }
}

pub(crate) fn register_range(instruction: &Instruction) -> Result<Option<std::ops::Range<u64>>> {
    let name = instruction.opcode.name;
    let base = name
        .strip_prefix("wide.")
        .or_else(|| name.strip_prefix("deprecated."))
        .unwrap_or(name);
    if !matches!(
        base,
        "callrange"
            | "callthisrange"
            | "supercallthisrange"
            | "supercallarrowrange"
            | "newobjrange"
    ) {
        return Ok(None);
    }
    let (Some(Operand::Register(start)), Some(Operand::Unsigned(count))) = (
        instruction.operands.last(),
        instruction
            .operands
            .iter()
            .rev()
            .find(|operand| matches!(operand, Operand::Unsigned(_))),
    ) else {
        return Err(invalid(
            "ABC register-range instruction has invalid operands",
        ));
    };
    if base == "newobjrange" && *count == 0 {
        return Err(invalid("ABC newobjrange has no constructor register"));
    }
    let extra = u64::from(
        base == "callthisrange" || base == "callrange" && name.starts_with("deprecated."),
    );
    let end = u64::from(*start)
        .checked_add(*count)
        .and_then(|end| end.checked_add(extra))
        .ok_or_else(|| invalid("ABC variadic argument register range overflows"))?;
    Ok(Some(u64::from(*start)..end))
}

fn range_registers(instruction: &Instruction, registers: u32) -> Result<()> {
    if register_range(instruction)?.is_some_and(|range| range.end > u64::from(registers)) {
        return Err(invalid(
            "ABC variadic argument register range exceeds the frame",
        ));
    }
    Ok(())
}

impl<'a> Abc<'a> {
    pub fn decode(&self, method: &Method) -> Result<Decoded<'a>> {
        let code = self.code(method)?;
        let registers = code
            .vregs
            .checked_add(code.args)
            .ok_or_else(|| invalid("ABC register count overflow"))?;
        let mut instructions = Vec::new();
        let mut pc = 0usize;
        while pc < code.bytes.len() {
            let first = code.bytes[pc];
            let (encoded, header) = if opcodes::is_prefix(first) {
                let second = code
                    .bytes
                    .get(pc + 1)
                    .ok_or_else(|| invalid("truncated ABC opcode prefix"))?;
                (u16::from(first) | (u16::from(*second) << 8), 2)
            } else {
                (u16::from(first), 1)
            };
            let index = opcodes::OPCODES
                .binary_search_by_key(&encoded, |opcode| opcode.code)
                .map_err(|_| {
                    invalid(format!(
                        "unknown ABC opcode {encoded:#x} at method:0x{:x}+{pc:#x}",
                        method.offset
                    ))
                })?;
            let opcode = &opcodes::OPCODES[index];
            let end = pc
                .checked_add(usize::from(opcode.size))
                .filter(|end| *end <= code.bytes.len())
                .ok_or_else(|| invalid("truncated ABC instruction operands"))?;
            self.charge_decode(1 + opcode.parameters.len())?;
            let mut operands = Vec::new();
            operands
                .try_reserve_exact(opcode.parameters.len())
                .map_err(|_| resource("ABC operand allocation failed"))?;
            let mut position = 0usize;
            let body = &code.bytes[pc + header..end];
            for parameter in opcode.parameters {
                let value = read_bits(body, position, parameter.bits);
                position += usize::from(parameter.bits);
                let operand = match parameter.kind {
                    OperandType::Register => {
                        if value >= u64::from(registers) {
                            return Err(invalid("ABC register operand exceeds the frame"));
                        }
                        Operand::Register(value as u32)
                    }
                    OperandType::Unsigned => Operand::Unsigned(value),
                    OperandType::Signed => Operand::Signed(signed(value, parameter.bits)),
                    OperandType::Float => Operand::Float(value),
                    OperandType::Id(kind) => {
                        let index = u32::try_from(value)
                            .map_err(|_| invalid("ABC entity index overflow"))?;
                        let offset = self.resolve_entity(method, index)?;
                        match kind {
                            EntityKind::String => {
                                self.name(offset)?;
                            }
                            EntityKind::Method => {
                                self.method(offset).map_err(input_reference)?;
                            }
                            EntityKind::Literal => {
                                self.literals(offset)?;
                            }
                            EntityKind::Type | EntityKind::Field => {
                                return Err(Error::new(
                                    "UNSUPPORTED_INPUT",
                                    "static ABC entity operand is outside dynamic bytecode support",
                                ));
                            }
                        }
                        Operand::Id {
                            kind,
                            index,
                            offset,
                        }
                    }
                };
                operands.push(operand);
            }
            let branch = if opcode.is_jump() {
                let displacement = operands
                    .iter()
                    .rev()
                    .find_map(|operand| match operand {
                        Operand::Signed(value) => Some(*value),
                        _ => None,
                    })
                    .ok_or_else(|| invalid("ABC branch lacks a signed displacement"))?;
                let target = (pc as i64)
                    .checked_add(displacement)
                    .filter(|target| *target >= 0 && (*target as usize) < code.bytes.len())
                    .ok_or_else(|| invalid("ABC branch target is outside its code item"))?;
                Some(target as u32)
            } else {
                None
            };
            let instruction = Instruction {
                pc: pc as u32,
                opcode,
                operands,
                branch,
            };
            range_registers(&instruction, registers)?;
            instructions
                .try_reserve(1)
                .map_err(|_| resource("ABC instruction allocation failed"))?;
            instructions.push(instruction);
            pc = end;
        }
        let boundary = |pc: u32, allow_end: bool| -> bool {
            allow_end && pc as usize == code.bytes.len()
                || instructions
                    .binary_search_by_key(&pc, |instruction| instruction.pc)
                    .is_ok()
        };
        for instruction in &instructions {
            if instruction
                .branch
                .is_some_and(|target| !boundary(target, false))
            {
                return Err(invalid("ABC branch target is not an instruction boundary"));
            }
        }
        for block in &code.tries {
            if !boundary(block.pc, false) || !boundary(block.pc + block.length, true) {
                return Err(invalid(
                    "ABC try range is not aligned to instruction boundaries",
                ));
            }
            for handler in &block.catches {
                if !boundary(handler.pc, false) || !boundary(handler.pc + handler.length, true) {
                    return Err(invalid(
                        "ABC handler range is not aligned to instruction boundaries",
                    ));
                }
            }
        }
        Ok(Decoded {
            method: method.offset,
            code,
            instructions,
        })
    }

    fn charge_decode(&self, work: usize) -> Result<()> {
        let count = self
            .decoded_instructions
            .get()
            .checked_add(1)
            .filter(|count| self.budget.instructions.is_none_or(|limit| *count <= limit))
            .ok_or_else(|| resource("ABC explicit instruction budget exceeded"))?;
        let work = self
            .decode_work
            .get()
            .checked_add(work)
            .filter(|work| self.budget.work.is_none_or(|limit| *work <= limit))
            .ok_or_else(|| resource("ABC explicit decode work budget exceeded"))?;
        self.decoded_instructions.set(count);
        self.decode_work.set(work);
        Ok(())
    }

    fn write_instruction(
        &self,
        instruction: &Instruction,
        output: &mut rxd_core::output::Output,
    ) -> Result<()> {
        output
            .write_all(instruction.opcode.name.as_bytes())
            .map_err(output_error)?;
        for (index, operand) in instruction.operands.iter().enumerate() {
            output
                .write_all(if index == 0 { b" " } else { b", " })
                .map_err(output_error)?;
            match operand {
                Operand::Register(value) => write!(output, "v{value}").map_err(output_error)?,
                Operand::Unsigned(value) => write!(output, "{value}").map_err(output_error)?,
                Operand::Signed(value) => write!(output, "{value}").map_err(output_error)?,
                Operand::Float(bits) => {
                    write!(output, "f64:0x{bits:016x}").map_err(output_error)?
                }
                Operand::Id { kind, offset, .. } => {
                    let prefix = match kind {
                        EntityKind::String => "str",
                        EntityKind::Method => "method",
                        EntityKind::Literal => "literal",
                        EntityKind::Type => "type",
                        EntityKind::Field => "field",
                    };
                    write!(output, "{prefix}:0x{offset:x}").map_err(output_error)?;
                    if *kind == EntityKind::String {
                        output.write_all(b" ").map_err(output_error)?;
                        self.name(*offset)?
                            .write_quoted(output)
                            .map_err(output_error)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn instruction_text(&self, instruction: &Instruction) -> Result<String> {
        let mut output = rxd_core::output::Output::new(self.budget.output_bytes);
        self.write_instruction(instruction, &mut output)?;
        String::from_utf8(output.into_bytes())
            .map_err(|_| Error::new("INTERNAL_ERROR", "invalid instruction text"))
    }

    fn write_raw_method(
        &self,
        method: &Method,
        output: &mut rxd_core::output::Output,
    ) -> Result<()> {
        if method.code_offset.is_none() {
            writeln!(output, "// method:0x{:x} has no code", method.offset)
                .map_err(output_error)?;
            return Ok(());
        }
        let decoded = self.decode(method)?;
        write!(output, "// method:0x{:x} ", method.offset).map_err(output_error)?;
        method.name.write_quoted(output).map_err(output_error)?;
        writeln!(
            output,
            " vregs={} args={} code_offset=0x{:x}",
            decoded.code.vregs, decoded.code.args, decoded.code.instruction_offset
        )
        .map_err(output_error)?;
        for instruction in &decoded.instructions {
            write!(output, "{:#x}\t", instruction.pc).map_err(output_error)?;
            self.write_instruction(instruction, output)?;
            output.write_all(b"\n").map_err(output_error)?;
        }
        Ok(())
    }

    pub fn raw_target(&self, target: crate::Coordinate) -> Result<Vec<u8>> {
        let mut output = rxd_core::output::Output::new(self.budget.output_bytes);
        match target {
            crate::Coordinate::Method(offset) => {
                self.write_raw_method(&self.method(offset)?, &mut output)?
            }
            crate::Coordinate::Record(offset) => {
                let record = self.record(offset)?;
                write!(&mut output, "// record:0x{offset:x} ").map_err(output_error)?;
                record
                    .name
                    .write_quoted(&mut output)
                    .map_err(output_error)?;
                output.write_all(b"\n").map_err(output_error)?;
                for method in &self.members(&record)?.methods {
                    self.write_raw_method(method, &mut output)?;
                }
            }
        }
        Ok(output.into_bytes())
    }
}
