use super::{CapturedInput, QueryBudgets, QueryError, schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::ffi::OsString;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub backend: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub capabilities: Vec<String>,
}

struct Spec {
    name: &'static str,
    fields: &'static str,
    capability: &'static str,
    description: &'static str,
}
const SPECS: &[Spec] = &[
    #[cfg(feature = "backend-js")]
    Spec {
        name: "js_outline",
        fields: "within:s depth:n kind:s name:s source_type:mode syntax:syntax",
        capability: "ENUMERATE",
        description: "Enumerate JS/TS program, functions, classes and methods; optional exact kind (comma-separated list allowed) and name filters; directories recursively preserve per-file envelopes.",
    },
    #[cfg(feature = "backend-js")]
    Spec {
        name: "js_find",
        fields: "string:s! resolve_static:b source_type:mode syntax:syntax",
        capability: "SEARCH",
        description: "Find case-sensitive decoded literal substrings; with resolve_static, additionally match bounded AST constant evaluation results.",
    },
    #[cfg(feature = "backend-js")]
    Spec {
        name: "js_identifiers",
        fields: "name:s! role:identrole within:s source_type:mode syntax:syntax",
        capability: "SEARCH",
        description: "Find exact lexical identifier occurrences (binding or reference); computed properties are lexical, static member names are not.",
    },
    #[cfg(feature = "backend-js")]
    Spec {
        name: "js_calls",
        fields: "name:s! within:s source_type:mode syntax:syntax",
        capability: "XREF",
        description: "Find call sites of a direct, member or new callee name; never a return value, call graph or cross-file resolution.",
    },
    #[cfg(feature = "backend-js")]
    Spec {
        name: "js_members",
        fields: "name:s! within:s source_type:mode syntax:syntax",
        capability: "SEARCH",
        description: "Find exact static member names, property keys and JSX attribute names, plus computed keys with a literal or constant-folded key; dynamic computed access is excluded.",
    },
    #[cfg(feature = "backend-js")]
    Spec {
        name: "js_locate",
        fields: "byte:n! source_type:mode syntax:syntax",
        capability: "FETCH",
        description: "Locate the innermost syntax structure covering an original UTF-8 byte offset.",
    },
    #[cfg(feature = "backend-js")]
    Spec {
        name: "js_refs",
        fields: "byte:n! source_type:mode syntax:syntax",
        capability: "XREF",
        description: "Find lexical references selected by original UTF-8 offset; unresolved/dynamic names are not guessed.",
    },
    #[cfg(feature = "backend-js")]
    Spec {
        name: "js_symbols",
        fields: "within:s source_type:mode syntax:syntax",
        capability: "ENUMERATE",
        description: "Enumerate lexical bindings and unresolved occurrences in a selected content-bound structure.",
    },
    #[cfg(feature = "backend-js")]
    Spec {
        name: "js_get",
        fields: "id:s! source_type:mode syntax:syntax",
        capability: "FETCH",
        description: "Fetch unchanged original source by content-bound structure, symbol, declaration or fragment ID.",
    },
    #[cfg(feature = "backend-asar")]
    Spec {
        name: "asar_list",
        fields: "",
        capability: "ENUMERATE",
        description: "Enumerate ASAR indexed paths and ranges. Linked entries are unsupported; unpacked dependencies use captured bytes.",
    },
    #[cfg(feature = "backend-asar")]
    Spec {
        name: "asar_extract",
        fields: "file:s! include_bytes:b",
        capability: "FETCH",
        description: "Extract an exact ASAR entry into a content-bound child target with original bytes and origin.",
    },
    #[cfg(feature = "backend-html")]
    Spec {
        name: "html_list",
        fields: "kind:htmlkind tag:s attr:s within:s depth:n",
        capability: "ENUMERATE",
        description: "Enumerate HTML element, text, comment and doctype records with exact filters and content-bound IDs. lexical_naive_v1 is lexical, not a DOM/browser-complete parse; no scripts execute or external resources load.",
    },
    #[cfg(feature = "backend-html")]
    Spec {
        name: "html_extract",
        fields: "id:s! content:b include_bytes:b",
        capability: "FETCH",
        description: "Extract an exact HTML record into a content-bound child target with original bytes and origin.",
    },
    #[cfg(feature = "backend-dex")]
    Spec {
        name: "dex_get_class",
        fields: "class:s! members:b",
        capability: "FETCH",
        description: "Reconstruct a DEX class as Java; optional original member table is retained. DEX lacks content-bound entity IDs.",
    },
    #[cfg(feature = "backend-dex")]
    Spec {
        name: "dex_entries",
        fields: "",
        capability: "ENUMERATE",
        description: "Enumerate APK central-directory entries in original order.",
    },
    #[cfg(feature = "backend-dex")]
    Spec {
        name: "dex_strings",
        fields: "filter:s limit:n offset:n",
        capability: "SEARCH",
        description: "Enumerate root-DEX strings with existing case-insensitive filter, offset and explicit limit semantics.",
    },
    #[cfg(feature = "backend-dex")]
    Spec {
        name: "dex_findrefs",
        fields: "kind:ref! value:s name:s class:s fuzzy_class:b",
        capability: "XREF",
        description: "Find explicit string/type/method/field references; descriptors, DEX entries and member selectors preserve existing semantics.",
    },
    #[cfg(feature = "backend-dex")]
    Spec {
        name: "dex_manifest",
        fields: "",
        capability: "FETCH",
        description: "Decode captured AndroidManifest.xml to unchanged emitted XML without loading the APK.",
    },
    #[cfg(feature = "backend-dex")]
    Spec {
        name: "dex_classes",
        fields: "filter:s",
        capability: "ENUMERATE",
        description: "Enumerate DEX classes with optional case-insensitive name substring. DEX identity navigation uses descriptors and entries.",
    },
    #[cfg(feature = "backend-dex")]
    Spec {
        name: "dex_fields_plan",
        fields: "descriptor:s! inputs:inputs",
        capability: "FETCH",
        description: "Resolve one DEX class field layout across ordered supplied inputs; not-found and ambiguous definitions remain distinct.",
    },
    #[cfg(feature = "backend-dex")]
    Spec {
        name: "dex_member_by_index",
        fields: "descriptor:s! method_index:n field_index:n inputs:inputs",
        capability: "FETCH",
        description: "Resolve a runtime method_ids index or instance-then-static field slot; ambiguous results retain all candidates.",
    },
    #[cfg(feature = "backend-jvm")]
    Spec {
        name: "jar_outline",
        fields: "class:s method:s release:n",
        capability: "ENUMERATE",
        description: "Enumerate class/JAR classes, fields and methods; release selects multi-release entries without Java/D8.",
    },
    #[cfg(feature = "backend-jvm")]
    Spec {
        name: "jar_strings",
        fields: "class:s filter:s release:n",
        capability: "SEARCH",
        description: "Find constant-pool literals with existing case-insensitive filter and optional exact class scope.",
    },
    #[cfg(feature = "backend-jvm")]
    Spec {
        name: "jar_findrefs",
        fields: "kind:ref! value:s name:s class:s release:n",
        capability: "XREF",
        description: "Find explicit JVM instruction references to string/type/method/field; preserve bytecode offsets and content-bound method context.",
    },
    #[cfg(feature = "backend-jvm")]
    Spec {
        name: "jar_get_method",
        fields: "id:s! bytecode:b release:n",
        capability: "FETCH",
        description: "Fetch one content-bound JVM method as Java, falling back to complete original instructions; bytecode forces original view.",
    },
    #[cfg(feature = "backend-jvm")]
    Spec {
        name: "jar_get_class",
        fields: "class:s! release:n",
        capability: "FETCH",
        description: "Reconstruct the exact internal/dotted/descriptor class as Java.",
    },
    #[cfg(feature = "backend-dotnet")]
    Spec {
        name: "dotnet_outline",
        fields: "type:s method:s",
        capability: "ENUMERATE",
        description: "Enumerate managed type/field/method metadata with content-bound IDs.",
    },
    #[cfg(feature = "backend-dotnet")]
    Spec {
        name: "dotnet_strings",
        fields: "contains:s",
        capability: "SEARCH",
        description: "Find decoded #US literals and every method use, retaining unreferenced values and lossless UTF-16 units.",
    },
    #[cfg(feature = "backend-dotnet")]
    Spec {
        name: "dotnet_refs",
        fields: "token:n id:s string:s semantic:b",
        capability: "XREF",
        description: "Find explicit IL operands by exactly one token/entity-ID/string selector; semantic matching requires provable metadata identity.",
    },
    #[cfg(feature = "backend-dotnet")]
    Spec {
        name: "dotnet_get_method",
        fields: "id:s! il:b",
        capability: "FETCH",
        description: "Fetch one content-bound managed method; optional ILSpy C# engine lazily reconstructs or falls back to complete IL/EH with reason.",
    },
    #[cfg(feature = "backend-dotnet")]
    Spec {
        name: "dotnet_get_class",
        fields: "id:s! il:b",
        capability: "FETCH",
        description: "Fetch selected type and nested members; optional ILSpy C# engine lazily reconstructs or returns complete original IL with reason.",
    },
    #[cfg(feature = "backend-wasm")]
    Spec {
        name: "wasm_outline",
        fields: "func_name:s func:n",
        capability: "ENUMERATE",
        description: "Enumerate validated WASM module structure and content-bound function IDs without instantiating the module.",
    },
    #[cfg(feature = "backend-wasm")]
    Spec {
        name: "wasm_strings",
        fields: "contains:s",
        capability: "SEARCH",
        description: "Find decodable UTF-8 data/name-section strings.",
    },
    #[cfg(feature = "backend-wasm")]
    Spec {
        name: "wasm_refs",
        fields: "func:n!",
        capability: "XREF",
        description: "Find explicit references to a merged function index; indirect calls remain explicitly indirect.",
    },
    #[cfg(feature = "backend-wasm")]
    Spec {
        name: "wasm_get_function",
        fields: "id:s! raw:b",
        capability: "FETCH",
        description: "Fetch content-bound function as structured C-like code; unsupported reconstruction falls back to complete original instructions.",
    },
    #[cfg(feature = "backend-pyc")]
    Spec {
        name: "pyc_outline",
        fields: "version:s name:s index:n",
        capability: "ENUMERATE",
        description: "Enumerate PYC code-object metadata for final CPython 3.8–3.13 without decoding instructions.",
    },
    #[cfg(feature = "backend-pyc")]
    Spec {
        name: "pyc_strings",
        fields: "version:s contains:s contains_hex:s",
        capability: "SEARCH",
        description: "Enumerate lossless str/bytes constant occurrences with code-owner IDs and original marshal ranges.",
    },
    #[cfg(feature = "backend-pyc")]
    Spec {
        name: "pyc_refs",
        fields: "version:s name:s const_id:s within:s",
        capability: "XREF",
        description: "Find explicit bytecode names or constant-occurrence references; unsupported instructions retain explicit coverage evidence.",
    },
    #[cfg(feature = "backend-pyc")]
    Spec {
        name: "pyc_get_code",
        fields: "code_id:s! version:s",
        capability: "FETCH",
        description: "Fetch content-bound code-object raw instructions, constant/name tables and original location evidence.",
    },
    #[cfg(feature = "backend-pyc")]
    Spec {
        name: "pyc_decompile",
        fields: "version:s",
        capability: "FETCH",
        description: "Run existing bounded embedded pycdc reconstruction; source completeness is independent from static navigation and may be false.",
    },
    #[cfg(feature = "backend-evm")]
    Spec {
        name: "evm_outline",
        fields: "encoding:encoding! include_bytecode:b",
        capability: "ENUMERATE",
        description: "Enumerate content-bound EVM function and basic-block candidates; boundaries, ownership and unresolved edges stay explicit.",
    },
    #[cfg(feature = "backend-evm")]
    Spec {
        name: "evm_strings",
        fields: "encoding:encoding! include_bytecode:b kind:literal! query:s",
        capability: "SEARCH",
        description: "Search original decoded PUSH and data candidates as text, exact bytes or U256 number; evidence and carrier offsets are retained.",
    },
    #[cfg(feature = "backend-evm")]
    Spec {
        name: "evm_refs",
        fields: "encoding:encoding! include_bytecode:b id:s!",
        capability: "XREF",
        description: "Find original literal-use, CODECOPY or static control-flow references by a returned content-bound ID; unknown resolution stays explicit.",
    },
    #[cfg(feature = "backend-evm")]
    Spec {
        name: "evm_get_block",
        fields: "encoding:encoding! include_bytecode:b id:s!",
        capability: "FETCH",
        description: "Fetch one returned EVM block ID with exact original instructions, carrier ranges and raw bytes.",
    },
    #[cfg(feature = "backend-evm")]
    Spec {
        name: "evm_get_function",
        fields: "encoding:encoding! include_bytecode:b id:s selector:s raw:b",
        capability: "FETCH",
        description: "Fetch one returned function ID or exact selector; preserve raw evidence and bounded native reconstruction/structuring status, without claiming original Solidity or arbitrary equivalence.",
    },
    #[cfg(feature = "backend-evm")]
    Spec {
        name: "evm_analyze",
        fields: "encoding:encoding! include_bytecode:b",
        capability: "ENUMERATE",
        description: "Run all native EVMole analyses; inferred ABI/storage/CFG are not verified runtime facts. No content-bound entity IDs, SEARCH/XREF/FETCH closure.",
    },
    #[cfg(feature = "backend-evm")]
    Spec {
        name: "evm_interface",
        fields: "encoding:encoding! include_bytecode:b",
        capability: "ENUMERATE",
        description: "Infer EVM function selectors, argument types and mutability; ABI verification and discovery completeness remain explicit.",
    },
    #[cfg(feature = "backend-evm")]
    Spec {
        name: "evm_storage",
        fields: "encoding:encoding! include_bytecode:b",
        capability: "ENUMERATE",
        description: "Infer persistent/transient EVM storage retaining candidate functions; inference remains explicit.",
    },
    #[cfg(feature = "backend-evm")]
    Spec {
        name: "evm_disassemble",
        fields: "encoding:encoding! include_bytecode:b",
        capability: "ENUMERATE",
        description: "Decode EVM offsets/opcodes and retain explicit undecoded-tail evidence.",
    },
    #[cfg(feature = "backend-evm")]
    Spec {
        name: "evm_basic_blocks",
        fields: "encoding:encoding! include_bytecode:b",
        capability: "ENUMERATE",
        description: "Enumerate native EVM basic blocks; end denotes the last opcode start.",
    },
    #[cfg(feature = "backend-evm")]
    Spec {
        name: "evm_cfg",
        fields: "encoding:encoding! include_bytecode:b",
        capability: "ENUMERATE",
        description: "Infer native EVM control-flow graph; destinations are upstream block identities.",
    },
    #[cfg(feature = "backend-evm")]
    Spec {
        name: "evm_metadata",
        fields: "encoding:encoding! include_bytecode:b",
        capability: "ENUMERATE",
        description: "Extract terminal CBOR compiler metadata from explicitly encoded EVM bytecode.",
    },
];

fn spec(name: &str) -> Result<&'static Spec, QueryError> {
    SPECS
        .iter()
        .find(|spec| spec.name == name)
        .ok_or_else(|| QueryError::new("INVALID_QUERY", format!("unknown analysis tool: {name}")))
}

pub fn catalog() -> Vec<ToolDefinition> {
    let result: Vec<ToolDefinition> = SPECS
        .iter()
        .map(|spec| ToolDefinition {
            name: spec.name.into(),
            backend: spec.name.split_once('_').unwrap().0.into(),
            description: spec.description.into(),
            input_schema: input_schema(spec),
            output_schema: schema::output(spec.name),
            capabilities: vec![spec.capability.into()],
        })
        .collect();
    #[cfg(feature = "bundled-kuna")]
    let result = {
        let mut result = result;
        result.extend(super::kuna::catalog());
        result
    };
    result
}

fn input_reference() -> Value {
    json!({"oneOf":[{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"],"additionalProperties":false},
        {"type":"object","properties":{"target_id":{"type":"string","minLength":1}},"required":["target_id"],"additionalProperties":false}]})
}

fn input_schema(spec: &Spec) -> Value {
    let mut properties = Map::new();
    properties.insert("input".into(), input_reference());
    let multi = matches!(spec.name, "dex_fields_plan" | "dex_member_by_index");
    let mut required = if multi { Vec::new() } else { vec!["input"] };
    let mut limit_names = vec!["input_bytes", "output_bytes", "records"];
    let backend = spec.name.split_once('_').unwrap().0;
    match backend {
        "js" => {
            limit_names.extend(["entities", "ast_depth"]);
            if matches!(spec.name, "js_refs" | "js_symbols" | "js_get") {
                limit_names.push("semantic_nodes");
            }
        }
        "asar" => limit_names.push("entities"),
        "html" => limit_names.push("ast_depth"),
        "jar" => limit_names.extend(["entities", "work"]),
        "dotnet" | "wasm" => limit_names.extend(["entities", "work", "instructions"]),
        "pyc" => {
            limit_names.push("work");
            if spec.name == "pyc_decompile" {
                limit_names.push("ast_depth");
            } else {
                limit_names.push("entities");
            }
            if matches!(spec.name, "pyc_refs" | "pyc_get_code") {
                limit_names.push("instructions");
            }
        }
        "evm"
            if matches!(
                spec.name,
                "evm_outline" | "evm_strings" | "evm_refs" | "evm_get_block" | "evm_get_function"
            ) =>
        {
            limit_names.push("work");
            if spec.name != "evm_get_block" {
                limit_names.extend(["states", "stack_cells"]);
            }
            if spec.name == "evm_get_function" {
                limit_names.extend(["reconstruction_work", "values"]);
            }
        }
        "dex" | "evm" => {}
        _ => unreachable!(),
    }
    let limits = limit_names
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                json!({"type":"integer","minimum":1,"maximum":u32::MAX}),
            )
        })
        .collect::<Map<_, _>>();
    properties.insert(
        "limits".into(),
        json!({"type":"object","properties":limits,"additionalProperties":false}),
    );
    for field in spec.fields.split_whitespace() {
        let (name, kind) = field.split_once(':').unwrap();
        let kind = kind.trim_end_matches('!');
        let value = match kind {
            "s" => json!({"type":"string"}),
            "n" => json!({"type":"integer","minimum":0,"maximum":u32::MAX}),
            "b" => json!({"type":"boolean"}),
            "mode" => json!({"type":"string","enum":["auto","script","module"]}),
            "syntax" => json!({"type":"string","enum":["auto","js","jsx","ts","tsx"]}),
            "identrole" => json!({"type":"string","enum":["all","declaration","reference"]}),
            "htmlkind" => json!({"type":"string","enum":["element","text","comment","doctype"]}),
            "encoding" => json!({"type":"string","enum":["raw","hex"]}),
            "literal" => json!({"type":"string","enum":["text","bytes","number"]}),
            "ref" => json!({"type":"string","enum":["string","type","method","field"]}),
            "inputs" => json!({"type":"array","minItems":1,"items":input_reference()}),
            _ => unreachable!(),
        };
        properties.insert(name.into(), value);
        if field.ends_with('!') {
            required.push(name);
        }
    }
    let mut schema = json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
    let exclusive = match spec.name {
        "evm_get_function" => Some(vec!["id", "selector"]),
        "dotnet_refs" => Some(vec!["token", "id", "string"]),
        "dex_member_by_index" => Some(vec!["method_index", "field_index"]),
        "pyc_refs" => Some(vec!["name", "const_id"]),
        _ => None,
    };
    if let Some(selectors) = exclusive {
        schema["allOf"] = json!([{"oneOf":selectors.into_iter().map(|selector|json!({"required":[selector]})).collect::<Vec<_>>()}]);
    }
    if multi {
        let choice = json!({"oneOf":[{"required":["input"]},{"required":["inputs"]}]});
        if let Some(rules) = schema["allOf"].as_array_mut() {
            rules.push(choice);
        } else {
            schema["allOf"] = json!([choice]);
        }
    }
    if spec.name == "pyc_strings" {
        schema["not"] = json!({"required":["contains","contains_hex"]});
    }
    if spec.name == "pyc_outline" {
        schema["not"] = json!({"required":["name","index"]});
    }
    schema
}

pub fn validate(name: &str, arguments: &Value) -> Result<(), QueryError> {
    #[cfg(feature = "bundled-kuna")]
    if name.starts_with("kuna_") {
        return super::kuna::validate(name, arguments);
    }
    let schema = input_schema(spec(name)?);
    validate_schema(arguments, &schema, "arguments")?;
    if matches!(name, "dex_fields_plan" | "dex_member_by_index")
        && (arguments.get("input").is_some() == arguments.get("inputs").is_some())
    {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "exactly one input or inputs selector is required",
        ));
    }
    if name == "dex_member_by_index"
        && (arguments.get("method_index").is_some() == arguments.get("field_index").is_some())
    {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "exactly one method_index or field_index is required",
        ));
    }
    if name == "evm_get_function"
        && (arguments.get("id").is_some() == arguments.get("selector").is_some())
    {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "exactly one EVM id or selector is required",
        ));
    }
    if name == "dotnet_refs" {
        if ["token", "id", "string"]
            .iter()
            .filter(|key| arguments.get(**key).is_some())
            .count()
            != 1
        {
            return Err(QueryError::new(
                "INVALID_QUERY",
                "exactly one token, id or string selector is required",
            ));
        }
        if arguments["semantic"] == true && arguments.get("token").is_none() {
            return Err(QueryError::new("INVALID_QUERY", "semantic requires token"));
        }
    }
    if name == "pyc_refs"
        && (arguments.get("name").is_some() == arguments.get("const_id").is_some())
    {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "exactly one name or const_id selector is required",
        ));
    }
    if name == "pyc_strings"
        && arguments.get("contains").is_some()
        && arguments.get("contains_hex").is_some()
    {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "contains and contains_hex are mutually exclusive",
        ));
    }
    if name == "pyc_outline" && arguments.get("name").is_some() && arguments.get("index").is_some()
    {
        return Err(QueryError::new(
            "INVALID_QUERY",
            "name and index are mutually exclusive",
        ));
    }
    if matches!(name, "dex_findrefs" | "jar_findrefs") {
        let kind = arguments["kind"].as_str().unwrap();
        if matches!(kind, "string" | "type")
            && arguments
                .get(if name.starts_with("jar") && kind == "type" {
                    "name"
                } else {
                    "value"
                })
                .is_none()
        {
            return Err(QueryError::new(
                "INVALID_QUERY",
                "string/type references require their value/name selector",
            ));
        }
        if matches!(kind, "method" | "field")
            && arguments.get("name").is_none()
            && (name.starts_with("jar") || arguments.get("class").is_none())
        {
            return Err(QueryError::new(
                "INVALID_QUERY",
                "member references require a member name or DEX class selector",
            ));
        }
    }
    Ok(())
}

fn validate_schema(value: &Value, schema: &Value, path: &str) -> Result<(), QueryError> {
    let invalid = || {
        QueryError::new(
            "INVALID_QUERY",
            format!("{path} does not match its tool schema"),
        )
    };
    if let Some(branches) = schema["oneOf"].as_array() {
        if branches
            .iter()
            .filter(|branch| validate_schema(value, branch, path).is_ok())
            .count()
            != 1
        {
            return Err(invalid());
        }
        return Ok(());
    }
    match schema["type"].as_str().unwrap() {
        "object" => {
            let object = value.as_object().ok_or_else(invalid)?;
            if let Some(required) = schema["required"].as_array() {
                for key in required {
                    if !object.contains_key(key.as_str().unwrap()) {
                        return Err(invalid());
                    }
                }
            }
            for (key, value) in object {
                let sub = &schema["properties"][key];
                if sub.is_null() {
                    return Err(QueryError::new(
                        "INVALID_QUERY",
                        format!("unknown {path} field: {key}"),
                    ));
                }
                validate_schema(value, sub, &format!("{path}.{key}"))?;
            }
        }
        "array" => {
            let values = value.as_array().ok_or_else(invalid)?;
            if schema["minItems"]
                .as_u64()
                .is_some_and(|minimum| values.len() < (minimum as usize))
            {
                return Err(invalid());
            }
            for value in values {
                validate_schema(value, &schema["items"], path)?;
            }
        }
        "string" => {
            let text = value.as_str().ok_or_else(invalid)?;
            if text.contains('\0') {
                return Err(QueryError::new(
                    "INVALID_QUERY",
                    format!("{path} contains NUL not supported by this backend's selector"),
                ));
            }
            if schema["minLength"]
                .as_u64()
                .is_some_and(|minimum| text.len() < (minimum as usize))
            {
                return Err(invalid());
            }
            if schema["enum"]
                .as_array()
                .is_some_and(|choices| !choices.contains(value))
            {
                return Err(invalid());
            }
        }
        "integer" => {
            let number = value.as_u64().ok_or_else(invalid)?;
            if schema["minimum"].as_u64().is_some_and(|min| number < min)
                || schema["maximum"].as_u64().is_some_and(|max| number > max)
            {
                return Err(invalid());
            }
        }
        "boolean" => {
            if !value.is_boolean() {
                return Err(invalid());
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

pub(super) fn argv(
    name: &str,
    arguments: &Value,
    inputs: &[CapturedInput],
    b: QueryBudgets,
) -> Result<Vec<OsString>, QueryError> {
    let spec = spec(name)?;
    let (backend, command) = name.split_once('_').unwrap();
    let command = command.replace('_', "-");
    let mut argv: Vec<OsString> = vec!["rxd".into(), backend.into(), command.into()];
    if matches!(name, "dex_fields_plan" | "dex_member_by_index") {
        for input in inputs {
            argv.push("--apk".into());
            argv.push(input.path.as_os_str().into());
        }
    } else {
        argv.push(inputs[0].path.as_os_str().into());
    }
    let mut positionals: Vec<OsString> = Vec::new();
    if matches!(
        name,
        "jar_get_method"
            | "dotnet_get_method"
            | "dotnet_get_class"
            | "wasm_get_function"
            | "evm_get_block"
    ) {
        positionals.push(arguments["id"].as_str().unwrap().into());
    }
    if name == "evm_get_function"
        && let Some(id) = arguments.get("id")
    {
        positionals.push(id.as_str().unwrap().into());
    }
    if name == "pyc_get_code" {
        positionals.push(arguments["code_id"].as_str().unwrap().into());
    }
    if matches!(name, "dex_get_class" | "jar_get_class") {
        positionals.push(arguments["class"].as_str().unwrap().into());
    }
    let refs = matches!(name, "dex_findrefs" | "jar_findrefs");

    for field in spec.fields.split_whitespace() {
        let (key, kind) = field.split_once(':').unwrap();
        if key == "inputs"
            || (key == "kind" && refs)
            || (key == "id" && name != "js_get" && name != "dotnet_refs" && name != "evm_refs")
            || key == "code_id"
            || (key == "class" && matches!(name, "dex_get_class" | "jar_get_class"))
        {
            continue;
        }
        let Some(value) = arguments.get(key) else {
            continue;
        };
        if refs && matches!(key, "value" | "name" | "class" | "fuzzy_class") {
            continue;
        }
        let flag = key.replace('_', "-");
        if kind == "b" {
            if value == true {
                argv.push(format!("--{flag}").into());
            }
        } else if key == "token" {
            argv.push(format!("--token=0x{:x}", value.as_u64().unwrap()).into());
        } else {
            argv.push(
                format!(
                    "--{flag}={}",
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                )
                .into(),
            );
        }
    }
    let mut bound = |key: &str, value: usize| argv.push(format!("--{key}={value}").into());
    if backend == "dex" {
        if !matches!(name, "dex_manifest" | "dex_entries") {
            bound("threads", b.threads);
        }
    } else {
        bound("max-input-bytes", b.input_bytes);
        bound("max-output-bytes", b.output_bytes);
        if backend == "jar" {
            bound("max-class-bytes", b.input_bytes.min(16 * 1024 * 1024));
            bound("max-expanded-bytes", b.input_bytes);
            bound("max-entries", b.entities);
            bound("max-work", b.work);
        } else if backend == "js" {
            bound("max-entities", b.entities);
            bound("max-records", b.records);
            bound("max-ast-depth", b.ast_depth);
            if matches!(name, "js_outline" | "js_find") {
                bound("threads", b.threads);
            }
            if matches!(name, "js_refs" | "js_symbols" | "js_get") {
                bound("max-semantic-nodes", b.semantic_nodes);
            }
        } else if backend == "html" {
            bound("max-records", b.records);
            bound("max-depth", b.ast_depth);
        } else if backend == "asar" {
            bound("max-records", b.records);
            bound("max-entries", b.entities);
            bound("max-header-bytes", b.input_bytes.min(8 * 1024 * 1024));
        } else if backend == "evm" {
            bound("max-records", b.records);
            if matches!(
                name,
                "evm_outline" | "evm_strings" | "evm_refs" | "evm_get_block" | "evm_get_function"
            ) {
                bound("max-work", b.work);
                bound("max-states", b.states);
                bound("max-stack-cells", b.stack_cells);
                bound("max-reconstruction-work", b.reconstruction_work);
                bound("max-values", b.values);
            }
        } else if name == "pyc_decompile" {
            bound("max-work", b.work);
            bound("max-ast-depth", b.ast_depth.min(128));
        } else {
            bound("max-entities", b.entities);
            bound("max-records", b.records);
            bound("max-work", b.work);
            if backend != "pyc" || matches!(name, "pyc_refs" | "pyc_get_code") {
                bound("max-instructions", b.instructions);
            }
        }
    }
    if refs {
        let kind = arguments["kind"].as_str().unwrap();
        argv.push(kind.into());
        let key = if kind == "string" || (backend == "dex" && kind == "type") {
            "value"
        } else {
            "name"
        };
        if let Some(value) = arguments.get(key) {
            positionals.push(value.as_str().unwrap().into());
        }
        if let Some(class) = arguments.get("class") {
            argv.push(format!("--class={}", class.as_str().unwrap()).into());
        }
        if arguments["fuzzy_class"] == true {
            argv.push("--fuzzy-class".into());
        }
    }
    if matches!(name, "js_get" | "pyc_decompile") {
        argv.push("--json".into());
    }
    if !positionals.is_empty() {
        // Selectors are data, even when they resemble a registered CLI flag.
        argv.push("--".into());
        argv.extend(positionals);
    }
    Ok(argv)
}
