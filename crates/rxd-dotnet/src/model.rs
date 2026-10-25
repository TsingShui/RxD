use serde::Serialize;
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub input_bytes: usize,
    pub work: usize,
    pub instructions: usize,
    pub metadata_bytes: usize,
    /// Optional caller quota for complete class/type projection, including nested bodies.
    pub output_bytes: Option<usize>,
}

impl Default for Limits {
    fn default() -> Self {
        let budget = rxd_core::Budget::default();
        Self {
            input_bytes: budget.input_bytes,
            work: budget.work,
            instructions: budget.instructions,
            metadata_bytes: budget.metadata_bytes,
            output_bytes: budget.output_bytes,
        }
    }
}

/// One metadata row that could not be turned into an indexed entry. The query keeps
/// answering from the rows it could read; `complete` is false while any exist.
#[derive(Clone, Debug, Serialize)]
pub struct Unreadable {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<u32>,
    pub kind: &'static str,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TypeInfo {
    #[serde(skip_serializing)]
    pub declaring_type_token: Option<u32>,
    pub extends_token: Option<u32>,
    pub kind: String,
    pub interfaces: Vec<u32>,
    pub generic_parameters: Vec<GenericParameter>,
    #[serde(skip_serializing)]
    pub token: u32,
    pub name: String,
    pub namespace: String,
    pub full_name: Arc<String>,
    #[serde(rename = "access_flags")]
    pub flags: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Field {
    #[serde(skip_serializing)]
    pub token: u32,
    #[serde(skip_serializing)]
    pub declaring_type_token: u32,
    pub declaring_type: Arc<String>,
    pub name: String,
    pub signature: Arc<String>,
    /// `#Blob` index; the signature fields are filled by `Input::resolve_field`.
    #[serde(skip)]
    pub(crate) signature_index: u32,
    #[serde(skip)]
    pub(crate) resolved: bool,
    #[serde(rename = "access_flags")]
    pub flags: u16,
}

#[derive(Clone, Debug, Serialize)]
pub struct Method {
    pub impl_flags: u16,
    pub generic_parameters: Vec<GenericParameter>,
    #[serde(skip_serializing)]
    pub token: u32,
    pub declaring_type: Arc<String>,
    #[serde(skip_serializing)]
    pub declaring_type_token: u32,
    pub declaring_type_is_value_type: bool,
    pub name: String,
    pub signature: Arc<String>,
    pub parameters: Arc<Vec<String>>,
    #[serde(skip_serializing)]
    pub signature_flags: u8,
    pub return_type: Arc<String>,
    pub has_this: bool,
    /// `#Blob` index; the signature fields are filled by `Input::resolve_method`.
    #[serde(skip)]
    pub(crate) signature_index: u32,
    #[serde(skip)]
    pub(crate) resolved: bool,
    pub has_body: bool,
    pub rva: u32,
    #[serde(rename = "access_flags")]
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
    pub parameters: Arc<Vec<String>>,
    pub signature_flags: u8,
    pub return_type: Arc<String>,
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
    pub semantics: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Property {
    pub token: u32,
    #[serde(skip_serializing)]
    pub declaring_type_token: u32,
    pub name: String,
    pub signature: String,
    pub signature_hex: String,
    #[serde(rename = "access_flags")]
    pub flags: u32,
    pub accessors: Vec<Accessor>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub token: u32,
    #[serde(skip_serializing)]
    pub declaring_type_token: u32,
    pub name: String,
    pub event_type_token: u32,
    #[serde(rename = "access_flags")]
    pub flags: u32,
    pub accessors: Vec<Accessor>,
}
