mod files;

use std::{collections::BTreeMap, io::Write, path::PathBuf};

use super::filter::CaseArgs;
use super::xref::Selector;
use clap::{Args, Subcommand};
use rxd_core::envelope::{self, RecordMeta};
use rxd_core::perf::Perf;
use rxd_core::{Budget, Result};
use rxd_dotnet::{Input, Limits};
use serde_json::{Map, Value, json};

#[derive(Subcommand)]
pub enum Command {
    /// List single-file bundle entries without decompressing them.
    #[command(
        mut_arg("entry", |arg| arg.hide(true).conflicts_with("input")),
        long_about = "Columns: at, kind, size, compressed_size\nList .NET single-file bundle entries in manifest order, without decompressing them.",
        after_long_help = "Examples:\nrxd ilspy entries app -f App.dll\nrxd ilspy methods app --entry '<entries.at>'\nrxd ilspy extract-entry app --at '<entries.at>' -o module.dll\nat is the exact case-sensitive bundle path. kind is assembly, native, deps_json, runtime_config_json, symbols or unknown.\nsize is the original byte length; compressed_size is zero for stored entries. Formats 1.0, 2.0 and 6.0 are supported.\nAnalysis queries require --entry even if the bundle contains only one assembly. NativeAOT machine code is not managed IL."
    )]
    Entries {
        input: PathBuf,
        /// Substring over bundle paths; folded unless --case sensitive.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Extract one bundle entry as original bytes.
    #[command(
        mut_arg("entry", |arg| arg.hide(true).conflicts_with("input")),
        long_about = "Extract one exact .NET single-file bundle entry. stdout contains only the original bytes; -o writes them atomically to FILE.",
        after_long_help = "Examples:\nrxd ilspy entries app\nrxd ilspy extract-entry app --at '<entries.at>' -o App.dll\nCopy the exact entries.at path, not a metadata token. Compressed entries are decoded with exact length validation.\nNo entry path is used as an output filename; -o cannot overwrite the input. This binary extractor is CLI-only."
    )]
    ExtractEntry {
        input: PathBuf,
        #[arg(long, required = true, value_name = "ENTRY_PATH")]
        at: String,
        #[command(flatten)]
        options: Options,
    },
    /// List manifest resources or one .resources container's members.
    #[command(
        long_about = "Columns: at, name, type, size, parent_at\nList manifest resources, or members of one .resources container selected with --within.",
        after_long_help = "Examples:\nrxd ilspy resources app.dll\nrxd ilspy resources app.dll --within '<resources.at>' -f settings\nrxd ilspy resources app --entry App.dll\nrxd ilspy extract-resource app.dll --at '<resources.at>' -o resource.bin\nRoot coordinates are ManifestResource tokens (0x28000001); members use TOKEN/item:N.\nRoot type is bytes, resources or linked; member type is the stored CLR type or null. Linked resources have no size and are not opened.\nsize is the extractable payload byte count. parent_at identifies a member's .resources container. No BAML conversion or object deserialization."
    )]
    Resources {
        input: PathBuf,
        /// Copy a root resources.at to enumerate that .resources container only.
        #[arg(long, value_name = "RESOURCE_AT")]
        within: Option<String>,
        /// Substring over resource names; folded unless --case sensitive.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Extract one embedded resource or .resources member as bytes.
    #[command(
        long_about = "Extract one resource selected by resources.at. stdout contains only the bytes; -o writes them atomically to FILE.",
        after_long_help = "Examples:\nrxd ilspy extract-resource app.dll --at 0x28000001 -o app.resources\nrxd ilspy extract-resource app.dll --at '0x28000001/item:0' -o item.bin\nrxd ilspy extract-resource app --entry App.dll --at 0x28000001 -o resource.bin\nRoot extraction preserves the entire original blob. Member strings emit their stored UTF-8 bytes; byte arrays/streams emit their original payload.\nPrimitives retain little-endian encoded bytes; user objects stay opaque serialized bytes. Type/length framing is removed, never deserialized.\nFor byte-exact container evidence extract the root. No implicit BAML-to-XAML conversion. This binary extractor is CLI-only."
    )]
    ExtractResource {
        input: PathBuf,
        #[arg(long, required = true, value_name = "RESOURCE_AT")]
        at: String,
        #[command(flatten)]
        options: Options,
    },
    /// List types (columns: at, full_name).
    #[command(
        long_about = "Columns: at, full_name\nList types. One # column header precedes the data rows.",
        after_long_help = "Examples:\nrxd ilspy classes app.dll -f JsonReader\nOutput: TAB-separated at, full_name; at is a TypeDef token, e.g. 0x0200000b.\nNext: rxd ilspy get-class app.dll --at '<class.at>' --raw.\n--class is an exact full type name, -f a substring. <Module> is not a fetchable type."
    )]
    Classes {
        /// One regular local managed PE DLL/EXE.
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
    /// List fields (columns: at, full_name).
    #[command(
        long_about = "Columns: at, full_name\nList fields. One # column header precedes the data rows.",
        after_long_help = "Examples:\nrxd ilspy fields app.dll --class Example.Client -f token\nOutput: TAB-separated at, full_name; at is a FieldDef token, e.g. 0x0400000b.\nNext: rxd ilspy xref app.dll --at '<field.at>' for explicit IL uses.\n--name is exact; -f matches field names, never declaring type names."
    )]
    Fields {
        /// One regular local managed PE DLL/EXE.
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
    /// List methods (columns: at, full_name).
    #[command(
        long_about = "Columns: at, full_name\nList methods. One # column header precedes the data rows.",
        after_long_help = "Examples:\nrxd ilspy methods app.dll --class Example.Client -f Send\nOutput: TAB-separated at, full_name; at is a MethodDef token, e.g. 0x0600000b.\nNext: rxd ilspy get-method app.dll --at '<method.at>' --raw.\n--name is exact and retains overloads; -f matches names, --class is one exact owner."
    )]
    Methods {
        /// One regular local managed PE DLL/EXE.
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
    /// Fetch one method as IL or C#.
    #[command(name = "get-method", after_long_help = "Examples:\nrxd ilspy methods app.dll -f Read\nrxd ilspy get-method app.dll --at '0x0600000b' --raw\nCopy methods.at or xref.context from this input. Token must have eight hex digits;\nnot a method name, method: prefix, table index or file hash.\nOutput: C# text; --raw prints the original IL as text. C# needs ILSpy.\nWithout the engine the IL text is printed with an explicit reason line.", mut_arg("at", |arg| arg.required(true).value_name("METHOD_TOKEN").help("Copy methods.at or xref.context: eight-digit MethodDef token, e.g. 0x0600000b")))]
    Get {
        /// One regular local managed PE DLL/EXE.
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        /// Print the original instruction listing instead of reconstructed source.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        options: Options,
    },
    /// Fetch one type and its nested types.
    #[command(name = "get-class", after_long_help = "Examples:\nrxd ilspy classes app.dll -f Client\nrxd ilspy get-class app.dll --at '0x0200000b' --raw\nCopy classes.at or a member's parent_at. TypeDef token, not a class name.\nOutput: C# text. --raw returns the metadata/IL inventory record.\nC# requires ILSpy and may explicitly fall back to IL.", mut_arg("at", |arg| arg.required(true).value_name("TYPE_TOKEN").help("Copy classes.at or member.parent_at: eight-digit TypeDef token, e.g. 0x0200000b")))]
    GetClass {
        /// One regular local managed PE DLL/EXE.
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        /// Print the original instruction listing instead of reconstructed source.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        options: Options,
    },
    /// List user-string heap literals (columns: at, value, context_at).
    #[command(
        long_about = "Columns: at, value, context_at\nList user-string heap literals. One # column header precedes the data rows.",
        after_long_help = "Examples:\nrxd ilspy strings app.dll -f Authorization\nOutput: TAB-separated at, value, context_at; at is a #US token, context_at an owning method.\nNext: rxd ilspy get-method app.dll --at '<string.context_at>' --raw,\nor rxd ilspy xref app.dll --at '<string.at>' for exact loads.\nUnreferenced strings have no context_at; equal values may be separate heap entries."
    )]
    Strings {
        /// One regular local managed PE DLL/EXE.
        input: PathBuf,
        ///  substring over the decoded literal. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Search member and type references.
    #[command(long_about = "Columns: context, match\nList explicit references as TAB-separated rows after one # column header. Successful zero-hit queries retain the header.", after_long_help = "Examples:\nrxd ilspy xref app.dll --at '0x0600000b'\nrxd ilspy xref app.dll -m Authorization --kind string\nRequired: exactly one of --at/--match; --match supports only --kind string.\nCopy member/class/string at tokens; --at literal text requires --kind string.\nOutput: TAB-separated context, match. context is the referencing MethodDef token; match is the resolved target token or exact string literal.\nNext: rxd ilspy get-method app.dll --at '<xref.context>' --raw.\nReferences are explicit IL operands, not reflection or runtime dispatch.", group(clap::ArgGroup::new("anchor").required(true).args(["at", "filter"])))]
    Xref {
        /// One regular local managed PE DLL/EXE.
        input: PathBuf,
        /// Exact metadata/#US token, including external members and generic instantiations.
        #[arg(long, value_name = "TOKEN", conflicts_with = "filter")]
        at: Option<String>,
        /// Substring of a decoded user-string literal; folded unless `--case sensitive`.
        #[arg(short = 'm', long = "match", value_name = "PATTERN", requires = "kind")]
        filter: Option<String>,
        /// Target kind: read off the token's table when it names one; required with
        /// `--match`, which can only search literals.
        #[arg(long, value_enum)]
        kind: Option<Kind>,
        /// Whether `--match` folds case: `insensitive` (default) or `sensitive`.
        #[arg(long, value_enum, default_value_t = crate::cli::filter::CaseMode::Insensitive)]
        case: crate::cli::filter::CaseMode,

        #[command(flatten)]
        options: Options,
    },
}

/// Infer kind from token table; literals and ambiguous MemberRef need an assertion.
type Kind = crate::cli::xref::Kind;

#[derive(Args)]
pub struct Options {
    /// Exact managed bundle path copied from entries.at; omit for a standalone assembly.
    #[arg(long, value_name = "PATH")]
    entry: Option<String>,
    /// Atomically write the result to FILE instead of stdout; never overwrite input.
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Write timing/resource JSONL to stderr without changing the result payload.
    #[arg(long)]
    perf: bool,
}

impl Options {
    /// The shared budget table, projected onto this backend's limits.
    fn limits(&self, budget: Budget) -> Limits {
        Limits {
            input_bytes: budget.input_bytes,
            work: budget.work,
            instructions: budget.instructions,
            metadata_bytes: budget.metadata_bytes,
            output_bytes: budget.output_bytes,
        }
    }
}

impl Command {
    fn options(&self) -> &Options {
        match self {
            Self::Entries { options, .. }
            | Self::ExtractEntry { options, .. }
            | Self::Resources { options, .. }
            | Self::ExtractResource { options, .. }
            | Self::Classes { options, .. }
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
            Self::Entries { .. } => "entries",
            Self::ExtractEntry { .. } => "extract-entry",
            Self::Resources { .. } => "resources",
            Self::ExtractResource { .. } => "extract-resource",
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

pub fn execute(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    sink: &mut super::Output<'_>,
) -> Result<()> {
    let emit_perf = command.perf();
    let mut perf = Perf::new(emit_perf, "analysis", "dotnet", command.name());
    let result = execute_inner(command, policy, &mut perf, sink).map_err(super::classify);
    super::finish(perf, emit_perf, result)
}

fn execute_inner(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    perf: &mut Perf,
    sink: &mut super::Output<'_>,
) -> anyhow::Result<()> {
    let (path, options) = match &command {
        Command::Entries { input, options, .. }
        | Command::ExtractEntry { input, options, .. }
        | Command::Resources { input, options, .. }
        | Command::ExtractResource { input, options, .. }
        | Command::Classes { input, options, .. }
        | Command::Fields { input, options, .. }
        | Command::Methods { input, options, .. }
        | Command::Get { input, options, .. }
        | Command::GetClass { input, options, .. }
        | Command::Strings { input, options, .. }
        | Command::Xref { input, options, .. } => (input, options),
    };
    files::validate_output(path, options.output.as_deref())?;
    let budget = policy.for_file(path)?;
    let mut limits = options.limits(budget);
    perf.resource_limits = Some(std::collections::BTreeMap::from([
        ("input_bytes", budget.input_bytes),
        ("metadata_bytes", budget.metadata_bytes),
    ]));
    for (name, limit) in [("work", budget.work), ("instructions", budget.instructions)] {
        if let Some(limit) = limit {
            perf.resource_limits.as_mut().unwrap().insert(name, limit);
        }
    }
    if let Some(output_bytes) = budget.output_bytes {
        perf.resource_limits
            .as_mut()
            .unwrap()
            .insert("output_bytes", output_bytes);
    }
    let bytes = perf.measure("read", || rxd_dotnet::read_file(path, limits))?;
    let bundle_query = matches!(
        command,
        Command::Entries { .. } | Command::ExtractEntry { .. }
    );
    let module = if bundle_query {
        anyhow::ensure!(
            options.entry.is_none(),
            "INVALID_QUERY: entries/extract-entry do not accept --entry; use extract-entry --at PATH"
        );
        std::borrow::Cow::Borrowed(bytes.as_slice())
    } else {
        perf.measure("select", || {
            rxd_dotnet::select_module(&bytes, options.entry.as_deref(), limits)
        })?
    };
    if !bundle_query && matches!(policy, rxd_core::BudgetPolicy::InputScaled) {
        limits = options.limits(Budget::for_input_bytes(module.len())?);
        perf.resource_limits
            .as_mut()
            .unwrap()
            .insert("metadata_bytes", limits.metadata_bytes);
    }
    anyhow::ensure!(
        module.len() <= limits.input_bytes,
        "RESOURCE_LIMIT: selected module bytes"
    );
    perf.counters.input_bytes = Some(bytes.len());
    if files::execute(&command, &module, limits, perf, sink)? {
        return Ok(());
    }
    let mut input = perf.measure("read_parse", || Input::parse(&module, limits))?;
    perf.source_id = Some(input.source_id.clone());
    perf.counters.input_bytes = Some(bytes.len());
    perf.counters.entities = Some(input.types.len() + input.fields.len() + input.methods.len());
    let query_start = perf.start_stage();
    let mut output = rxd_core::output::Output::new(budget.output_bytes);
    let mut records = 0;
    // One target, resolved before the scan: the same two faces every backend takes, and a
    // token is the only thing .NET can match on exactly.
    let xref_target = if let Command::Xref {
        at,
        filter,
        kind,
        case,
        ..
    } = &command
    {
        Some(
            dotnet_target(at.as_deref(), filter.as_deref(), *kind, *case)
                .map_err(super::classify)?,
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
    if let Command::Get { selector, raw, .. } = &command {
        let at = selector.resolve()?;
        // FETCH resolves and decodes only the selected method; unrelated signatures
        // are not parsed on this path.
        let token = input.method_by_id(&at)?.token;
        let body = perf.measure("decode", || input.body_token(token))?;
        let method = input.method_by_id(&at)?;
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
        if !*raw {
            // A reconstruction is the answer. Without the engine there is no C#, so
            // fall back to the original IL text rather than printing nothing.
            match decompilation.code.as_deref() {
                Some(source) if !source.trim().is_empty() => output.write_all(source.as_bytes())?,
                _ => {
                    let reason = decompilation.reason.as_deref().unwrap_or("no engine");
                    output.write_all(
                        format!("// reconstruction unavailable: {reason}\n").as_bytes(),
                    )?;
                    write_il_text(&mut output, &body)?;
                }
            }
            perf.record("query", query_start);
            perf.counters.records = Some(1);
            perf.counters.output_bytes = Some(output.len());
            perf.measure("write", || {
                super::write_output(&output, options.output.as_deref(), sink)
            })?;
            return Ok(());
        }
        // Original IL is the answer for `--raw`: one line per instruction with the
        // code attributes as comment lines, never a record wrapper.
        let mut text = String::new();
        text.push_str(&format!(
            "// max_stack={} init_locals={} code_offset={} code_size={}\n",
            body.max_stack, body.init_locals, body.code_offset, body.code_size
        ));
        for instruction in &body.instructions {
            text.push_str(&format!(
                "{:04x}\t{}",
                instruction.offset, instruction.mnemonic
            ));
            if let Some(operand) = il_operand_text(&instruction.operand) {
                text.push(' ');
                text.push_str(&operand);
            }
            text.push('\n');
        }
        for handler in &body.handlers {
            text.push_str(&format!("// handler {handler:?}\n"));
        }
        output.write_all(text.as_bytes())?;
    } else if let Command::GetClass { selector, raw, .. } = &command {
        let at = selector.resolve()?;
        let root = perf.measure("prepare", || input.prepare_class(&at))?;
        if *raw {
            input.prepare_class_fields(root)?;
        }
        let class = perf.measure("decode", || input.class_body(root))?;
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
        if !*raw {
            // A reconstruction is the answer; without an engine the member IL is, so a
            // class fetch never succeeds with no output. The record stays on --raw.
            let source = rendered.code.as_deref().unwrap_or_default();
            if source.trim().is_empty() {
                let reason = rendered.reason.as_deref().unwrap_or("no engine");
                output
                    .write_all(format!("// reconstruction unavailable: {reason}\n").as_bytes())?;
                for item in &class.methods {
                    let Some(body) = &item.body else {
                        continue;
                    };
                    output.write_all(
                        format!(
                            "// method 0x{:08x} {}::{}\n",
                            item.method.token, item.method.declaring_type, item.method.name
                        )
                        .as_bytes(),
                    )?;
                    write_il_text(&mut output, body)?;
                }
            } else {
                output.write_all(source.as_bytes())?;
            }
            perf.record("query", query_start);
            perf.counters.records = Some(1);
            perf.counters.output_bytes = Some(output.len());
            perf.measure("write", || {
                super::write_output(&output, options.output.as_deref(), sink)
            })?;
            return Ok(());
        }
        let mut payload = detail(
            json!({"work_used":class.work_used,"representation":rendered.representation,"decompiled":rendered.decompiled,"reason":rendered.reason,"reason_detail":rendered.reason.as_deref().map(|reason| describe_class_reason(reason, class.methods.len())),"warnings":rendered.warnings,"code":rendered.code,"mapping":rendered.mapping,"complete":true}),
        );
        if *raw {
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
                    relationship_record(
                        &input,
                        "property",
                        &coordinate(property.declaring_type_token),
                        property.token,
                        *property,
                    )
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let events = class
                .events
                .iter()
                .map(|event| {
                    relationship_record(
                        &input,
                        "event",
                        &coordinate(event.declaring_type_token),
                        event.token,
                        *event,
                    )
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let methods = class
                .methods
                .iter()
                .map(|item| class_method_record(&input, item, *raw))
                .collect::<anyhow::Result<Vec<_>>>()?;
            payload.insert("type".into(), type_record(&input, class.root)?);
            for (key, records) in [
                ("types", types),
                ("fields", fields),
                ("properties", properties),
                ("events", events),
                ("methods", methods),
            ] {
                payload.insert(key.into(), Value::Array(records));
            }
        }
        // Nested inventories carry the same entity references as the top level.
        let value = fetch_record(
            "class_body",
            command.name(),
            RecordMeta {
                at: Some(&at),
                parent_at: class.root.declaring_type_token.map(coordinate).as_deref(),
                ..metadata(&input)
            },
            payload,
        );
        output.json_line(&value)?;
    } else {
        match &command {
            Command::Classes { .. } | Command::Fields { .. } | Command::Methods { .. } => {
                output.write_all(crate::cli::table::header(&["at", "full_name"]).as_bytes())?;
                list(&mut input, &command, &mut output, &mut records)?;
            }
            Command::Strings { filter, case, .. } => {
                output.write_all(
                    crate::cli::table::header(&["at", "value", "context_at"]).as_bytes(),
                )?;
                let uses = perf.measure("strings", || {
                    input.string_uses(filter.as_deref(), case.sensitive())
                })?;
                let mut rows: Vec<String> = Vec::new();
                for group in &uses.groups {
                    let literal = &uses.literals[group.literal_index];
                    // One row per load site: `at` names the heap literal, `context_at`
                    // names the method that loads it, so equal values stay distinct.
                    let context = group.method_token.map(coordinate);
                    let at = format!("0x{:08x}", 0x7000_0000u32 | literal.offset);
                    rows.push(crate::cli::table::line(&[
                        crate::cli::table::Cell::Atom(&at),
                        crate::cli::table::Cell::Text(literal.value.as_deref().unwrap_or_default()),
                        match context.as_deref() {
                            Some(value) => crate::cli::table::Cell::Atom(value),
                            None => crate::cli::table::Cell::Null,
                        },
                    ])?);
                }
                emit_table(&mut output, &mut records, rows)?;
            }
            Command::Xref { .. } => {
                output.write_all(crate::cli::table::header(&["context", "match"]).as_bytes())?;
                if let Some(result) = reference_result.take() {
                    for reference in result.references {
                        let row = reference_line(
                            &input,
                            &reference.site,
                            reference.target.as_deref(),
                            None,
                        )?;
                        output.write_all(row.as_bytes())?;
                        records += 1;
                    }
                } else {
                    // Build literal inventory once; token and text anchors resolve to the same literal.
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
                        let row = reference_line(&input, &site, None, literal.map(String::as_str))?;
                        output.write_all(row.as_bytes())?;
                        records += 1;
                    }
                }
            }
            Command::Get { .. }
            | Command::GetClass { .. }
            | Command::Entries { .. }
            | Command::ExtractEntry { .. }
            | Command::Resources { .. }
            | Command::ExtractResource { .. } => unreachable!(),
        }
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
        super::write_output(&output, options.output.as_deref(), sink)
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
    envelope::attach_protocol(&mut value, "dotnet", command);
    value
}

fn reference_line(
    input: &Input<'_>,
    site: &rxd_dotnet::TokenSite,
    target: Option<&rxd_dotnet::SemanticTarget>,
    literal: Option<&str>,
) -> anyhow::Result<String> {
    use crate::cli::table::{self, Cell};
    let method = &input.methods[(site.method_token & 0xffffff) as usize - 1];
    let context = coordinate(method.token);
    let matched = literal
        .map(str::to_owned)
        .or_else(|| {
            target
                .and_then(|target| target.canonical_token)
                .map(coordinate)
        })
        .unwrap_or_else(|| format!("0x{:08x}", site.target_token));
    table::line(&[Cell::Atom(&context), Cell::Text(&matched)])
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
fn literal_values(input: &Input<'_>) -> BTreeMap<u32, String> {
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

fn engine_state() -> &'static str {
    if cfg!(feature = "bundled-ilspy") {
        "bundled engine failed to load"
    } else {
        "built without the bundled engine"
    }
}

/// Annotate every metadata-token operand with the member it provably denotes.
/// IL immediate operands as they appear in the original listing.
fn il_operand_text(operand: &rxd_dotnet::Operand) -> Option<String> {
    use rxd_dotnet::Operand;
    Some(match operand {
        Operand::None => return None,
        Operand::I32(value) => value.to_string(),
        Operand::I64(value) => value.to_string(),
        Operand::F32Bits(bits) => format!("0x{bits:08x}"),
        Operand::F64Bits(bits) => format!("0x{bits:016x}"),
        Operand::Variable(index) => format!("v{index}"),
        Operand::Token(token) => format!("0x{token:08x}"),
        Operand::Branch(target) => format!("->{target:04x}"),
        Operand::Switch(targets) => {
            let list: Vec<String> = targets.iter().map(|t| format!("{t:04x}")).collect();
            format!("[{}]", list.join(" "))
        }
    })
}

fn coordinate(token: u32) -> String {
    format!("0x{token:08x}")
}

fn method_record(input: &Input<'_>, method: &rxd_dotnet::Method) -> anyhow::Result<Value> {
    let fields = detail(serde_json::to_value(method)?);
    let record_at = coordinate(method.token);
    Ok(envelope::record(
        "method",
        RecordMeta {
            at: Some(&record_at),
            parent_at: Some(&coordinate(method.declaring_type_token)),
            ..metadata(input)
        },
        fields,
    ))
}

fn type_record(input: &Input<'_>, ty: &rxd_dotnet::TypeInfo) -> anyhow::Result<Value> {
    let mut fields = detail(serde_json::to_value(ty)?);
    fields.remove("id");

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
            at: Some(&coordinate(ty.token)),
            parent_at: ty.declaring_type_token.map(coordinate).as_deref(),
            ..metadata(input)
        },
        fields,
    ))
}

fn field_record(input: &Input<'_>, field: &rxd_dotnet::Field) -> anyhow::Result<Value> {
    let fields = detail(serde_json::to_value(field)?);
    // Nested references are copy-only coordinates, exactly as in `methods`/`classes`.
    let record_at = coordinate(field.token);
    Ok(envelope::record(
        "field",
        RecordMeta {
            at: Some(&record_at),
            parent_at: Some(&coordinate(field.declaring_type_token)),
            ..metadata(input)
        },
        fields,
    ))
}

fn class_method_record(
    input: &Input<'_>,
    item: &rxd_dotnet::ClassMethod<'_>,
    raw: bool,
) -> anyhow::Result<Value> {
    let body = item.body.as_ref();
    let mut fields = match (body, raw) {
        (Some(body), true) => detail(serde_json::to_value(body)?),
        (Some(body), false) => detail(json!({
            "work_used": body.work_used,
            "file_offset": body.file_offset,
            "code_offset": body.code_offset,
            "code_size": body.code_size,
        })),
        (None, true) => detail(
            json!({"work_used":null,"file_offset":null,"code_offset":null,"code_size":null,"max_stack":null,"init_locals":null,"local_signature_token":null,"locals":null,"instructions":null,"handlers":null,"il_hex":null}),
        ),
        (None, false) => {
            detail(json!({"work_used":null,"file_offset":null,"code_offset":null,"code_size":null}))
        }
    };
    fields.insert("method".into(), method_record(input, item.method)?);
    fields.insert("body_present".into(), json!(body.is_some()));
    fields.insert("complete".into(), json!(true));
    fields.insert("representation".into(), json!("il"));
    fields.insert("decompiled".into(), json!(false));
    fields.insert("reason".into(), json!("member_inventory"));
    fields.insert("code".into(), Value::Null);
    fields.insert("mapping".into(), json!([]));
    let record_at = coordinate(item.method.token);
    Ok(envelope::record(
        "method_body",
        RecordMeta {
            at: Some(&record_at),
            parent_at: Some(&coordinate(item.method.declaring_type_token)),
            start: body.map(|body| body.code_offset),
            end: body.map(|body| body.code_offset + body.code_size),
            ..metadata(input)
        },
        fields,
    ))
}

fn relationship_record<T: serde::Serialize>(
    input: &Input<'_>,
    kind: &str,
    owner: &str,
    token: u32,
    relationship: &T,
) -> anyhow::Result<Value> {
    // Properties/events are reached through accessor methods, not FETCH coordinates.
    // Keep their tokens as evidence and their owner as parent_at.
    let _ = token;
    Ok(envelope::record(
        kind,
        RecordMeta {
            parent_at: Some(owner),
            ..metadata(input)
        },
        // Nested references are copy-only coordinates, as in every other listing.
        detail(serde_json::to_value(relationship)?),
    ))
}

/// Per-row metadata: identity is carried once by the stream header, as in DEX and the
/// JVM backend, so a row does not repeat the assembly's content id.
fn metadata(_input: &Input<'_>) -> RecordMeta<'static> {
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
    input: &mut Input<'_>,
    command: &Command,
    output: &mut rxd_core::output::Output,
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

    let matches = |name: &str| pattern.as_ref().is_none_or(|pattern| pattern.matches(name));
    // Resolve signatures only for the rows this query can emit. Filtering by name and
    // declaring type never needs them, so unrelated corrupt members stay unread.
    let selected_classes: Option<std::collections::BTreeSet<u32>> = class.as_ref().map(|wanted| {
        input
            .types
            .iter()
            .filter(|ty| ty.full_name.as_ref() == wanted)
            .map(|ty| ty.token)
            .collect()
    });
    let owner_selected = |token: u32| {
        selected_classes
            .as_ref()
            .is_none_or(|selected| selected.contains(&token))
    };
    let selected: Vec<usize> = match listing {
        Listing::Fields => input
            .fields
            .iter()
            .enumerate()
            .filter(|(_, field)| {
                owner_selected(field.declaring_type_token)
                    && name.is_none_or(|wanted| wanted == &field.name)
                    && matches(&field.name)
            })
            .map(|(index, _)| index)
            .collect(),
        Listing::Methods => input
            .methods
            .iter()
            .enumerate()
            .filter(|(_, method)| {
                owner_selected(method.declaring_type_token)
                    && name.is_none_or(|wanted| wanted == &method.name)
                    && matches(&method.name)
            })
            .map(|(index, _)| index)
            .collect(),
        Listing::Classes => Vec::new(),
    };
    let mut selected_by_type: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    let mut rows: Vec<String> = Vec::new();
    // Resolve only selected members, in original table order. Retain their indices
    // for output instead of grouping and filtering the entire table a second time.
    for index in selected {
        let owner = match listing {
            Listing::Fields => {
                input.resolve_field(index)?;
                input.fields[index].declaring_type_token
            }
            Listing::Methods => {
                input.resolve_method(index)?;
                input.methods[index].declaring_type_token
            }
            Listing::Classes => unreachable!("class listings have no selected members"),
        };
        selected_by_type.entry(owner).or_default().push(index);
    }
    for ty in &input.types {
        if class
            .as_ref()
            .is_some_and(|name| name != ty.full_name.as_ref())
        {
            continue;
        }
        if listing == Listing::Classes {
            if matches(&ty.full_name) {
                rows.push(crate::cli::table::line(&[
                    crate::cli::table::Cell::Atom(&coordinate(ty.token)),
                    crate::cli::table::Cell::Text(ty.full_name.as_ref()),
                ])?);
            }
            continue;
        }
        for &index in selected_by_type.get(&ty.token).into_iter().flatten() {
            rows.push(match listing {
                Listing::Fields => {
                    let field = &input.fields[index];
                    let name = format!(
                        "{}::{}: {}",
                        field.declaring_type, field.name, field.signature
                    );
                    crate::cli::table::line(&[
                        crate::cli::table::Cell::Atom(&coordinate(field.token)),
                        crate::cli::table::Cell::Text(&name),
                    ])?
                }
                Listing::Methods => {
                    let method = &input.methods[index];
                    let name = format!(
                        "{}::{}({}): {}",
                        method.declaring_type,
                        method.name,
                        method.parameters.join(", "),
                        method.return_type
                    );
                    crate::cli::table::line(&[
                        crate::cli::table::Cell::Atom(&coordinate(method.token)),
                        crate::cli::table::Cell::Text(&name),
                    ])?
                }
                Listing::Classes => unreachable!("handled above"),
            });
        }
    }
    emit_table(output, records, rows)
}

fn detail(value: Value) -> Map<String, Value> {
    let Value::Object(fields) = value else {
        unreachable!(".NET record detail is a statically defined object")
    };
    fields
}

/// Emit one listing as Unix-table lines: one record per line, TAB-separated.
fn emit_table(
    output: &mut rxd_core::output::Output,
    records: &mut usize,
    rows: Vec<String>,
) -> anyhow::Result<()> {
    use std::io::Write as _;
    *records = rows.len();
    for row in rows {
        output.write_all(row.as_bytes())?;
    }
    Ok(())
}

/// One metadata-token or decoded-literal reference target.
enum Target {
    /// A metadata or `#US` token: an identity, matched exactly.
    Token { token: u32 },
    /// A decoded user-string literal: an exact anchor, or a substring search.
    Literal {
        pattern: String,
        exact: bool,
        case_sensitive: bool,
    },
}

/// Tokens identify file-local rows; --kind validates their table, not their spelling.
fn dotnet_target(
    at: Option<&str>,
    filter: Option<&str>,
    kind: Option<Kind>,
    case: crate::cli::filter::CaseMode,
) -> anyhow::Result<Target> {
    let faces = usize::from(at.is_some()) + usize::from(filter.is_some());
    anyhow::ensure!(
        faces == 1,
        "INVALID_QUERY: exactly one reference target is required (`--at TOKEN` or `-f \
         PATTERN --kind string`)"
    );
    if let Some(pattern) = filter {
        anyhow::ensure!(
            kind == Some(crate::cli::xref::Kind::String),
            "INVALID_QUERY: a .NET member or type identity is a metadata token, not a name, so \
             `--match` can only search literals: give `--kind string`, or anchor with `--at TOKEN"
        );
        return Ok(Target::Literal {
            pattern: pattern.to_owned(),
            exact: false,
            case_sensitive: case.is_sensitive(),
        });
    }
    let coordinate = at.expect("checked above").to_owned();
    // Parse tokens before literal fallback so a contradictory kind remains an error.
    if kind == Some(crate::cli::xref::Kind::String) {
        return match parse_token(&coordinate) {
            Ok(token) => {
                let inferred = token_kind(token);
                if token >> 24 != 0x70 || inferred != Some(crate::cli::xref::Kind::String) {
                    anyhow::bail!(
                        "INVALID_QUERY: --at {coordinate} is a {} token, but --kind string was given",
                        inferred.map_or("non-string", |kind| kind.as_str())
                    );
                }
                Ok(Target::Token { token })
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
            "INVALID_QUERY: --at {coordinate} is a {} token, but --kind {} was given",
            inferred.as_str(),
            given.as_str()
        );
    }
    Ok(Target::Token { token })
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

/// Original IL as text: one line per instruction, attributes as comment lines.
fn write_il_text(
    output: &mut rxd_core::output::Output,
    body: &rxd_dotnet::MethodBody,
) -> anyhow::Result<()> {
    let mut text = String::new();
    text.push_str(&format!(
        "// max_stack={} init_locals={} code_offset={} code_size={}\n",
        body.max_stack, body.init_locals, body.code_offset, body.code_size
    ));
    for instruction in &body.instructions {
        text.push_str(&format!(
            "{:04x}\t{}",
            instruction.offset, instruction.mnemonic
        ));
        if let Some(operand) = il_operand_text(&instruction.operand) {
            text.push(' ');
            text.push_str(&operand);
        }
        text.push('\n');
    }
    for handler in &body.handlers {
        text.push_str(&format!("// handler {handler:?}\n"));
    }
    output.write_all(text.as_bytes())?;
    Ok(())
}
