pub mod identity;
use std::{fmt, fs::OpenOptions, io::Read, path::Path};

#[cfg(feature = "runtime-bundle")]
pub mod bundle;
pub mod envelope;
pub mod filter;
pub mod perf;

#[derive(Debug)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}

impl Error {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Build an error whose message may already start with its own code.
    ///
    /// Backend errors often carry the code inside the message (for example
    /// `anyhow::Context::context("INVALID_QUERY: source mismatch")`) and the CLI
    /// re-applies the code it detected. Display always prints `code: message`, so
    /// leading repetitions of this error's own `code:` are folded away here; the
    /// printed diagnostic carries the code exactly once.
    pub fn coded(code: &'static str, message: impl Into<String>) -> Self {
        let message = message.into();
        let prefix = format!("{code}: ");
        let mut rest = message.as_str();
        while let Some(stripped) = rest.strip_prefix(prefix.as_str()) {
            rest = stripped;
        }
        if rest.len() == message.len() {
            Self::new(code, message)
        } else {
            Self::new(code, rest.to_string())
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new("IO_ERROR", error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Default and hard ceiling for one input file: 8 GiB. Every supported rxd target
/// is 64-bit. No CLI flag can raise or lower it; `Budget` is the only source.
pub const MAX_INPUT_BYTES: usize = 8 << 30;

/// One archive entry: 256 MiB. DEX applies this to a single inflated entry and the JVM
/// backend to one class file, so the ceiling is a property of the format family, not of
/// a backend.
pub const MAX_ENTRY_BYTES: usize = 256 << 20;

/// Physical archive entries: one million. A production APK or fat JAR stays far below
/// it, and the cap exists so a crafted directory cannot make the reader allocate.
pub const MAX_ARCHIVE_ENTRIES: usize = 1 << 20;

/// Decoded metadata a single analysis may hold: 128 MiB.
pub const MAX_METADATA_BYTES: usize = 128 << 20;

/// Work units one query may spend: one million.
pub const MAX_WORK_UNITS: usize = 1_000_000;

/// Bytes one command may print or write: 32 MiB.
pub const MAX_OUTPUT_BYTES: usize = 32 << 20;

/// The resource budget every backend runs under.
///
/// One table for the whole tool: a backend projects it onto its own `Limits` type
/// instead of restating the numbers, so a ceiling is changed in exactly one place.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// One input file.
    pub input_bytes: usize,
    /// One archive entry (a DEX entry, a class file, an embedded resource).
    pub entry_bytes: usize,
    /// Decoded metadata one analysis may hold.
    pub metadata_bytes: usize,
    /// Physical archive entries.
    pub entries: usize,
    /// Entities one analysis may enumerate (types, methods, strings).
    pub entities: usize,
    /// Instructions one method may decode to.
    pub instructions: usize,
    /// Work units one query may spend.
    pub work: usize,
    /// Records one command may print.
    pub records: usize,
    /// Bytes one command may print or write.
    pub output_bytes: usize,
    /// AST nesting one syntax tree may reach, checked after parsing.
    pub ast_depth: usize,
    /// Semantic index nodes one query may allocate.
    pub semantic_nodes: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            input_bytes: MAX_INPUT_BYTES,
            entry_bytes: MAX_ENTRY_BYTES,
            metadata_bytes: MAX_METADATA_BYTES,
            entries: MAX_ARCHIVE_ENTRIES,
            entities: 100_000,
            instructions: 100_000,
            work: MAX_WORK_UNITS,
            records: 100_000,
            output_bytes: MAX_OUTPUT_BYTES,
            ast_depth: 512,
            semantic_nodes: 1_000_000,
        }
    }
}

/// Content spans are 32-bit, so no backend can address an input at or above 4 GiB.
/// This is a structural limit of the formats, not a policy budget.
pub const MAX_SPAN_BYTES: usize = u32::MAX as usize;

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Limits {
    pub input_bytes: usize,
    pub entities: usize,
    pub records: usize,
    pub output_bytes: usize,
    pub ast_depth: usize,
    pub semantic_nodes: usize,
}

impl Default for Limits {
    /// The budget table is the only place these numbers live.
    fn default() -> Self {
        let budget = Budget::default();
        Self {
            input_bytes: budget.input_bytes,
            entities: budget.entities,
            records: budget.records,
            output_bytes: budget.output_bytes,
            ast_depth: budget.ast_depth,
            semantic_nodes: budget.semantic_nodes,
        }
    }
}

/// Routing metadata; never substitutes for the selected file's content identity.
#[derive(Clone, serde::Serialize)]
pub struct Origin {
    pub container: String,
    pub format: &'static str,
    pub entry: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_id: Option<String>,
}

/// Read one bounded binary buffer from a regular local file.
pub fn read_bytes(path: &Path, max_input_bytes: usize) -> Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    // Opening a FIFO must not block before we can reject non-file inputs.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    // Allow directory handles on Windows so the opened-handle check rejects them.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(Error::new("INVALID_INPUT", "expected a regular local file"));
    }
    let budget = max_input_bytes.min(MAX_SPAN_BYTES);
    if metadata.len() > budget as u64 {
        return Err(if metadata.len() > max_input_bytes as u64 {
            Error::new("RESOURCE_LIMIT", "input byte budget exceeded")
        } else {
            Error::new(
                "RESOURCE_LIMIT",
                "input exceeds the 4 GiB content span limit",
            )
        });
    }
    let mut bytes = Vec::new();
    file.take((budget as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_input_bytes || bytes.len() > MAX_SPAN_BYTES {
        return Err(Error::new("RESOURCE_LIMIT", "input byte budget exceeded"));
    }
    Ok(bytes)
}

pub struct Source {
    pub text: String,
    pub id: String,
    pub path: String,
    pub origin: Option<Origin>,
}

impl Source {
    pub fn read(path: &Path, limits: Limits, perf: &mut perf::Perf) -> Result<Self> {
        let bytes = perf.measure("read", || read_bytes(path, limits.input_bytes))?;
        Self::from_bytes(path.to_string_lossy().into_owned(), bytes, limits, perf)
    }

    /// Analyze one fixed buffer, including sources read directly from a container.
    pub fn from_bytes(
        path: String,
        bytes: Vec<u8>,
        limits: Limits,
        perf: &mut perf::Perf,
    ) -> Result<Self> {
        if bytes.len() > limits.input_bytes || bytes.len() > u32::MAX as usize {
            return Err(Error::new("RESOURCE_LIMIT", "input byte budget exceeded"));
        }
        perf.counters.input_bytes = Some(bytes.len());
        let id = perf.measure("hash", || identity::content_id(Path::new(&path), &bytes));
        let text = perf.measure("utf8", || {
            String::from_utf8(bytes)
                .map_err(|_| Error::new("INVALID_INPUT", "input must be valid UTF-8"))
        })?;
        if perf.enabled() {
            perf.source_id = Some(id.clone());
        }
        Ok(Self {
            text,
            id,
            path,
            origin: None,
        })
    }

    pub fn entity_id(&self, kind: &str, start: u32, end: u32) -> String {
        format!("rxd1:js:{}:{kind}:{start}:{end}", self.id)
    }
}
