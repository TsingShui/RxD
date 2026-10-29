use anyhow::{Context, Result, ensure};

pub(crate) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(length)
            .context("INVALID_INPUT: byte range overflow")?;
        let bytes = self
            .bytes
            .get(self.pos..end)
            .context("INVALID_INPUT: truncated data")?;
        self.pos = end;
        Ok(bytes)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }

    pub fn count(&mut self) -> Result<usize> {
        let value = self.u32()?;
        ensure!(
            value <= i32::MAX as u32,
            "INVALID_INPUT: negative count or offset"
        );
        Ok(value as usize)
    }

    pub fn offset(&mut self) -> Result<usize> {
        let value = self.u64()?;
        ensure!(
            value <= i64::MAX as u64,
            "INVALID_INPUT: negative size or offset"
        );
        usize::try_from(value).context("UNSUPPORTED_INPUT: size is not addressable")
    }

    pub fn seven(&mut self) -> Result<u32> {
        let mut value = 0;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            ensure!(
                shift != 28 || byte <= 15,
                "INVALID_INPUT: invalid 7-bit integer"
            );
            value |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        anyhow::bail!("INVALID_INPUT: unterminated 7-bit integer")
    }

    pub fn string_bytes(&mut self) -> Result<&'a [u8]> {
        let length = self.seven()?;
        ensure!(
            length <= i32::MAX as u32,
            "INVALID_INPUT: negative string length"
        );
        self.take(length as usize)
    }

    pub fn text(&mut self) -> Result<&'a str> {
        std::str::from_utf8(self.string_bytes()?).context("INVALID_INPUT: invalid UTF-8")
    }
}
