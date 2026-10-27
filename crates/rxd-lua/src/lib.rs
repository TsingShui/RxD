//! Bounded Lua/LuaJIT/Luau bytecode parsing through the pinned unluac parser.
//!
//! Listings read the raw proto tree only; `decompile` runs the full pipeline, so
//! navigation stays cheap and reconstruction remains an explicit request.
use std::path::{Path, PathBuf};

use rxd_core::{Error, Result};
use unluac::decompile::{DecompileDialect, DecompileOptions, decompile};
use unluac::parser::{ParseOptions, detect_dialect, parse_chunk_with_dialect};
use unluac::parser::{RawChunk, RawLiteralConst, RawProto, RawString, format_raw_instr};

/// One admitted chunk: the input bytes and their parsed proto tree.
pub struct Chunk {
    #[allow(dead_code)]
    path: PathBuf,
    bytes: Vec<u8>,
    dialect: DecompileDialect,
    raw: RawChunk,
}

/// A proto with its listing coordinate and row label.
pub struct Code {
    pub at: String,
    pub parent_at: Option<String>,
    pub label: String,
}

/// A string constant with its owning proto and pool index.
pub struct StringConst {
    pub at: String,
    pub value: String,
    pub utf8: bool,
}

impl Chunk {
    /// Read one local chunk; the dialect comes from its header.
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|error| Error::new("IO_ERROR", format!("{}: {error}", path.display())))?;
        Self::parse(path, bytes, DecompileDialect::Auto)
    }

    pub fn parse(path: &Path, bytes: Vec<u8>, requested: DecompileDialect) -> Result<Self> {
        let dialect = match requested {
            DecompileDialect::Auto => detect_dialect(&bytes).map_err(parse_error)?,
            explicit => explicit,
        };
        let raw = parse_chunk_with_dialect(dialect, &bytes, ParseOptions::default())
            .map_err(parse_error)?;
        Ok(Self {
            path: path.to_path_buf(),
            bytes,
            dialect,
            raw,
        })
    }

    pub fn input_bytes(&self) -> usize {
        self.bytes.len()
    }

    pub fn dialect(&self) -> &'static str {
        dialect_name(self.dialect)
    }

    /// Depth-first proto order, which is the order `codes.at` addresses.
    pub fn codes(&self) -> Vec<Code> {
        let mut protos = Vec::new();
        push(&self.raw.main, &mut protos);
        let mut codes = Vec::new();
        for (index, proto) in protos.iter().enumerate() {
            codes.push(Code {
                at: format!("proto:{index}"),
                parent_at: parent_of(&protos, index),
                label: format!(
                    "params={} vararg={} lines={}-{} instructions={}",
                    proto.common.signature.num_params,
                    proto.common.signature.is_vararg,
                    proto.common.line_range.defined_start,
                    proto.common.line_range.defined_end,
                    proto.common.instructions.len()
                ),
            });
        }
        codes
    }

    /// String constants in proto order with the pool index that addresses them.
    pub fn strings(&self) -> Vec<StringConst> {
        let protos = self.protos();
        let mut strings = Vec::new();
        for (code, proto) in self.codes().iter().zip(&protos) {
            for (slot, constant) in proto.common.constants.common.literals.iter().enumerate() {
                let RawLiteralConst::String(text) = constant else {
                    continue;
                };
                strings.push(StringConst {
                    at: format!("{}:const:{slot}", code.at),
                    value: string_text(text),
                    utf8: text.text.is_some(),
                });
            }
        }
        strings
    }

    /// Instructions of one proto in original byte order.
    pub fn instructions(&self, at: &str) -> Result<Vec<Instruction>> {
        let index = at
            .strip_prefix("proto:")
            .and_then(|rest| rest.parse::<usize>().ok())
            .ok_or_else(|| Error::new("INVALID_QUERY", format!("unknown code coordinate {at}")))?;
        let protos = self.protos();
        let proto = protos
            .get(index)
            .ok_or_else(|| Error::new("INVALID_QUERY", format!("unknown code coordinate {at}")))?;
        Ok(proto
            .common
            .instructions
            .iter()
            .map(|instruction| Instruction {
                offset: instruction.origin.span.offset,
                text: format_raw_instr(instruction),
            })
            .collect())
    }

    /// Instructions that name one string constant through an explicit operand.
    ///
    /// Only opcodes whose operand *is* a constant index are reported; dialects
    /// without a verified mapping fail instead of returning an empty result.
    pub fn xref(&self, at: &str) -> Result<Vec<Reference>> {
        let (code_at, slot) = parse_constant_coordinate(at)?;
        let index = proto_index(&code_at).ok_or_else(|| {
            Error::new(
                "INVALID_QUERY",
                format!("unknown code coordinate {code_at}"),
            )
        })?;
        let protos = self.protos();
        let proto = protos.get(index).ok_or_else(|| {
            Error::new(
                "INVALID_QUERY",
                format!("unknown code coordinate {code_at}"),
            )
        })?;
        let mut references = Vec::new();
        for (position, instruction) in proto.common.instructions.iter().enumerate() {
            if constant_operand(self.dialect, instruction)? == Some(slot) {
                references.push(Reference {
                    at: format!("{code_at}:ins:{position}"),
                    context_at: code_at.clone(),
                    instruction: format_raw_instr(instruction),
                });
            }
        }
        Ok(references)
    }

    /// Reconstruct whole-chunk source through the unluac pipeline.
    pub fn decompile(&self) -> Result<String> {
        let options = DecompileOptions {
            dialect: self.dialect,
            ..DecompileOptions::default()
        };
        let result = decompile(&self.bytes, options)
            .map_err(|error| Error::new("DECOMPILE_FAILED", error.to_string()))?;
        result
            .state
            .generated
            .map(|chunk| chunk.source)
            .ok_or_else(|| Error::new("DECOMPILE_INCOMPLETE", "no source was generated"))
    }

    fn protos(&self) -> Vec<&RawProto> {
        let mut protos = Vec::new();
        push(&self.raw.main, &mut protos);
        protos
    }
}

/// A constant load: the instruction that names the constant, with its owner.
pub struct Reference {
    pub at: String,
    pub context_at: String,
    pub instruction: String,
}

/// One decoded instruction with its original byte offset.
pub struct Instruction {
    pub offset: usize,
    pub text: String,
}

fn parse_constant_coordinate(at: &str) -> Result<(String, usize)> {
    let (code_at, slot) = at.rsplit_once(":const:").ok_or_else(|| {
        Error::new(
            "INVALID_QUERY",
            format!("expected proto:N:const:I, got {at}"),
        )
    })?;
    let slot = slot.parse::<usize>().map_err(|_| {
        Error::new(
            "INVALID_QUERY",
            format!("expected a constant index in {at}"),
        )
    })?;
    Ok((code_at.to_owned(), slot))
}

fn proto_index(at: &str) -> Option<usize> {
    at.strip_prefix("proto:")?.parse().ok()
}

/// The constant index an instruction explicitly names, if any.
fn constant_operand(
    dialect: DecompileDialect,
    instruction: &unluac::parser::RawInstr,
) -> Result<Option<usize>> {
    use unluac::parser::{Lua51Opcode, Lua51Operands, RawInstrOpcode, RawInstrOperands};
    let index = match (&instruction.opcode, &instruction.operands) {
        // All supported PUC Lua versions load a pool constant by absolute index.
        (
            RawInstrOpcode::Lua51(Lua51Opcode::LoadK),
            RawInstrOperands::Lua51(Lua51Operands::ABx { bx, .. }),
        ) => Some(*bx),
        (RawInstrOpcode::Lua52(op), RawInstrOperands::Lua52(operands)) => {
            lua52_constant(op, operands)
        }
        (RawInstrOpcode::Lua53(op), RawInstrOperands::Lua53(operands)) => {
            lua53_constant(op, operands)
        }
        (RawInstrOpcode::Lua54(op), RawInstrOperands::Lua54(operands)) => {
            lua54_constant(op, operands)
        }
        (RawInstrOpcode::LuaJit(_), _) | (RawInstrOpcode::Luau(_), _) => {
            return Err(Error::new(
                "UNSUPPORTED_DIALECT",
                format!(
                    "constant references are not verified for {}",
                    dialect_name(dialect)
                ),
            ));
        }
        _ => None,
    };
    Ok(index.map(|value| value as usize))
}

fn lua52_constant(
    opcode: &unluac::parser::Lua52Opcode,
    operands: &unluac::parser::Lua52Operands,
) -> Option<u32> {
    use unluac::parser::{Lua52Opcode as Op, Lua52Operands as Args};
    match (opcode, operands) {
        (Op::LoadK, Args::ABx { bx, .. }) => Some(*bx),
        (Op::GetTabUp | Op::SetTabUp, Args::ABC { c, .. }) => rk_constant(*c),
        _ => None,
    }
}

fn lua53_constant(
    opcode: &unluac::parser::Lua53Opcode,
    operands: &unluac::parser::Lua53Operands,
) -> Option<u32> {
    use unluac::parser::{Lua53Opcode as Op, Lua53Operands as Args};
    match (opcode, operands) {
        (Op::LoadK, Args::ABx { bx, .. }) => Some(*bx),
        (Op::GetTabUp | Op::SetTabUp, Args::ABC { c, .. }) => rk_constant(*c),
        _ => None,
    }
}

fn lua54_constant(
    opcode: &unluac::parser::Lua54Opcode,
    operands: &unluac::parser::Lua54Operands,
) -> Option<u32> {
    use unluac::parser::{Lua54Opcode as Op, Lua54Operands as Args};
    match (opcode, operands) {
        (Op::LoadK, Args::ABx { bx, .. }) => Some(*bx),
        (Op::GetTabUp | Op::SetTabUp, Args::ABCk { c, k: true, .. }) => Some(u32::from(*c)),
        (Op::GetTabUp | Op::SetTabUp, Args::ABk { b, k: true, .. }) => Some(u32::from(*b)),
        _ => None,
    }
}

/// Lua 5.2/5.3 RK operands hold a constant index when this bit is set.
fn rk_constant(operand: u16) -> Option<u32> {
    (operand & 0x100 != 0).then(|| u32::from(operand & 0xFF))
}

fn push<'a>(proto: &'a RawProto, protos: &mut Vec<&'a RawProto>) {
    protos.push(proto);
    for child in &proto.common.children {
        push(child, protos);
    }
}

/// Pre-order index of the parent prototype, resolved through child identity.
fn parent_of(protos: &[&RawProto], index: usize) -> Option<String> {
    if index == 0 {
        return None;
    }
    let target = protos[index];
    let mut parent = None;
    for (candidate, proto) in protos.iter().enumerate().take(index) {
        if proto
            .common
            .children
            .iter()
            .any(|child| std::ptr::eq(child.as_ref(), target))
        {
            parent = Some(candidate);
        }
    }
    parent.map(|candidate| format!("proto:{candidate}"))
}

fn string_text(text: &RawString) -> String {
    match &text.text {
        Some(decoded) => decoded.value.to_string(),
        None => String::from_utf8_lossy(&text.bytes).into_owned(),
    }
}

fn dialect_name(dialect: DecompileDialect) -> &'static str {
    match dialect {
        DecompileDialect::Auto => "auto",
        DecompileDialect::Lua51 => "lua5.1",
        DecompileDialect::Lua52 => "lua5.2",
        DecompileDialect::Lua53 => "lua5.3",
        DecompileDialect::Lua54 => "lua5.4",
        DecompileDialect::Lua55 => "lua5.5",
        DecompileDialect::Luajit => "luajit",
        DecompileDialect::Luau => "luau",
    }
}

fn parse_error(error: unluac::parser::ParseError) -> Error {
    Error::new("INVALID_INPUT", error.to_string())
}
