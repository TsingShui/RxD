//! Closed backend record schemas. Detail fields are enumerated by record kind;
//! recursive semantic signatures and typed maps have explicit value schemas.
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

fn object(fields: &[(&str, Value)], required: bool) -> Value {
    let properties = fields
        .iter()
        .map(|(name, schema)| ((*name).to_owned(), schema.clone()))
        .collect::<Map<_, _>>();
    let mut schema = json!({"type":"object","properties":properties,"additionalProperties":false});
    if required {
        schema["required"] = json!(fields.iter().map(|(name, _)| name).collect::<Vec<_>>());
    }
    schema
}
fn nullable(schema: Value) -> Value {
    json!({"anyOf":[schema,{"type":"null"}]})
}
fn reference(name: &str) -> Value {
    json!({"$ref":format!("#/$defs/{name}")})
}
fn array(schema: Value) -> Value {
    json!({"type":"array","items":schema})
}
fn fields(spec: &str) -> Value {
    let values = spec
        .split_whitespace()
        .map(|field| {
            let (name, kind) = field.split_once(':').unwrap();
            (name, kind_schema(kind))
        })
        .collect::<Vec<_>>();
    object(&values, false)
}
fn kind_schema(kind: &str) -> Value {
    if let Some(kind) = kind.strip_prefix('?') {
        return nullable(kind_schema(kind));
    }
    if let Some(kind) = kind.strip_prefix("[]") {
        return array(kind_schema(kind));
    }
    if let Some(kind) = kind.strip_prefix('@') {
        return reference(kind);
    }
    match kind {
        "s" => json!({"type":"string"}),
        "n" => json!({"type":"integer"}),
        "b" => json!({"type":"boolean"}),
        "i" => array(json!({"type":"integer"})),
        "a" => array(json!({"type":"string"})),
        _ => panic!("unknown schema kind {kind}"),
    }
}
fn body(kind: &str, detail: Value) -> Value {
    let mut schema = fields(
        "record:s at:?s source_id:?s parent_at:?s context_at:?s start:?n end:?n backend:s command:s schema_version:n",
    );
    schema["properties"]["record"] = json!({"const":kind});
    schema["properties"]["detail"] = detail;
    schema["required"] = json!([
        "record",
        "at",
        "source_id",
        "parent_at",
        "context_at",
        "start",
        "end",
        "detail"
    ]);
    schema
}
fn put(defs: &mut Map<String, Value>, name: &str, schema: Value) {
    defs.insert(name.into(), schema);
}
fn detail(defs: &mut Map<String, Value>, name: &str, kind: &str, spec: &str) {
    put(defs, name, body(kind, fields(spec)));
}
fn union(names: &[&str]) -> Value {
    json!({"anyOf":names.iter().map(|name|reference(name)).collect::<Vec<_>>()})
}
fn envelope(names: &[&str], header_detail: Value, query: Value, summary_detail: Value) -> Value {
    let mut header =
        fields("record:s backend:s command:s source_id:?s input_bytes:?n schema_version:n");
    header["properties"]["record"] = json!({"const":"header"});
    header["properties"]["detail"] = header_detail;
    header["properties"]["query"] = query;
    header["required"] = json!([
        "record",
        "backend",
        "command",
        "source_id",
        "input_bytes",
        "schema_version",
        "query",
        "detail"
    ]);
    let mut summary = fields("record:s records:n complete:b");
    summary["properties"]["record"] = json!({"const":"summary"});
    summary["properties"]["detail"] = summary_detail;
    summary["required"] = json!(["record", "records", "complete", "detail"]);
    object(
        &[
            ("header", header),
            ("records", array(union(names))),
            ("summary", summary),
        ],
        true,
    )
}

pub(super) fn output(name: &str) -> Value {
    let mut defs = Map::new();
    common(&mut defs);
    let mut success = if name.starts_with("js_") {
        js(name, &mut defs)
    } else if name.starts_with("jar_") {
        jvm(name, &mut defs)
    } else if name.starts_with("dotnet_") {
        dotnet(name, &mut defs)
    } else if name.starts_with("wasm_") {
        wasm(name, &mut defs)
    } else if name.starts_with("pyc_") {
        pyc(name, &mut defs)
    } else if name.starts_with("evm_") {
        let analysis = evm(&mut defs);
        if matches!(
            name,
            "evm_outline" | "evm_strings" | "evm_xref" | "evm_get_block" | "evm_get_function"
        ) {
            evm_navigation(name, &mut defs)
        } else {
            analysis
        }
    } else if name.starts_with("asar_") {
        asar(name, &mut defs)
    } else if name.starts_with("html_") {
        html(name, &mut defs)
    } else {
        dex(name, &mut defs)
    };
    let candidates = nullable(json!({"anyOf":[success.clone(),fields("definitions:a")]}));
    let error = object(
        &[(
            "error",
            object(
                &[
                    ("code", kind_schema("s")),
                    ("message", kind_schema("s")),
                    ("candidates", candidates),
                ],
                false,
            ),
        )],
        true,
    );
    // MCP requires an object root even when alternative result representations exist.
    if success.get("type").is_none() {
        success["type"] = json!("object");
    }
    let mut result = json!({"type":"object","anyOf":[success,error]});
    let mut needed = BTreeSet::new();
    collect_refs(&result, &mut needed);
    loop {
        let previous = needed.len();
        for key in needed.clone() {
            if let Some(value) = defs.get(&key) {
                collect_refs(value, &mut needed);
            }
        }
        if needed.len() == previous {
            break;
        }
    }
    result["$defs"] = Value::Object(
        defs.into_iter()
            .filter(|(key, _)| needed.contains(key))
            .collect(),
    );
    result
}
fn collect_refs(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(name) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|name| name.strip_prefix("#/$defs/"))
            {
                names.insert(name.into());
            }
            for value in map.values() {
                collect_refs(value, names);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_refs(value, names);
            }
        }
        _ => {}
    }
}
fn common(defs: &mut Map<String, Value>) {
    put(defs, "Span", fields("start:n end:n"));
    put(
        defs,
        "Origin",
        fields("container:s format:s entry:s header_id:?s"),
    );
    put(
        defs,
        "Text",
        object(
            &[
                ("representation", kind_schema("s")),
                ("source", kind_schema("s")),
                ("source_id", kind_schema("?s")),
                ("complete", kind_schema("b")),
            ],
            true,
        ),
    );
    put(
        defs,
        "TargetInfo",
        fields(
            "target_id:s source_id:s snapshot_id:s input_bytes:n captured_bytes:n kind:s inputs:[]@InputInfo origin:?@TargetOrigin",
        ),
    );
    put(
        defs,
        "InputInfo",
        fields("original_path:s source_id:s input_bytes:n relative_path:?s dependency:b"),
    );
    put(
        defs,
        "TargetOrigin",
        fields("container_source_id:s entry:s parent_target_id:?s"),
    );
}
fn js(name: &str, defs: &mut Map<String, Value>) -> Value {
    put(
        defs,
        "JsString",
        json!({"anyOf":[{"type":"string"},{"type":"object","properties":{"utf16_units":{"type":"array","items":{"type":"integer"}}},"required":["utf16_units"],"additionalProperties":false}]}),
    );
    put(defs, "JsFlags", fields("async:b generator:b static:b"));
    put(defs, "JsLocation", fields("start:n end:n entity_id:s"));
    put(
        defs,
        "JsSelected",
        fields(
            "name:@JsString role:s start:n end:n entity_id:s context_id:?s type_reference:?b value_reference:?b",
        ),
    );
    detail(
        defs,
        "JsEntity",
        "entity",
        "kind:s name:?@JsString label:?@JsString label_origin:s depth:n flags:@JsFlags key_span:?@Span",
    );
    detail(defs, "JsFragment", "fragment", "kind:s entity_id:s");
    detail(
        defs,
        "JsBinding",
        "binding",
        "name:@JsString space:?s declaration_ids:?a",
    );
    detail(
        defs,
        "JsMatch",
        "match",
        "literal_kind:s value:@JsString dynamic:b resolution:?s raw:?s",
    );
    detail(
        defs,
        "JsIdentifier",
        "identifier",
        "name:@JsString role:s syntax_kind:s",
    );
    detail(
        defs,
        "JsCall",
        "call",
        "callee:?@JsString call_kind:s optional:b argument_count:n",
    );
    detail(
        defs,
        "JsMember",
        "member",
        "name:@JsString member_kind:s key_resolution:?s",
    );
    detail(
        defs,
        "JsSymbol",
        "symbol",
        "name:@JsString relation:s space:?s declaration_ids:?a",
    );
    detail(
        defs,
        "JsUnresolved",
        "unresolved_reference",
        "name:s type_reference:?b value_reference:?b",
    );
    detail(
        defs,
        "JsReference",
        "reference",
        "read:b write:b member_write:b type_reference:?b value_reference:?b",
    );
    put(
        defs,
        "JsDeclaration",
        fields("kind:s start:n end:n variable_kind:?s initializer:?@Span"),
    );
    detail(
        defs,
        "JsSource",
        "source",
        "complete:b representation:s source:s entity:?@JsEntity symbol:?@JsBinding declaration:?@JsDeclaration fragment:?@JsFragment syntax:?s declaration_file:?b origin:?@Origin",
    );
    detail(
        defs,
        "JsLocate",
        "locate",
        "complete:b entity:@JsEntity ancestors:[]@JsEntity syntax:?s declaration_file:?b origin:?@Origin",
    );
    if name == "js_get" {
        return reference("JsSource");
    }
    if name == "js_locate" {
        return reference("JsLocate");
    }
    let records = match name {
        "js_outline" => vec!["JsEntity"],
        "js_find" => vec!["JsMatch"],
        "js_identifiers" => vec!["JsIdentifier"],
        "js_calls" => vec!["JsCall"],
        "js_members" => vec!["JsMember"],
        "js_symbols" => vec!["JsSymbol", "JsUnresolved"],
        _ => vec!["JsReference"],
    };
    let regular = envelope(
        &records,
        fields(
            "path:s root_id:s source_type:s syntax:?s declaration_file:?b origin:?@Origin analysis:?s resolution:s within_id:s selected:@JsSelected declaration:?@JsLocation declaration_ids:?a",
        ),
        fields(
            "within:?s depth:?n kind:?s name:?s role:?s string:s resolve_static:?b byte:n id:s json:b",
        ),
        fields(""),
    );
    if matches!(name, "js_outline" | "js_find") {
        let mut scan = fields("record:s schema_version:n backend:s command:s records:n complete:b");
        scan["properties"]["detail"] = fields("files:n");
        json!({"anyOf":[regular,object(&[("files",array(regular.clone())),("scan_summary",scan)],true)]})
    } else {
        regular
    }
}
fn asar(name: &str, defs: &mut Map<String, Value>) -> Value {
    detail(
        defs,
        "AsarFile",
        "file",
        "path:s bytes:n storage:s range_valid:b range_error:s link:s",
    );
    if name == "asar_extract" {
        let mut result = body(
            "file",
            fields("complete:b file_bytes:n path:?s storage:s representation:s origin:@Origin"),
        );
        result["properties"]["child_target"] = reference("TargetInfo");
        result["properties"]["bytes_base64"] = nullable(kind_schema("s"));
        return result;
    }
    envelope(
        &["AsarFile"],
        fields("analysis:s container:s header_id:s"),
        nullable(fields("")),
        fields("invalid_ranges:n"),
    )
}
fn html_fields(spec: &str) -> Value {
    let mut schema = fields(spec);
    schema["required"] = json!(
        spec.split_whitespace()
            .map(|field| field.split_once(':').unwrap().0)
            .collect::<Vec<_>>()
    );
    schema
}
fn html(name: &str, defs: &mut Map<String, Value>) -> Value {
    put(defs, "HtmlAttr", html_fields("name:s value:s"));
    put(
        defs,
        "HtmlElement",
        body(
            "element",
            html_fields(
                "tag:s attrs:[]@HtmlAttr self_closing:b void:b depth:n end_tag:s content_start:n content_end:n",
            ),
        ),
    );
    for (name, kind) in [
        ("HtmlText", "text"),
        ("HtmlComment", "comment"),
        ("HtmlDoctype", "doctype"),
    ] {
        put(defs, name, body(kind, html_fields("value:s depth:n")));
    }
    if name == "html_extract" {
        put(
            defs,
            "HtmlOrigin",
            html_fields("container:s format:s source_id:s node_id:s kind:s part:s start:n end:n"),
        );
        let mut result = body(
            "file",
            html_fields(
                "complete:b file_bytes:n path:?s representation:s part:s origin:@HtmlOrigin",
            ),
        );
        result["properties"]["child_target"] = reference("TargetInfo");
        result["properties"]["bytes_base64"] = nullable(kind_schema("s"));
        return result;
    }
    envelope(
        &["HtmlElement", "HtmlText", "HtmlComment", "HtmlDoctype"],
        html_fields("path:s analysis:s parser:s parser_version:s lexical_profile:s"),
        html_fields("kind:?s tag:?s attr:?s depth:?n within:?s"),
        html_fields(
            "nodes:n elements:n texts:n comments:n doctypes:n tokenizer_errors:n unmatched_end_tags:n missing_end_tags:n",
        ),
    )
}
fn dex(name: &str, defs: &mut Map<String, Value>) -> Value {
    detail(
        defs,
        "DexClass",
        "class",
        "entry:s descriptor:s name:s package:s simple_name:s",
    );
    detail(
        defs,
        "DexString",
        "string",
        "entry:s index:n value:s class:?s",
    );
    detail(
        defs,
        "DexReference",
        "reference",
        "entry:s class:?s member:s matched:s",
    );
    detail(
        defs,
        "DexEntry",
        "entry",
        "name:s compression:s compressed_size:n uncompressed_size:n",
    );
    detail(
        defs,
        "DexField",
        "field",
        "entry:s class:s name:s type:s access_flags:n static:b field_index:n slot:?n",
    );
    detail(
        defs,
        "DexMethod",
        "method",
        "entry:s class:s name:s access_flags:n method_index:n",
    );
    put(
        defs,
        "DexPlanField",
        fields("name:s type:s field_index:n slot:n access_flags:n static:b category:s"),
    );
    put(defs, "DexCounts", fields("instance:n static:n"));
    detail(
        defs,
        "DexPlan",
        "field_plan",
        "descriptor:s dex_class_def_idx:n dex_type_idx:n class_access_flags:n instance_fields:[]@DexPlanField static_fields:[]@DexPlanField counts:@DexCounts instance_ref_mask:s",
    );
    if matches!(name, "dex_manifest" | "dex_get_class") {
        return reference("Text");
    }
    if name == "dex_fields_plan" {
        return reference("DexPlan");
    }
    let records = match name {
        "dex_entries" => vec!["DexEntry"],
        "dex_strings" => vec!["DexString"],
        "dex_classes" => vec!["DexClass"],
        "dex_xref" => vec!["DexReference"],
        _ => vec!["DexField", "DexMethod"],
    };
    envelope(
        &records,
        fields(""),
        fields(
            "filter:?s offset:?n limit:?n kind:s value:s name:?s class:?s fuzzy_class:b descriptor:s method_index:?n field_index:?n",
        ),
        fields(""),
    )
}
fn jvm(name: &str, defs: &mut Map<String, Value>) -> Value {
    detail(
        defs,
        "JvmClass",
        "class",
        "entry:?s name:s super:?s interfaces:a major:n minor:n access:n signature:?s source_file:?s fields:n methods:n",
    );
    detail(
        defs,
        "JvmField",
        "field",
        "entry:?s class:s name:s descriptor:s access:n signature:?s constant_index:?n",
    );
    detail(
        defs,
        "JvmMethod",
        "method",
        "entry:?s class:s name:s descriptor:s access:n signature:?s exceptions:a has_code:b",
    );
    detail(
        defs,
        "JvmString",
        "string",
        "entry:?s class:s index:n value:s utf16:?i",
    );
    detail(
        defs,
        "JvmReference",
        "reference",
        "entry:?s class:s method:s method_descriptor:s byte_offset:n mnemonic:s target:s",
    );
    put(
        defs,
        "JvmHandler",
        fields("start:n end:n target:n catch_type:?s"),
    );
    put(
        defs,
        "JvmInstruction",
        fields(
            "byte_offset:n opcode:n mnemonic:s bytes:i operands:i targets:i constant_index:?n reference:?s",
        ),
    );
    detail(
        defs,
        "JvmBytecode",
        "bytecode",
        "representation:s entry:?s class:s name:s descriptor:s max_stack:n max_locals:n handlers:[]@JvmHandler instructions:[]@JvmInstruction complete:b",
    );
    if name == "jar_get_class" {
        return reference("Text");
    }
    if name == "jar_get_method" {
        return union(&["Text", "JvmBytecode"]);
    }
    let records = match name {
        "jar_outline" => vec!["JvmClass", "JvmField", "JvmMethod"],
        "jar_strings" => vec!["JvmString"],
        _ => vec!["JvmReference"],
    };
    envelope(
        &records,
        fields("analysis:s release:n"),
        fields("class:?s method:?s filter:?s kind:s name:s value:s"),
        fields("classes:n"),
    )
}
fn dotnet(name: &str, defs: &mut Map<String, Value>) -> Value {
    put(
        defs,
        "IlGenericParameter",
        fields("token:n number:n flags:n name:s constraints:i"),
    );
    detail(
        defs,
        "IlType",
        "type",
        "declaring_type_token:?n declaring_type_id:?s extends_token:?n kind:s interfaces:i generic_parameters:[]@IlGenericParameter token:n name:s namespace:s full_name:s flags:n",
    );
    detail(
        defs,
        "IlField",
        "field",
        "declaring_type_id:s token:n declaring_type_token:n declaring_type:s name:s signature:s flags:n",
    );
    detail(
        defs,
        "IlMethod",
        "method",
        "declaring_type_id:s impl_flags:n generic_parameters:[]@IlGenericParameter token:n declaring_type:s declaring_type_token:n declaring_type_is_value_type:b name:s signature:s parameters:a signature_flags:n return_type:s has_this:b has_body:b rva:n flags:n",
    );
    put(defs, "IlOperand", fields("kind:s value:n"));
    defs["IlOperand"]["properties"]["value"] =
        json!({"anyOf":[{"type":"integer"},{"type":"array","items":{"type":"integer"}}]});
    put(
        defs,
        "IlInstruction",
        fields("offset:n size:n opcode:n mnemonic:s operand:@IlOperand"),
    );
    put(
        defs,
        "IlHandler",
        fields(
            "kind:s try_offset:n try_length:n handler_offset:n handler_length:n class_token:?n filter_offset:?n",
        ),
    );
    put(
        defs,
        "IlMapping",
        fields("il_offset:n line:n method_id:s method_token:n"),
    );
    put(
        defs,
        "IlAccessor",
        fields("method_token:n method_id:s semantics:n"),
    );
    detail(
        defs,
        "IlProperty",
        "property",
        "token:n declaring_type_token:n declaring_type_id:s name:s signature:s signature_hex:s flags:n accessors:[]@IlAccessor",
    );
    detail(
        defs,
        "IlEvent",
        "event",
        "token:n declaring_type_token:n declaring_type_id:s name:s event_type_token:n flags:n accessors:[]@IlAccessor",
    );
    put(
        defs,
        "IlScope",
        fields(
            "kind:s name:s version:i culture:s flags:n key:i parent_token:n parent_scope:@IlScope token:n",
        ),
    );
    put(
        defs,
        "IlTypeSignature",
        fields(
            "kind:s element:n token:n value:@IlTypeSignature index:n rank:n sizes:i bounds:i arguments:[]@IlTypeSignature constructor_arguments:[]@IlTypeSignature required:b signature:@IlMethodSignature",
        ),
    );
    put(
        defs,
        "IlMethodSignature",
        fields(
            "flags:n generic_arity:n sentinel:?n return_type:@IlTypeSignature parameters:[]@IlTypeSignature",
        ),
    );
    put(
        defs,
        "IlTarget",
        fields(
            "target_kind:s original_token:n canonical_token:?n canonical_id:?s resolution_status:s scope:@IlScope owner_token:n original_owner_token:n owner_name:s name:s signature:?@IlMethodSignature type_signature:?@IlTypeSignature owner_arguments:[]@IlTypeSignature method_arguments:[]@IlTypeSignature",
        ),
    );
    detail(
        defs,
        "IlReference",
        "reference",
        "method_token:n declaring_type:s declaring_type_id:s name:s il_offset:n opcode:s target_token:n target_kind:s canonical_target_id:?s canonical_target_token:?n resolution_status:s scope:?@IlScope owner_token:?n original_owner_token:?n owner_name:?s target_name:?s signature:?@IlMethodSignature type_signature:?@IlTypeSignature owner_arguments:[]@IlTypeSignature method_arguments:[]@IlTypeSignature",
    );
    detail(
        defs,
        "IlString",
        "string",
        "offset:n token:n value:?s utf16_units:?i status:s method_token:?n declaring_type_id:?s il_offsets:i",
    );
    detail(
        defs,
        "IlBody",
        "method_body",
        "method:@IlMethod body_present:b representation:s decompiled:b reason:?s code:?s mapping:[]@IlMapping file_offset:?n code_offset:?n code_size:?n work_used:?n max_stack:?n init_locals:?b locals:?a local_signature_token:?n instructions:?[]@IlInstruction handlers:?[]@IlHandler il_hex:?s complete:b",
    );
    detail(
        defs,
        "IlClassBody",
        "class_body",
        "type:@IlType types:[]@IlType fields:[]@IlField properties:[]@IlProperty events:[]@IlEvent methods:[]@IlBody work_used:n representation:s decompiled:b reason:?s code:?s mapping:[]@IlMapping complete:b",
    );
    if name == "dotnet_get_method" {
        return reference("IlBody");
    }
    if name == "dotnet_get_class" {
        return reference("IlClassBody");
    }
    let records = match name {
        "dotnet_outline" => vec!["IlType", "IlField", "IlMethod"],
        "dotnet_strings" => vec!["IlString"],
        _ => vec!["IlReference"],
    };
    envelope(
        &records,
        fields("target:?@IlTarget"),
        fields("type:?s method:?s contains:?s token:?n id:?s string:?s semantic:b"),
        fields(""),
    )
}
fn wasm(name: &str, defs: &mut Map<String, Value>) -> Value {
    detail(defs, "WasmType", "type", "type_index:n params:a results:a");
    detail(
        defs,
        "WasmImport",
        "import",
        "module:s name:s kind:s func_index:?n type_index:?n",
    );
    detail(defs, "WasmExport", "export", "name:s kind:s index:n");
    detail(
        defs,
        "WasmFunction",
        "function",
        "func_index:n type_index:n name:?s params:a results:a body_file_offset:n body_size:n",
    );
    detail(
        defs,
        "WasmTable",
        "table",
        "index:n element_type:s initial:n maximum:?n",
    );
    detail(
        defs,
        "WasmMemory",
        "memory",
        "index:n initial:n maximum:?n memory64:b shared:b",
    );
    detail(
        defs,
        "WasmGlobal",
        "global",
        "index:n value_type:s mutable:b name:?s",
    );
    detail(
        defs,
        "WasmData",
        "data",
        "index:n kind:s memory_index:?n offset_expr:?s byte_length:n",
    );
    detail(defs, "WasmElement", "element", "index:n kind:s count:n");
    detail(
        defs,
        "WasmString",
        "string",
        "origin:s segment_index:?n offset:n value:?s bytes:?i",
    );
    detail(
        defs,
        "WasmReference",
        "reference",
        "caller_index:n caller_name:?s offset:n opcode:s target_index:n",
    );
    put(defs, "WasmMemArg", fields("align:n offset:n memory:n"));
    put(
        defs,
        "WasmOperand",
        fields(
            "kind:s value:n bits:n index:n memarg:@WasmMemArg depth:n targets:i default:n block_type:s type_index:n table_index:n",
        ),
    );
    put(
        defs,
        "WasmInstruction",
        fields("offset:n opcode:s operand:@WasmOperand"),
    );
    put(defs, "WasmLocal", fields("count:n value_type:s"));
    put(
        defs,
        "WasmFunc",
        fields("func_index:n type_index:n name:?s params:a results:a"),
    );
    detail(
        defs,
        "WasmView",
        "function_view",
        "representation:s decompiled:b reason:?s code:?s func:@WasmFunc body_file_offset:n body_size:n locals:[]@WasmLocal instructions:[]@WasmInstruction mapping:[]i bytecode_hex:s complete:b",
    );
    if name == "wasm_get_function" {
        return reference("WasmView");
    }
    let records = match name {
        "wasm_outline" => vec![
            "WasmType",
            "WasmImport",
            "WasmExport",
            "WasmFunction",
            "WasmTable",
            "WasmMemory",
            "WasmGlobal",
            "WasmData",
            "WasmElement",
        ],
        "wasm_strings" => vec!["WasmString"],
        _ => vec!["WasmReference"],
    };
    envelope(
        &records,
        fields("analysis:s imported_funcs:n total_funcs:n"),
        fields("func_name:?s func:?n contains:?s"),
        fields(""),
    )
}
fn pyc(name: &str, defs: &mut Map<String, Value>) -> Value {
    put(
        defs,
        "PycField",
        fields(
            "start:n end:n definition_start:?n definition_end:?n payload_start:?n payload_end:?n reference:b tag:?n",
        ),
    );
    put(
        defs,
        "PycIncoming",
        fields(
            "owner_id:s constant_path:i start:?n end:?n alias_start:?n alias_end:?n reference:b",
        ),
    );
    let mut metadata = fields(
        "index:n name:?s qualname:?s filename:?s name_bytes:?s qualname_bytes:?s filename_bytes:?s kind:s arg_count:n posonly_arg_count:n kwonly_arg_count:n n_locals:n stack_size:n flags:n first_line:n body_status:s bytecode_bytes:?n consts_count:n names_count:n varnames_count:n freevars_count:n cellvars_count:n localsplus_count:?n line_table_bytes:n exception_table_bytes:n incoming:[]@PycIncoming incoming_complete:b ownership:s",
    );
    metadata["properties"]["fields"] =
        json!({"type":"object","additionalProperties":reference("PycField")});
    put(defs, "PycMetadata", metadata.clone());
    put(defs, "PycCode", body("code", metadata));
    put(defs, "PycAlias", fields("start:n end:n constant_path:i"));
    detail(
        defs,
        "PycString",
        "string",
        "code_index:n constant_slot:n constant_path:i kind:s value:?s raw_bytes:s definition_start:n definition_end:n payload_start:n payload_end:n occurrence_start:?n occurrence_end:?n reference:b aliases:[]@PycAlias",
    );
    put(defs, "PycTarget", fields("table:s index:n id:s value_id:s"));
    detail(
        defs,
        "PycReference",
        "reference",
        "code_index:n offset:n opcode:n name:s raw_operand:?n operand:?n target:@PycTarget operation:s constant_id:?s constant_slot:?n constant_path:?i literal_kind:?s relation:?s",
    );
    put(
        defs,
        "PycUnsupported",
        fields("code_index:n context_id:s offset:n start:n end:n opcode:n name:s status:s"),
    );
    put(
        defs,
        "PycPrefix",
        fields("offset:n opcode:n raw_operand:n raw_hex:s"),
    );
    put(
        defs,
        "PycUnit",
        fields(
            "offset:n size:n start:n end:n opcode:n name:s raw_operand:?n operand:?n raw_hex:s role:s owner_offset:?n logical_start:?n logical_size:?n prefixes:[]@PycPrefix jump_target:?n targets:[]@PycTarget status:s line:?n",
        ),
    );
    put(
        defs,
        "PycEntry",
        fields(
            "index:n id:s value_id:s start:?n end:?n alias_start:?n alias_end:?n reference:b kind:?n",
        ),
    );
    put(
        defs,
        "PycTable",
        fields(
            "present:b start:?n end:?n definition_start:?n definition_end:?n entries:[]@PycEntry",
        ),
    );
    put(
        defs,
        "PycValueChild",
        fields("index:n value_id:s start:n end:n reference:b"),
    );
    put(
        defs,
        "PycValue",
        fields(
            "id:s tag:n kind:s value:?s raw_hex:?s definition_start:n definition_end:n payload_start:?n payload_end:?n children:[]@PycValueChild code_id:?s",
        ),
    );
    put(
        defs,
        "PycLocation",
        fields(
            "start:n end:n line:?n end_line:?n column:?n end_column:?n table_start:n table_end:n",
        ),
    );
    put(
        defs,
        "PycException",
        fields("start:n end:n target:n depth:n lasti:b table_start:n table_end:n"),
    );
    let mut view_detail = fields(
        "input_bytes:n python_version:s representation:s complete:b query:@PycQuery metadata:@PycMetadata bytecode_hex:s instructions:[]@PycUnit values:[]@PycValue line_table_hex:s line_mapping:[]@PycLocation exception_table_hex:s exceptions:[]@PycException",
    );
    view_detail["properties"]["tables"] =
        json!({"type":"object","additionalProperties":reference("PycTable")});
    put(defs, "PycView", body("code_view", view_detail));
    put(defs, "PycQuery", fields("code_id:s version:?s"));
    detail(
        defs,
        "PycSource",
        "source",
        "input_bytes:n python_version:s analysis:s representation:s complete:b code:s source:s diagnostics:a warnings:a",
    );
    if name == "pyc_get_code" {
        return reference("PycView");
    }
    if name == "pyc_decompile" {
        return reference("PycSource");
    }
    let records = match name {
        "pyc_outline" => vec!["PycCode"],
        "pyc_strings" => vec!["PycString"],
        _ => vec!["PycReference"],
    };
    envelope(
        &records,
        fields(
            "analysis:s python_version:s code_count:n instruction_validation:s containers:a scope:s",
        ),
        fields("version:?s name:?s index:?n contains:?s contains_hex:?s const_id:?s within:?s"),
        fields(
            "code_count:n analysis:s instruction_validation:s scanned_code_count:n instructions:n unsupported_count:n unsupported:[]@PycUnsupported",
        ),
    )
}
fn evm(defs: &mut Map<String, Value>) -> Value {
    put(defs, "EvmEngine", fields("name:s version:s"));
    put(defs, "EvmCarrier", fields("path:s encoding:s bytes:n"));
    put(defs, "EvmBytecode", fields("sha256:s bytes:n hex:?s"));
    put(defs, "EvmTail", fields("start:n bytes:n hex:s"));
    put(
        defs,
        "EvmFunction",
        fields("selector:s bytecodeOffset:n dispatch:s arguments:?s stateMutability:?s"),
    );
    put(
        defs,
        "EvmStorage",
        fields("slot:s offset:n type:s reads:a writes:a"),
    );
    put(defs, "EvmDynamic", fields("path:i to:?n"));
    put(
        defs,
        "EvmBlockData",
        fields("success:b to:n true_to:n false_to:n"),
    );
    defs["EvmBlockData"]["properties"]["to"] =
        json!({"anyOf":[{"type":"integer"},array(reference("EvmDynamic"))]});
    defs["EvmBlockData"]["properties"]["true_to"] =
        json!({"anyOf":[{"type":"integer"},array(reference("EvmDynamic"))]});
    put(
        defs,
        "EvmBlock",
        fields("id:n start:n end:n type:s data:@EvmBlockData"),
    );
    put(defs, "EvmCfg", fields("blocks:[]@EvmBlock"));
    put(defs, "EvmCborValue", fields("type:s"));
    defs["EvmCborValue"]["properties"]["value"] = json!({"anyOf":[{"type":"string"},{"type":"integer"},{"type":"boolean"},array(kind_schema("n"))]});
    put(defs, "EvmCborEntry", fields("key:s value:@EvmCborValue"));
    put(
        defs,
        "EvmMetadata",
        fields("bytecodeOffset:n cborLength:n entries:[]@EvmCborEntry"),
    );
    put(
        defs,
        "EvmContract",
        fields(
            "functions:?[]@EvmFunction storage:?[]@EvmStorage transientStorage:?[]@EvmStorage basicBlocks:?[]i controlFlowGraph:?@EvmCfg metadata:@EvmMetadata",
        ),
    );
    defs["EvmContract"]["properties"]["disassembled"] = nullable(array(
        json!({"type":"array","prefixItems":[kind_schema("n"),kind_schema("s")],"minItems":2,"maxItems":2}),
    ));
    body(
        "evm_analysis",
        fields(
            "engine:@EvmEngine carrier:@EvmCarrier bytecode:@EvmBytecode offset_unit:s cfg_destination_unit:s basic_block_end:s disassembly_complete:?b undecoded_tail:?@EvmTail abi_verified:b discovery_complete:?b input_role:s hardfork:?s result:@EvmContract complete:b",
        ),
    )
}

fn required_fields(spec: &str) -> Value {
    let mut schema = fields(spec);
    schema["required"] = json!(
        schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>()
    );
    schema
}
fn required_detail(defs: &mut Map<String, Value>, name: &str, kind: &str, spec: &str) {
    put(defs, name, body(kind, required_fields(spec)));
}
fn evm_navigation(name: &str, defs: &mut Map<String, Value>) -> Value {
    // Fields follow the backend's fixed serde models and record writers in
    // navigation/functions/literals/data/references, including nullable coverage.
    put(
        defs,
        "EvmInstruction",
        required_fields("decoded_offset:n decoded_end:n opcode:n operand_hex:s text:s"),
    );
    put(
        defs,
        "EvmNavTail",
        required_fields("decoded_start:n decoded_end:n carrier_start:n carrier_end:n hex:s"),
    );
    put(
        defs,
        "EvmRange",
        required_fields("decoded_start:n decoded_end:n"),
    );
    put(
        defs,
        "EvmMatch",
        required_fields("decoded_start:n decoded_end:n carrier_start:n carrier_end:n"),
    );
    put(
        defs,
        "EvmText",
        required_fields("decoded_start:n decoded_end:n text:s"),
    );
    put(defs, "EvmAbstract", required_fields("kind:s hex:?s"));
    put(
        defs,
        "EvmMemory",
        required_fields("offset:n value_hex:s write_pc:n"),
    );
    put(
        defs,
        "EvmState",
        required_fields("stack:[]@EvmAbstract memory_words:[]@EvmMemory"),
    );
    put(
        defs,
        "EvmEvidence",
        required_fields("decoded_offset:n kind:s destination_hex:?s reason:s"),
    );
    put(
        defs,
        "EvmEdge",
        required_fields("instruction_offset:n kind:s target:?n target_context:?n resolution:s"),
    );
    put(
        defs,
        "EvmContext",
        required_fields(
            "context:n block_id:s entry_state:@EvmState exit_state:@EvmState edges:[]@EvmEdge problems:[]@EvmEvidence",
        ),
    );
    put(
        defs,
        "EvmSegment",
        required_fields("decoded_start:n decoded_end:n carrier_start:n carrier_end:n raw_hex:?s"),
    );
    put(
        defs,
        "EvmUnresolved",
        required_fields(
            "decoded_start:n decoded_end:n carrier_start:n carrier_end:n raw_hex:s reason:s",
        ),
    );
    put(
        defs,
        "EvmReconstruction",
        required_fields(
            "engine:@EvmEngine language:s status:s reason:?s failure_offset:?n complete:b text:?s control_flow_status:s structuring_reason:?s reconstructed_instruction_count:n reconstructed_block_count:n reconstructed_context_count:n value_count:n work:n structured_context_count:n structured_edge_count:n structuring_work:n structured_loop_count:n structured_backedge_count:n",
        ),
    );
    put(
        defs,
        "EvmUnknownCopy",
        required_fields(
            "instruction_offset:n block_id:s context_id:?s context_index:?n context_entry_state:@EvmState entry_evidence_block_ids:a source_offset_hex:?s length_hex:?s destination_offset_hex:?s reason:s",
        ),
    );
    let block = "unit_kind:s decoded_start:n decoded_end:n last_opcode_start:n instruction_count:n upstream_block_id:?n boundary_status:s owner_ids:?a shared:?b";
    required_detail(defs, "EvmNavBlock", "basic_block", block);
    required_detail(
        defs,
        "EvmFetchedBlock",
        "block",
        &format!(
            "{block} carrier:@EvmCarrier bytecode:@EvmBytecode raw_hex:s instructions:[]@EvmInstruction disassembly_complete:b undecoded_tail:?@EvmNavTail complete:b"
        ),
    );
    let function = "unit_kind:s selector:s dispatch:s decoded_entry:n entry_evidence:s entry_state_status:s entry_states:[]@EvmState included_block_ids:a segments:[]@EvmSegment blocks:[]@EvmNavBlock instruction_count:n context_count:n boundary_status:s boundary_complete:b ownership_status:s cross_entries:i unknown_successors:[]@EvmEvidence discovery_complete:?b";
    required_detail(defs, "EvmNavFunction", "function", function);
    required_detail(
        defs,
        "EvmFunctionCode",
        "function_code",
        &format!(
            "{function} carrier:@EvmCarrier bytecode:@EvmBytecode instructions:[]@EvmInstruction unresolved_evidence:[]@EvmUnresolved control_flow:[]@EvmContext reconstruction:@EvmReconstruction complete:b"
        ),
    );
    let literal = "literal_id:s literal_kind:s instruction_offset:n instruction_end:n opcode:n width_bytes:n decoded_start:n decoded_end:n bytes_hex:s number_hex:s block_id:s owner_count:n context_kind:s context_boundary_status:s context_boundary_complete:?b runtime_string_proven:b";
    required_detail(
        defs,
        "EvmLiteral",
        "literal",
        &format!(
            "{literal} query_kind:s evidence_kind:s match_status:s text_candidates:[]@EvmText matches:[]@EvmMatch"
        ),
    );
    required_detail(
        defs,
        "EvmLiteralReference",
        "literal_reference",
        &format!(
            "{literal} evidence_kind:s reference_kind:s instruction_carrier_start:n instruction_carrier_end:n"
        ),
    );
    required_detail(
        defs,
        "EvmDataLiteral",
        "data_literal",
        "literal_id:s literal_kind:s region_kind:s decoded_start:n decoded_end:n bytes_hex:s number_hex:?s query_kind:s evidence_kind:s match_status:s text_candidates:[]@EvmText matches:[]@EvmMatch reference_status:s reference_count:n context_kind:?s context_index:?n runtime_string_proven:b",
    );
    required_detail(
        defs,
        "EvmDataReference",
        "data_reference",
        "literal_id:s reference_kind:s evidence_kind:s instruction_offset:n instruction_end:n instruction_carrier_start:n instruction_carrier_end:n source_offset_hex:?s length_hex:?s destination_offset_hex:?s available_source_start:n available_source_end:n zero_padding_hex:s copied_decoded_start:n copied_decoded_end:n copied_bytes_hex:s block_id:s context_kind:s context_index:?n context_entry_state:@EvmState entry_evidence_block_ids:a resolution:s runtime_string_proven:b",
    );
    required_detail(
        defs,
        "EvmCodeReference",
        "code_reference",
        "target_id:s target_kind:s target_decoded_offset:n reference_kind:s resolution:s instruction_offset:n opcode:n operand_hex:s context_kind:s referencing_block_id:s referencing_context_index:?n target_context_index:?n incoming_stack_words:?n evidence_kind:s owner_ids:a shared:b ownership_status:s source_call_verified:b abi_call_verified:b",
    );
    match name {
        "evm_get_block" => reference("EvmFetchedBlock"),
        "evm_get_function" => reference("EvmFunctionCode"),
        _ => envelope(
            if name == "evm_outline" {
                &["EvmNavBlock", "EvmNavFunction"]
            } else if name == "evm_strings" {
                &["EvmLiteral", "EvmDataLiteral"]
            } else {
                &[
                    "EvmLiteralReference",
                    "EvmDataReference",
                    "EvmCodeReference",
                ]
            },
            required_fields(
                "carrier:?@EvmCarrier bytecode:?@EvmBytecode offset_unit:?s basic_block_end:?s boundary_status:?s target_kind:?s target_decoded_offset:?n target_semantics:?s coverage:?s literal_scope:?s excluded_unindexed_pushes:?n discovery_complete:?b disassembly_complete:?b undecoded_tail:?@EvmNavTail source_call_graph_complete:?b abi_verified:?b data_scope:?s metadata_range:?@EvmRange copy_witnesses:?n unknown_copy_witnesses:?n unvisited_copy_sites:?n data_reference_coverage:?s unknown_copy_evidence:?[]@EvmUnknownCopy",
            ),
            required_fields(if name == "evm_outline" {
                "encoding:s"
            } else if name == "evm_strings" {
                "encoding:s kind:s query:?s match_mode:s"
            } else {
                "encoding:s id:s reference_scope:s"
            }),
            required_fields(
                "blocks:?n functions:?n target_kind:?s target_decoded_offset:?n references:?n literals:?n data_literals:?n reference_status:?s excluded_unindexed_pushes:?n resolved_edges:?n unknown_jumps:?n invalid_jumps:?n analysis_problems:?n scanned_contexts:?n scanned_blocks:?n external_call_sites:?n resolution_complete:?b discovery_complete:?b disassembly_complete:?b source_call_graph_complete:?b",
            ),
        ),
    }
}
