use rxd_core::{Budget, Error, Result};

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

pub(crate) struct Decoded {
    pub value: Value,
    pub work: usize,
    pub metadata: usize,
}

pub(crate) fn decode(bytes: &[u8], limits: Budget) -> Result<Decoded> {
    let mut reader = Decoder {
        bytes,
        position: 0,
        references: Vec::new(),
        admission: Admission {
            limits,
            nodes: 0,
            metadata: 0,
            work: 0,
        },
    };
    let value = reader.value(0)?;
    if reader.position != bytes.len() {
        return Err(invalid("trailing PYZ TOC marshal data"));
    }
    Ok(Decoded {
        value,
        work: reader.admission.work,
        metadata: reader.admission.metadata,
    })
}

struct Admission {
    limits: Budget,
    nodes: usize,
    metadata: usize,
    work: usize,
}

impl Admission {
    fn charge(&mut self, nodes: usize, bytes: usize) -> Result<()> {
        let work = nodes
            .checked_add(bytes)
            .ok_or_else(|| resource("PYZ TOC work overflow"))?;
        if nodes > self.limits.semantic_nodes.saturating_sub(self.nodes)
            || bytes > self.limits.metadata_bytes.saturating_sub(self.metadata)
            || self
                .limits
                .work
                .is_some_and(|limit| work > limit.saturating_sub(self.work))
        {
            return Err(resource("PYZ TOC admission exceeded"));
        }
        self.nodes += nodes;
        self.metadata += bytes;
        self.work = self
            .work
            .checked_add(work)
            .ok_or_else(|| resource("PYZ TOC work overflow"))?;
        Ok(())
    }

    fn copied(&mut self, value: &Value, depth: usize) -> Result<()> {
        if depth >= MAX_DEPTH {
            return Err(resource("PYZ TOC reference expansion is too deep"));
        }
        self.charge(1, std::mem::size_of::<Value>())?;
        match value {
            Value::Text(text) => self.charge(0, text.len())?,
            Value::Bytes(bytes) => self.charge(0, bytes.len())?,
            Value::Seq(items) => {
                for item in items {
                    self.copied(item, depth + 1)?;
                }
            }
            Value::Map(items) => {
                for (key, value) in items {
                    self.copied(key, depth + 1)?;
                    self.copied(value, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    position: usize,
    references: Vec<Option<Value>>,
    admission: Admission,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new("INVALID_INPUT", message)
}
fn resource(message: impl Into<String>) -> Error {
    Error::new("RESOURCE_LIMIT", message)
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
    fn string_bytes(&mut self, length: usize) -> Result<Vec<u8>> {
        self.admission.charge(0, length)?;
        let bytes = self.take(length)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(length)
            .map_err(|_| resource("PYZ TOC string allocation failed"))?;
        output.extend_from_slice(bytes);
        Ok(output)
    }
    fn long(&mut self) -> Result<i64> {
        let count = self.i32()?;
        let digits = count.unsigned_abs() as usize;
        if digits > MAX_LONG_DIGITS {
            return Err(invalid("marshal long does not fit in 64 bits"));
        }
        let mut magnitude = 0i128;
        for index in 0..digits {
            let digit = u16::from_le_bytes(self.take(2)?.try_into().expect("two bytes"));
            if digit >= 1 << 15 {
                return Err(invalid("invalid marshal long digit"));
            }
            magnitude |= i128::from(digit) << (15 * index);
        }
        i64::try_from(if count < 0 { -magnitude } else { magnitude })
            .map_err(|_| invalid("marshal long does not fit in 64 bits"))
    }
    fn slot(&mut self, flagged: bool) -> Result<Option<usize>> {
        if !flagged {
            return Ok(None);
        }
        self.admission
            .charge(0, std::mem::size_of::<Option<Value>>())?;
        self.references
            .try_reserve(1)
            .map_err(|_| resource("PYZ TOC reference allocation failed"))?;
        let index = self.references.len();
        self.references.push(None);
        Ok(Some(index))
    }
    fn cache(&mut self, slot: Option<usize>, value: &Value) -> Result<()> {
        if let Some(slot) = slot {
            self.admission.copied(value, 0)?;
            self.references[slot] = Some(value.clone());
        }
        Ok(())
    }
    fn sequence(&mut self, count: usize, depth: usize, flagged: bool) -> Result<Value> {
        if count
            > self
                .admission
                .limits
                .semantic_nodes
                .saturating_sub(self.admission.nodes)
            || count > self.bytes.len().saturating_sub(self.position)
            || count.saturating_mul(std::mem::size_of::<Value>())
                > self
                    .admission
                    .limits
                    .metadata_bytes
                    .saturating_sub(self.admission.metadata)
        {
            return Err(resource("PYZ TOC collection admission exceeded"));
        }
        let slot = self.slot(flagged)?;
        let mut items = Vec::new();
        items
            .try_reserve_exact(count)
            .map_err(|_| resource("PYZ TOC collection allocation failed"))?;
        for _ in 0..count {
            items.push(self.value(depth + 1)?);
        }
        let value = Value::Seq(items);
        self.cache(slot, &value)?;
        Ok(value)
    }
    fn mapping(&mut self, depth: usize, flagged: bool) -> Result<Value> {
        let slot = self.slot(flagged)?;
        let mut pairs = Vec::new();
        loop {
            let key = self.value(depth + 1)?;
            if matches!(key, Value::Null) {
                break;
            }
            let value = self.value(depth + 1)?;
            pairs
                .try_reserve(1)
                .map_err(|_| resource("PYZ TOC dictionary allocation failed"))?;
            pairs.push((key, value));
        }
        let value = Value::Map(pairs);
        self.cache(slot, &value)?;
        Ok(value)
    }
    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth >= MAX_DEPTH {
            return Err(resource("PYZ TOC marshal nesting is too deep"));
        }
        self.admission.charge(1, std::mem::size_of::<Value>())?;
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
                Value::Bytes(self.string_bytes(length)?)
            }
            b'u' | b'a' | b'A' | b'z' | b'Z' => {
                let length = if matches!(code & 0x7f, b'z' | b'Z') {
                    self.byte()? as usize
                } else {
                    self.u32()? as usize
                };
                Value::Text(
                    String::from_utf8(self.string_bytes(length)?)
                        .map_err(|_| invalid("marshal text is not UTF-8"))?,
                )
            }
            b'r' => {
                let index = self.u32()? as usize;
                let value = self
                    .references
                    .get(index)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| invalid("cyclic or out-of-range marshal reference"))?;
                self.admission.copied(value, depth)?;
                return Ok(value.clone());
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
        let slot = self.slot(flagged)?;
        self.cache(slot, &value)?;
        Ok(value)
    }
}
