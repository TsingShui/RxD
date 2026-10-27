use std::{
    fs::{self, OpenOptions},
    io::{self, Cursor, Read, Seek, SeekFrom},
    path::Path,
};

use rxd_core::{Budget, Error, Result};

pub(super) fn invalid(message: impl Into<String>) -> Error {
    Error::new("INVALID_INPUT", message)
}

pub(super) fn resource(message: impl Into<String>) -> Error {
    Error::new("RESOURCE_LIMIT", message)
}

pub(super) fn zip_error(error: zip::result::ZipError) -> Error {
    let code = match error {
        zip::result::ZipError::UnsupportedArchive(_) => "UNSUPPORTED_INPUT",
        _ => "INVALID_INPUT",
    };
    Error::new(code, error.to_string())
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

// Keep reverse footer searches in a bounded cache instead of seeking once per comment byte.
pub(super) struct Reader {
    file: Box<dyn ReadSeek>,
    tail: Vec<u8>,
    tail_start: u64,
    len: u64,
    search_end: u64,
    position: u64,
    disk_position: Option<u64>,
}

impl Read for Reader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.tail_start {
            let start = self.position.saturating_sub(self.tail_start);
            let bytes = usize::try_from(start)
                .ok()
                .and_then(|start| self.tail.get(start..))
                .unwrap_or_default();
            let len = bytes.len().min(buffer.len());
            buffer[..len].copy_from_slice(&bytes[..len]);
            self.position += len as u64;
            return Ok(len);
        }
        if self.disk_position != Some(self.position) {
            self.file.seek(SeekFrom::Start(self.position))?;
        }
        let bound = (self.tail_start - self.position).min(buffer.len() as u64) as usize;
        let len = self.file.read(&mut buffer[..bound])?;
        self.position += len as u64;
        self.disk_position = Some(self.position);
        Ok(len)
    }
}

impl Seek for Reader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(at) => i128::from(at),
            SeekFrom::End(at) => i128::from(self.search_end) + i128::from(at),
            SeekFrom::Current(at) => i128::from(self.position) + i128::from(at),
        };
        self.position = u64::try_from(position)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid ZIP seek"))?;
        Ok(self.position)
    }
}

pub(super) struct Directory {
    pub start: u64,
    pub end: u64,
    pub flags: Vec<u16>,
    pub methods: Vec<u16>,
}

pub(super) fn open(input: &Path, budget: Budget) -> Result<(Reader, Directory)> {
    if input.to_str().is_none() || !fs::symlink_metadata(input)?.is_file() {
        return Err(invalid(
            "ZIP input must be a regular UTF-8 local file, not a symlink",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(input)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("ZIP input must be a regular file"));
    }
    let len = metadata.len();
    open_reader(Box::new(file), len, budget)
}

pub(super) fn open_bytes(bytes: Vec<u8>, budget: Budget) -> Result<(Reader, Directory)> {
    let len = bytes.len() as u64;
    open_reader(Box::new(Cursor::new(bytes)), len, budget)
}

fn open_reader(
    mut file: Box<dyn ReadSeek>,
    len: u64,
    budget: Budget,
) -> Result<(Reader, Directory)> {
    if len > budget.input_bytes as u64 {
        return Err(resource("ZIP input exceeds admitted byte length"));
    }
    let tail_len = len.min(65557) as usize;
    if tail_len > budget.metadata_bytes {
        return Err(resource("ZIP footer exceeds metadata budget"));
    }
    let mut tail = Vec::new();
    tail.try_reserve_exact(tail_len)
        .map_err(|_| resource("ZIP footer allocation failed"))?;
    tail.resize(tail_len, 0);
    file.seek(SeekFrom::End(-(tail_len as i64)))?;
    file.read_exact(&mut tail)?;
    let mut reader = Reader {
        file,
        tail,
        tail_start: len - tail_len as u64,
        len,
        search_end: len,
        position: 0,
        disk_position: None,
    };
    let directory = preflight(&mut reader, budget)?;
    Ok((reader, directory))
}

fn header<const N: usize>(reader: &mut Reader, at: u64) -> Result<[u8; N]> {
    if at.checked_add(N as u64).is_none_or(|end| end > reader.len) {
        return Err(invalid("ZIP header is outside input"));
    }
    reader.seek(SeekFrom::Start(at))?;
    let mut bytes = [0; N];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| invalid(error.to_string()))?;
    Ok(bytes)
}

// Validate counts and record lengths before the dependency allocates its central index.
fn preflight(reader: &mut Reader, budget: Budget) -> Result<Directory> {
    let relative = reader
        .tail
        .windows(22)
        .enumerate()
        .rfind(|(at, bytes)| {
            bytes.starts_with(b"PK\x05\x06")
                && at + 22 + usize::from(u16_at(bytes, 20)) == reader.tail.len()
        })
        .map(|(at, _)| at)
        .ok_or_else(|| invalid("missing ZIP end-of-central-directory record"))?;
    let footer = &reader.tail[relative..relative + 22];
    if u16_at(footer, 4) != 0 || u16_at(footer, 6) != 0 {
        return Err(Error::new(
            "UNSUPPORTED_INPUT",
            "split ZIP archives are unsupported",
        ));
    }
    let footer_at = reader.tail_start + relative as u64;
    // The pinned decoder starts at this EOCD, not at lookalike signatures inside its comment.
    reader.search_end = footer_at + 22;
    let mut count = u64::from(u16_at(footer, 10));
    let disk_count = u16_at(footer, 8);
    let mut size = u64::from(u32_at(footer, 12));
    let mut start = u64::from(u32_at(footer, 16));
    let mut trailer = footer_at;
    let requires_zip64 =
        count == u16::MAX as u64 || size == u32::MAX as u64 || start == u32::MAX as u64;
    let locator = footer_at
        .checked_sub(20)
        .map(|at| header::<20>(reader, at))
        .transpose()?;
    if let Some(locator) = locator.filter(|bytes| bytes.starts_with(b"PK\x06\x07")) {
        if u32_at(&locator, 4) != 0 || u32_at(&locator, 16) != 1 {
            return Err(Error::new(
                "UNSUPPORTED_INPUT",
                "split ZIP64 archives are unsupported",
            ));
        }
        trailer = u64_at(&locator, 8);
        let record = header::<56>(reader, trailer)?;
        if !record.starts_with(b"PK\x06\x06")
            || u64_at(&record, 4) < 44
            || trailer
                .checked_add(12)
                .and_then(|at| at.checked_add(u64_at(&record, 4)))
                != footer_at.checked_sub(20)
        {
            return Err(invalid("invalid ZIP64 directory record"));
        }
        if u32_at(&record, 16) != 0
            || u32_at(&record, 20) != 0
            || u64_at(&record, 24) != u64_at(&record, 32)
        {
            return Err(Error::new(
                "UNSUPPORTED_INPUT",
                "split ZIP64 archives are unsupported",
            ));
        }
        count = u64_at(&record, 32);
        size = u64_at(&record, 40);
        start = u64_at(&record, 48);
    } else if requires_zip64 {
        return Err(invalid("missing ZIP64 directory locator"));
    } else if disk_count as u64 != count {
        return Err(invalid("inconsistent ZIP entry counts"));
    }
    let end = start
        .checked_add(size)
        .filter(|end| *end <= trailer)
        .ok_or_else(|| invalid("ZIP central directory is outside input"))?;
    if count > size / 46 {
        return Err(invalid("ZIP entry count exceeds central directory size"));
    }
    let count =
        usize::try_from(count).map_err(|_| resource("ZIP entry count is not addressable"))?;
    let accounting = count
        .checked_mul(1024)
        .and_then(|n| {
            size.checked_mul(4)
                .and_then(|size| n.checked_add(usize::try_from(size).ok()?))
        })
        .ok_or_else(|| resource("ZIP index accounting overflow"))?;
    if count > budget.entries || accounting > budget.metadata_bytes {
        return Err(resource("ZIP central index exceeds metadata budget"));
    }
    let mut flags = Vec::new();
    let mut methods = Vec::new();
    flags
        .try_reserve_exact(count)
        .map_err(|_| resource("ZIP index allocation failed"))?;
    methods
        .try_reserve_exact(count)
        .map_err(|_| resource("ZIP index allocation failed"))?;
    let mut cursor = start;
    for _ in 0..count {
        if cursor.checked_add(46).is_none_or(|at| at > end) {
            return Err(invalid("truncated ZIP central directory"));
        }
        let bytes = header::<46>(reader, cursor)?;
        if !bytes.starts_with(b"PK\x01\x02") {
            return Err(invalid("invalid ZIP central header"));
        }
        if u16_at(&bytes, 34) != 0 {
            return Err(Error::new(
                "UNSUPPORTED_INPUT",
                "split ZIP entries are unsupported",
            ));
        }
        let variable = u64::from(u16_at(&bytes, 28))
            + u64::from(u16_at(&bytes, 30))
            + u64::from(u16_at(&bytes, 32));
        cursor = cursor
            .checked_add(46 + variable)
            .filter(|at| *at <= end)
            .ok_or_else(|| invalid("ZIP central entry exceeds directory range"))?;
        flags.push(u16_at(&bytes, 8));
        methods.push(u16_at(&bytes, 10));
    }
    if cursor != end {
        return Err(Error::new(
            "UNSUPPORTED_INPUT",
            "unsupported ZIP central directory extension",
        ));
    }
    Ok(Directory {
        start,
        end,
        flags,
        methods,
    })
}
