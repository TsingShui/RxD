use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use cildec::il::{Operand as CilOperand, decode_at};

use crate::{
    ExceptionHandler, Input, Instruction, Method, MethodBody, Operand,
    input::{Work, WorkingSet},
};

pub(crate) fn body(input: &Input<'_>, method: &Method) -> Result<MethodBody> {
    body_with_budget(input, method, input.limits.work)
}

pub(crate) fn body_with_budget(
    input: &Input<'_>,
    method: &Method,
    work_limit: Option<usize>,
) -> Result<MethodBody> {
    decode_body(input, method, work_limit, None)
}

pub(crate) fn scan_body(
    input: &Input<'_>,
    method: &Method,
    work_limit: Option<usize>,
    strings: &BTreeSet<u32>,
) -> Result<MethodBody> {
    decode_body(input, method, work_limit, Some(strings))
}

fn decode_body(
    input: &Input<'_>,
    method: &Method,
    work_limit: Option<usize>,
    strings: Option<&BTreeSet<u32>>,
) -> Result<MethodBody> {
    ensure!(
        method.has_body,
        "INVALID_QUERY: method has no ordinary IL body"
    );
    let bytes = input.bytes;
    let file_offset = input
        .image
        .rva_to_offset(method.rva)
        .context("INVALID_INPUT: method RVA")?;
    let first = *bytes
        .get(file_offset)
        .context("INVALID_INPUT: method header")?;
    let (header_size, code_size, max_stack, init_locals, local_signature_token, more_sections) =
        match first & 3 {
            2 => (1usize, (first >> 2) as usize, 8u16, false, 0u32, false),
            3 => {
                let header = slice(bytes, file_offset, 12)?;
                let flags = u16::from_le_bytes([header[0], header[1]]);
                ensure!(flags >> 12 == 3, "UNSUPPORTED: extended method header");
                ensure!(
                    flags & 0x0fe4 == 0,
                    "INVALID_INPUT: reserved method header flags"
                );
                (
                    12,
                    read_u32(header, 4)? as usize,
                    u16::from_le_bytes([header[2], header[3]]),
                    flags & 0x10 != 0,
                    read_u32(header, 8)?,
                    flags & 8 != 0,
                )
            }
            _ => bail!("INVALID_INPUT: method body format"),
        };
    let code_offset = file_offset
        .checked_add(header_size)
        .context("INVALID_INPUT: code offset")?;
    ensure!(code_size > 0, "INVALID_INPUT: empty method code");
    ensure!(
        work_limit.is_none_or(|limit| code_size <= limit),
        "RESOURCE_LIMIT: method code work"
    );
    let code = slice(bytes, code_offset, code_size)?;
    let last_rva = u32::try_from(
        (method.rva as usize)
            .checked_add(header_size)
            .and_then(|v| v.checked_add(code_size - 1))
            .context("INVALID_INPUT: method RVA overflow")?,
    )
    .context("INVALID_INPUT: method RVA overflow")?;
    ensure!(
        input
            .image
            .rva_to_offset(last_rva)
            .context("INVALID_INPUT: method RVA")?
            == code_offset + code_size - 1,
        "INVALID_INPUT: method crosses noncontiguous PE sections"
    );
    let mut work = Work::new(work_limit, input.limits.metadata_bytes);
    work.charge(code_size)?;
    let locals = if local_signature_token == 0 {
        Vec::new()
    } else {
        input.check_token(local_signature_token, &[17])?;
        let row = input
            .tables()
            .stand_alone_sig(local_signature_token & 0xffffff)
            .context("INVALID_INPUT: locals signature")?;
        crate::metadata::locals(input, input.blob(row.signature)?, &mut work)?
    };
    let mut instructions = Vec::new();
    let mut starts = BTreeSet::new();
    let mut forbidden_targets = BTreeSet::new();
    let mut prefixed = false;
    let mut user_strings = BTreeSet::new();
    // Only explicit caller quotas bound decoded output; arithmetic stays checked.
    let mut footprint = WorkingSet::new(input.limits.output_bytes, "IL working set");
    if strings.is_none() {
        footprint.add(code_size * 2)?;
    }
    let mut offset: u32 = 0;
    while (offset as usize) < code.len() {
        ensure!(
            input
                .limits
                .instructions
                .is_none_or(|limit| instructions.len() < limit),
            "RESOURCE_LIMIT: instructions"
        );
        work.charge(1)?;
        let position = offset as usize;
        starts.insert(offset);
        if prefixed {
            forbidden_targets.insert(offset);
        }
        // Preflight switch reservation against the explicit work budget before the
        // decoder constructs its operand vector.
        if code[position] == 0x45 {
            let count = read_u32(code, position + 1)? as usize;
            work.charge(count)?;
        }
        let cil = decode_at(code, offset).context("INVALID_INPUT: IL decoding")?;
        let info = cil.info();
        let opcode = match info.size {
            1 => info.encoding[0] as u16,
            _ => ((info.encoding[0] as u16) << 8) | info.encoding[1] as u16,
        };
        prefixed = cil.opcode.is_prefix();
        let size = cil.size;
        let operand = match cil.operand {
            CilOperand::None => Operand::None,
            CilOperand::I8(n) => Operand::I32(n as i32),
            CilOperand::I32(n) => Operand::I32(n),
            CilOperand::I64(n) => Operand::I64(n),
            CilOperand::R32(n) => Operand::F32Bits(n.to_bits()),
            CilOperand::R64(n) => Operand::F64Bits(n.to_bits()),
            CilOperand::BranchTarget(n) => Operand::Branch(n),
            CilOperand::Switch(targets) => {
                footprint.add(
                    targets
                        .len()
                        .checked_mul(4)
                        .context("RESOURCE_LIMIT: switch")?,
                )?;
                Operand::Switch(targets)
            }
            CilOperand::Var(n) => Operand::Variable(n),
            CilOperand::Method(t)
            | CilOperand::Field(t)
            | CilOperand::Type(t)
            | CilOperand::Tok(t)
            | CilOperand::Sig(t) => {
                let token = t.value();
                validate_operand_token(input, opcode, token, &mut work)?;
                if opcode == 0x72 {
                    user_strings.insert(token & 0xffffff);
                }
                Operand::Token(token)
            }
            CilOperand::String(t) => {
                let token = t.value();
                validate_operand_token(input, opcode, token, &mut work)?;
                if opcode == 0x72 {
                    user_strings.insert(token & 0xffffff);
                }
                Operand::Token(token)
            }
            _ => bail!("INVALID_INPUT: IL operand"),
        };
        validate_variable(
            opcode,
            &operand,
            method.parameters.len() + usize::from(method.has_this),
            locals.len(),
        )?;
        footprint.add(96)?;
        instructions.push(Instruction {
            offset,
            size,
            opcode,
            mnemonic: if strings.is_none() || matches!(operand, Operand::Token(_)) {
                info.name.to_string()
            } else {
                String::new()
            },
            operand,
        });
        offset = position
            .checked_add(size as usize)
            .context("INVALID_INPUT: instruction size")?
            .try_into()
            .context("INVALID_INPUT: instruction offset")?;
        ensure!(offset as usize <= code.len(), "INVALID_INPUT: truncated IL");
    }
    ensure!(!prefixed, "INVALID_INPUT: unterminated IL prefix");
    for instr in &instructions {
        let check = |target: &u32| -> Result<()> {
            ensure!(
                starts.contains(target) && !forbidden_targets.contains(target),
                "INVALID_INPUT: branch targets non-instruction or prefix interior"
            );
            Ok(())
        };
        match &instr.operand {
            Operand::Branch(n) => check(n)?,
            Operand::Switch(ns) => {
                for n in ns {
                    check(n)?;
                }
            }
            _ => {}
        }
    }
    if let Some(strings) = strings {
        work.charge(user_strings.len())?;
        ensure!(
            user_strings.is_subset(strings),
            "INVALID_INPUT: ldstr does not reference a user string entry"
        );
    } else {
        validate_user_strings(input, &user_strings, &mut work)?;
    }
    let eh_starts = starts.difference(&forbidden_targets).copied().collect();
    let handlers = if more_sections {
        handlers(
            input,
            bytes,
            code_offset + code_size,
            code_size,
            &eh_starts,
            &mut work,
        )?
    } else {
        Vec::new()
    };
    let digits = b"0123456789abcdef";
    let mut il_hex = String::new();
    if strings.is_none() {
        il_hex.reserve(code_size * 2);
        for byte in code {
            il_hex.push(digits[(byte >> 4) as usize] as char);
            il_hex.push(digits[(byte & 15) as usize] as char);
        }
    }
    Ok(MethodBody {
        file_offset,
        code_offset,
        code_size,
        max_stack,
        init_locals,
        local_signature_token,
        locals,
        instructions,
        handlers,
        il_hex,
        work_used: work.used(),
    })
}

fn validate_variable(opcode: u16, operand: &Operand, args: usize, locals: usize) -> Result<()> {
    let arg = match opcode {
        0x02..=0x05 => Some((opcode - 2) as usize),
        0x0e..=0x10 | 0xfe09..=0xfe0b => match operand {
            Operand::Variable(n) => Some(*n as usize),
            _ => None,
        },
        _ => None,
    };
    let local = match opcode {
        0x06..=0x09 => Some((opcode - 6) as usize),
        0x0a..=0x0d => Some((opcode - 10) as usize),
        0x11..=0x13 | 0xfe0c..=0xfe0e => match operand {
            Operand::Variable(n) => Some(*n as usize),
            _ => None,
        },
        _ => None,
    };
    ensure!(
        arg.is_none_or(|n| n < args) && local.is_none_or(|n| n < locals),
        "INVALID_INPUT: local or argument index"
    );
    Ok(())
}
fn validate_operand_token(
    input: &Input<'_>,
    opcode: u16,
    token: u32,
    work: &mut Work,
) -> Result<()> {
    if opcode == 0x72 {
        ensure!(
            token >> 24 == 0x70 && token & 0xffffff != 0,
            "INVALID_INPUT: ldstr token"
        );
        return Ok(());
    }
    let allowed: &[u8] = match opcode {
        0x27 | 0x28 | 0x6f | 0x73 | 0xfe06 | 0xfe07 => &[6, 10, 43],
        0x29 => &[17],
        0x7b..=0x80 => &[4, 10],
        0xd0 => &[1, 2, 27, 4, 6, 10, 43],
        _ => &[1, 2, 27],
    };
    input.check_token(token, allowed)?;
    if token >> 24 == 10 {
        if matches!(opcode, 0x27 | 0x28 | 0x6f | 0x73 | 0xfe06 | 0xfe07) {
            ensure!(
                input.member_kind(token)? == Some(true),
                "INVALID_INPUT: call references field"
            );
        } else if matches!(opcode, 0x7b..=0x80) {
            ensure!(
                input.member_kind(token)? == Some(false),
                "INVALID_INPUT: field access references method"
            );
        }
    }
    if opcode == 0x29 {
        let row = input
            .tables()
            .stand_alone_sig(token & 0xffffff)
            .context("INVALID_INPUT: calli signature")?;
        crate::metadata::method_signature(input, input.blob(row.signature)?, work)?;
    }
    Ok(())
}
pub(crate) fn validate_user_strings(
    input: &Input<'_>,
    requested: &BTreeSet<u32>,
    work: &mut Work,
) -> Result<()> {
    if requested.is_empty() {
        return Ok(());
    }
    let bytes = input.metadata.user_strings().data();
    let mut remaining = requested.clone();
    let mut pos = 1usize;
    while pos < bytes.len() && !remaining.is_empty() {
        let start = pos as u32;
        let (len, _) = crate::metadata::compressed(bytes, &mut pos, work)?;
        let len = len as usize;
        let value = slice(bytes, pos, len)?;
        work.charge(len)?;
        if remaining.remove(&start) {
            ensure!(
                len > 0 && len % 2 == 1 && value[len - 1] <= 1,
                "INVALID_INPUT: ldstr string entry"
            );
        }
        pos += len;
    }
    ensure!(
        remaining.is_empty(),
        "INVALID_INPUT: ldstr does not reference a user string entry"
    );
    Ok(())
}
fn handlers(
    input: &Input<'_>,
    bytes: &[u8],
    end: usize,
    code_size: usize,
    starts: &BTreeSet<u32>,
    work: &mut Work,
) -> Result<Vec<ExceptionHandler>> {
    let rva_end = input
        .offset_to_rva(end - 1)?
        .checked_add(1)
        .context("INVALID_INPUT: EH RVA overflow")?;
    let mut pos = align4(end)?;
    let mut result = Vec::new();
    loop {
        work.charge(1)?;
        let header = slice(bytes, pos, 4)?;
        let kind = header[0];
        ensure!(kind & 0x3f == 1, "UNSUPPORTED: non-EH method data section");
        let fat = kind & 0x40 != 0;
        let size = if fat {
            (header[1] as usize) | ((header[2] as usize) << 8) | ((header[3] as usize) << 16)
        } else {
            ensure!(
                header[2] == 0 && header[3] == 0,
                "INVALID_INPUT: EH section reserved bytes"
            );
            header[1] as usize
        };
        let row_size = if fat { 24 } else { 12 };
        ensure!(
            size >= 4 && (size - 4) % row_size == 0,
            "INVALID_INPUT: EH section size"
        );
        let data = slice(bytes, pos, size)?;
        let start_rva = rva_end
            .checked_add((pos - end) as u32)
            .context("INVALID_INPUT: EH RVA overflow")?;
        let last_rva = start_rva
            .checked_add((size - 1) as u32)
            .context("INVALID_INPUT: EH RVA overflow")?;
        ensure!(
            input.image.rva_to_offset(start_rva) == Some(pos)
                && input.image.rva_to_offset(last_rva) == Some(pos + size - 1),
            "INVALID_INPUT: EH section crosses noncontiguous mapped data"
        );
        let count = (size - 4) / row_size;
        work.charge(count)?;
        ensure!(
            result
                .len()
                .checked_add(count)
                .is_some_and(|n| input.limits.instructions.is_none_or(|limit| n <= limit)),
            "RESOURCE_LIMIT: exception clauses"
        );
        for row in data[4..].chunks_exact(row_size) {
            let (flags, try_offset, try_length, handler_offset, handler_length, extra) = if fat {
                (
                    read_u32(row, 0)?,
                    read_u32(row, 4)?,
                    read_u32(row, 8)?,
                    read_u32(row, 12)?,
                    read_u32(row, 16)?,
                    read_u32(row, 20)?,
                )
            } else {
                (
                    read_u16(row, 0)? as u32,
                    read_u16(row, 2)? as u32,
                    row[4] as u32,
                    read_u16(row, 5)? as u32,
                    row[7] as u32,
                    read_u32(row, 8)?,
                )
            };
            let end_try = try_offset
                .checked_add(try_length)
                .context("INVALID_INPUT: EH try range")?;
            let end_handler = handler_offset
                .checked_add(handler_length)
                .context("INVALID_INPUT: EH handler range")?;
            let boundary = |n: u32| starts.contains(&n) || n as usize == code_size;
            ensure!(
                try_length > 0
                    && handler_length > 0
                    && starts.contains(&try_offset)
                    && starts.contains(&handler_offset)
                    && boundary(end_try)
                    && boundary(end_handler),
                "INVALID_INPUT: EH instruction ranges"
            );
            ensure!(
                end_try <= handler_offset || end_handler <= try_offset,
                "INVALID_INPUT: overlapping EH try and handler"
            );
            let (kind, class_token, filter_offset) = match flags {
                0 => {
                    input.check_token(extra, &[1, 2, 27])?;
                    ("catch", Some(extra), None)
                }
                1 => {
                    ensure!(
                        starts.contains(&extra) && extra < handler_offset,
                        "INVALID_INPUT: EH filter offset"
                    );
                    ("filter", None, Some(extra))
                }
                2 => {
                    ensure!(extra == 0, "INVALID_INPUT: finally token");
                    ("finally", None, None)
                }
                4 => {
                    ensure!(extra == 0, "INVALID_INPUT: fault token");
                    ("fault", None, None)
                }
                _ => bail!("INVALID_INPUT: EH clause flags"),
            };
            result.push(ExceptionHandler {
                kind: kind.to_string(),
                try_offset,
                try_length,
                handler_offset,
                handler_length,
                class_token,
                filter_offset,
            });
        }
        if kind & 0x80 == 0 {
            break;
        }
        pos = align4(
            pos.checked_add(size)
                .context("INVALID_INPUT: EH next section")?,
        )?;
    }
    Ok(result)
}
fn align4(value: usize) -> Result<usize> {
    Ok(value
        .checked_add(3)
        .context("INVALID_INPUT: alignment overflow")?
        & !3)
}
fn slice(bytes: &[u8], start: usize, len: usize) -> Result<&[u8]> {
    bytes
        .get(
            start
                ..start
                    .checked_add(len)
                    .context("INVALID_INPUT: range overflow")?,
        )
        .context("INVALID_INPUT: truncated bytes")
}
fn read_u32(bytes: &[u8], pos: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(slice(bytes, pos, 4)?.try_into()?))
}
fn read_u16(bytes: &[u8], pos: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(slice(bytes, pos, 2)?.try_into()?))
}
