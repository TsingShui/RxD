use serde::Serialize;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub input_bytes: usize,
    pub entities: usize,
    pub work: usize,
    pub instructions: usize,
    pub output_bytes: usize,
    pub records: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            input_bytes: rxd_core::MAX_INPUT_BYTES,
            entities: 100_000,
            work: 16 * 1024 * 1024,
            instructions: 100_000,
            output_bytes: 32 * 1024 * 1024,
            records: 100_000,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TypeInfo {
    pub id: String,
    pub declaring_type_token: Option<u32>,
    pub declaring_type_id: Option<String>,
    pub extends_token: Option<u32>,
    pub kind: String,
    pub interfaces: Vec<u32>,
    pub generic_parameters: Vec<GenericParameter>,
    pub token: u32,
    pub name: String,
    pub namespace: String,
    pub full_name: String,
    pub flags: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Field {
    pub id: String,
    pub declaring_type_id: String,
    pub token: u32,
    pub declaring_type_token: u32,
    pub declaring_type: String,
    pub name: String,
    pub signature: String,
    pub flags: u16,
}

#[derive(Clone, Debug, Serialize)]
pub struct Method {
    pub declaring_type_id: String,
    pub impl_flags: u16,
    pub generic_parameters: Vec<GenericParameter>,
    pub id: String,
    pub token: u32,
    pub declaring_type: String,
    pub declaring_type_token: u32,
    pub declaring_type_is_value_type: bool,
    pub name: String,
    pub signature: String,
    pub parameters: Vec<String>,
    pub signature_flags: u8,
    pub return_type: String,
    pub has_this: bool,
    pub has_body: bool,
    pub rva: u32,
    pub flags: u16,
}

#[derive(Clone, Debug, Serialize)]
pub struct Instruction {
    pub offset: u32,
    pub size: u32,
    pub opcode: u16,
    pub mnemonic: String,
    pub operand: Operand,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Operand {
    None,
    I32(i32),
    I64(i64),
    F32Bits(u32),
    F64Bits(u64),
    Variable(u16),
    Token(u32),
    Branch(u32),
    Switch(Vec<u32>),
}

#[derive(Clone, Debug, Serialize)]
pub struct ExceptionHandler {
    pub kind: String,
    pub try_offset: u32,
    pub try_length: u32,
    pub handler_offset: u32,
    pub handler_length: u32,
    pub class_token: Option<u32>,
    pub filter_offset: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MethodBody {
    pub work_used: usize,
    pub file_offset: usize,
    pub code_offset: usize,
    pub code_size: usize,
    pub max_stack: u16,
    pub init_locals: bool,
    pub local_signature_token: u32,
    pub locals: Vec<String>,
    pub instructions: Vec<Instruction>,
    pub handlers: Vec<ExceptionHandler>,
    pub il_hex: String,
}

#[derive(Clone, Debug)]
pub struct CallSignature {
    pub token: u32,
    pub name: String,
    pub parameters: Vec<String>,
    pub signature_flags: u8,
    pub return_type: String,
    pub has_this: bool,
    pub declaring_type_token: u32,
    pub method_arguments: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StringRecord {
    pub offset: u32,
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utf16_units: Option<Vec<u16>>,
}

/// Complete per-heap-entry/method string discovery, including unused inventory.
pub struct StringUses {
    pub literals: Vec<StringRecord>,
    pub groups: Vec<StringUse>,
}

pub struct StringUse {
    pub literal_index: usize,
    pub method_token: Option<u32>,
    pub il_offsets: Vec<u32>,
}

pub type StringReference = crate::TokenSite;

#[derive(Clone, Debug, Serialize)]
pub struct SourceMap {
    pub il_offset: u32,
    pub line: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Decompilation {
    pub representation: String,
    pub decompiled: bool,
    pub reason: Option<String>,
    /// Reconstruction diagnostics the engine reported while still producing code.
    pub warnings: Vec<String>,
    pub code: Option<String>,
    pub mapping: Vec<SourceMap>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ClassSourceMap {
    pub method_id: String,
    pub method_token: u32,
    pub il_offset: u32,
    pub line: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ClassDecompilation {
    pub representation: String,
    pub decompiled: bool,
    pub reason: Option<String>,
    /// Reconstruction diagnostics the engine reported while still producing code.
    pub warnings: Vec<String>,
    pub code: Option<String>,
    pub mapping: Vec<ClassSourceMap>,
}

/// The complete original selected type scope; methods without IL remain declarations.
pub struct ClassBody<'a> {
    pub root: &'a TypeInfo,
    pub types: Vec<&'a TypeInfo>,
    pub fields: Vec<&'a Field>,
    pub properties: Vec<&'a Property>,
    pub events: Vec<&'a Event>,
    pub methods: Vec<ClassMethod<'a>>,
    pub work_used: usize,
}

pub struct ClassMethod<'a> {
    pub method: &'a Method,
    pub body: Option<MethodBody>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GenericParameter {
    pub token: u32,
    pub number: u32,
    pub flags: u32,
    pub name: String,
    pub constraints: Vec<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Accessor {
    pub method_token: u32,
    pub method_id: String,
    pub semantics: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Property {
    pub token: u32,
    pub declaring_type_token: u32,
    pub declaring_type_id: String,
    pub name: String,
    pub signature: String,
    pub signature_hex: String,
    pub flags: u32,
    pub accessors: Vec<Accessor>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub token: u32,
    pub declaring_type_token: u32,
    pub declaring_type_id: String,
    pub name: String,
    pub event_type_token: u32,
    pub flags: u32,
    pub accessors: Vec<Accessor>,
}
