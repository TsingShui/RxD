use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Native producers emit these fields even when null. Keep requiredness separate
// from the nullable value: transparent Option schema derivation conflates them.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Nullable<T>(pub Option<T>);

impl<T: JsonSchema> JsonSchema for Nullable<T> {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("Nullable_{}", T::schema_name()).into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <Option<T>>::json_schema(generator)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectLocation {
    pub section_index: u64,
    pub section: String,
    pub offset: u64,
    pub offset_hex: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FunctionEntry {
    pub name: String,
    pub address: u64,
    pub address_hex: String,
    pub aliases: Vec<String>,
    pub object_location: Nullable<ObjectLocation>,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FunctionInventory {
    pub binary: String,
    pub count: u64,
    pub total: u64,
    pub error: Nullable<String>,
    pub functions: Vec<FunctionEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LineMapping {
    pub line_number: u64,
    pub addresses: Vec<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VariableKind {
    Arg,
    Stack,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub kind: VariableKind,
    pub arg_index: Nullable<u64>,
    pub stack_offset: Nullable<i64>,
    pub size: i64,
    pub line_numbers: Vec<u64>,
    pub addresses: Vec<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeDefinition {
    pub name: String,
    pub definition: String,
    pub size: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Interior {
    pub name: String,
    pub address: u64,
    pub address_hex: String,
    pub offset: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecompiledFunction {
    pub name: String,
    pub address: u64,
    pub address_hex: String,
    pub aliases: Vec<String>,
    pub object_location: Nullable<ObjectLocation>,
    pub size: u64,
    pub code: Nullable<String>,
    pub error: Nullable<String>,
    pub unstructured_gotos: u64,
    pub line_mappings: Vec<LineMapping>,
    pub variables: Vec<Variable>,
    pub types: Vec<TypeDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interior_to: Option<Interior>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Assertion {
    pub directive: String,
    pub kind: String,
    pub phase: String,
    pub subphase: String,
    pub status: String,
    pub detail: Nullable<String>,
    pub fatal: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Decompilation {
    pub binary: String,
    pub language: String,
    pub count: u64,
    pub error: Nullable<String>,
    pub functions: Vec<DecompiledFunction>,
    pub assertions: Vec<Assertion>,
}
