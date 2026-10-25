pub mod budget;
pub mod output;
pub use budget::BudgetPolicy;
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

/// Failure classes determine diagnostic codes and exit status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Malformed or contradictory query.
    Query,
    /// Valid query with no selected record.
    NotFound,
    /// Multiple identities match; select a coordinate.
    Ambiguous,
    /// Corrupt bytes or wrong format.
    Input,
    /// Valid but unsupported input feature.
    Unsupported,
    /// Selector/stream belongs to different input bytes.
    Mismatch,
    /// Explicit resource budget exhausted.
    Resource,
    /// Source reconstruction failed after input analysis.
    Decompile,

    Io,
    /// Broken invariant or failed worker.
    Internal,
}

impl Failure {
    pub const ALL: [Failure; 10] = [
        Failure::Query,
        Failure::NotFound,
        Failure::Ambiguous,
        Failure::Input,
        Failure::Unsupported,
        Failure::Mismatch,
        Failure::Resource,
        Failure::Decompile,
        Failure::Io,
        Failure::Internal,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Failure::Query => "INVALID_QUERY",
            Failure::NotFound => "NOT_FOUND",
            Failure::Ambiguous => "AMBIGUOUS",
            Failure::Input => "INVALID_INPUT",
            Failure::Unsupported => "UNSUPPORTED_INPUT",
            Failure::Mismatch => "SOURCE_MISMATCH",
            Failure::Resource => "RESOURCE_LIMIT",
            Failure::Decompile => "DECOMPILE_ERROR",
            Failure::Io => "IO_ERROR",
            Failure::Internal => "INTERNAL_ERROR",
        }
    }

    /// Fixable query failures exit 2; all other failures exit 1.
    pub fn exit_code(self) -> u8 {
        match self {
            Failure::Query | Failure::NotFound | Failure::Ambiguous => 2,
            _ => 1,
        }
    }

    /// Classify a code, including the spellings older releases printed.
    pub fn from_code(code: &str) -> Option<Failure> {
        Some(match code {
            "INVALID_QUERY" => Failure::Query,
            "NOT_FOUND" => Failure::NotFound,
            "AMBIGUOUS" => Failure::Ambiguous,
            "INVALID_INPUT" => Failure::Input,
            "UNSUPPORTED_INPUT" | "UNSUPPORTED" => Failure::Unsupported,
            "SOURCE_MISMATCH" => Failure::Mismatch,
            "RESOURCE_LIMIT" => Failure::Resource,
            "DECOMPILE_ERROR" => Failure::Decompile,
            "IO_ERROR" => Failure::Io,
            "INTERNAL_ERROR" => Failure::Internal,

            "INVALID_ID" | "SOURCE_MISMATCH_ID" => Failure::Query,
            "FORMAT_ERROR" => Failure::Input,
            _ => return None,
        })
    }
}

impl Error {
    pub fn failure(kind: Failure, message: impl Into<String>) -> Self {
        Self::coded(kind.code(), message)
    }

    pub fn kind(&self) -> Option<Failure> {
        Failure::from_code(self.code)
    }

    pub fn exit_code(&self) -> u8 {
        self.kind().map_or(1, Failure::exit_code)
    }
}

impl Error {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Build a coded error, removing repeated leading diagnostic codes.
    pub fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, strip_codes(&message.into()))
    }
}

/// Strip only known leading diagnostic codes, never embedded text.
pub fn strip_codes(message: &str) -> String {
    let mut rest = message;
    while let Some((head, tail)) = rest.split_once(": ") {
        if Failure::from_code(head).is_none() {
            break;
        }
        rest = tail;
    }
    rest.to_owned()
}

pub fn leading_code(message: &str) -> (Option<Failure>, &str) {
    if let Some((head, tail)) = message.split_once(": ")
        && let Some(kind) = Failure::from_code(head)
    {
        return (Some(kind), tail);
    }
    (None, message)
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::failure(
            if error.kind() == std::io::ErrorKind::OutOfMemory {
                Failure::Resource
            } else {
                Failure::Io
            },
            error.to_string(),
        )
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Default explicit input quota; CLI admission uses actual file length.
pub const MAX_INPUT_BYTES: usize = 8 << 30;

/// Default explicit entry quota; unset for CLI/MCP.
pub const MAX_ENTRY_BYTES: usize = 256 << 20;

/// Default archive-index allowance.
pub const MAX_ARCHIVE_ENTRIES: usize = 1 << 20;

/// Default retained-metadata allowance.
pub const MAX_METADATA_BYTES: usize = 128 << 20;

/// Default explicit work quota; unset for CLI/MCP.
pub const MAX_WORK_UNITS: usize = 1_000_000;

/// Explicit caller quotas; CLI metadata/index bounds use for_input_bytes.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub input_bytes: usize,
    /// Optional per-entry quota, separate from format widths.
    pub entry_bytes: Option<usize>,
    /// Decoded metadata one analysis may hold.
    pub metadata_bytes: usize,
    /// Optional output quota; unset for CLI serialization.
    pub output_bytes: Option<usize>,

    pub entries: usize,
    /// Optional per-method instruction quota.
    pub instructions: Option<usize>,
    /// Optional work quota; unset for CLI/MCP.
    pub work: Option<usize>,
    /// AST nesting one syntax tree may reach, checked after parsing.
    pub ast_depth: usize,
    /// Semantic index nodes one query may allocate.
    pub semantic_nodes: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            input_bytes: MAX_INPUT_BYTES,
            entry_bytes: Some(MAX_ENTRY_BYTES),
            metadata_bytes: MAX_METADATA_BYTES,
            output_bytes: Some(MAX_METADATA_BYTES),
            entries: MAX_ARCHIVE_ENTRIES,
            instructions: Some(100_000),
            work: Some(MAX_WORK_UNITS),
            ast_depth: 512,
            semantic_nodes: 1_000_000,
        }
    }
}

/// 32-bit content-span bound; archive carriers are not content spans.
pub const MAX_SPAN_BYTES: usize = u32::MAX as usize;

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Limits {
    pub input_bytes: usize,
    pub ast_depth: usize,
    pub semantic_nodes: usize,
    pub metadata_bytes: usize,
    pub output_bytes: Option<usize>,
    pub work: Option<usize>,
}

impl From<Budget> for Limits {
    fn from(budget: Budget) -> Self {
        Self {
            input_bytes: budget.input_bytes,
            ast_depth: budget.ast_depth,
            semantic_nodes: budget.semantic_nodes,
            metadata_bytes: budget.metadata_bytes,
            output_bytes: budget.output_bytes,
            work: budget.work,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Budget::default().into()
    }
}

/// Routing metadata; never substitutes for the selected file's content identity.
#[derive(Clone, serde::Serialize)]
pub struct Origin {
    pub container: String,
    pub format: &'static str,
    pub entry: String,
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
        let id = perf.measure("hash", || identity::input_id(&bytes));
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

    /// File-scoped span coordinate: `<kind>:<start>:<end>`.
    pub fn entity_id(&self, kind: &str, start: u32, end: u32) -> String {
        format!("{kind}:{start}:{end}")
    }
}
