use std::path::PathBuf;

use clap::{Args, Subcommand};
use rxd_core::envelope::{RecordMeta, SCHEMA_VERSION, header, record, summary};
use rxd_core::{Error, Result};
use rxd_jvm::{Input, Limits};
use serde_json::json;

#[derive(Subcommand)]
pub enum Command {
    /// List the class metadata an archive defines as JSONL.
    Classes {
        input: PathBuf,
        /// Exact internal, dotted or descriptor class name.
        #[arg(long)]
        class: Option<String>,
        /// Case-insensitive substring over the internal class name; dots are accepted.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        options: Options,
    },
    /// List the entries an archive holds, in central-directory order.
    Entries {
        input: PathBuf,
        #[command(flatten)]
        options: Options,
    },
    /// List the fields an archive declares as JSONL.
    Fields {
        input: PathBuf,
        /// Only list fields of this class: internal, dotted or descriptor name.
        #[arg(long)]
        class: Option<String>,
        /// Case-insensitive substring over the field name.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        options: Options,
    },
    /// List the methods an archive defines as JSONL.
    Methods {
        input: PathBuf,
        /// Only list methods of this class: internal, dotted or descriptor name.
        #[arg(long)]
        class: Option<String>,
        /// Case-insensitive substring over the method name.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        options: Options,
    },
    /// Extract one content-bound method as Java or original JVM instructions.
    #[command(name = "get-method")]
    Get {
        input: PathBuf,
        /// `Lclass;->name(proto)`: the `at` a `methods` row prints.
        #[arg(long)]
        at: String,
        #[arg(long)]
        bytecode: bool,
        #[command(flatten)]
        options: Options,
    },
    /// Decompile a whole class to Java by its internal, dotted or descriptor name.
    #[command(name = "get-class")]
    GetClass {
        input: PathBuf,
        /// `Lx/y;` or `x.y`: the `at` a `classes` row prints.
        #[arg(long, conflicts_with = "class")]
        at: Option<String>,
        class: Option<String>,
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
    Xref {
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

/// Which enumeration a listing command performs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listing {
    Classes,
    Fields,
    Methods,
}

impl Listing {
    const fn name(self) -> &'static str {
        match self {
            Self::Classes => "classes",
            Self::Fields => "fields",
            Self::Methods => "methods",
        }
    }
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Classes { .. } => "classes",
            Self::Fields { .. } => "fields",
            Self::Entries { .. } => "entries",
            Self::Methods { .. } => "methods",
            Self::Get { .. } => "get-method",
            Self::GetClass { .. } => "get-class",
            Self::Strings { .. } => "strings",
            Self::Xref { .. } => "xref",
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
        Error::coded(code, message)
    })
}

fn execute_inner(command: Command) -> anyhow::Result<()> {
    if let Command::GetClass {
        input,
        class,
        at,
        options,
    } = command
    {
        let coordinate = at.or(class).ok_or_else(|| {
            anyhow::anyhow!("a class coordinate is required: pass it positionally or with --at")
        })?;
        let limits = options.limits();
        let input = Input::read(&input, limits, options.release)?;
        let source = rxd_jvm::render_class(&input, &coordinate, limits)?;
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
        let wanted = class.as_deref().map(rxd_jvm::normalize_class_name);
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
                json!({"analysis":"strings","release":options.release})
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
                            ..RecordMeta::default()
                        },
                        json!({
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
    if let Command::Xref {
        input,
        kind,
        options,
    } = command
    {
        let limits = options.limits();
        let input = Input::read(&input, limits, options.release)?;
        // The filter matches internal slashed names, so accept the same dotted and
        // descriptor spellings `get-class` already accepts instead of returning zero hits.
        let owner_class = match &kind {
            RefKind::Method { class, .. } | RefKind::Field { class, .. } => {
                class.as_deref().map(rxd_jvm::normalize_class_name)
            }
            RefKind::String { .. } | RefKind::Type { .. } => None,
        };
        let query = match &kind {
            RefKind::String { value } => rxd_jvm::RefQuery::String(value),
            RefKind::Type { name } => rxd_jvm::RefQuery::Type(name),
            RefKind::Method { name, .. } => rxd_jvm::RefQuery::Method {
                name,
                class: owner_class.as_deref(),
            },
            RefKind::Field { name, .. } => rxd_jvm::RefQuery::Field {
                name,
                class: owner_class.as_deref(),
            },
        };
        let hits = rxd_jvm::find_refs(&input, &query, limits)?;
        let mut output = Vec::new();
        append(
            &mut output,
            &header(
                "jvm",
                "xref",
                Some(&input.source_id),
                Some(input.file_bytes),
                kind.query_json(),
                json!({"analysis":"xref","release":options.release})
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
                        context_at: Some(&hit.method_at),
                        ..RecordMeta::default()
                    },
                    json!({
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
    if let Command::Entries { input, options } = command {
        let limits = options.limits();
        let input = Input::read(&input, limits, options.release)?;
        let mut output = Vec::new();
        append(
            &mut output,
            &header(
                "jvm",
                "entries",
                Some(&input.source_id),
                Some(input.file_bytes),
                json!({}),
                json!({"analysis":"entries","release":options.release})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            limits.output_bytes,
        )?;
        let mut records = 0usize;
        for entry in &input.entries {
            let compression = match entry.compression {
                0 => "stored",
                8 => "deflated",
                _ => "other",
            };
            append(
                &mut output,
                &record(
                    "entry",
                    RecordMeta::default(),
                    json!({
                        "name":entry.name,
                        "compression":compression,
                        "compressed_size":entry.compressed_size,
                        "uncompressed_size":entry.uncompressed_size,
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
                limits.output_bytes,
            )?;
            records += 1;
        }
        append(
            &mut output,
            &summary(records, true, serde_json::Map::new()),
            limits.output_bytes,
        )?;
        super::write_output(&output, options.output.as_deref())?;
        return Ok(());
    }
    let (path, options, request, class_filter, method_filter, listing) = match command {
        Command::Classes {
            input,
            options,
            class,
            filter,
        } => (
            input,
            options,
            None,
            class,
            filter.map(|value| value.to_lowercase()),
            Listing::Classes,
        ),
        Command::Fields {
            input,
            options,
            class,
            filter,
        } => (
            input,
            options,
            None,
            class,
            filter.map(|value| value.to_lowercase()),
            Listing::Fields,
        ),
        Command::Methods {
            input,
            options,
            class,
            filter,
        } => (
            input,
            options,
            None,
            class,
            filter.map(|value| value.to_lowercase()),
            Listing::Methods,
        ),
        Command::Get {
            input,
            at,
            bytecode,
            options,
        } => (
            input,
            options,
            Some((at, bytecode)),
            None,
            None,
            Listing::Classes,
        ),
        Command::GetClass { .. } => unreachable!("handled above"),
        Command::Entries { .. } => unreachable!("handled above"),
        Command::Strings { .. } => unreachable!("handled above"),
        Command::Xref { .. } => unreachable!("handled above"),
    };
    let command_name = listing.name();
    let limits = options.limits();
    let input = Input::read(&path, limits, options.release)?;
    let mut output = Vec::new();
    if let Some((at, raw)) = request {
        if raw {
            let (selected, index) = input.method_by_at(&at)?;
            let method = &selected.class.methods[index];
            let code = method
                .code
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("INVALID_QUERY: method has no Code attribute"))?;
            let mut bytecode = record(
                "bytecode",
                RecordMeta {
                    at: Some(Input::method_at(selected, index).as_str()),
                    start: Some(code.range.start),
                    end: Some(code.range.end),
                    ..RecordMeta::default()
                },
                json!({
                    "representation":"bytecode",
                    "class":selected.class.name,
                    "proto":method.descriptor,
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
            let source = rxd_jvm::render_method(&input, &at, limits)?;
            anyhow::ensure!(
                source.len() <= limits.output_bytes,
                "RESOURCE_LIMIT: output byte budget exceeded"
            );
            output.extend_from_slice(source.as_bytes());
        }
    } else {
        let class_name = class_filter.as_deref().map(rxd_jvm::normalize_class_name);
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
                command_name,
                Some(&input.source_id),
                Some(input.file_bytes),
                json!({"class":class_filter,"filter":method_filter}),
                json!({"analysis":command_name,"release":options.release})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            limits.output_bytes,
        )?;
        let mut records = 0usize;
        for selected in &selected_classes {
            let class = &selected.class;
            if listing == Listing::Classes
                && let Some(pattern) = method_filter.as_ref()
                && !class
                    .name
                    .to_lowercase()
                    .contains(&pattern.replace('.', "/"))
            {
                continue;
            }
            if listing == Listing::Fields {
                for field in &class.fields {
                    if method_filter
                        .as_ref()
                        .is_some_and(|pattern| !field.name.to_lowercase().contains(pattern))
                    {
                        continue;
                    }
                    let at = format!("L{};->{}:{}", class.name, field.name, field.descriptor);
                    append(
                        &mut output,
                        &record(
                            "field",
                            RecordMeta {
                                at: Some(&at),
                                ..RecordMeta::default()
                            },
                            json!({
                                "access_flags":field.access,
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
                continue;
            }
            if listing == Listing::Methods {
                for (index, method) in class.methods.iter().enumerate() {
                    if method_filter
                        .as_ref()
                        .is_some_and(|pattern| !method.name.to_lowercase().contains(pattern))
                    {
                        continue;
                    }
                    append(
                        &mut output,
                        &record(
                            "method",
                            RecordMeta {
                                at: Some(&Input::method_at(selected, index)),
                                ..RecordMeta::default()
                            },
                            json!({
                                "proto":method.descriptor,
                                "access_flags":method.access,
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
                continue;
            }
            append(
                &mut output,
                &record(
                    "class",
                    RecordMeta {
                        at: Some(&format!("L{};", class.name)),
                        ..RecordMeta::default()
                    },
                    json!({
                        "super":class.super_name,
                        "interfaces":class.interfaces,
                        "major":class.major,
                        "minor":class.minor,
                        "access_flags":class.access,
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
            if listing == Listing::Classes {
                continue;
            }
            for (index, method) in class.methods.iter().enumerate() {
                if method_filter
                    .as_ref()
                    .is_some_and(|pattern| !method.name.to_lowercase().contains(pattern))
                {
                    continue;
                }
                append(
                    &mut output,
                    &record(
                        "method",
                        RecordMeta {
                            at: Some(&Input::method_at(selected, index)),
                            ..RecordMeta::default()
                        },
                        json!({
                            "proto":method.descriptor,
                            "access_flags":method.access,
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
                json!({"classes":selected_classes.len(), "records":records})
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
