use rxd_core::{Error, Result};

const MAX_ELEMENTS: usize = 1_000_000;
const MAX_DEPTH: usize = 64;
const MAX_LONG_DIGITS: usize = 5;

#[derive(Clone)]
pub(crate) enum Value {
    Null,
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
    Seq(Vec<Value>),
    Map(Vec<(Value, Value)>),
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Value> {
    Decoder {
        bytes,
        position: 0,
        references: Vec::new(),
    }
    .value(0)
}

struct Decoder<'a> {
    bytes: &'a [u8],
    position: usize,
    references: Vec<Value>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new("INVALID_INPUT", message)
}

impl Decoder<'_> {
    fn byte(&mut self) -> Result<u8> {
        let byte = *self
            .bytes
            .get(self.position)
            .ok_or_else(|| invalid("truncated marshal payload"))?;
        self.position += 1;
        Ok(byte)
    }

    fn take(&mut self, length: usize) -> Result<&[u8]> {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| invalid("truncated marshal payload"))?;
        let bytes = &self.bytes[self.position..end];
        self.position = end;
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("four bytes"),
        ))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(
            self.take(4)?.try_into().expect("four bytes"),
        ))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(
            self.take(8)?.try_into().expect("eight bytes"),
        ))
    }

    fn text(&mut self, length: usize) -> Result<String> {
        String::from_utf8(self.take(length)?.to_vec())
            .map_err(|_| invalid("marshal text is not UTF-8"))
    }

    fn long(&mut self) -> Result<i64> {
        let count = self.i32()?;
        let digits = count.unsigned_abs() as usize;
        if digits == 0 {
            return Ok(0);
        }
        if digits > MAX_LONG_DIGITS {
            return Err(invalid("marshal long does not fit in 64 bits"));
        }
        let mut magnitude = 0i128;
        for index in 0..digits {
            let digit = i128::from(u16::from_le_bytes(
                self.take(2)?.try_into().expect("two bytes"),
            ));
            magnitude |= digit << (15 * index);
        }
        let value = if count < 0 { -magnitude } else { magnitude };
        i64::try_from(value).map_err(|_| invalid("marshal long does not fit in 64 bits"))
    }

    fn slot(&mut self, flagged: bool) -> Option<usize> {
        if !flagged {
            return None;
        }
        self.references.push(Value::Null);
        Some(self.references.len() - 1)
    }

    fn sequence(&mut self, count: usize, depth: usize, flagged: bool) -> Result<Value> {
        if depth >= MAX_DEPTH {
            return Err(invalid("marshal nesting is too deep"));
        }
        if count > MAX_ELEMENTS {
            return Err(invalid("marshal collection is too large"));
        }
        let slot = self.slot(flagged);
        let mut items = Vec::new();
        for _ in 0..count {
            items.push(self.value(depth + 1)?);
        }
        let value = Value::Seq(items);
        if let Some(slot) = slot {
            self.references[slot] = value.clone();
        }
        Ok(value)
    }

    fn mapping(&mut self, depth: usize, flagged: bool) -> Result<Value> {
        if depth >= MAX_DEPTH {
            return Err(invalid("marshal nesting is too deep"));
        }
        let slot = self.slot(flagged);
        let mut pairs = Vec::new();
        loop {
            let key = self.value(depth + 1)?;
            if matches!(key, Value::Null) {
                break;
            }
            let value = self.value(depth + 1)?;
            pairs.push((key, value));
            if pairs.len() > MAX_ELEMENTS {
                return Err(invalid("marshal collection is too large"));
            }
        }
        let value = Value::Map(pairs);
        if let Some(slot) = slot {
            self.references[slot] = value.clone();
        }
        Ok(value)
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        let code = self.byte()?;
        let flagged = code & 0x80 != 0;
        let value = match code & 0x7f {
            b'0' | b'N' => Value::Null,
            b'F' => Value::Int(0),
            b'T' => Value::Int(1),
            b'i' => Value::Int(i64::from(self.i32()?)),
            b'I' => Value::Int(self.i64()?),
            b'l' => Value::Int(self.long()?),
            b's' | b't' => {
                let length = self.u32()? as usize;
                Value::Bytes(self.take(length)?.to_vec())
            }
            b'u' | b'a' | b'A' => {
                let length = self.u32()? as usize;
                Value::Text(self.text(length)?)
            }
            b'z' | b'Z' => {
                let length = self.byte()? as usize;
                Value::Text(self.text(length)?)
            }
            b'r' => {
                let index = self.u32()? as usize;
                return self
                    .references
                    .get(index)
                    .cloned()
                    .ok_or_else(|| invalid("marshal reference is out of range"));
            }
            b'(' | b'[' | b'<' | b'>' => {
                let count = self.u32()? as usize;
                return self.sequence(count, depth, flagged);
            }
            b')' => {
                let count = self.byte()? as usize;
                return self.sequence(count, depth, flagged);
            }
            b'{' => return self.mapping(depth, flagged),
            code => return Err(invalid(format!("unsupported marshal type 0x{code:02x}"))),
        };
        if flagged {
            self.references.push(value.clone());
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_integers_and_text() {
        let mut bytes = vec![b'(', 3, 0, 0, 0];
        bytes.extend_from_slice(b"i\xff\xff\xff\xff");
        bytes.extend_from_slice(b"z\x03abc");
        bytes.extend_from_slice(b"l\x01\x00\x00\x00\x11\x00");
        match decode(&bytes).unwrap() {
            Value::Seq(items) => match &items[..] {
                [Value::Int(-1), Value::Text(text), Value::Int(17)] => assert_eq!(text, "abc"),
                _ => panic!("unexpected tuple contents"),
            },
            _ => panic!("expected a tuple"),
        }
    }

    #[test]
    fn references_resolve_to_flagged_objects() {
        let mut bytes = vec![b'[', 2, 0, 0, 0];
        bytes.extend_from_slice(b"\xfa\x03abc");
        bytes.extend_from_slice(b"r\x00\x00\x00\x00");
        match decode(&bytes).unwrap() {
            Value::Seq(items) => match &items[..] {
                [Value::Text(first), Value::Text(second)] => assert_eq!(first, second),
                _ => panic!("unexpected list contents"),
            },
            _ => panic!("expected a list"),
        }
    }

    #[test]
    fn rejects_unsupported_and_truncated_values() {
        assert!(decode(b"c").is_err());
        assert!(decode(b"i\x01").is_err());
        assert!(decode(b"r\x07\x00\x00\x00").is_err());
        let depth = MAX_DEPTH + 1;
        let mut bytes = Vec::new();
        for _ in 0..depth {
            bytes.extend_from_slice(b"[");
            bytes.extend_from_slice(&1u32.to_le_bytes());
        }
        bytes.push(b'0');
        assert!(decode(&bytes).is_err());
    }
}
