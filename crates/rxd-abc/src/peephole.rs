// Local line peephole for reconstructed source. It never touches string contents.
use crate::reader::resource;
use rxd_core::Result;

const MAX_PASSES: usize = 4;

enum Action {
    DropCurrent,
    DropPending,
    Replace(Vec<u8>),
    Flush,
}

pub(crate) fn compact(mut source: Vec<u8>) -> Result<Vec<u8>> {
    if source.is_empty() {
        return Ok(source);
    }
    for _ in 0..MAX_PASSES {
        let (next, changed) = pass(&source)?;
        source = next;
        if !changed {
            break;
        }
    }
    Ok(source)
}

fn pass(source: &[u8]) -> Result<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    output
        .try_reserve(source.len())
        .map_err(|_| resource("ABC peephole output allocation failed"))?;
    let mut pending = Vec::new();
    pending
        .try_reserve(256)
        .map_err(|_| resource("ABC peephole line allocation failed"))?;
    let mut has_pending = false;
    let mut changed = false;
    let body = source.strip_suffix(b"\n").unwrap_or(source);
    for line in body.split(|byte| *byte == b'\n') {
        if !has_pending {
            pending.extend_from_slice(line);
            has_pending = true;
            continue;
        }
        match combine(&pending, line) {
            Action::Flush => {
                output.extend_from_slice(&pending);
                output.push(b'\n');
                pending.clear();
                pending.extend_from_slice(line);
            }
            Action::DropCurrent => {
                changed = true;
            }
            Action::DropPending => {
                pending.clear();
                pending.extend_from_slice(line);
                changed = true;
            }
            Action::Replace(replacement) => {
                pending.clear();
                pending.extend_from_slice(&replacement);
                changed = true;
            }
        }
    }
    if has_pending && pending != b"void 0;" {
        output.extend_from_slice(&pending);
        output.push(b'\n');
    } else if has_pending {
        changed = true;
    }
    Ok((output, changed))
}

fn combine(pending: &[u8], current: &[u8]) -> Action {
    let (pending_depth, pending_text) = split_indent(pending);
    let (current_depth, current_text) = split_indent(current);
    if pending_text == b"void 0;" {
        return Action::DropPending;
    }
    if pending_depth != current_depth {
        return Action::Flush;
    }
    let acc_reg = acc_register(pending_text);
    let reg_acc = register_acc(pending_text);
    let current_acc = acc_register(current_text);
    let current_reg = register_acc(current_text);

    if let (Some(reg), Some(other)) = (reg_acc, current_reg) {
        if reg == other {
            return Action::DropCurrent;
        }
        return Action::Replace(chain(current_depth, reg, other, b"acc"));
    }
    if let (Some(reg), Some(other)) = (acc_reg, current_reg) {
        if other == reg {
            return Action::DropCurrent;
        }
        return Action::Replace(chain(current_depth, other, b"acc", reg));
    }
    if let Some(reg) = reg_acc
        && current_acc == Some(reg)
    {
        return Action::DropCurrent;
    }
    if let Some(reg) = acc_reg {
        if current_reg == Some(reg) {
            return Action::DropCurrent;
        }
        if let Some(next) = current_acc
            && next == reg
        {
            return Action::DropCurrent;
        }
        if let Some(rhs) = assignment_rhs(current_text)
            && !has_top_level_comma(rhs)
        {
            return Action::Replace(sequence(current_depth, reg, rhs));
        }
    }
    if let Some(reg) = reg_acc {
        if current_text == pending_text {
            return Action::DropCurrent;
        }
        if current_acc == Some(reg) {
            return Action::DropCurrent;
        }
    }
    if acc_reg.is_some() && current_text == pending_text {
        return Action::DropCurrent;
    }
    Action::Flush
}

fn split_indent(line: &[u8]) -> (usize, &[u8]) {
    let depth = line.iter().take_while(|byte| **byte == b'\t').count();
    (depth, &line[depth..])
}

fn register_token<'a>(text: &'a [u8], prefix: &[u8], suffix: &[u8]) -> Option<&'a [u8]> {
    let inner = text.strip_prefix(prefix)?.strip_suffix(suffix)?;
    if inner.len() > 1 && inner[0] == b'v' && inner[1..].iter().all(u8::is_ascii_digit) {
        Some(inner)
    } else {
        None
    }
}

fn acc_register(text: &[u8]) -> Option<&[u8]> {
    register_token(text, b"acc = ", b";")
}

fn register_acc(text: &[u8]) -> Option<&[u8]> {
    register_token(text, b"", b" = acc;")
}

fn assignment_rhs(text: &[u8]) -> Option<&[u8]> {
    let rhs = text.strip_prefix(b"acc = ")?.strip_suffix(b";")?;
    (!rhs.is_empty()).then_some(rhs)
}

fn indent(depth: usize) -> Vec<u8> {
    vec![b'\t'; depth]
}

fn chain(depth: usize, first: &[u8], second: &[u8], source: &[u8]) -> Vec<u8> {
    let mut line = indent(depth);
    line.extend_from_slice(first);
    line.extend_from_slice(b" = ");
    line.extend_from_slice(second);
    line.extend_from_slice(b" = ");
    line.extend_from_slice(source);
    line.push(b';');
    line
}

fn sequence(depth: usize, register: &[u8], rhs: &[u8]) -> Vec<u8> {
    let mut line = indent(depth);
    line.extend_from_slice(b"acc = (acc = ");
    line.extend_from_slice(register);
    line.extend_from_slice(b", ");
    line.extend_from_slice(rhs);
    line.extend_from_slice(b");");
    line
}

fn has_top_level_comma(text: &[u8]) -> bool {
    let mut quote = None;
    let mut depth = 0usize;
    let mut index = 0usize;
    while index < text.len() {
        let byte = text[index];
        if let Some(mark) = quote {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == mark {
                quote = None;
            }
        } else {
            match byte {
                b'\'' | b'"' | b'`' => quote = Some(byte),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => return true,
                _ => {}
            }
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::compact;

    fn check(input: &str, expected: &str) {
        let output = compact(input.as_bytes().to_vec()).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), expected);
    }

    #[test]
    fn chains_register_copies_in_the_right_direction() {
        check("acc = v1;\nv2 = acc;\n", "v2 = acc = v1;\n");
        check("v1 = acc;\nv2 = acc;\n", "v1 = v2 = acc;\n");
    }

    #[test]
    fn drops_redundant_reloads_and_void_statements() {
        check("v1 = acc;\nacc = v1;\n", "v1 = acc;\n");
        check("\tvoid 0;\n\tacc = v1;\n", "\tacc = v1;\n");
    }

    #[test]
    fn preserves_the_assignment_when_folding_acc_reads() {
        check(
            "acc = v1;\nacc = acc[\"x\"];\n",
            "acc = (acc = v1, acc[\"x\"]);\n",
        );
        check("acc = v1;\nacc = a, b;\n", "acc = v1;\nacc = a, b;\n");
    }
}
