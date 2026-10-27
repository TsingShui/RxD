use rxd_core::{Error, Result};

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::new("INVALID_INPUT", message)
}

pub(crate) fn resource(message: impl Into<String>) -> Error {
    Error::new("RESOURCE_LIMIT", message)
}

pub(crate) fn range(data: &[u8], offset: u32, count: usize) -> Result<&[u8]> {
    let start = offset as usize;
    start
        .checked_add(count)
        .and_then(|end| data.get(start..end))
        .ok_or_else(|| invalid(format!("ABC range outside input: {offset:#x} + {count}")))
}

pub(crate) fn table(data: &[u8], offset: u32, count: u32, width: usize) -> Result<&[u8]> {
    if !offset.is_multiple_of(4) {
        return Err(invalid("ABC index offset is not aligned"));
    }
    let bytes = (count as usize)
        .checked_mul(width)
        .ok_or_else(|| invalid("ABC index length overflow"))?;
    range(data, offset, bytes)
}

pub(crate) fn word(data: &[u8], index: usize) -> u32 {
    u32::from_le_bytes(data[index * 4..index * 4 + 4].try_into().unwrap())
}

pub(crate) struct Reader<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) position: usize,
    end: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8], start: u32, end: usize) -> Result<Self> {
        if start as usize > end || end > data.len() {
            return Err(invalid("ABC cursor outside input"));
        }
        Ok(Self {
            data,
            position: start as usize,
            end,
        })
    }

    pub(crate) fn remaining(&self) -> usize {
        self.end - self.position
    }

    pub(crate) fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(size)
            .filter(|end| *end <= self.end)
            .ok_or_else(|| invalid(format!("truncated ABC entity at {:#x}", self.position)))?;
        let bytes = &self.data[self.position..end];
        self.position = end;
        Ok(bytes)
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub(crate) fn uleb(&mut self) -> Result<u32> {
        let mut value = 0u32;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            if shift == 28 && byte & 0xf0 != 0 {
                return Err(invalid("ABC ULEB128 overflows u32"));
            }
            value |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(invalid("unterminated ABC ULEB128"))
    }

    pub(crate) fn sleb(&mut self) -> Result<i32> {
        let mut value = 0i64;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            value |= i64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                if byte & 0x40 != 0 {
                    value |= !0i64 << (shift + 7);
                }
                return i32::try_from(value).map_err(|_| invalid("ABC SLEB128 overflows i32"));
            }
        }
        Err(invalid("unterminated ABC SLEB128"))
    }
}

#[derive(Debug)]
pub struct Name {
    pub offset: u32,
    pub end: u32,
    pub units: Vec<u16>,
    pub text: String,
}

impl Name {
    pub fn scalar_text(&self) -> Option<&str> {
        self.units
            .iter()
            .copied()
            .eq(self.text.encode_utf16())
            .then_some(self.text.as_str())
    }

    pub(crate) fn read(reader: &mut Reader<'_>, metadata_limit: usize) -> Result<Self> {
        let offset = reader.position as u32;
        let encoded = reader.uleb()?;
        let length = (encoded >> 1) as usize;
        let ascii = encoded & 1 != 0;
        if length >= reader.remaining() {
            return Err(invalid("ABC string length exceeds its byte range"));
        }
        if length
            .checked_mul(5)
            .is_none_or(|size| size > metadata_limit)
        {
            return Err(resource("ABC string exceeds metadata budget"));
        }
        let mut units = Vec::new();
        units
            .try_reserve_exact(length)
            .map_err(|_| resource("ABC string allocation failed"))?;
        for _ in 0..length {
            let first = reader.u8()?;
            let unit = match first {
                1..=0x7f => u16::from(first),
                0xc0..=0xdf if !ascii => {
                    let second = reader.u8()?;
                    if second & 0xc0 != 0x80 {
                        return Err(invalid("invalid MUTF8 continuation"));
                    }
                    let value = (u16::from(first & 0x1f) << 6) | u16::from(second & 0x3f);
                    if value < 0x80 && !(first == 0xc0 && second == 0x80) {
                        return Err(invalid("overlong MUTF8 sequence"));
                    }
                    value
                }
                0xe0..=0xef if !ascii => {
                    let second = reader.u8()?;
                    let third = reader.u8()?;
                    if second & 0xc0 != 0x80 || third & 0xc0 != 0x80 {
                        return Err(invalid("invalid MUTF8 continuation"));
                    }
                    let value = (u16::from(first & 15) << 12)
                        | (u16::from(second & 63) << 6)
                        | u16::from(third & 63);
                    if value < 0x800 {
                        return Err(invalid("overlong MUTF8 sequence"));
                    }
                    value
                }
                _ => return Err(invalid("invalid MUTF8 string or ASCII flag")),
            };
            units.push(unit);
        }
        if reader.u8()? != 0 {
            return Err(invalid("ABC string lacks its NUL terminator"));
        }
        let text = String::from_utf16_lossy(&units);
        Ok(Self {
            offset,
            end: reader.position as u32,
            units,
            text,
        })
    }

    pub fn write_quoted(&self, output: &mut impl std::io::Write) -> std::io::Result<()> {
        if let Some(text) = self.scalar_text() {
            return serde_json::to_writer(output, text).map_err(|error| {
                std::io::Error::new(
                    error
                        .io_error_kind()
                        .unwrap_or(std::io::ErrorKind::InvalidData),
                    error,
                )
            });
        }
        output.write_all(b"\"")?;
        for unit in &self.units {
            write!(output, "\\u{unit:04x}")?;
        }
        output.write_all(b"\"")
    }

    /// Preserve lone UTF-16 surrogates instead of replacing them in source or listing cells.
    pub fn quoted(&self) -> String {
        if let Ok(text) = String::from_utf16(&self.units) {
            return serde_json::to_string(&text).expect("text serializes");
        }
        use std::fmt::Write;
        let mut result = String::from("\"");
        for unit in &self.units {
            write!(&mut result, "\\u{unit:04x}").unwrap();
        }
        result.push('"');
        result
    }
}
