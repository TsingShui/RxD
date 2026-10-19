use serde::Serialize;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub input_bytes: usize,
    pub entities: usize,
    pub work: usize,
    pub instructions: usize,
    pub records: usize,
    pub output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            input_bytes: rxd_core::MAX_INPUT_BYTES,
            entities: 100_000,
            work: 16 * 1024 * 1024,
            instructions: 1_000_000,
            records: 100_000,
            output_bytes: 32 * 1024 * 1024,
        }
    }
}

/// A function signature from the type section.
#[derive(Clone, Debug, Serialize)]
pub struct FuncType {
    pub type_index: u32,
    pub params: Vec<String>,
    pub results: Vec<String>,
}

/// One imported entry; only function imports consume the function index space.
#[derive(Clone, Debug, Serialize)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub kind: &'static str,
    /// For `kind == "func"`, the merged function index assigned to this import.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub func_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_index: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Export {
    pub name: String,
    pub kind: &'static str,
    pub index: u32,
}

/// A defined (module-local) function. Imported functions are not listed here.
#[derive(Clone, Debug, Serialize)]
pub struct Function {
    pub id: String,
    pub func_index: u32,
    pub type_index: u32,
    /// Name-section label when present; navigation only, never identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub params: Vec<String>,
    pub results: Vec<String>,
    /// Byte range of the function's code body within the module file.
    pub body_file_offset: usize,
    pub body_size: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Table {
    pub index: u32,
    pub element_type: String,
    pub initial: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Memory {
    pub index: u32,
    pub initial: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<u64>,
    pub memory64: bool,
    pub shared: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Global {
    pub index: u32,
    pub value_type: String,
    pub mutable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DataSegment {
    pub index: u32,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_index: Option<u32>,
    /// Declared offset-expression text; never folded or evaluated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_expr: Option<String>,
    pub byte_length: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ElementSegment {
    pub index: u32,
    pub kind: &'static str,
    pub count: usize,
}

/// A decoded string from a data segment or the name section.
#[derive(Clone, Debug, Serialize)]
pub struct StringRecord {
    pub origin: &'static str,
    /// Owning data-segment index when `origin == "data"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segment_index: Option<u32>,
    /// In-segment byte offset for data strings.
    pub offset: usize,
    /// Decoded UTF-8 value, or null when the bytes are not valid UTF-8.
    pub value: Option<String>,
    /// Raw bytes preserved verbatim when `value` is null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
}

/// An explicit function reference discovered in a defined function body.
#[derive(Clone, Debug, Serialize)]
pub struct Reference {
    pub id: String,
    pub caller_index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_name: Option<String>,
    /// Byte offset within the caller's code body.
    pub offset: usize,
    pub opcode: &'static str,
    pub target_index: u32,
}

/// One instruction in the conservative single-function instruction view.
#[derive(Clone, Debug, Serialize)]
pub struct Instruction {
    /// Byte offset within the function code body.
    pub offset: usize,
    pub opcode: String,
    /// Decoded immediate operand(s); `Operand::None` when the opcode has none.
    pub operand: Operand,
}

/// A decoded immediate operand. Reference indices are preserved explicitly;
/// `call_indirect` keeps its type/table indices and is never a direct target.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operand {
    None,
    I32 { value: i32 },
    I64 { value: i64 },
    F32 { bits: u32 },
    F64 { bits: u64 },
    Local { index: u32 },
    Global { index: u32 },
    Function { index: u32 },
    Type { index: u32 },
    Table { index: u32 },
    Memory { memarg: MemArg },
    Branch { depth: u32 },
    BranchTable { targets: Vec<u32>, default: u32 },
    Block { block_type: String },
    CallIndirect { type_index: u32, table_index: u32 },
}

#[derive(Clone, Debug, Serialize)]
pub struct MemArg {
    pub align: u8,
    pub offset: u64,
    pub memory: u32,
}

/// A single-function instruction-view record.
#[derive(Clone, Debug, Serialize)]
pub struct FunctionView {
    pub representation: &'static str,
    pub decompiled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub code: Option<String>,
    pub func_index: u32,
    pub type_index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub params: Vec<String>,
    pub results: Vec<String>,
    pub locals: Vec<LocalDeclView>,
    pub body_file_offset: usize,
    pub body_size: usize,
    pub bytecode_hex: String,
    pub instructions: Vec<Instruction>,
    /// Output line (1-based) -> code-body byte offset, for the clike path.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mapping: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LocalDeclView {
    pub count: u32,
    pub value_type: String,
}
