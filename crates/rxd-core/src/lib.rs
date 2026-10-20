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

/// One failure class, chosen by whoever knows the reason (ADR 0015).
///
/// A failure carries exactly one code as data. The set is closed: a new code is a
/// contract change, not a message. Grouping is by what the caller can do about it, which
/// is also what decides the exit status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The query is malformed or its arguments contradict each other.
    Query,
    /// The query is well formed and names no record.
    NotFound,
    /// A name matches more than one identity; the caller must use a coordinate.
    Ambiguous,
    /// The input bytes are corrupt or are not this backend's format.
    Input,
    /// The input is well formed and uses something this build does not implement.
    Unsupported,
    /// A coordinate or stream came from a different input.
    Mismatch,
    /// A declared budget was reached; the message names it.
    Resource,
    /// Understanding the input worked and recovering source from it did not.
    Decompile,
    /// The filesystem, a pipe or another device failed.
    Io,
    /// An invariant broke: a bug, or a worker that died.
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

    /// The exit status is a property of the class, not of each command: a query the caller
    /// can fix exits 2, everything else exits 1.
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
            // Retired codes stay readable, so output from an older release still
            // classifies: an identifier is a coordinate now, and a format error is an
            // input error.
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

    /// The class of this error, when its code is one the contract knows.
    pub fn kind(&self) -> Option<Failure> {
        Failure::from_code(self.code)
    }

    /// The process exit status this error must produce.
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

    /// Build an error whose message may already start with its own code.
    ///
    /// Backend errors often carry the code inside the message (for example
    /// `anyhow::Context::context("INVALID_QUERY: source mismatch")`) and the CLI
    /// re-applies the code it detected. Display always prints `code: message`, so
    /// leading repetitions of this error's own `code:` are folded away here; the
    /// printed diagnostic carries the code exactly once.
    pub fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, strip_codes(&message.into()))
    }
}

/// Remove leading `CODE: ` tokens from a message.
///
/// A backend error may already begin with its code, because `anyhow` contexts are
/// strings; the boundary applies the class once, so the printed diagnostic never reads
/// `INVALID_QUERY: INVALID_INPUT: ...`. Only a leading, known code is removed - a code
/// word inside a sentence is data, not a prefix.
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

/// When a message starts with a code, that code and the rest of the message.
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
    /// Instructions one method may decode to.
    pub instructions: usize,
    /// Work units one query may spend.
    pub work: usize,
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
            instructions: 100_000,
            work: MAX_WORK_UNITS,
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
    pub ast_depth: usize,
    pub semantic_nodes: usize,
}

impl Default for Limits {
    /// The budget table is the only place these numbers live.
    fn default() -> Self {
        let budget = Budget::default();
        Self {
            input_bytes: budget.input_bytes,
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

    /// The coordinate of a span: `<kind>:<start>:<end>`.
    ///
    /// The input identity is the stream header's `source_id`, so an id does not repeat it;
    /// a coordinate is what a row prints and what `--at` takes back, unchanged.
    pub fn entity_id(&self, kind: &str, start: u32, end: u32) -> String {
        format!("{kind}:{start}:{end}")
    }
}
