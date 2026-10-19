use std::path::PathBuf;

use clap::{Args, Subcommand};
use rxd_core::envelope::{RecordMeta, SCHEMA_VERSION, header, record, summary};
use rxd_core::{Error, Result};
use rxd_jvm::{Input, Limits};
use serde_json::json;

#[derive(Subcommand)]
pub enum Command {
    /// Enumerate selected class, field and method metadata as JSONL.
    Outline {
        input: PathBuf,
        /// Exact internal, dotted or descriptor class name.
        #[arg(long)]
        class: Option<String>,
        /// Exact method name; preserves overloads and their descriptors.
        #[arg(long)]
        method: Option<String>,
        #[command(flatten)]
        options: Options,
    },
    /// Extract one content-bound method as Java or original JVM instructions.
    #[command(name = "get-method")]
    Get {
        input: PathBuf,
        id: String,
        #[arg(long)]
        bytecode: bool,
        #[command(flatten)]
        options: Options,
    },
    /// Decompile a whole class to Java by its internal, dotted or descriptor name.
    #[command(name = "get-class")]
    GetClass {
        input: PathBuf,
        class: String,
        #[command(flatten)]
        options: Options,
    },
    /// List string literals from every selected class constant pool as JSONL.
    Strings {
        input: PathBuf,
        /// Exact internal, dotted or descriptor class name to scope the listing.
        #[arg(long)]
        class: Option<String>,
        /// Only strings containing this text, matched case-insensitively.
        #[arg(long)]
        filter: Option<String>,
        #[command(flatten)]
        options: Options,
    },
    /// Find code references to a string, type, method or field across all classes.
    Findrefs {
        input: PathBuf,
        #[command(subcommand)]
        kind: RefKind,
        #[command(flatten)]
        options: Options,
    },
}

#[derive(Subcommand)]
pub enum RefKind {
    /// Instructions loading a string literal containing VALUE (case-sensitive substring).
    String { value: String },
    /// Instructions referencing a type whose name contains NAME (case-sensitive substring).
    Type { name: String },
    /// Call sites of a method named NAME, optionally within owner classes matching --class.
    Method {
        name: String,
        /// Owner class substring filter (case-sensitive).
        #[arg(long)]
        class: Option<String>,
    },
    /// Access sites of a field named NAME, optionally within owner classes matching --class.
    Field {
        name: String,
        /// Owner class substring filter (case-sensitive).
        #[arg(long)]
        class: Option<String>,
    },
}

impl RefKind {
    fn query_json(&self) -> serde_json::Value {
        match self {
            Self::String { value } => json!({"kind":"string","value":value}),
            Self::Type { name } => json!({"kind":"type","name":name}),
            Self::Method { name, class } => json!({"kind":"method","name":name,"class":class}),
            Self::Field { name, class } => json!({"kind":"field","name":name,"class":class}),
        }
    }
}

#[derive(Args)]
pub struct Options {
    /// JVM release used to select multi-release JAR entries.
    #[arg(long, default_value_t = 8)]
    release: u16,
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Input byte cap in bytes (1 through 8589934592); 8 GiB when omitted.
    #[arg(long, value_parser = super::input_bytes)]
    max_input_bytes: Option<usize>,
    #[arg(long, default_value_t = 16 * 1024 * 1024, value_parser = super::positive)]
    max_class_bytes: usize,
    #[arg(long, default_value_t = 128 * 1024 * 1024, value_parser = super::positive)]
    max_expanded_bytes: usize,
    #[arg(long, default_value_t = 10_000, value_parser = super::positive)]
    max_entries: usize,
    #[arg(long, default_value_t = 1_000_000, value_parser = super::positive)]
    max_work: usize,
    #[arg(long, default_value_t = 32 * 1024 * 1024, value_parser = super::positive)]
    max_output_bytes: usize,
}

impl Options {
    fn limits(&self) -> Limits {
        Limits {
            input_bytes: super::input_limit(self.max_input_bytes),
            class_bytes: self.max_class_bytes,
            expanded_bytes: self.max_expanded_bytes,
            entries: self.max_entries,
            work: self.max_work,
            output_bytes: self.max_output_bytes,
        }
    }
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Outline { .. } => "outline",
            Self::Get { .. } => "get-method",
            Self::GetClass { .. } => "get-class",
            Self::Strings { .. } => "strings",
            Self::Findrefs { .. } => "findrefs",
        }
    }
}

pub fn execute(command: Command) -> Result<()> {
    let result = execute_inner(command);
    result.map_err(|error| {
        let message = format!("{error:#}");
        let code = [
            "RESOURCE_LIMIT",
            "INVALID_QUERY",
            "INVALID_INPUT",
            "UNSUPPORTED_INPUT",
            "UNSUPPORTED_BYTECODE",
            "DECOMPILE_ERROR",
        ]
        .into_iter()
        .find(|code| message.contains(code))
        .unwrap_or("IO_ERROR");
        Error::new(code, message)
    })
}

fn execute_inner(command: Command) -> anyhow::Result<()> {
    if let Command::GetClass {
        input,
        class,
        options,
    } = command
    {
        let limits = options.limits();
        let input = Input::read(&input, limits, options.release)?;
        let source = rxd_jvm::render_class(&input, &class, limits)?;
        anyhow::ensure!(
            source.len() <= limits.output_bytes,
            "RESOURCE_LIMIT: output byte budget exceeded"
        );
        super::write_output(source.as_bytes(), options.output.as_deref())?;
        return Ok(());
    }
    if let Command::Strings {
        input,
        class,
        filter,
        options,
    } = command
    {
        let limits = options.limits();
        let input = Input::read(&input, limits, options.release)?;
        let wanted = class.as_deref().map(|name| {
            name.strip_prefix('L')
                .and_then(|n| n.strip_suffix(';'))
                .unwrap_or(name)
                .replace('.', "/")
        });
        let needle = filter.as_deref().map(str::to_lowercase);
        let mut output = Vec::new();
        append(
            &mut output,
            &header(
                "jvm",
                "strings",
                Some(&input.source_id),
                Some(input.file_bytes),
                json!({"class":class,"filter":filter}),
                json!({"analysis":"metadata","release":options.release})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            limits.output_bytes,
        )?;
        let mut records = 0usize;
        for selected in &input.classes {
            if wanted
                .as_ref()
                .is_some_and(|name| *name != selected.class.name)
            {
                continue;
            }
            for (index, value) in selected.class.string_constants() {
                let text = value.to_string_lossy();
                if let Some(needle) = &needle
                    && !text.to_lowercase().contains(needle)
                {
                    continue;
                }
                // Lossless escape: when decoding is lossy (unpaired surrogates),
                // keep the raw UTF-16 code units so no consumer loses data.
                let lossless = String::from_utf16(value.as_utf16()).is_ok();
                let utf16: Option<Vec<u16>> = (!lossless).then(|| value.as_utf16().to_vec());
                append(
                    &mut output,
                    &record(
                        "string",
                        RecordMeta {
                            source_id: Some(&selected.source_id),
                            ..RecordMeta::default()
                        },
                        json!({
                            "entry":selected.entry,
                            "class":selected.class.name,
                            "index":index,
                            "value":text,
                            "utf16":utf16,
                        })
                        .as_object()
                        .unwrap()
                        .clone(),
                    ),
                    limits.output_bytes,
                )?;
                records += 1;
                anyhow::ensure!(
                    records <= limits.entries,
                    "RESOURCE_LIMIT: record budget exceeded"
                );
            }
        }
        append(
            &mut output,
            &summary(records, true, serde_json::Map::new()),
            limits.output_bytes,
        )?;
        super::write_output(&output, options.output.as_deref())?;
        return Ok(());
    }
    if let Command::Findrefs {
        input,
        kind,
        options,
    } = command
    {
        let limits = options.limits();
        let input = Input::read(&input, limits, options.release)?;
        let query = match &kind {
            RefKind::String { value } => rxd_jvm::RefQuery::String(value),
            RefKind::Type { name } => rxd_jvm::RefQuery::Type(name),
            RefKind::Method { name, class } => rxd_jvm::RefQuery::Method {
                name,
                class: class.as_deref(),
            },
            RefKind::Field { name, class } => rxd_jvm::RefQuery::Field {
                name,
                class: class.as_deref(),
            },
        };
        let hits = rxd_jvm::find_refs(&input, &query, limits)?;
        let mut output = Vec::new();
        append(
            &mut output,
            &header(
                "jvm",
                "findrefs",
                Some(&input.source_id),
                Some(input.file_bytes),
                kind.query_json(),
                json!({"analysis":"metadata","release":options.release})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            limits.output_bytes,
        )?;
        for hit in &hits {
            append(
                &mut output,
                &record(
                    "reference",
                    RecordMeta {
                        source_id: Some(&hit.source_id),
                        context_id: Some(&hit.method_id),
                        ..RecordMeta::default()
                    },
                    json!({
                        "entry":hit.entry,
                        "class":hit.class,
                        "method":hit.method,
                        "method_descriptor":hit.method_descriptor,
                        "byte_offset":hit.byte_offset,
                        "mnemonic":hit.mnemonic,
                        "target":hit.target,
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
                limits.output_bytes,
            )?;
        }
        append(
            &mut output,
            &summary(hits.len(), true, serde_json::Map::new()),
            limits.output_bytes,
        )?;
        super::write_output(&output, options.output.as_deref())?;
        return Ok(());
    }
    let (path, options, request, class_filter, method_filter) = match command {
        Command::Outline {
            input,
            options,
            class,
            method,
        } => (input, options, None, class, method),
        Command::Get {
            input,
            id,
            bytecode,
            options,
        } => (input, options, Some((id, bytecode)), None, None),
        Command::GetClass { .. } => unreachable!("handled above"),
        Command::Strings { .. } => unreachable!("handled above"),
        Command::Findrefs { .. } => unreachable!("handled above"),
    };
    let limits = options.limits();
    let input = Input::read(&path, limits, options.release)?;
    let mut output = Vec::new();
    if let Some((id, raw)) = request {
        if raw {
            let (selected, index) = input.method_by_id(&id)?;
            let method = &selected.class.methods[index];
            let code = method
                .code
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("INVALID_QUERY: method has no Code attribute"))?;
            let mut bytecode = record(
                "bytecode",
                RecordMeta {
                    id: Some(&id),
                    source_id: Some(&selected.source_id),
                    start: Some(code.range.start),
                    end: Some(code.range.end),
                    ..RecordMeta::default()
                },
                json!({
                    "representation":"bytecode",
                    "entry":selected.entry,
                    "class":selected.class.name,
                    "name":method.name,
                    "descriptor":method.descriptor,
                    "max_stack":code.max_stack,
                    "max_locals":code.max_locals,
                    "handlers":code.handlers,
                    "instructions":rxd_jvm::bytecode(&selected.class, method)?,
                    "complete":true,
                })
                .as_object()
                .unwrap()
                .clone(),
            );
            // Single-record FETCH has no header; keep protocol identity outside detail.
            bytecode["backend"] = json!("jvm");
            bytecode["command"] = json!("get-method");
            bytecode["schema_version"] = json!(SCHEMA_VERSION);
            append(&mut output, &bytecode, limits.output_bytes)?;
        } else {
            let source = rxd_jvm::render_method(&input, &id, limits)?;
            anyhow::ensure!(
                source.len() <= limits.output_bytes,
                "RESOURCE_LIMIT: output byte budget exceeded"
            );
            output.extend_from_slice(source.as_bytes());
        }
    } else {
        let class_name = class_filter.as_deref().map(|name| {
            name.strip_prefix('L')
                .and_then(|n| n.strip_suffix(';'))
                .unwrap_or(name)
                .replace('.', "/")
        });
        let selected_classes: Vec<_> = input
            .classes
            .iter()
            .filter(|item| {
                class_name
                    .as_ref()
                    .is_none_or(|name| *name == item.class.name)
            })
            .collect();
        append(
            &mut output,
            &header(
                "jvm",
                "outline",
                Some(&input.source_id),
                Some(input.file_bytes),
                json!({"class":class_filter,"method":method_filter}),
                json!({"analysis":"metadata","release":options.release})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            limits.output_bytes,
        )?;
        let mut records = 0usize;
        for selected in &selected_classes {
            let class = &selected.class;
            append(
                &mut output,
                &record(
                    "class",
                    RecordMeta {
                        source_id: Some(&selected.source_id),
                        ..RecordMeta::default()
                    },
                    json!({
                        "entry":selected.entry,
                        "name":class.name,
                        "super":class.super_name,
                        "interfaces":class.interfaces,
                        "major":class.major,
                        "minor":class.minor,
                        "access":class.access,
                        "signature":class.signature,
                        "source_file":class.source_file,
                        "fields":class.fields.len(),
                        "methods":class.methods.len(),
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
                limits.output_bytes,
            )?;
            records += 1;
            for field in &class.fields {
                if method_filter.is_some() {
                    break;
                }
                append(
                    &mut output,
                    &record(
                        "field",
                        RecordMeta {
                            source_id: Some(&selected.source_id),
                            ..RecordMeta::default()
                        },
                        json!({
                            "entry":selected.entry,
                            "class":class.name,
                            "name":field.name,
                            "descriptor":field.descriptor,
                            "access":field.access,
                            "signature":field.signature,
                            "constant_index":field.constant_index,
                        })
                        .as_object()
                        .unwrap()
                        .clone(),
                    ),
                    limits.output_bytes,
                )?;
                records += 1;
            }
            for (index, method) in class.methods.iter().enumerate() {
                if method_filter
                    .as_ref()
                    .is_some_and(|name| *name != method.name)
                {
                    continue;
                }
                append(
                    &mut output,
                    &record(
                        "method",
                        RecordMeta {
                            id: Some(&Input::method_id(selected, index)),
                            source_id: Some(&selected.source_id),
                            ..RecordMeta::default()
                        },
                        json!({
                            "entry":selected.entry,
                            "class":class.name,
                            "name":method.name,
                            "descriptor":method.descriptor,
                            "access":method.access,
                            "signature":method.signature,
                            "exceptions":method.exceptions,
                            "has_code":method.code.is_some(),
                        })
                        .as_object()
                        .unwrap()
                        .clone(),
                    ),
                    limits.output_bytes,
                )?;
                records += 1;
            }
        }
        append(
            &mut output,
            &summary(
                records,
                true,
                json!({"classes":selected_classes.len()})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            limits.output_bytes,
        )?;
    }
    super::write_output(&output, options.output.as_deref())?;
    Ok(())
}

fn append(output: &mut Vec<u8>, record: &serde_json::Value, limit: usize) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(record)?;
    anyhow::ensure!(
        bytes.len() < limit.saturating_sub(output.len()),
        "RESOURCE_LIMIT: output byte budget exceeded"
    );
    output.extend_from_slice(&bytes);
    output.push(b'\n');
    Ok(())
}
