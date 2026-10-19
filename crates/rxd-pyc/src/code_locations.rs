use rxd_core::{Error, Result};
use serde::Serialize;
use std::collections::HashSet;

#[derive(Serialize)]
pub struct Location {
    pub start: usize,
    pub end: usize,
    pub line: Option<u32>,
    pub end_line: Option<u32>,
    pub column: Option<u32>,
    pub end_column: Option<u32>,
    pub table_start: usize,
    pub table_end: usize,
}
fn invalid(message: &str) -> Error {
    Error::new("INVALID_INPUT", message)
}
fn data(bytes: &[u8], at: &mut usize) -> Result<u8> {
    let b = *bytes
        .get(*at)
        .ok_or_else(|| invalid("truncated location table"))?;
    *at += 1;
    if b & 128 != 0 {
        return Err(invalid("location payload contains an entry marker"));
    }
    Ok(b)
}
fn uvar(bytes: &[u8], at: &mut usize) -> Result<u32> {
    let mut value = 0_u32;
    let mut shift = 0;
    loop {
        let b = data(bytes, at)?;
        let chunk = (b & 63) as u32;
        if shift >= 32 || chunk > (u32::MAX >> shift) {
            return Err(invalid("location varint overflow"));
        }
        value |= chunk << shift;
        if b & 64 == 0 {
            return Ok(value);
        }
        shift += 6;
    }
}
fn svar(bytes: &[u8], at: &mut usize) -> Result<i64> {
    let value = uvar(bytes, at)?;
    Ok(if value & 1 != 0 {
        -((value >> 1) as i64)
    } else {
        (value >> 1) as i64
    })
}
/// Exact 3.11+ location evidence, independent of decompiler line-start approximations.
fn modern_locations(bytes: &[u8], first_line: u32, length: usize) -> Result<Vec<Location>> {
    let mut at = 0;
    let mut offset = 0_usize;
    let mut line = i64::from(first_line);
    let mut result = Vec::new();
    while at < bytes.len() {
        let table_start = at;
        let header = bytes[at];
        at += 1;
        if header & 128 == 0 {
            return Err(invalid("location entry lacks marker"));
        }
        let kind = (header >> 3) & 15;
        let end = offset
            .checked_add(((header & 7) as usize + 1) * 2)
            .ok_or_else(|| invalid("location range overflow"))?;
        if end > length {
            return Err(invalid("location range exceeds bytecode"));
        }
        let mut end_delta = 0;
        let (column, end_column) = match kind {
            0..=9 => {
                let second = data(bytes, &mut at)?;
                let col = u32::from(kind) * 8 + u32::from((second >> 4) & 7);
                (Some(col), Some(col + u32::from(second & 15)))
            }
            10..=12 => {
                line += i64::from(kind - 10);
                (
                    Some(u32::from(data(bytes, &mut at)?)),
                    Some(u32::from(data(bytes, &mut at)?)),
                )
            }
            13 => {
                line += svar(bytes, &mut at)?;
                (None, None)
            }
            14 => {
                line += svar(bytes, &mut at)?;
                end_delta = uvar(bytes, &mut at)?;
                (
                    uvar(bytes, &mut at)?.checked_sub(1),
                    uvar(bytes, &mut at)?.checked_sub(1),
                )
            }
            _ => (None, None),
        };
        let current = if kind == 15 {
            None
        } else {
            Some(u32::try_from(line).map_err(|_| invalid("location line is out of range"))?)
        };
        let end_line = current
            .map(|l| {
                l.checked_add(end_delta)
                    .ok_or_else(|| invalid("end line overflow"))
            })
            .transpose()?;
        if end_delta == 0 && column.zip(end_column).is_some_and(|(a, b)| b < a) {
            return Err(invalid("location columns are reversed"));
        }
        result.push(Location {
            start: offset,
            end,
            line: current,
            end_line,
            column,
            end_column,
            table_start,
            table_end: at,
        });
        offset = end;
    }
    if !bytes.is_empty() && offset != length {
        return Err(invalid("location table does not cover bytecode"));
    }
    Ok(result)
}
/// Original per-version line tables. Older formats store no column/end-line data.
pub fn locations(bytes: &[u8], first_line: u32, length: usize, minor: u8) -> Result<Vec<Location>> {
    if minor >= 11 {
        return modern_locations(bytes, first_line, length);
    }
    if !bytes.len().is_multiple_of(2) {
        return Err(invalid("truncated line table pair"));
    }
    let mut offset = 0usize;
    let mut line = i64::from(first_line);
    let mut result = Vec::new();
    for (index, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let end = offset
            .checked_add(usize::from(pair[0]))
            .ok_or_else(|| invalid("line range overflow"))?;
        if end > length {
            return Err(invalid("line range exceeds bytecode"));
        }
        let delta = i64::from(pair[1] as i8);
        // 3.10's delta describes the range ending here. 3.8/3.9's
        // lnotab delta changes the line beginning at the new address.
        if minor == 10 && delta != -128 {
            line += delta;
        }
        let current = if minor == 10 && delta == -128 {
            None
        } else {
            Some(u32::try_from(line).map_err(|_| invalid("line is out of range"))?)
        };
        if end > offset {
            result.push(Location {
                start: offset,
                end,
                line: current,
                end_line: None,
                column: None,
                end_column: None,
                table_start: index * 2,
                table_end: index * 2 + 2,
            });
        }
        if minor < 10 {
            line += delta;
        }
        offset = end;
    }
    if minor == 10 {
        if !bytes.is_empty() && offset != length {
            return Err(invalid("line table does not cover bytecode"));
        }
    } else if offset < length {
        result.push(Location {
            start: offset,
            end: length,
            line: Some(u32::try_from(line).map_err(|_| invalid("line is out of range"))?),
            end_line: None,
            column: None,
            end_column: None,
            table_start: bytes.len(),
            table_end: bytes.len(),
        });
    }
    Ok(result)
}
fn exception_var(bytes: &[u8], at: &mut usize, first: bool) -> Result<u32> {
    let mut value = 0_u32;
    let mut initial = true;
    loop {
        let b = *bytes
            .get(*at)
            .ok_or_else(|| invalid("truncated exception table"))?;
        *at += 1;
        if (b & 128 != 0) != (first && initial) {
            return Err(invalid("invalid exception entry marker"));
        }
        value = value
            .checked_mul(64)
            .and_then(|v| v.checked_add(u32::from(b & 63)))
            .ok_or_else(|| invalid("exception varint overflow"))?;
        if b & 64 == 0 {
            return Ok(value);
        }
        initial = false;
    }
}
#[derive(Serialize)]
pub struct Exception {
    pub start: usize,
    pub end: usize,
    pub target: usize,
    pub depth: u32,
    pub lasti: bool,
    pub table_start: usize,
    pub table_end: usize,
}
pub fn exceptions(
    bytes: &[u8],
    length: usize,
    stack: u32,
    boundaries: &HashSet<usize>,
) -> Result<Vec<Exception>> {
    let mut at = 0;
    let mut result = Vec::new();
    let mut previous = 0;
    while at < bytes.len() {
        let table_start = at;
        let start = exception_var(bytes, &mut at, true)? as usize * 2;
        let size = exception_var(bytes, &mut at, false)? as usize * 2;
        let target = exception_var(bytes, &mut at, false)? as usize * 2;
        let flags = exception_var(bytes, &mut at, false)?;
        let end = start
            .checked_add(size)
            .ok_or_else(|| invalid("exception range overflow"))?;
        if size == 0
            || start < previous
            || end > length
            || target >= length
            || !boundaries.contains(&start)
            || (end != length && !boundaries.contains(&end))
            || !boundaries.contains(&target)
            || flags >> 1 > stack
        {
            return Err(invalid(
                "invalid exception range, boundary, target or depth",
            ));
        }
        result.push(Exception {
            start,
            end,
            target,
            depth: flags >> 1,
            lasti: flags & 1 != 0,
            table_start,
            table_end: at,
        });
        previous = start;
    }
    Ok(result)
}
