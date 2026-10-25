use std::io::{self, Write};

pub(crate) struct Output<'a> {
    writer: &'a mut dyn Write,
    remaining: Option<usize>,
}
impl<'a> Output<'a> {
    pub fn new(writer: &'a mut dyn Write, limit: Option<usize>) -> Self {
        Self {
            writer,
            remaining: limit,
        }
    }
    #[cfg(feature = "backend-dex")]
    pub fn remaining(&self) -> Option<usize> {
        self.remaining
    }
}
impl Write for Output<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.remaining.is_some_and(|limit| bytes.len() > limit) {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "query output byte budget exceeded",
            ));
        }
        let written = self.writer.write(bytes)?;
        if let Some(remaining) = self.remaining.as_mut() {
            *remaining -= written;
        }
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}
