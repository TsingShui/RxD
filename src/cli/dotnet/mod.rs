use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::PathBuf,
};

use clap::{Args, Subcommand};
use rxd_core::envelope::{self, RecordMeta};
use rxd_core::perf::Perf;
use rxd_core::{Error, Result};
use rxd_dotnet::{Input, Limits};
use serde_json::{Map, Value, json};

#[derive(Subcommand)]
pub enum Command {
    /// Enumerate type, field and method metadata as JSONL.
    Outline {
        input: PathBuf,
        #[arg(long = "type")]
        type_name: Option<String>,
        #[arg(long)]
        method: Option<String>,
        #[command(flatten)]
        options: Options,
    },
    /// Get one content-bound method as conservative code or original IL.
    #[command(name = "get-method")]
    Get {
        input: PathBuf,
        id: String,
        #[arg(long)]
        il: bool,
        #[command(flatten)]
        options: Options,
    },
    /// Fetch a selected type and every recursively owned nested type.
    #[command(name = "get-class")]
    GetClass {
        input: PathBuf,
        id: String,
        #[arg(long)]
        il: bool,
        #[command(flatten)]
        options: Options,
    },
    /// Query decoded user-string heap literals without executing input.
    Strings {
        input: PathBuf,
        #[arg(long)]
        contains: Option<String>,
        #[command(flatten)]
        options: Options,
    },
    /// Find exact/semantic member and type operands or decoded user-string loads.
    Refs {
        input: PathBuf,
        #[arg(long, value_parser = token, required_unless_present_any = ["id","string"], conflicts_with_all = ["id","string"])]
        token: Option<u32>,
        #[arg(long, required_unless_present_any = ["token","string"], conflicts_with_all = ["token","string"])]
        id: Option<String>,
        #[arg(long, required_unless_present_any = ["token","id"], conflicts_with_all = ["token","id"])]
        string: Option<String>,
        #[arg(long, requires = "token")]
        semantic: bool,
        #[command(flatten)]
        options: Options,
    },
}

#[derive(Args)]
pub struct Options {
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    #[arg(long)]
    perf: bool,
    #[arg(long)]
    debug: bool,
    /// Input byte cap in bytes (1 through 8589934592); 8 GiB when omitted.
    #[arg(long, value_parser = super::input_bytes)]
    max_input_bytes: Option<usize>,
    #[arg(long, default_value_t = 100_000, value_parser = super::positive)]
    max_entities: usize,
    #[arg(long, default_value_t = 16 * 1024 * 1024, value_parser = super::positive)]
    max_work: usize,
    #[arg(long, default_value_t = 100_000, value_parser = super::positive)]
    max_instructions: usize,
    #[arg(long, default_value_t = 100_000, value_parser = super::positive)]
    max_records: usize,
    #[arg(long, default_value_t = 32 * 1024 * 1024, value_parser = super::positive)]
    max_output_bytes: usize,
}

impl Options {
    fn limits(&self) -> Limits {
        Limits {
            input_bytes: super::input_limit(self.max_input_bytes),
            entities: self.max_entities,
            work: self.max_work,
            instructions: self.max_instructions,
            output_bytes: self.max_output_bytes,
            records: self.max_records,
        }
    }
}

impl Command {
    fn options(&self) -> &Options {
        match self {
            Self::Outline { options, .. }
            | Self::Get { options, .. }
            | Self::GetClass { options, .. }
            | Self::Strings { options, .. }
            | Self::Refs { options, .. } => options,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Outline { .. } => "outline",
            Self::Get { .. } => "get-method",
            Self::GetClass { .. } => "get-class",
            Self::Strings { .. } => "strings",
            Self::Refs { .. } => "refs",
        }
    }

    pub fn perf(&self) -> bool {
        self.options().perf
    }

    pub fn debug(&self) -> bool {
        self.options().debug
    }
}

pub fn execute(command: Command) -> Result<()> {
    let emit_perf = command.perf();
    let debug = command.debug();
    let mut perf = Perf::new(emit_perf || debug, "analysis", "dotnet", command.name());
    let result = execute_inner(command, &mut perf).map_err(|error| {
        let message = format!("{error:#}");
        let code = [
            "RESOURCE_LIMIT",
            "INVALID_QUERY",
            "INVALID_INPUT",
            "UNSUPPORTED_INPUT",
            "UNSUPPORTED_IL",
        ]
        .into_iter()
        .find(|code| message.contains(code))
        .unwrap_or(if message.contains("UNSUPPORTED:") {
            "UNSUPPORTED_INPUT"
        } else {
            "IO_ERROR"
        });
        Error::new(code, message)
    });
    super::finish(perf, emit_perf, debug, result)
}

fn execute_inner(command: Command, perf: &mut Perf) -> anyhow::Result<()> {
    let (path, options) = match &command {
        Command::Outline { input, options, .. }
        | Command::Get { input, options, .. }
        | Command::GetClass { input, options, .. }
        | Command::Strings { input, options, .. }
        | Command::Refs { input, options, .. } => (input, options),
    };
    let limits = options.limits();
    let input = perf.measure("read_parse", || Input::read(path, limits))?;
    perf.source_id = Some(input.source_id.clone());
    perf.counters.input_bytes = Some(input.file_bytes);
    perf.counters.entities = Some(input.types.len() + input.fields.len() + input.methods.len());
    let query_start = perf.start_stage();
    let mut output = Vec::new();
    let mut records = 0;
    let mut reference_result = if let Command::Refs {
        token,
        id,
        string,
        semantic,
        ..
    } = &command
    {
        anyhow::ensure!(
            !*semantic || token.is_some(),
            "INVALID_QUERY: --semantic requires --token"
        );
        anyhow::ensure!(
            usize::from(token.is_some())
                + usize::from(id.is_some())
                + usize::from(string.is_some())
                == 1,
            "INVALID_QUERY: exactly one reference selector is required"
        );
        if string.is_some() || (!*semantic && token.is_some_and(|token| token >> 24 == 0x70)) {
            None
        } else {
            let target = if let Some(id) = id {
                input.reference_token_by_id(id)?
            } else {
                token.unwrap()
            };
            Some(perf.measure("references", || {
                input.references(target, *semantic || id.is_some())
            })?)
        }
    } else {
        None
    };
    if let Command::Get { id, il, .. } = &command {
        let method = input.method_by_id(id)?;
        let body = perf.measure("decode", || input.body(method))?;
        let decompilation = if *il {
            rxd_dotnet::Decompilation {
                representation: "il".into(),
                decompiled: false,
                reason: None,
                code: None,
                mapping: Vec::new(),
            }
        } else {
            perf.measure("reconstruct", || {
                rxd_dotnet::render(&input, method, &body, limits)
            })?
        };
        let value = fetch_record(
            "method_body",
            command.name(),
            RecordMeta {
                id: Some(id),
                source_id: Some(&input.source_id),
                parent_id: Some(&method.declaring_type_id),
                start: Some(body.code_offset),
                end: Some(body.code_offset + body.code_size),
                ..RecordMeta::default()
            },
            detail(
                json!({"method":method_record(&input, method)?,"body_present":true,"representation":decompilation.representation,"decompiled":decompilation.decompiled,"reason":decompilation.reason,"code":decompilation.code,"mapping":decompilation.mapping,"file_offset":body.file_offset,"code_offset":body.code_offset,"code_size":body.code_size,"work_used":body.work_used,"max_stack":body.max_stack,"init_locals":body.init_locals,"locals":body.locals,"local_signature_token":body.local_signature_token,"instructions":body.instructions,"handlers":body.handlers,"il_hex":body.il_hex,"complete":true}),
            ),
        );
        append(&mut output, &value, limits.output_bytes)?;
    } else if let Command::GetClass { id, il, .. } = &command {
        let class = perf.measure("decode", || input.class_body(id))?;
        let rendered = if *il {
            rxd_dotnet::ClassDecompilation {
                representation: "il".into(),
                decompiled: false,
                reason: None,
                code: None,
                mapping: vec![],
            }
        } else {
            perf.measure("reconstruct", || {
                rxd_dotnet::render_class(&input, &class, limits)
            })?
        };
        let types = class
            .types
            .iter()
            .map(|ty| type_record(&input, ty))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let fields = class
            .fields
            .iter()
            .map(|field| field_record(&input, field))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let properties = class
            .properties
            .iter()
            .map(|property| {
                relationship_record(&input, "property", &property.declaring_type_id, *property)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let events = class
            .events
            .iter()
            .map(|event| relationship_record(&input, "event", &event.declaring_type_id, *event))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let methods = class
            .methods
            .iter()
            .map(|item| class_method_record(&input, item))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut payload = detail(
            json!({"type":type_record(&input, class.root)?,"work_used":class.work_used,"representation":rendered.representation,"decompiled":rendered.decompiled,"reason":rendered.reason,"code":rendered.code,"mapping":rendered.mapping,"complete":true}),
        );
        // The inventories are already bounded JSON records. Move them into the
        // aggregate rather than serializing and duplicating every original body.
        for (key, records) in [
            ("types", types),
            ("fields", fields),
            ("properties", properties),
            ("events", events),
            ("methods", methods),
        ] {
            payload.insert(key.into(), Value::Array(records));
        }
        let value = fetch_record(
            "class_body",
            command.name(),
            RecordMeta {
                id: Some(id),
                parent_id: class.root.declaring_type_id.as_deref(),
                ..metadata(&input)
            },
            payload,
        );
        append(&mut output, &value, limits.output_bytes)?;
    } else {
        if let Some(target) = reference_result
            .as_ref()
            .and_then(|result| result.target.as_deref())
        {
            descriptor_fits(target, limits.output_bytes.saturating_sub(output.len()))?;
        }
        let query = match &command {
            Command::Outline {
                type_name, method, ..
            } => json!({"type":type_name,"method":method}),
            Command::Strings { contains, .. } => json!({"contains":contains}),
            Command::Refs {
                token,
                id,
                string,
                semantic,
                ..
            } => json!({"token":token,"id":id,"string":string,"semantic":semantic}),
            Command::Get { .. } | Command::GetClass { .. } => unreachable!(),
        };
        append(
            &mut output,
            &envelope::header(
                "dotnet",
                command.name(),
                Some(&input.source_id),
                Some(input.file_bytes),
                query,
                if matches!(command, Command::Refs { .. }) {
                    detail(
                        json!({"target":reference_result.as_ref().and_then(|result|result.target.as_deref())}),
                    )
                } else {
                    Map::new()
                },
            ),
            limits.output_bytes,
        )?;
        match &command {
            Command::Outline {
                type_name, method, ..
            } => {
                let mut fields_by_type: BTreeMap<u32, Vec<_>> = BTreeMap::new();
                let mut methods_by_type: BTreeMap<u32, Vec<_>> = BTreeMap::new();
                if method.is_none() {
                    for field in &input.fields {
                        fields_by_type
                            .entry(field.declaring_type_token)
                            .or_default()
                            .push(field);
                    }
                }
                for item in &input.methods {
                    methods_by_type
                        .entry(item.declaring_type_token)
                        .or_default()
                        .push(item);
                }
                for ty in &input.types {
                    if type_name.as_ref().is_some_and(|name| *name != ty.full_name) {
                        continue;
                    }
                    record(
                        &mut output,
                        &mut records,
                        options,
                        &type_record(&input, ty)?,
                    )?;
                    if method.is_none() {
                        for field in fields_by_type.get(&ty.token).into_iter().flatten() {
                            record(
                                &mut output,
                                &mut records,
                                options,
                                &field_record(&input, field)?,
                            )?;
                        }
                    }
                    for item in methods_by_type.get(&ty.token).into_iter().flatten() {
                        if method.as_ref().is_some_and(|name| *name != item.name) {
                            continue;
                        }
                        let value = method_record(&input, item)?;
                        record(&mut output, &mut records, options, &value)?;
                    }
                }
            }
            Command::Strings { contains, .. } => {
                let uses = perf.measure("strings", || input.string_uses(contains.as_deref()))?;
                for group in &uses.groups {
                    let literal = &uses.literals[group.literal_index];
                    let method = group.method_token.map(|token| {
                        &input.methods[input
                            .methods
                            .binary_search_by_key(&token, |method| method.token)
                            .expect("checked string owner")]
                    });
                    record(
                        &mut output,
                        &mut records,
                        options,
                        &envelope::record(
                            "string",
                            RecordMeta {
                                parent_id: method.map(|method| method.declaring_type_id.as_str()),
                                context_id: method.map(|method| method.id.as_str()),
                                ..metadata(&input)
                            },
                            detail(
                                json!({"offset":literal.offset,"token":0x70000000u32 | literal.offset,"value":literal.value,"utf16_units":literal.utf16_units,"status":if method.is_some(){"referenced"}else{"unreferenced"},"method_token":group.method_token,"declaring_type_id":method.map(|method| &method.declaring_type_id),"il_offsets":group.il_offsets}),
                            ),
                        ),
                    )?;
                }
            }
            Command::Refs { token, string, .. } => {
                if let Some(result) = reference_result.take() {
                    for reference in result.references {
                        let value = reference_record(
                            &input,
                            &reference.site,
                            reference.target.as_deref(),
                            limits.output_bytes.saturating_sub(output.len()),
                        )?;
                        record(&mut output, &mut records, options, &value)?;
                    }
                } else {
                    let sites = if let Some(value) = string {
                        perf.measure("strings", || input.string_references(value))?
                    } else {
                        perf.measure("strings", || input.user_string_references(token.unwrap()))?
                    };
                    for site in sites {
                        let value = reference_record(
                            &input,
                            &site,
                            None,
                            limits.output_bytes.saturating_sub(output.len()),
                        )?;
                        record(&mut output, &mut records, options, &value)?;
                    }
                }
            }
            Command::Get { .. } | Command::GetClass { .. } => unreachable!(),
        }
        append(
            &mut output,
            &envelope::summary(records, true, Map::new()),
            limits.output_bytes,
        )?;
    }
    perf.record("query", query_start);
    perf.counters.records = Some(
        if matches!(command, Command::Get { .. } | Command::GetClass { .. }) {
            1
        } else {
            records
        },
    );
    perf.counters.output_bytes = Some(output.len());
    perf.measure("write", || {
        super::write_output(&output, options.output.as_deref())
    })?;
    Ok(())
}

// Single-record FETCH carries protocol identity without a separate header.
fn fetch_record(
    record: &str,
    command: &str,
    meta: RecordMeta<'_>,
    detail: Map<String, Value>,
) -> Value {
    let mut value = envelope::record(record, meta, detail);
    value["backend"] = json!("dotnet");
    value["command"] = json!(command);
    value["schema_version"] = json!(envelope::SCHEMA_VERSION);
    value
}

fn reference_record(
    input: &Input,
    site: &rxd_dotnet::TokenSite,
    target: Option<&rxd_dotnet::SemanticTarget>,
    remaining: usize,
) -> anyhow::Result<Value> {
    if let Some(target) = target {
        descriptor_fits(target, remaining)?;
    }
    let method = &input.methods[(site.method_token & 0xffffff) as usize - 1];
    let start = site.code_offset + site.il_offset as usize;
    Ok(envelope::record(
        "reference",
        RecordMeta {
            context_id: Some(&method.id),
            parent_id: Some(&method.declaring_type_id),
            start: Some(start),
            end: Some(start + site.size as usize),
            ..metadata(input)
        },
        detail(
            json!({"method_token":method.token,"declaring_type":method.declaring_type,"declaring_type_id":method.declaring_type_id,"name":method.name,"il_offset":site.il_offset,"opcode":site.mnemonic,"target_token":site.target_token,"target_kind":if let Some(target)=target{target.target_kind}else if site.target_token >> 24 == 0x70 {"string"}else{"token"},"canonical_target_id":target.and_then(|target|target.canonical_id.as_ref()),"canonical_target_token":target.and_then(|target|target.canonical_token),"resolution_status":target.map_or("exact",|target|target.resolution_status.as_str()),"scope":target.map(|target|&target.scope),"owner_token":target.map(|target|target.owner_token),"original_owner_token":target.map(|target|target.original_owner_token),"owner_name":target.map(|target|&target.owner_name),"target_name":target.map(|target|&target.name),"signature":target.and_then(|target|target.signature.as_ref()),"type_signature":target.and_then(|target|target.type_signature.as_ref()),"owner_arguments":target.map_or(&[][..],|target|target.owner_arguments.as_slice()),"method_arguments":target.map_or(&[][..],|target|target.method_arguments.as_slice())}),
        ),
    ))
}

// Count encoded descriptor bytes without constructing a serde_json::Value.
// The cap therefore applies before json! duplicates expanded signature trees.
fn descriptor_fits(value: &impl serde::Serialize, limit: usize) -> anyhow::Result<()> {
    struct Count {
        used: usize,
        limit: usize,
    }
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.used) {
                return Err(io::Error::other(
                    "RESOURCE_LIMIT: descriptor output byte budget exceeded",
                ));
            }
            self.used += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Count { used: 0, limit }, value).map_err(|error| {
        anyhow::anyhow!("RESOURCE_LIMIT: descriptor output byte budget exceeded: {error}")
    })
}

fn record(
    output: &mut Vec<u8>,
    records: &mut usize,
    options: &Options,
    value: &Value,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        *records < options.max_records,
        "RESOURCE_LIMIT: record budget exceeded"
    );
    append(output, value, options.max_output_bytes)?;
    *records += 1;
    Ok(())
}

fn append(output: &mut Vec<u8>, value: &Value, limit: usize) -> anyhow::Result<()> {
    let mut writer = BoundedOutput { output, limit };
    serde_json::to_writer(&mut writer, value)
        .map_err(|error| anyhow::anyhow!("RESOURCE_LIMIT: output byte budget exceeded: {error}"))?;
    writer.write_all(b"\n")?;
    Ok(())
}

struct BoundedOutput<'a> {
    output: &'a mut Vec<u8>,
    limit: usize,
}
impl Write for BoundedOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.output.len()) {
            return Err(io::Error::other(
                "RESOURCE_LIMIT: output byte budget exceeded",
            ));
        }
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn method_record(input: &Input, method: &rxd_dotnet::Method) -> anyhow::Result<Value> {
    let mut fields = detail(serde_json::to_value(method)?);
    fields.remove("id");
    Ok(envelope::record(
        "method",
        RecordMeta {
            id: Some(&method.id),
            parent_id: Some(&method.declaring_type_id),
            ..metadata(input)
        },
        fields,
    ))
}

fn type_record(input: &Input, ty: &rxd_dotnet::TypeInfo) -> anyhow::Result<Value> {
    let mut fields = detail(serde_json::to_value(ty)?);
    fields.remove("id");
    Ok(envelope::record(
        "type",
        RecordMeta {
            id: Some(&ty.id),
            parent_id: ty.declaring_type_id.as_deref(),
            ..metadata(input)
        },
        fields,
    ))
}

fn field_record(input: &Input, field: &rxd_dotnet::Field) -> anyhow::Result<Value> {
    let mut fields = detail(serde_json::to_value(field)?);
    fields.remove("id");
    Ok(envelope::record(
        "field",
        RecordMeta {
            id: Some(&field.id),
            parent_id: Some(&field.declaring_type_id),
            ..metadata(input)
        },
        fields,
    ))
}

fn class_method_record(input: &Input, item: &rxd_dotnet::ClassMethod<'_>) -> anyhow::Result<Value> {
    let body = item.body.as_ref();
    let mut fields = if let Some(body) = body {
        detail(serde_json::to_value(body)?)
    } else {
        detail(
            json!({"work_used":null,"file_offset":null,"code_offset":null,"code_size":null,"max_stack":null,"init_locals":null,"local_signature_token":null,"locals":null,"instructions":null,"handlers":null,"il_hex":null}),
        )
    };
    fields.insert("method".into(), method_record(input, item.method)?);
    fields.insert("body_present".into(), json!(body.is_some()));
    fields.insert("complete".into(), json!(true));
    fields.insert("representation".into(), json!("il"));
    fields.insert("decompiled".into(), json!(false));
    fields.insert("reason".into(), Value::Null);
    fields.insert("code".into(), Value::Null);
    fields.insert("mapping".into(), json!([]));
    Ok(envelope::record(
        "method_body",
        RecordMeta {
            id: Some(&item.method.id),
            parent_id: Some(&item.method.declaring_type_id),
            start: body.map(|body| body.code_offset),
            end: body.map(|body| body.code_offset + body.code_size),
            ..metadata(input)
        },
        fields,
    ))
}

fn relationship_record<T: serde::Serialize>(
    input: &Input,
    kind: &str,
    owner: &str,
    relationship: &T,
) -> anyhow::Result<Value> {
    Ok(envelope::record(
        kind,
        RecordMeta {
            parent_id: Some(owner),
            context_id: Some(owner),
            ..metadata(input)
        },
        detail(serde_json::to_value(relationship)?),
    ))
}

fn metadata(input: &Input) -> RecordMeta<'_> {
    RecordMeta {
        source_id: Some(&input.source_id),
        ..RecordMeta::default()
    }
}

fn detail(value: Value) -> Map<String, Value> {
    let Value::Object(fields) = value else {
        unreachable!(".NET record detail is a statically defined object")
    };
    fields
}

fn token(value: &str) -> std::result::Result<u32, String> {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u32::from_str_radix(value, 16).map_err(|_| "expected a hexadecimal metadata token".into())
}
