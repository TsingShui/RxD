use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::{Path, PathBuf},
};

use super::filter::CaseArgs;
use super::xref::Selector;
use clap::{Args, Subcommand};
use rxd_core::envelope::{self, RecordMeta};
use rxd_core::perf::Perf;
use rxd_core::{Budget, Error, Result};
use rxd_dotnet::{Input, Limits};
use serde_json::{Map, Value, json};

#[derive(Subcommand)]
pub enum Command {
    /// List the types an assembly defines as JSONL.
    Classes {
        input: PathBuf,
        /// Exact full type name.
        #[arg(long)]
        class: Option<String>,
        ///  substring over the full type name. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// List the fields an assembly defines as JSONL.
    Fields {
        input: PathBuf,
        /// Exact full type name.
        #[arg(long)]
        class: Option<String>,
        /// Exact field name; not a substring.
        #[arg(long)]
        name: Option<String>,
        ///  substring over the field name. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// List the methods an assembly defines as JSONL.
    Methods {
        input: PathBuf,
        /// Exact full type name.
        #[arg(long)]
        class: Option<String>,
        /// Exact method name; not a substring.
        #[arg(long)]
        name: Option<String>,
        ///  substring over the method name. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Get one content-bound method as conservative code or original instructions.
    #[command(name = "get-method")]
    Get {
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        /// Print the original instruction listing instead of reconstructed source.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        options: Options,
    },
    /// Fetch a selected type and every recursively owned nested type.
    #[command(name = "get-class")]
    GetClass {
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        /// Print the original instruction listing instead of reconstructed source.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        options: Options,
    },
    /// Query decoded user-string heap literals without executing input.
    Strings {
        input: PathBuf,
        ///  substring over the decoded literal. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Find exact/semantic member and type operands or decoded user-string loads.
    Xref {
        input: PathBuf,
        /// Exact target: the metadata token of a member or type, such as `0x0a00001f`; it
        /// also covers external members, generic instantiations and `#US` string tokens
        /// (`0x70…`).
        #[arg(long, value_name = "TOKEN", conflicts_with_all = ["filter", "from"])]
        at: Option<String>,
        /// Substring of a decoded user-string literal; folded unless `--case sensitive`.
        #[arg(short = 'f', long, value_name = "PATTERN")]
        filter: Option<String>,
        /// Target kind: read off the token's table when it names one; required with
        /// `--filter`, which can only search literals.
        #[arg(long, value_enum)]
        kind: Option<Kind>,
        /// Whether `-f` folds case: `insensitive` (default) or `sensitive`.
        #[arg(long, value_enum, default_value_t = crate::cli::filter::CaseMode::Insensitive)]
        case: crate::cli::filter::CaseMode,
        /// Read the target from a previous JSONL stream (a file, or `-` for stdin);
        /// `method`, `field` and `type` records are accepted.
        #[arg(long, value_name = "JSONL", conflicts_with_all = ["at", "filter"])]
        from: Option<PathBuf>,

        #[command(flatten)]
        options: Options,
    },
}

/// Which kind of target a reference query names.
///
/// The kind is read off a token's table, so it is only needed where the token cannot say:
/// a `--filter` content search, and asserting a table that covers two kinds (`MemberRef`).
type Kind = crate::cli::xref::Kind;

#[derive(Args)]
pub struct Options {
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    #[arg(long)]
    perf: bool,
}

impl Options {
    /// The shared budget table, projected onto this backend's limits.
    fn limits(&self) -> Limits {
        let budget = Budget::default();
        Limits {
            input_bytes: budget.input_bytes,
            entities: budget.entities,
            work: budget.work,
            instructions: budget.instructions,
            output_bytes: budget.output_bytes,
            records: budget.records,
        }
    }

    fn max_output_bytes(&self) -> usize {
        Budget::default().output_bytes
    }

    fn max_records(&self) -> usize {
        Budget::default().records
    }
}

impl Command {
    fn options(&self) -> &Options {
        match self {
            Self::Classes { options, .. }
            | Self::Fields { options, .. }
            | Self::Methods { options, .. }
            | Self::Get { options, .. }
            | Self::GetClass { options, .. }
            | Self::Strings { options, .. }
            | Self::Xref { options, .. } => options,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Classes { .. } => "classes",
            Self::Fields { .. } => "fields",
            Self::Methods { .. } => "methods",
            Self::Get { .. } => "get-method",
            Self::GetClass { .. } => "get-class",
            Self::Strings { .. } => "strings",
            Self::Xref { .. } => "xref",
        }
    }

    pub fn perf(&self) -> bool {
        self.options().perf
    }
}

pub fn execute(command: Command) -> Result<()> {
    let emit_perf = command.perf();
    let mut perf = Perf::new(emit_perf, "analysis", "dotnet", command.name());
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
        Error::coded(code, message)
    });
    super::finish(perf, emit_perf, result)
}

fn execute_inner(command: Command, perf: &mut Perf) -> anyhow::Result<()> {
    let (path, options) = match &command {
        Command::Classes { input, options, .. }
        | Command::Fields { input, options, .. }
        | Command::Methods { input, options, .. }
        | Command::Get { input, options, .. }
        | Command::GetClass { input, options, .. }
        | Command::Strings { input, options, .. }
        | Command::Xref { input, options, .. } => (input, options),
    };
    let limits = options.limits();
    let input = perf.measure("read_parse", || Input::read(path, limits))?;
    perf.source_id = Some(input.source_id.clone());
    perf.counters.input_bytes = Some(input.file_bytes);
    perf.counters.entities = Some(input.types.len() + input.fields.len() + input.methods.len());
    let query_start = perf.start_stage();
    let mut output = Vec::new();
    let mut records = 0;
    // One target, resolved before the scan: the same two faces every backend takes, and a
    // token is the only thing .NET can match on exactly.
    let xref_target = if let Command::Xref {
        at,
        filter,
        kind,
        case,
        from,
        ..
    } = &command
    {
        Some(
            dotnet_target(
                at.as_deref(),
                filter.as_deref(),
                *kind,
                *case,
                from.as_deref(),
                &input.source_id,
            )
            .map_err(|error| anyhow::anyhow!("INVALID_QUERY: {error:#}"))?,
        )
    } else {
        None
    };
    let mut reference_result = match xref_target.as_ref() {
        // A user-string token (0x70 table) has no member identity; it takes the string route.
        Some(Target::Token { token, .. }) if token >> 24 != 0x70 => {
            Some(perf.measure("references", || input.references(*token, true))?)
        }
        _ => None,
    };
    // The header names the reference target: a member identity, or the `#US` literal
    // when the query went through the string route.
    let reference_target = match (&reference_result, &command) {
        (Some(result), _) => match result.target.as_deref() {
            Some(target) => serde_json::to_value(target)?,
            None => Value::Null,
        },
        (None, _) => match xref_target.as_ref() {
            Some(Target::Literal { pattern, exact, .. }) => {
                if *exact {
                    json!({"target_kind": "user_string", "value": pattern})
                } else {
                    json!({"target_kind": "user_string", "filter": pattern})
                }
            }
            Some(Target::Token { token, .. }) => {
                let mut target =
                    json!({"target_kind": "user_string", "token": format!("0x{token:08x}")});
                if let Some(record) = user_string(&input, *token) {
                    match (record.value, record.utf16_units) {
                        (Some(text), _) => target["value"] = Value::from(text),
                        (None, Some(units)) => {
                            target["utf16_units"] = serde_json::to_value(units)?;
                        }
                        (None, None) => {}
                    }
                }
                target
            }
            None => Value::Null,
        },
    };
    if let Command::Get { selector, raw, .. } = &command {
        let at = selector.resolve(
            &["method"],
            crate::cli::xref::Expect::Content(&input.source_id),
        )?;
        let method = input.method_by_id(&at)?;
        let body = perf.measure("decode", || input.body(method))?;
        let decompilation = if *raw {
            rxd_dotnet::Decompilation {
                representation: "il".into(),
                decompiled: false,
                reason: None,
                warnings: Vec::new(),
                code: None,
                mapping: Vec::new(),
            }
        } else {
            perf.measure("reconstruct", || {
                rxd_dotnet::render(&input, method, &body, limits)
            })?
        };
        // Each metadata-token operand carries the resolved member it denotes, so a
        // fallback to IL does not require a separate --token --semantic round trip.
        let instructions = annotate_operands(&input, &body.instructions)?;
        let reason_detail = decompilation.reason.as_deref().map(|reason| {
            describe_method_reason(
                reason,
                body.instructions.len(),
                body.handlers.len(),
                &unresolved_operands(&instructions),
            )
        });
        let value = fetch_record(
            "method_body",
            command.name(),
            RecordMeta {
                at: Some(&at),
                parent_at: Some(&method.declaring_type_id),
                start: Some(body.code_offset),
                end: Some(body.code_offset + body.code_size),
                ..RecordMeta::default()
            },
            detail(
                json!({"method":method_record(&input, method)?,"body_present":true,"representation":decompilation.representation,"decompiled":decompilation.decompiled,"reason":decompilation.reason,"reason_detail":reason_detail,"warnings":decompilation.warnings,"code":decompilation.code,"mapping":decompilation.mapping,"file_offset":body.file_offset,"code_offset":body.code_offset,"code_size":body.code_size,"work_used":body.work_used,"max_stack":body.max_stack,"init_locals":body.init_locals,"locals":body.locals,"local_signature_token":body.local_signature_token,"instructions":instructions,"handlers":body.handlers,"il_hex":body.il_hex,"complete":true}),
            ),
        );
        append(&mut output, &value, limits.output_bytes)?;
    } else if let Command::GetClass { selector, raw, .. } = &command {
        let at = selector.resolve(
            &["type"],
            crate::cli::xref::Expect::Content(&input.source_id),
        )?;
        let class = perf.measure("decode", || input.class_body(&at))?;
        let rendered = if *raw {
            rxd_dotnet::ClassDecompilation {
                representation: "il".into(),
                decompiled: false,
                reason: None,
                warnings: Vec::new(),
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
            json!({"type":type_record(&input, class.root)?,"work_used":class.work_used,"representation":rendered.representation,"decompiled":rendered.decompiled,"reason":rendered.reason,"reason_detail":rendered.reason.as_deref().map(|reason| describe_class_reason(reason, class.methods.len())),"warnings":rendered.warnings,"code":rendered.code,"mapping":rendered.mapping,"complete":true}),
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
        // Nested inventories carry the same entity references as the top level.
        shorten_detail_ids(&mut payload);
        let value = fetch_record(
            "class_body",
            command.name(),
            RecordMeta {
                at: Some(&at),
                parent_at: class.root.declaring_type_id.as_deref(),
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
            Command::Classes { class, .. } => json!({"class":class}),
            Command::Fields { class, name, .. } => json!({"class":class,"name":name}),
            Command::Methods { class, name, .. } => json!({"class":class,"name":name}),
            Command::Strings { filter, .. } => json!({"filter":filter}),
            Command::Xref {
                at, filter, from, ..
            } => json!({
                "kind": xref_target.as_ref().map(Target::kind).map(Kind::as_str),
                "at": at,
                "filter": filter,
                "class": Value::Null,
                "from": from.as_ref().map(|path| path.display().to_string()),
            }),
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
                if matches!(command, Command::Xref { .. }) {
                    detail(json!({"target": reference_target}))
                } else {
                    Map::new()
                },
            ),
            limits.output_bytes,
        )?;
        match &command {
            Command::Classes { .. } | Command::Fields { .. } | Command::Methods { .. } => {
                list(&input, &command, options, &mut output, &mut records)?;
            }
            Command::Strings { filter, case, .. } => {
                let uses = perf.measure("strings", || {
                    input.string_uses(filter.as_deref(), case.sensitive())
                })?;
                for group in &uses.groups {
                    let literal = &uses.literals[group.literal_index];
                    // `index` is the backend's own identifier for this string in its
                    // inventory: here, its position among the assembly's literals.
                    let index = group.literal_index;
                    let method = group.method_token.map(|token| {
                        &input.methods[input
                            .methods
                            .binary_search_by_key(&token, |method| method.token)
                            .expect("checked string owner")]
                    });
                    record(&mut output, &mut records, options, &{
                        // The `#US` token is the literal's identity, so it is the row's
                        // `at`: that is what `--at`/`--from` take back, and what the
                        // reference rows point at.
                        let token = format!("0x{:08x}", 0x7000_0000u32 | literal.offset);
                        envelope::record(
                            "string",
                            RecordMeta {
                                at: Some(&token),
                                parent_at: method.map(|method| method.declaring_type_id.as_str()),
                                context_at: method.map(|method| method.id.as_str()),
                                ..metadata(&input)
                            },
                            detail(
                                json!({"index":index,"value":literal.value,"utf16_units":literal.utf16_units,"status":if method.is_some(){"referenced"}else{"unreferenced"},"il_offsets":group.il_offsets}),
                            ),
                        )
                    })?;
                }
            }
            Command::Xref { .. } => {
                if let Some(result) = reference_result.take() {
                    for reference in result.references {
                        let value = reference_record(
                            &input,
                            &reference.site,
                            reference.target.as_deref(),
                            None,
                            limits.output_bytes.saturating_sub(output.len()),
                        )?;
                        record(&mut output, &mut records, options, &value)?;
                    }
                } else {
                    // A string-route row names the text it loads, so the literal inventory is
                    // built once instead of resolving a token per row. Both anchors print the
                    // same target: a `#US` token and its text are two ways to name one literal,
                    // and the row says which text it was.
                    let literals = Some(literal_values(&input));
                    let sites = match xref_target.as_ref().expect("resolved here") {
                        Target::Literal {
                            pattern,
                            exact,
                            case_sensitive,
                        } => perf.measure("strings", || {
                            input.string_references(pattern, *exact, *case_sensitive)
                        })?,
                        Target::Token { token, .. } => {
                            perf.measure("strings", || input.user_string_references(*token))?
                        }
                    };
                    for site in sites {
                        let literal = literals
                            .as_ref()
                            .and_then(|map| map.get(&(site.target_token & 0x00ff_ffff)));
                        let value = reference_record(
                            &input,
                            &site,
                            None,
                            literal.map(String::as_str),
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
    // The decoded `#US` literal this site loads, when the query went through the string
    // route: a token alone does not say which text the row is about.
    literal: Option<&str>,
    remaining: usize,
) -> anyhow::Result<Value> {
    if let Some(target) = target {
        descriptor_fits(target, remaining)?;
    }
    let method = &input.methods[(site.method_token & 0xffffff) as usize - 1];
    let start = site.code_offset + site.il_offset as usize;
    Ok(envelope::reference(
        RecordMeta {
            context_at: Some(&method.id),
            parent_at: Some(&method.declaring_type_id),
            start: Some(start),
            end: Some(start + site.size as usize),
            ..metadata(input)
        },
        detail(
            json!({"target":literal.map(str::to_owned).or_else(||target.and_then(|target|target.canonical_id.as_deref()).map(envelope::at_of).map(|id|id.into_owned())).unwrap_or_else(||format!("0x{:08x}",site.target_token)),"declaring_type":method.declaring_type,"name":method.name,"il_offset":site.il_offset,"opcode":site.mnemonic,"target_token":site.target_token,"target_kind":if let Some(target)=target{target.target_kind}else if site.target_token >> 24 == 0x70 {"string"}else{"token"},"resolution_status":target.map_or("token_only",|target|target.resolution_status.as_str()),"scope":target.map(|target|&target.scope),"owner_token":target.map(|target|target.owner_token),"original_owner_token":target.map(|target|target.original_owner_token),"owner_name":target.map(|target|&target.owner_name),"target_name":target.map(|target|&target.name),"signature":target.and_then(|target|target.signature.as_ref()),"type_signature":target.and_then(|target|target.type_signature.as_ref()),"owner_arguments":target.map_or(&[][..],|target|target.owner_arguments.as_slice()),"method_arguments":target.map_or(&[][..],|target|target.method_arguments.as_slice())}),
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
        *records < options.max_records(),
        "RESOURCE_LIMIT: record budget exceeded"
    );
    append(output, value, options.max_output_bytes())?;
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

/// Names of token operands the resolver could not place, deduplicated and truncated so
/// the reason stays readable.
fn unresolved_operands(instructions: &Value) -> Vec<String> {
    let mut names = Vec::new();
    let Some(list) = instructions.as_array() else {
        return names;
    };
    for instruction in list {
        let Some(operand) = instruction.get("operand") else {
            continue;
        };
        if operand.get("kind").and_then(Value::as_str) != Some("token") {
            continue;
        }
        match operand.get("status").and_then(Value::as_str) {
            Some("resolved" | "external") => {}
            _ => {
                // A user-string operand (0x70 table) has no member identity at all, so
                // it is expected to be nameless rather than unresolved.
                if operand.get("value").and_then(Value::as_u64).unwrap_or(0) >> 24 == 0x70 {
                    continue;
                }
                // An unresolved operand has no owner or name; the token is the only
                // thing an Agent can act on.
                let label = match (
                    operand.get("owner").and_then(Value::as_str),
                    operand.get("name").and_then(Value::as_str),
                ) {
                    (Some(owner), Some(name)) => format!("{owner}::{name}"),
                    _ => format!(
                        "token {:#010x}",
                        operand.get("value").and_then(Value::as_u64).unwrap_or(0)
                    ),
                };
                if !names.contains(&label) {
                    names.push(label);
                }
            }
        }
    }
    names.truncate(4);
    names
}

/// Explain, in the record itself, why this body is IL instead of C#.
fn describe_method_reason(
    reason: &str,
    instructions: usize,
    handlers: usize,
    unresolved: &[String],
) -> String {
    let unresolved_detail = if unresolved.is_empty() {
        "every member operand resolves, so the refusal is the engine's own reconstruction limit rather than missing metadata".to_string()
    } else {
        format!(
            "{} unresolved member operand(s): {}",
            unresolved.len(),
            unresolved.join(", ")
        )
    };
    match reason {
        "engine_unavailable" => format!(
            "this build cannot load the pinned C# engine ({}), so only the original IL is available: {instructions} instructions, {unresolved_detail}",
            engine_state(),
        ),
        other => format!(
            "the C# engine rejected this body (`{other}`); the original IL is returned: {instructions} instructions, {handlers} exception handlers, {unresolved_detail}"
        ),
    }
}

fn describe_class_reason(reason: &str, methods: usize) -> String {
    match reason {
        "class_engine_unavailable" => format!(
            "this build cannot load the pinned C# engine ({}); the {methods}-member inventory and original IL are complete, whole-class C# is not available",
            engine_state(),
        ),
        "class_abi_unavailable" => format!(
            "the loaded engine exposes no whole-class entry point; the {methods}-member inventory and original IL are complete. Fetch one member with `get-method --at <method:token>`"
        ),
        other => format!(
            "the C# engine rejected this class (`{other}`); the {methods}-member inventory and original IL are complete and no partial C# is returned. The refusal names no member, so fetch each member with `get-method --at <at>`: a member the engine also refuses reports `il_warning` and still returns its original IL"
        ),
    }
}

/// Decode one `#US` literal by token so a record can carry the text, not just the token.
/// Decoded `#US` literals keyed by their heap offset, for one query.
fn literal_values(input: &Input) -> BTreeMap<u32, String> {
    input
        .strings()
        .map(|records| {
            records
                .into_iter()
                .filter_map(|record| record.value.map(|value| (record.offset, value)))
                .collect()
        })
        .unwrap_or_default()
}

fn user_string(input: &Input, token: u32) -> Option<rxd_dotnet::StringRecord> {
    input
        .strings()
        .ok()?
        .into_iter()
        .find(|record| record.offset == token & 0x00ff_ffff)
}

fn engine_state() -> &'static str {
    if cfg!(feature = "bundled-ilspy") {
        "bundled engine failed to load"
    } else {
        "built without the bundled engine"
    }
}

/// Annotate every metadata-token operand with the member it provably denotes.
fn annotate_operands(
    input: &Input,
    instructions: &[rxd_dotnet::Instruction],
) -> anyhow::Result<Value> {
    let mut value = serde_json::to_value(instructions)?;
    let Some(list) = value.as_array_mut() else {
        return Ok(value);
    };
    let mut resolver = input.target_resolver()?;
    // A `#US` operand has no member identity, but its token is a usable coordinate and
    // the literal is already decoded in the heap, so one lookup answers both questions.
    let literals = if instructions.iter().any(|instruction| {
        matches!(instruction.operand, rxd_dotnet::Operand::Token(token) if token >> 24 == 0x70)
    }) {
        let mut map = BTreeMap::new();
        for record in input.strings()? {
            map.insert(record.offset, record);
        }
        Some(map)
    } else {
        None
    };
    for instruction in list.iter_mut() {
        let Some(operand) = instruction.get_mut("operand") else {
            continue;
        };
        if operand.get("kind").and_then(Value::as_str) != Some("token") {
            continue;
        }
        let Some(token) = operand.get("value").and_then(Value::as_u64) else {
            continue;
        };
        let token = u32::try_from(token).unwrap_or(0);
        if token >> 24 == 0x70 {
            operand["target_at"] = Value::from(format!("0x{token:08x}"));
            if let Some(record) = literals
                .as_ref()
                .and_then(|map| map.get(&(token & 0x00ff_ffff)))
            {
                match (&record.value, &record.utf16_units) {
                    (Some(text), _) => operand["string"] = Value::from(text.clone()),
                    (None, Some(units)) => {
                        operand["utf16_units"] = serde_json::to_value(units)?;
                    }
                    (None, None) => {}
                }
            }
            continue;
        }
        let Some(target) = resolver.target(token)? else {
            continue;
        };
        operand["owner"] = Value::from(target.owner_name.clone());
        operand["name"] = Value::from(target.name.clone());
        operand["status"] = Value::from(target.resolution_status.clone());
        operand["scope"] = serde_json::to_value(&target.scope)?;
        if let Some(canonical) = &target.canonical_id {
            operand["target_at"] = Value::from(envelope::at_of(canonical).into_owned());
        }
    }
    Ok(value)
}

/// Detail-level entity references use the same copy-only coordinate as the outer
/// envelope; a caller never receives a hash it did not ask for. Nested inventories
/// (class bodies, refs rows) carry the same keys, so the rewrite is recursive.
fn shorten_ids(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields.iter_mut() {
                if let Value::String(text) = child
                    && (key == "id" || key.ends_with("_id"))
                    && text.starts_with("rxd1:")
                {
                    *text = envelope::at_of(text).into_owned();
                    continue;
                }
                shorten_ids(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(shorten_ids),
        _ => {}
    }
}

fn shorten_detail_ids(fields: &mut serde_json::Map<String, Value>) {
    let mut value = Value::Object(std::mem::take(fields));
    shorten_ids(&mut value);
    if let Value::Object(restored) = value {
        *fields = restored;
    }
}

fn method_record(input: &Input, method: &rxd_dotnet::Method) -> anyhow::Result<Value> {
    let mut fields = detail(serde_json::to_value(method)?);
    fields.remove("id");
    shorten_detail_ids(&mut fields);
    Ok(envelope::record(
        "method",
        RecordMeta {
            at: Some(&method.id),
            parent_at: Some(&method.declaring_type_id),
            ..metadata(input)
        },
        fields,
    ))
}

fn type_record(input: &Input, ty: &rxd_dotnet::TypeInfo) -> anyhow::Result<Value> {
    let mut fields = detail(serde_json::to_value(ty)?);
    fields.remove("id");
    shorten_detail_ids(&mut fields);
    // A base type and an interface are tokens; a reader navigates by name. Both stay,
    // because the token is the identity `get-class --at` takes back and a name is not
    // unique across assemblies. An omitted default keeps the row at its old size.
    if let Some(token) = ty.extends_token
        && let Some(name) = input.type_display(token)
        && name != "System.Object"
    {
        fields.insert("super".to_owned(), Value::from(name));
    }
    if !ty.interfaces.is_empty() {
        let names: Vec<Value> = ty
            .interfaces
            .iter()
            .filter_map(|token| input.type_display(*token))
            .map(Value::from)
            .collect();
        let tokens: Vec<Value> = ty.interfaces.iter().map(|t| Value::from(*t)).collect();
        // Names are a projection of the tokens; print them only for the whole list so the
        // two arrays always line up.
        if names.len() == ty.interfaces.len() {
            fields.insert("interfaces".to_owned(), Value::Array(names));
        }
        fields.insert("interface_tokens".to_owned(), Value::Array(tokens));
    }
    Ok(envelope::record(
        "class",
        RecordMeta {
            at: Some(&ty.id),
            parent_at: ty.declaring_type_id.as_deref(),
            ..metadata(input)
        },
        fields,
    ))
}

fn field_record(input: &Input, field: &rxd_dotnet::Field) -> anyhow::Result<Value> {
    let mut fields = detail(serde_json::to_value(field)?);
    fields.remove("id");
    // Nested references are copy-only coordinates, exactly as in `methods`/`classes`.
    shorten_detail_ids(&mut fields);
    Ok(envelope::record(
        "field",
        RecordMeta {
            at: Some(&field.id),
            parent_at: Some(&field.declaring_type_id),
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
    fields.insert("reason".into(), json!("member_inventory"));
    fields.insert(
        "reason_detail".into(),
        json!("whole-class C# lives in `class_body.code`; a member record carries original IL only. Use `get-method --at <method:token>` for one member's C#."),
    );
    fields.insert("code".into(), Value::Null);
    fields.insert("mapping".into(), json!([]));
    Ok(envelope::record(
        "method_body",
        RecordMeta {
            at: Some(&item.method.id),
            parent_at: Some(&item.method.declaring_type_id),
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
            parent_at: Some(owner),
            context_at: Some(owner),
            ..metadata(input)
        },
        {
            // Nested references are copy-only coordinates, as in every other listing.
            let mut fields = detail(serde_json::to_value(relationship)?);
            shorten_detail_ids(&mut fields);
            fields
        },
    ))
}

/// Per-row metadata: identity is carried once by the stream header, as in DEX and the
/// JVM backend, so a row does not repeat the assembly's content id.
fn metadata(_input: &Input) -> RecordMeta<'_> {
    RecordMeta::default()
}

/// Which member kind a listing prints.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listing {
    Classes,
    Fields,
    Methods,
}

/// Print one member kind, grouped by declaring type in declaration order.
fn list(
    input: &Input,
    command: &Command,
    options: &Options,
    output: &mut Vec<u8>,
    records: &mut usize,
) -> anyhow::Result<()> {
    let (listing, class, name, filter, case) = match command {
        Command::Classes {
            class,
            filter,
            case,
            ..
        } => (Listing::Classes, class, None, filter, case),
        Command::Fields {
            class,
            name,
            filter,
            case,
            ..
        } => (Listing::Fields, class, name.as_ref(), filter, case),
        Command::Methods {
            class,
            name,
            filter,
            case,
            ..
        } => (Listing::Methods, class, name.as_ref(), filter, case),
        _ => unreachable!("only listing commands reach here"),
    };
    let pattern = filter
        .as_deref()
        .map(|needle| rxd_core::filter::Contains::new(needle, case.sensitive()));
    // A filter answers "does this record's own name contain the needle?". The declaring
    // type is a separate question that `--class` answers exactly, so a member is never
    // reported for a match on its owner's name (ADR 0014).
    let matches = |name: &str| pattern.as_ref().is_none_or(|pattern| pattern.matches(name));
    let mut fields_by_type: BTreeMap<u32, Vec<_>> = BTreeMap::new();
    let mut methods_by_type: BTreeMap<u32, Vec<_>> = BTreeMap::new();
    if listing == Listing::Fields {
        for field in &input.fields {
            fields_by_type
                .entry(field.declaring_type_token)
                .or_default()
                .push(field);
        }
    }
    if listing == Listing::Methods {
        for item in &input.methods {
            methods_by_type
                .entry(item.declaring_type_token)
                .or_default()
                .push(item);
        }
    }
    for ty in &input.types {
        if class.as_ref().is_some_and(|name| *name != ty.full_name) {
            continue;
        }
        match listing {
            Listing::Classes => {
                if matches(&ty.full_name) {
                    record(output, records, options, &type_record(input, ty)?)?;
                }
            }
            Listing::Fields => {
                for field in fields_by_type.get(&ty.token).into_iter().flatten() {
                    if name.is_some_and(|wanted| wanted != &field.name) || !matches(&field.name) {
                        continue;
                    }
                    record(output, records, options, &field_record(input, field)?)?;
                }
            }
            Listing::Methods => {
                for item in methods_by_type.get(&ty.token).into_iter().flatten() {
                    if name.is_some_and(|wanted| wanted != &item.name) || !matches(&item.name) {
                        continue;
                    }
                    record(output, records, options, &method_record(input, item)?)?;
                }
            }
        }
    }
    Ok(())
}

fn detail(value: Value) -> Map<String, Value> {
    let Value::Object(fields) = value else {
        unreachable!(".NET record detail is a statically defined object")
    };
    fields
}

/// A dotnet coordinate is the ECMA-335 metadata token itself: eight hex digits,
/// optional `0x`. A kind prefix is refused with the value to use instead.
/// The one target a .NET reference query names.
enum Target {
    /// A metadata or `#US` token: an identity, matched exactly.
    Token { token: u32, kind: Kind },
    /// A decoded user-string literal: an exact anchor, or a substring search.
    Literal {
        pattern: String,
        exact: bool,
        case_sensitive: bool,
    },
}

impl Target {
    fn kind(&self) -> Kind {
        match self {
            Self::Token { kind, .. } => *kind,
            Self::Literal { .. } => Kind::String,
        }
    }
}

/// Resolve the reference target from its two faces.
///
/// A token says its own kind through its table, so `--kind` is only an assertion here. That
/// matters because a token is a per-module row number: `0x0600000b` is a different method in
/// every assembly, which is why a `--from` stream is checked against this input's identity.
fn dotnet_target(
    at: Option<&str>,
    filter: Option<&str>,
    kind: Option<Kind>,
    case: crate::cli::filter::CaseMode,
    from: Option<&Path>,
    source_id: &str,
) -> anyhow::Result<Target> {
    let faces =
        usize::from(at.is_some()) + usize::from(filter.is_some()) + usize::from(from.is_some());
    anyhow::ensure!(
        faces == 1,
        "INVALID_QUERY: exactly one reference target is required (`--at TOKEN`, `-f PATTERN \
         --kind string`, or `--from JSONL`)"
    );
    if let Some(pattern) = filter {
        anyhow::ensure!(
            kind == Some(crate::cli::xref::Kind::String),
            "INVALID_QUERY: a .NET member or type identity is a metadata token, not a name, so \
             `-f` can only search literals: give `--kind string`, or anchor with `--at TOKEN` / \
             `--from JSONL`"
        );
        return Ok(Target::Literal {
            pattern: pattern.to_owned(),
            exact: false,
            case_sensitive: case.is_sensitive(),
        });
    }
    let coordinate = match (at, from) {
        (Some(coordinate), _) => coordinate.to_owned(),
        (None, Some(path)) => crate::cli::xref::selector_stream(
            path,
            &["method", "field", "type", "string"],
            crate::cli::xref::Expect::Content(source_id),
        )?,
        (None, None) => unreachable!("checked above"),
    };
    // `--kind string` reads `--at` as a literal, which is the one target that cannot
    // describe itself. A token still wins when it parses as one: `0x0600000b --kind string`
    // is a contradiction to report, not a search for the text `0x0600000b`.
    if kind == Some(crate::cli::xref::Kind::String) {
        return match parse_token(&coordinate) {
            Ok(token) => {
                let inferred = token_kind(token);
                if token >> 24 != 0x70 || inferred != Some(crate::cli::xref::Kind::String) {
                    anyhow::bail!(
                        "--at {coordinate} is a {} token, but --kind string was given",
                        inferred.map_or("non-string", |kind| kind.as_str())
                    );
                }
                Ok(Target::Token {
                    token,
                    kind: crate::cli::xref::Kind::String,
                })
            }
            Err(_) => Ok(Target::Literal {
                pattern: coordinate,
                exact: true,
                case_sensitive: true,
            }),
        };
    }
    let token = parse_token(&coordinate).map_err(|error| {
        anyhow::anyhow!("{error:#}; pass `--kind string` to read `--at` as an exact literal")
    })?;
    let inferred = token_kind(token);
    if let (Some(given), Some(inferred)) = (kind, inferred)
        && given != inferred
    {
        anyhow::bail!(
            "--at {coordinate} is a {} token, but --kind {} was given",
            inferred.as_str(),
            given.as_str()
        );
    }
    Ok(Target::Token {
        token,
        kind: kind.or(inferred).unwrap_or(Kind::Method),
    })
}

/// The kind a token's table names, or `None` where the table covers two (`MemberRef`).
fn token_kind(token: u32) -> Option<Kind> {
    match token >> 24 {
        0x70 => Some(Kind::String),
        0x01 | 0x02 | 0x1b => Some(Kind::Type),
        0x04 => Some(Kind::Field),
        0x06 | 0x2b => Some(Kind::Method),
        _ => None,
    }
}

fn parse_token(value: &str) -> anyhow::Result<u32> {
    if let Some((kind, _)) = value.split_once(':') {
        anyhow::bail!(
            "INVALID_QUERY: `{value}` carries a `{kind}:` prefix; a dotnet coordinate is the metadata token alone, for example `{}`",
            value.rsplit(':').next().unwrap_or(value)
        );
    }
    let body = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    anyhow::ensure!(
        body.len() == 8 && body.bytes().all(|b| b.is_ascii_hexdigit()),
        "INVALID_QUERY: expected a metadata token such as `0x0600000b` (eight hex digits, `0x` optional)"
    );
    Ok(u32::from_str_radix(body, 16)?)
}
