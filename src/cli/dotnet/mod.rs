use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::Context;
use clap::{Args, Subcommand};
use rxd_core::envelope::{self, RecordMeta};
use rxd_core::perf::Perf;
use rxd_core::{Budget, Error, Result};
use rxd_dotnet::{Input, Limits};
use serde_json::{Map, Value, json};

/// One FETCH selector: a coordinate copied from a record, or a previous JSONL
/// stream to read that coordinate from.
#[derive(Args, Clone, Debug)]
pub struct Selector {
    /// Coordinate copied from a record: the metadata token, such as `0x06000003`
    /// (`0x` optional).
    #[arg(long, required_unless_present = "from", conflicts_with = "from")]
    at: Option<String>,
    /// Read the coordinate from a previous JSONL stream (a file, or `-` for stdin).
    /// The stream's `source_id` must match this input, so a stale stream cannot
    /// silently fetch a member from another file.
    #[arg(
        long,
        value_name = "JSONL",
        required_unless_present = "at",
        conflicts_with = "at"
    )]
    from: Option<PathBuf>,
}

impl Selector {
    /// Resolve the coordinate to fetch; `units` are the record kinds the stream may
    /// carry (a stream is not trusted to name the unit the command needs).
    fn resolve(&self, units: &[&str], source_id: &str) -> anyhow::Result<String> {
        match &self.at {
            Some(at) => Ok(at.clone()),
            None => read_selector(
                self.from
                    .as_deref()
                    .expect("clap requires exactly one selector"),
                units,
                source_id,
            ),
        }
    }
}

/// Bounded read of one JSONL stream: the first record whose `record` is in `units`
/// and which carries an `at` coordinate wins. A mismatched `source_id` is an error,
/// never a silent fetch from another input.
fn read_selector(path: &Path, units: &[&str], source_id: &str) -> anyhow::Result<String> {
    const LIMIT: u64 = 64 * 1024;
    let mut text = String::new();
    if path == Path::new("-") {
        // The parent reads stdin because the isolated worker has none; a direct
        // invocation (no worker) falls back to reading it here.
        match std::env::var("RXD_SELECTOR_STDIN") {
            Ok(stream) => text = stream,
            Err(_) => {
                io::stdin()
                    .take(LIMIT)
                    .read_to_string(&mut text)
                    .context("INVALID_QUERY: cannot read --from stdin")?;
            }
        }
    } else {
        std::fs::File::open(path)
            .and_then(|file| file.take(LIMIT).read_to_string(&mut text).map(|_| ()))
            .with_context(|| format!("INVALID_QUERY: cannot read --from {}", path.display()))?;
    }
    // Identity is carried once by the stream header, so a record's own `source_id` is
    // normally null: the header supplies the binding a unit row is checked against.
    let mut header_source: Option<String> = None;
    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = row
            .get("record")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind == "header" {
            header_source = row
                .get("source_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            continue;
        }
        if !units.contains(&kind) {
            continue;
        }
        let Some(at) = row.get("at").and_then(Value::as_str) else {
            continue;
        };
        let found = row
            .get("source_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| header_source.clone());
        if let Some(found) = found {
            anyhow::ensure!(
                found == source_id,
                "INVALID_QUERY: --from stream came from a different input (stream {}…, this input {}…); re-run the listing for this file",
                short_source(&found),
                short_source(source_id)
            );
        }
        return Ok(at.to_string());
    }
    anyhow::bail!(
        "INVALID_QUERY: --from stream carries no {} record with an `at` coordinate",
        units.join("/")
    )
}

fn short_source(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

#[derive(Subcommand)]
pub enum Command {
    /// List the types an assembly defines as JSONL.
    Classes {
        input: PathBuf,
        /// Exact full type name.
        #[arg(long = "type")]
        type_name: Option<String>,
        /// Case-insensitive substring over the full type name.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        options: Options,
    },
    /// List the fields an assembly defines as JSONL.
    Fields {
        input: PathBuf,
        /// Exact full type name.
        #[arg(long = "type")]
        type_name: Option<String>,
        /// Case-insensitive substring over the field or declaring type name.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        options: Options,
    },
    /// List the methods an assembly defines as JSONL.
    Methods {
        input: PathBuf,
        /// Exact full type name.
        #[arg(long = "type")]
        type_name: Option<String>,
        /// Exact method name.
        #[arg(long)]
        method: Option<String>,
        /// Case-insensitive substring over the method or declaring type name.
        #[arg(short, long)]
        filter: Option<String>,
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
        /// Case-insensitive substring over the decoded literal.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        options: Options,
    },
    /// Find exact/semantic member and type operands or decoded user-string loads.
    Xref {
        input: PathBuf,
        /// Reference target: the metadata token of a member or type, such as
        /// `0x0a00001f`; it also covers external members and generic instantiations.
        #[arg(long, required_unless_present_any = ["string","from"], conflicts_with_all = ["string","from"])]
        at: Option<String>,
        #[arg(long, required_unless_present_any = ["at","from"], conflicts_with_all = ["at","from"])]
        string: Option<String>,
        /// Read the target from a previous JSONL stream (a file, or `-` for stdin);
        /// `method`, `field` and `type` records are accepted.
        #[arg(long, value_name = "JSONL", required_unless_present_any = ["at","string"], conflicts_with_all = ["at","string"])]
        from: Option<PathBuf>,

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
        Error::coded(code, message)
    });
    super::finish(perf, emit_perf, debug, result)
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
    let mut reference_result = if let Command::Xref {
        at, string, from, ..
    } = &command
    {
        anyhow::ensure!(
            usize::from(at.is_some()) + usize::from(string.is_some()) + usize::from(from.is_some())
                == 1,
            "INVALID_QUERY: exactly one reference target is required (`--at`, `--string` or `--from`)"
        );
        let at = match (at, from) {
            (Some(at), _) => Some(at.clone()),
            (None, Some(path)) => Some(read_selector(
                path,
                &["method", "field", "type"],
                &input.source_id,
            )?),
            (None, None) => None,
        };
        let target = at.as_deref().map(parse_token).transpose()?;
        // A user-string token (0x70 table) has no member identity; it takes the
        // string route.
        let string_route = string.is_some() || target.is_some_and(|token| token >> 24 == 0x70);
        if string_route {
            None
        } else {
            Some(perf.measure("references", || {
                input.references(target.expect("clap requires one target"), true)
            })?)
        }
    } else {
        None
    };
    // The header names the reference target: a member identity, or the `#US` literal
    // when the query went through the string route.
    let reference_target = match (&reference_result, &command) {
        (Some(result), _) => match result.target.as_deref() {
            Some(target) => serde_json::to_value(target)?,
            None => Value::Null,
        },
        (None, Command::Xref { at, string, .. }) => {
            if let Some(value) = string {
                json!({"target_kind": "user_string", "value": value})
            } else if let Some(token) = at.as_deref().and_then(|value| parse_token(value).ok()) {
                let mut target =
                    json!({"target_kind": "user_string", "token": format!("0x{token:08x}")});
                if let Some(record) = user_string(&input, token) {
                    match (record.value, record.utf16_units) {
                        (Some(text), _) => target["value"] = Value::from(text),
                        (None, Some(units)) => {
                            target["utf16_units"] = serde_json::to_value(units)?;
                        }
                        (None, None) => {}
                    }
                }
                target
            } else {
                Value::Null
            }
        }
        _ => Value::Null,
    };
    if let Command::Get { selector, raw, .. } = &command {
        let at = selector.resolve(&["method"], &input.source_id)?;
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
        let at = selector.resolve(&["type"], &input.source_id)?;
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
            Command::Classes { type_name, .. } | Command::Fields { type_name, .. } => {
                json!({"type":type_name})
            }
            Command::Methods {
                type_name, method, ..
            } => json!({"type":type_name,"method":method}),
            Command::Strings { filter, .. } => json!({"filter":filter}),
            Command::Xref {
                at, string, from, ..
            } => json!({"at":at,"from":from,"string":string}),
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
            Command::Strings { filter, .. } => {
                let uses = perf.measure("strings", || input.string_uses(filter.as_deref()))?;
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
                    record(
                        &mut output,
                        &mut records,
                        options,
                        &envelope::record(
                            "string",
                            RecordMeta {
                                parent_at: method.map(|method| method.declaring_type_id.as_str()),
                                context_at: method.map(|method| method.id.as_str()),
                                ..metadata(&input)
                            },
                            detail(
                                json!({"index":index,"offset":literal.offset,"token":0x70000000u32 | literal.offset,"value":literal.value,"utf16_units":literal.utf16_units,"status":if method.is_some(){"referenced"}else{"unreferenced"},"method_token":group.method_token,"declaring_type_id":method.map(|method| envelope::at_of(&method.declaring_type_id)),"il_offsets":group.il_offsets}),
                            ),
                        ),
                    )?;
                }
            }
            Command::Xref { at, string, .. } => {
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
                        let token = at
                            .as_deref()
                            .and_then(|value| parse_token(value).ok())
                            .expect("a `0x70` token routes here only with `--at`");
                        perf.measure("strings", || input.user_string_references(token))?
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
            context_at: Some(&method.id),
            parent_at: Some(&method.declaring_type_id),
            start: Some(start),
            end: Some(start + site.size as usize),
            ..metadata(input)
        },
        detail(
            json!({"method_token":method.token,"declaring_type":method.declaring_type,"declaring_type_id":envelope::at_of(&method.declaring_type_id),"name":method.name,"il_offset":site.il_offset,"opcode":site.mnemonic,"target_token":site.target_token,"target_kind":if let Some(target)=target{target.target_kind}else if site.target_token >> 24 == 0x70 {"string"}else{"token"},"canonical_target_id":target.and_then(|target|target.canonical_id.as_deref()).map(envelope::at_of),"canonical_target_token":target.and_then(|target|target.canonical_token),"resolution_status":target.map_or("token_only",|target|target.resolution_status.as_str()),"scope":target.map(|target|&target.scope),"owner_token":target.map(|target|target.owner_token),"original_owner_token":target.map(|target|target.original_owner_token),"owner_name":target.map(|target|&target.owner_name),"target_name":target.map(|target|&target.name),"signature":target.and_then(|target|target.signature.as_ref()),"type_signature":target.and_then(|target|target.type_signature.as_ref()),"owner_arguments":target.map_or(&[][..],|target|target.owner_arguments.as_slice()),"method_arguments":target.map_or(&[][..],|target|target.method_arguments.as_slice())}),
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
                if key == "id" || key.ends_with("_id") {
                    if let Value::String(text) = child {
                        if text.starts_with("rxd1:") {
                            *text = envelope::at_of(text).into_owned();
                            continue;
                        }
                    }
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
    Ok(envelope::record(
        "type",
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
        detail(serde_json::to_value(relationship)?),
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
    let (listing, type_name, method, filter) = match command {
        Command::Classes {
            type_name, filter, ..
        } => (Listing::Classes, type_name, None, filter),
        Command::Fields {
            type_name, filter, ..
        } => (Listing::Fields, type_name, None, filter),
        Command::Methods {
            type_name,
            method,
            filter,
            ..
        } => (Listing::Methods, type_name, method.as_ref(), filter),
        _ => unreachable!("only listing commands reach here"),
    };
    let pattern = filter.as_deref().map(str::to_lowercase);
    let matches = |name: &str, owner: &str| {
        pattern.as_ref().is_none_or(|pattern| {
            name.to_lowercase().contains(pattern) || owner.to_lowercase().contains(pattern)
        })
    };
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
        if type_name.as_ref().is_some_and(|name| *name != ty.full_name) {
            continue;
        }
        match listing {
            Listing::Classes => {
                if matches(&ty.full_name, &ty.full_name) {
                    record(output, records, options, &type_record(input, ty)?)?;
                }
            }
            Listing::Fields => {
                for field in fields_by_type.get(&ty.token).into_iter().flatten() {
                    if !matches(&field.name, &ty.full_name) {
                        continue;
                    }
                    record(output, records, options, &field_record(input, field)?)?;
                }
            }
            Listing::Methods => {
                for item in methods_by_type.get(&ty.token).into_iter().flatten() {
                    if method.is_some_and(|name| name != &item.name)
                        || !matches(&item.name, &ty.full_name)
                    {
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
