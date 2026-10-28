//! Instruction decoding with full cross-version normalization.
//!
//! The decoder absorbs every historical instruction-encoding difference so
//! that consumers (disassembler, decompiler) see one uniform stream:
//!
//! * pre-3.6 variable width (1 or 3 bytes) vs 3.6+ fixed 2-byte wordcode
//! * EXTENDED_ARG chains folded into the final argument
//! * absolute vs relative jumps, byte vs word-unit targets, and 3.11's
//!   backward-jump variants all resolved to absolute byte offsets
//! * 3.11+ inline CACHE entries skipped
//! * instruction sizes include caches so block-end arithmetic stays in bytes

use std::collections::HashSet;

use crate::code::CodeObject;
use crate::error::{PycError, Result};
use crate::object::PyObject;
use crate::opcode::{Op, OpcodeTable};
use crate::version::PythonVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction {
    /// Byte offset of the logical instruction (after any EXTENDED_ARG prefix).
    pub offset: usize,
    /// Byte offset including the EXTENDED_ARG prefix.
    pub start: usize,
    /// Total bytes consumed: prefix + instruction + trailing CACHE entries.
    pub size: usize,
    pub opcode: u8,
    pub op: Op,
    /// Merged argument (EXTENDED_ARG folded in); 0 for no-arg instructions.
    pub arg: u32,
    pub has_arg: bool,
    /// Normalized absolute jump target in byte offsets, when this is a jump.
    pub target: Option<usize>,
    /// True for relative jumps whose target precedes the instruction end.
    pub is_backward: bool,
    /// Source line, when the line table provides one.
    pub line: Option<u32>,
}

impl Instruction {
    pub fn is_jump(&self) -> bool {
        let _budget_frame = crate::budget::enter();
        self.target.is_some()
    }

    /// Offset immediately after this instruction (including caches).
    pub fn end(&self) -> usize {
        let _budget_frame = crate::budget::enter();
        self.start + self.size
    }
}

/// Decode a code object's instruction stream.
fn decode_core<F: FnMut(usize) -> Result<()>>(
    code: &CodeObject,
    table: &OpcodeTable,
    version: PythonVersion,
    with_lines: bool,
    mut charge: F,
) -> Result<Vec<Instruction>> {
    let _budget_frame = crate::budget::enter();
    let bytes = &code.code;
    if table.wordcode && bytes.len() % 2 != 0 {
        return Err(PycError::InvalidBytecode {
            offset: bytes.len() - 1,
            reason: "truncated wordcode instruction",
        });
    }
    let mut out: Vec<Instruction> = Vec::new();
    let mut i = 0usize;
    let mut ext_arg: u32 = 0;
    let mut ext_count = 0;
    let mut ext_start: Option<usize> = None;

    // Instructions and line starts both have ascending byte offsets. Keep
    // one cursor instead of scanning the line table for every instruction.
    let lines = if with_lines { code.line_starts() } else { &[] };
    let mut next_line = 0;
    let mut current_line = None;
    let mut lookup_line = |off: usize| -> Option<u32> {
        crate::budget::charge(1);

        while next_line < lines.len() && lines[next_line].0 <= off {
            crate::budget::charge(1);
            current_line = Some(lines[next_line].1);
            next_line += 1;
        }
        current_line
    };

    let jumps_in_words = version.jumps_in_words();

    while i < bytes.len() {
        crate::budget::charge(1);
        charge(1)?;
        let opcode = bytes[i];
        if ext_start.is_some() && table.op(opcode) != Op::EXTENDED_ARG && !table.has_arg(opcode) {
            return Err(PycError::InvalidBytecode {
                offset: i,
                reason: "EXTENDED_ARG does not prefix an operand",
            });
        }

        // 3.11+: skip stray CACHE entries defensively (normally consumed below)
        if version.has_cache_entries() && table.name(opcode) == "CACHE" {
            i += 2;
            continue;
        }

        if table.wordcode {
            let arg_byte = bytes[i + 1] as u32;
            let op_start = ext_start.unwrap_or(i);
            let offset = i;
            i += 2;

            if table.op(opcode) == Op::EXTENDED_ARG {
                ext_count += 1;
                if ext_count > 3 {
                    return Err(PycError::InvalidBytecode {
                        offset,
                        reason: "EXTENDED_ARG exceeds 32-bit operand width",
                    });
                }
                ext_arg = ext_arg
                    .checked_add(arg_byte)
                    .and_then(|arg| {
                        crate::budget::charge(1);
                        arg.checked_mul(256)
                    })
                    .ok_or(PycError::InvalidBytecode {
                        offset,
                        reason: "EXTENDED_ARG operand overflow",
                    })?;
                if ext_start.is_none() {
                    ext_start = Some(offset);
                }
                continue;
            }

            let has_arg = table.has_arg(opcode);
            let arg = if has_arg { ext_arg + arg_byte } else { 0 };
            let cache_bytes = if version.has_cache_entries() {
                table.cache_entries(opcode) as usize * 2
            } else {
                0
            };
            if cache_bytes > bytes.len() - i {
                return Err(PycError::InvalidBytecode {
                    offset,
                    reason: "truncated inline cache span",
                });
            }
            crate::budget::charge(cache_bytes / 2);
            charge(cache_bytes / 2)?;
            let size = (offset + 2 + cache_bytes) - op_start;
            let target = normalize_jump(
                table,
                opcode,
                arg,
                offset + 2 + cache_bytes,
                jumps_in_words,
                true,
            );
            out.push(Instruction {
                offset,
                start: op_start,
                size,
                opcode,
                op: table.op(opcode),
                arg,
                has_arg,
                is_backward: target.map_or(false, |t| {
                    crate::budget::charge(1);
                    t < offset + 2 + cache_bytes
                }),
                target,
                line: lookup_line(offset),
            });
            ext_arg = 0;
            ext_count = 0;
            ext_start = None;
            i += cache_bytes;
        } else {
            let op_start = ext_start.unwrap_or(i);
            let offset = i;
            i += 1;

            if table.op(opcode) == Op::EXTENDED_ARG {
                if i + 1 >= bytes.len() {
                    return Err(PycError::InvalidBytecode {
                        offset,
                        reason: "truncated instruction operand",
                    });
                }
                let lo = bytes[i] as u32;
                let hi = bytes[i + 1] as u32;
                i += 2;
                ext_count += 1;
                if ext_count > 1 {
                    return Err(PycError::InvalidBytecode {
                        offset,
                        reason: "EXTENDED_ARG exceeds 32-bit operand width",
                    });
                }
                ext_arg = ext_arg
                    .checked_add(lo | (hi << 8))
                    .and_then(|arg| {
                        crate::budget::charge(1);
                        arg.checked_mul(65536)
                    })
                    .ok_or(PycError::InvalidBytecode {
                        offset,
                        reason: "EXTENDED_ARG operand overflow",
                    })?;
                if ext_start.is_none() {
                    ext_start = Some(offset);
                }
                continue;
            }

            let has_arg = table.has_arg(opcode);
            let mut arg = 0u32;
            if has_arg {
                if i + 1 >= bytes.len() {
                    return Err(PycError::InvalidBytecode {
                        offset,
                        reason: "truncated instruction operand",
                    });
                }
                let lo = bytes[i] as u32;
                let hi = bytes[i + 1] as u32;
                i += 2;
                arg = ext_arg + (lo | (hi << 8));
            }
            let size = i - op_start;
            let target = normalize_jump(table, opcode, arg, i, jumps_in_words, false);
            out.push(Instruction {
                offset,
                start: op_start,
                size,
                opcode,
                op: table.op(opcode),
                arg,
                has_arg,
                is_backward: target.map_or(false, |t| {
                    crate::budget::charge(1);
                    t < i
                }),
                target,
                line: lookup_line(offset),
            });
            ext_arg = 0;
            ext_count = 0;
            ext_start = None;
        }
    }
    if let Some(offset) = ext_start {
        return Err(PycError::InvalidBytecode {
            offset,
            reason: "unterminated EXTENDED_ARG",
        });
    }

    Ok(out)
}

/// One original instruction unit, projected from the shared strict scanner's spans.
#[derive(Debug, Clone)]
pub struct RawInstruction {
    pub offset: usize,
    pub size: usize,
    pub opcode: u8,
    pub raw_arg: Option<u32>,
    pub role: &'static str,
    pub owner_offset: Option<usize>,
    pub effective_arg: Option<u32>,
    pub target: Option<usize>,
}

/// Decode for source recovery, retaining its historical target normalization.
pub fn decode_instructions(
    code: &CodeObject,
    table: &OpcodeTable,
    version: PythonVersion,
) -> Result<Vec<Instruction>> {
    let _budget_frame = crate::budget::enter();
    let mut out = decode_core(code, table, version, true, |_| Ok(()))?;
    // 3.8/3.9 wordcode: a backward `continue`-shaped jump can target a
    // loop top's EXTENDED_ARG PREFIX byte (the FOR_ITER operand needs
    // the extension bits and the compiler aims the jump at the prefix).
    // The prefix is not an instruction start - remap such targets to
    // the guarded instruction so block matching sees the real loop top
    // (_strptime 3.8/3.9 _strptime: the final elif's PJFF->458 opened
    // If[1480,458] with its end BEFORE its start; the Z arm folded
    // empty and its body flattened out of the chain)
    if version.major == 3 && (version.minor == 8 || version.minor == 9) {
        let starts: HashSet<usize> = out
            .iter()
            .map(|x| {
                crate::budget::charge(1);
                x.offset
            })
            .collect();
        for ins in out.iter_mut() {
            crate::budget::charge(1);
            if let Some(t) = ins.target {
                if !starts.contains(&t) && starts.contains(&(t + 2)) {
                    ins.target = Some(t + 2);
                    ins.is_backward = t + 2 < ins.offset + 2;
                }
            }
        }
    }
    Ok(out)
}

/// Lossless raw evidence from the same strict decoder. No line cache or AST is built.
/// Caller must bound code units/work/output before invoking this allocation.
pub fn decode_evidence(
    code: &CodeObject,
    table: &OpcodeTable,
    version: PythonVersion,
) -> Result<(Vec<Instruction>, Vec<RawInstruction>)> {
    decode_evidence_limited(code, table, version, |_| Ok(()))
}
/// Bounded evidence scanner; the supplied cumulative charge runs before allocations.
pub fn decode_evidence_limited<F: FnMut(usize) -> Result<()>>(
    code: &CodeObject,
    table: &OpcodeTable,
    version: PythonVersion,
    charge: F,
) -> Result<(Vec<Instruction>, Vec<RawInstruction>)> {
    decode_evidence_capped(code, table, version, None, charge)
}

/// Enforce an optional original-instruction cap before retaining decoder state.
pub fn decode_evidence_capped<F: FnMut(usize) -> Result<()>>(
    code: &CodeObject,
    table: &OpcodeTable,
    version: PythonVersion,
    max_instructions: Option<usize>,
    mut charge: F,
) -> Result<(Vec<Instruction>, Vec<RawInstruction>)> {
    if table.wordcode && max_instructions.is_some_and(|limit| code.code.len() / 2 > limit) {
        return Err(PycError::ResourceLimit(
            "scanned code instruction budget exceeded",
        ));
    }
    let mut scanned = 0usize;
    let logical = decode_core(code, table, version, false, |amount| {
        scanned = scanned.saturating_add(amount);
        if max_instructions.is_some_and(|limit| scanned > limit) {
            return Err(PycError::ResourceLimit(
                "scanned code instruction budget exceeded",
            ));
        }
        charge(amount)
    })?;
    let units = if table.wordcode {
        code.code.len() / 2
    } else {
        scanned
    };
    charge(units)?;
    let mut raw = Vec::new();
    let mut cursor = 0;
    for ins in &logical {
        // Stray CACHE entries skipped by the legacy projection remain original evidence.
        while cursor < ins.start {
            raw.push(RawInstruction {
                offset: cursor,
                size: 2,
                opcode: code.code[cursor],
                raw_arg: Some(code.code[cursor + 1] as u32),
                role: "cache",
                owner_offset: None,
                effective_arg: None,
                target: None,
            });
            cursor += 2;
        }
        let unit = if table.wordcode { 2 } else { 3 };
        while cursor < ins.offset {
            let arg = if table.wordcode {
                code.code[cursor + 1] as u32
            } else {
                u16::from_le_bytes([code.code[cursor + 1], code.code[cursor + 2]]) as u32
            };
            raw.push(RawInstruction {
                offset: cursor,
                size: unit,
                opcode: code.code[cursor],
                raw_arg: Some(arg),
                role: "extended_arg",
                owner_offset: Some(ins.offset),
                effective_arg: None,
                target: None,
            });
            cursor += unit;
        }
        let size = if table.wordcode {
            2
        } else if ins.has_arg {
            3
        } else {
            1
        };
        let arg = if table.wordcode {
            Some(code.code[cursor + 1] as u32)
        } else if ins.has_arg {
            Some(u16::from_le_bytes([code.code[cursor + 1], code.code[cursor + 2]]) as u32)
        } else {
            None
        };
        raw.push(RawInstruction {
            offset: cursor,
            size,
            opcode: ins.opcode,
            raw_arg: arg,
            role: "instruction",
            owner_offset: None,
            effective_arg: ins.has_arg.then_some(ins.arg),
            target: ins.target,
        });
        cursor += size;
        while cursor < ins.end() {
            raw.push(RawInstruction {
                offset: cursor,
                size: 2,
                opcode: code.code[cursor],
                raw_arg: Some(code.code[cursor + 1] as u32),
                role: "cache",
                owner_offset: Some(ins.offset),
                effective_arg: None,
                target: None,
            });
            cursor += 2;
        }
    }
    while cursor < code.code.len() {
        raw.push(RawInstruction {
            offset: cursor,
            size: 2,
            opcode: code.code[cursor],
            raw_arg: Some(code.code[cursor + 1] as u32),
            role: "cache",
            owner_offset: None,
            effective_arg: None,
            target: None,
        });
        cursor += 2;
    }
    Ok((logical, raw))
}

/// Resolve a jump operand to an absolute byte offset.
///
/// * `next` is the byte offset directly after the jump instruction's operand
///   (pre-CACHE-skip position for wordcode).
/// * 3.10+ count jump deltas in instruction words, earlier versions in bytes.
/// * 3.11 renamed backward relative jumps (JUMP_BACKWARD,
///   POP_JUMP_BACKWARD_IF_*); 3.12 made all remaining relative jumps
///   forward-relative again.
fn normalize_jump(
    table: &OpcodeTable,
    opcode: u8,
    arg: u32,
    next: usize,
    jumps_in_words: bool,
    wordcode: bool,
) -> Option<usize> {
    let _budget_frame = crate::budget::enter();
    let name = table.name(opcode);
    let backward = name.contains("BACKWARD");
    let unit = if jumps_in_words { 2usize } else { 1usize };
    let delta = arg as usize * unit;

    if table.is_jabs(opcode) {
        // absolute target; 3.10+ counted in words
        let _ = (next, wordcode);
        return Some(delta);
    }
    if table.is_jrel(opcode) {
        return if backward {
            next.checked_sub(delta)
        } else {
            next.checked_add(delta)
        };
    }
    None
}

/// Render one instruction the way `dis` does, for the disassembler.
pub fn disasm_line(
    inst: &Instruction,
    code: &CodeObject,
    table: &OpcodeTable,
    version: PythonVersion,
) -> String {
    let _budget_frame = crate::budget::enter();
    format_line(
        inst,
        table.name(inst.opcode),
        &operand_repr(inst, code, table, version),
    )
}

pub(crate) fn operand_repr(
    inst: &Instruction,
    code: &CodeObject,
    table: &OpcodeTable,
    version: PythonVersion,
) -> String {
    let _budget_frame = crate::budget::enter();
    let mut argrepr = String::new();
    if inst.has_arg {
        let a = inst.arg;
        match inst.op {
            Op::LOAD_CONST | Op::RETURN_CONST => {
                if let Some(c) = code.consts.get(a as usize) {
                    argrepr = format!("({})", short_const_repr(c, version));
                } else {
                    argrepr = format!("{a}");
                }
            }
            Op::LOAD_FAST
            | Op::STORE_FAST
            | Op::DELETE_FAST
            | Op::LOAD_FAST_CHECK
            | Op::LOAD_FAST_BORROW
            | Op::LOAD_FAST_AND_CLEAR => {
                // 3.11+: LOAD_FAST arg indexes localsplus; earlier: varnames
                let idx = a as usize;
                let n = if version.at_least(3, 11) {
                    code.deref_names
                        .get(idx)
                        .map(|value| crate::budget_clone!(value))
                } else {
                    None
                }
                .or_else(|| {
                    crate::budget::charge(1);
                    code.varnames
                        .get(idx)
                        .map(|value| crate::budget_clone!(value))
                });
                argrepr = match n {
                    Some(s) => format!("{a} ({s})"),
                    None => format!("{a}"),
                };
            }
            Op::LOAD_NAME
            | Op::STORE_NAME
            | Op::DELETE_NAME
            | Op::LOAD_ATTR
            | Op::STORE_ATTR
            | Op::DELETE_ATTR
            | Op::LOAD_GLOBAL
            | Op::STORE_GLOBAL
            | Op::DELETE_GLOBAL
            | Op::IMPORT_NAME
            | Op::IMPORT_FROM
            | Op::LOAD_METHOD
            | Op::LOAD_FROM_DICT_OR_GLOBALS
            | Op::LOAD_SUPER_ATTR => {
                // LOAD_GLOBAL (3.11+) and LOAD_SUPER_ATTR (3.12+) pack a
                // "push NULL" flag into the low bits of the index.
                let idx = match inst.op {
                    Op::LOAD_GLOBAL if version.at_least(3, 11) => (a >> 1) as usize,
                    Op::LOAD_SUPER_ATTR if version.at_least(3, 12) => (a >> 2) as usize,
                    // 3.12: LOAD_ATTR low bit = "method" (pushes NULL+self);
                    // STORE_ATTR/DELETE_ATTR use the plain index
                    Op::LOAD_ATTR if version.at_least(3, 12) => (a >> 1) as usize,
                    _ => a as usize,
                };
                let nm = code.names.get(idx).and_then(|n| {
                    crate::budget::charge(1);

                    n.as_str()
                        .map(|name| crate::budget_string!(name))
                        .or_else(|| {
                            crate::budget::charge(1);

                            n.as_bytes()
                                .and_then(|b| {
                                    crate::budget::charge(1);
                                    std::str::from_utf8(b).ok()
                                })
                                .map(|name| crate::budget_string!(name))
                        })
                });
                argrepr = match nm {
                    Some(s) => format!("{a} ({s})"),
                    None => format!("{a}"),
                };
            }
            Op::LOAD_DEREF
            | Op::STORE_DEREF
            | Op::DELETE_DEREF
            | Op::LOAD_CLOSURE
            | Op::LOAD_CLASSDEREF
            | Op::LOAD_FROM_DICT_OR_DEREF
            | Op::MAKE_CELL => {
                if let Some(n) = code.deref_name(a as usize) {
                    argrepr = format!("{a} ({n})");
                } else {
                    argrepr = format!("{a}");
                }
            }
            Op::COMPARE_OP => {
                // 3.12 packs a specialization flag in the low bits; 3.13+
                // uses the plain index again
                let idx = compare_op_index(a, version);
                argrepr = table
                    .cmp_op
                    .get(idx)
                    .map(|s| {
                        crate::budget::charge(1);
                        format!("{a} ({s})")
                    })
                    .unwrap_or_else(|| {
                        crate::budget::charge(1);
                        format!("{a}")
                    });
            }
            Op::BINARY_OP => {
                argrepr = binary_op_name(a, version)
                    .map(|s| {
                        crate::budget::charge(1);
                        format!("{a} ({s})")
                    })
                    .unwrap_or_else(|| {
                        crate::budget::charge(1);
                        format!("{a}")
                    });
            }
            _ => {
                if let Some(t) = inst.target {
                    argrepr = format!("to {t}");
                } else {
                    argrepr = format!("{a}");
                }
            }
        }
    }
    argrepr
}

fn format_line(inst: &Instruction, name: &str, argrepr: &str) -> String {
    let _budget_frame = crate::budget::enter();
    if argrepr.is_empty() {
        format!("{:>6} {:<24}", inst.offset, name)
    } else {
        format!("{:>6} {:<24} {}", inst.offset, name, argrepr)
    }
}

/// Short repr of a constant for disassembly output.
pub fn short_const_repr(obj: &crate::object::PyObject, version: PythonVersion) -> String {
    let _budget_frame = crate::budget::enter();
    match obj {
        PyObject::None => "None".into(),
        PyObject::True => "True".into(),
        PyObject::False => "False".into(),
        PyObject::Ellipsis => "Ellipsis".into(),
        PyObject::Int(i) => crate::budget_string!(i),
        PyObject::Long(l) => {
            if version.major == 2 && !l.from_int64 {
                format!("{}L", l)
            } else {
                crate::budget_clone!(l.decimal)
            }
        }
        PyObject::Float(_, Some(s)) => crate::budget_clone!(s),
        PyObject::Float(f, None) => float_repr(*f),
        PyObject::Complex(re, im) => format!("{}+{}j", re, im),
        PyObject::Str(s) => {
            if s.len() > 40 {
                format!("{:?}...", truncate_str(s, 37))
            } else {
                format!("{s:?}")
            }
        }
        PyObject::Bytes(b) => {
            let s = String::from_utf8_lossy(b);
            if b.len() > 40 {
                format!("b{:?}...", truncate_str(&s, 37))
            } else if version.major >= 3 {
                format!("b{:?}", s)
            } else {
                format!("{:?}", s)
            }
        }
        PyObject::Tuple(items) => format!("tuple[{}]", items.len()),
        PyObject::List(items) => format!("list[{}]", items.len()),
        PyObject::Dict(items) => format!("dict[{}]", items.len()),
        PyObject::Set(items) => format!("set[{}]", items.len()),
        PyObject::FrozenSet(items) => format!("frozenset[{}]", items.len()),
        PyObject::Slice(..) => "slice".into(),
        PyObject::Code(c) => format!("<code object {} at line {}>", c.name, c.first_line),
        PyObject::StopIteration => "StopIteration".into(),
    }
}

/// Truncate to at most `max` bytes on a char boundary.
fn truncate_str(s: &str, max: usize) -> &str {
    let _budget_frame = crate::budget::enter();
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        crate::budget::charge(1);
        end -= 1;
    }
    &s[..end]
}

/// repr of an f64 in Python style (always with .0 for integral values).
pub fn float_repr(f: f64) -> String {
    let _budget_frame = crate::budget::enter();
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if f.is_nan() {
        return "nan".into();
    }
    let s = format!("{f:?}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        format!("{s}.0")
    }
}

/// COMPARE_OP argument -> cmp_op index. 3.12 packs specialization flags in
/// the low 4 bits, 3.13+ in the low 5 bits; earlier versions use the raw arg.
pub fn compare_op_index(arg: u32, version: PythonVersion) -> usize {
    let _budget_frame = crate::budget::enter();
    if version.at_least(3, 13) {
        (arg >> 5) as usize
    } else if version.tuple() == (3, 12) {
        (arg >> 4) as usize
    } else {
        arg as usize
    }
}

/// BINARY_OP (3.11+) argument -> operator text.
pub fn binary_op_name(arg: u32, version: PythonVersion) -> Option<&'static str> {
    let _budget_frame = crate::budget::enter();
    let id = arg;
    Some(match id {
        0 => "+",
        1 => "&",
        2 => "//",
        3 => "<<",
        4 => "@",
        5 => "*",
        6 => "%",
        7 => "|",
        8 => "**",
        9 => ">>",
        10 => "-",
        11 => "/",
        12 => "^",
        13 => "+=",
        14 => "&=",
        15 => "//=",
        16 => "<<=",
        17 => "@=",
        18 => "*=",
        19 => "%=",
        20 => "|=",
        21 => "**=",
        22 => ">>=",
        23 => "-=",
        24 => "/=",
        25 => "^=",
        26 if version.at_least(3, 14) => "[]",
        _ => return None,
    })
}
