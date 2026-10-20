use std::path::PathBuf;

use clap::{Args, Subcommand};
use rxd_core::envelope::{RecordMeta, SCHEMA_VERSION, header, record, summary};
use rxd_core::{Error, Result};
use rxd_jvm::{Input, Limits};
use serde_json::json;

use super::filter::CaseArgs;
use super::xref::{Anchor, Expect, Kind as XrefKind, Target, TargetArgs, selector_stream};

#[derive(Subcommand)]
pub enum Command {
    /// List the class metadata an archive defines as JSONL.
    Classes {
        input: PathBuf,
        /// Exact internal, dotted or descriptor class name.
        #[arg(long)]
        class: Option<String>,
        ///  substring over the internal class name; dots are accepted. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// List the entries an archive holds, in central-directory order.
    Entries {
        input: PathBuf,
        ///  substring over the entry name. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// List the fields an archive declares as JSONL.
    Fields {
        input: PathBuf,
        /// Only list fields of this class: internal, dotted or descriptor name.
        #[arg(long)]
        class: Option<String>,
        ///  substring over the field name. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// List the methods an archive defines as JSONL.
    Methods {
        input: PathBuf,
        /// Only list methods of this class: internal, dotted or descriptor name.
        #[arg(long)]
        class: Option<String>,
        ///  substring over the method name. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
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
        /// Print the original instruction listing instead of reconstructed source.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        options: Options,
    },
    /// Decompile a whole class to Java by its internal, dotted or descriptor name.
    #[command(name = "get-class")]
    GetClass {
        input: PathBuf,
        /// `Lx/y;` or `x.y`: the `at` a `classes` row prints.
        #[arg(long)]
        at: String,
        #[command(flatten)]
        options: Options,
    },
    /// List string literals from every selected class constant pool as JSONL.
    Strings {
        input: PathBuf,
        /// Exact internal, dotted or descriptor class name to scope the listing.
        #[arg(long)]
        class: Option<String>,
        /// Only strings containing this text. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// Find code references to a string, type, method or field across all classes.
    Xref {
        input: PathBuf,
        #[command(flatten)]
        target: TargetArgs,
        #[command(flatten)]
        options: Options,
    },
}

#[derive(Args)]
pub struct Options {
    /// JVM release used to select multi-release JAR entries.
    #[arg(long, default_value_t = 8)]
    release: u16,
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    perf: bool,
}

/// The budgets every JVM analysis runs under.
///
/// They are safety floors, not tuning knobs: an agent should not have to reason about six
/// limits per backend, and DEX enforces the same class of budget without exposing any.
fn limits() -> Limits {
    Limits::default()
}

impl Options {
    fn limits(&self) -> Limits {
        limits()
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

/// Owns the normalized class spellings a query borrows, so the query can be built from
/// temporary strings without leaking them.
struct Normalized {
    owned: Vec<String>,
}

impl Normalized {
    /// The internal spelling the constant pool holds (`com/google/gson/Gson`).
    fn class(&mut self, class: &str) -> &str {
        self.owned.push(rxd_jvm::normalize_class_name(class));
        self.owned.last().expect("just pushed")
    }

    /// The descriptor spelling a type row prints (`Lcom/google/gson/Gson;`).
    fn descriptor(&mut self, class: &str) -> &str {
        if class.starts_with('[') {
            self.owned.push(class.to_owned());
        } else {
            self.owned
                .push(format!("L{};", rxd_jvm::normalize_class_name(class)));
        }
        self.owned.last().expect("just pushed")
    }
}

/// The backend query a resolved target asks for.
///
/// The owner class is compared against internal names, so a descriptor or dotted spelling is
/// normalized the same way `get-class` already accepts it.
fn ref_query<'a>(target: &'a Target, names: &'a mut Normalized) -> rxd_jvm::RefQuery<'a> {
    use rxd_jvm::{RefPattern, RefQuery};
    match (&target.anchor, target.filter.as_deref()) {
        (
            Some(Anchor::Method {
                class,
                name,
                descriptor,
            }),
            _,
        ) => RefQuery::Method {
            name: RefPattern::coordinate(name),
            class: Some(RefPattern::coordinate(names.class(class))),
            descriptor: Some(descriptor),
        },
        (
            Some(Anchor::Field {
                class,
                name,
                descriptor,
            }),
            _,
        ) => RefQuery::Field {
            name: RefPattern::coordinate(name),
            class: Some(RefPattern::coordinate(names.class(class))),
            descriptor: Some(descriptor),
        },
        (Some(Anchor::Type(name)), _) => {
            RefQuery::Type(RefPattern::coordinate(names.descriptor(name)))
        }
        (Some(Anchor::String(value)), _) => RefQuery::String(RefPattern::coordinate(value)),
        (None, Some(pattern)) => match target.kind {
            XrefKind::String => {
                RefQuery::String(RefPattern::search(pattern, target.case_sensitive))
            }
            // A descriptor never contains a dot, so a dotted type pattern is a package
            // separator: `java.lang.String` must find `Ljava/lang/String;`.
            XrefKind::Type => RefQuery::Type(RefPattern::search(
                names.class(pattern),
                target.case_sensitive,
            )),
            XrefKind::Method | XrefKind::Field => {
                let name = RefPattern::search(pattern, target.case_sensitive);
                let class = target
                    .class
                    .as_deref()
                    .map(|class| RefPattern::search(names.class(class), target.case_sensitive));
                if target.kind == XrefKind::Method {
                    RefQuery::Method {
                        name,
                        class,
                        descriptor: None,
                    }
                } else {
                    RefQuery::Field {
                        name,
                        class,
                        descriptor: None,
                    }
                }
            }
        },
        (None, None) => unreachable!("resolve() rejects an empty target"),
    }
}

impl Command {
    /// Whether this invocation asked for the perf sidecar.
    pub fn perf(&self) -> bool {
        match self {
            Self::Classes { options, .. }
            | Self::Entries { options, .. }
            | Self::Fields { options, .. }
            | Self::Methods { options, .. }
            | Self::Strings { options, .. }
            | Self::Xref { options, .. }
            | Self::Get { options, .. }
            | Self::GetClass { options, .. } => options.perf,
        }
    }

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
    let emit_perf = command.perf();
    let name = command.name();
    let mut perf = rxd_core::perf::Perf::new(emit_perf, "analysis", "jvm", name);
    let result = perf.measure("query", || execute_inner(command));
    perf.finish(result.as_ref().err().map(|_| "IO_ERROR"));
    if emit_perf {
        perf.emit();
    }
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
        at: coordinate,
        options,
    } = command
    {
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
        case,
        options,
    } = command
    {
        let limits = options.limits();
        let input = Input::read(&input, limits, options.release)?;
        let wanted = class.as_deref().map(rxd_jvm::normalize_class_name);
        let needle = filter
            .as_deref()
            .map(|needle| rxd_core::filter::Contains::new(needle, case.sensitive()));
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
                    && !needle.matches(&text)
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
        target,
        options,
    } = command
    {
        let limits = options.limits();
        let input = Input::read(&input, limits, options.release)?;
        let target = match target.selector() {
            Some(path) => target
                .resolve_from(selector_stream(
                    path,
                    &["method", "field", "type"],
                    Expect::Content(&input.source_id),
                )?)
                .map_err(|error| anyhow::anyhow!("INVALID_QUERY: {error:#}"))?,
            None => target
                .resolve()
                .map_err(|error| anyhow::anyhow!("INVALID_QUERY: {error:#}"))?,
        };
        let mut names = Normalized { owned: Vec::new() };
        let query = ref_query(&target, &mut names);
        let hits = rxd_jvm::find_refs(&input, &query, limits)?;
        // A coordinate names one identity: if no class carries it, the answer is NOT_FOUND
        // rather than an empty reference list.
        if query.is_anchor() && !rxd_jvm::resolves(&input, &query) {
            anyhow::bail!("{}", query.anchor_not_found());
        }
        let mut output = Vec::new();
        append(
            &mut output,
            &header(
                "jvm",
                "xref",
                Some(&input.source_id),
                Some(input.file_bytes),
                target.json(),
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
                &rxd_core::envelope::reference(
                    RecordMeta {
                        context_at: Some(&hit.method_at),
                        ..RecordMeta::default()
                    },
                    json!({
                        "target": hit.target,
                        // The reference site itself: bytecode position and the instruction
                        // that performs it. pyc keeps offset/opcode, dotnet keeps
                        // il_offset/opcode, evm keeps instruction_offset, so jar keeps these.
                        "byte_offset": hit.byte_offset,
                        "mnemonic": hit.mnemonic,
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
    if let Command::Entries {
        input,
        options,
        filter,
        case,
    } = command
    {
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
        let pattern = filter.map(|value| rxd_core::filter::Contains::new(&value, case.sensitive()));
        for entry in &input.entries {
            if pattern
                .as_ref()
                .is_some_and(|pattern| !pattern.matches(&entry.name))
            {
                continue;
            }
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
    let (path, options, request, class_filter, method_filter, case_sensitive, listing) =
        match command {
            Command::Classes {
                input,
                options,
                class,
                filter,
                case,
            } => (
                input,
                options,
                None,
                class,
                filter,
                case.sensitive(),
                Listing::Classes,
            ),
            Command::Fields {
                input,
                options,
                class,
                filter,
                case,
            } => (
                input,
                options,
                None,
                class,
                filter,
                case.sensitive(),
                Listing::Fields,
            ),
            Command::Methods {
                input,
                options,
                class,
                filter,
                case,
            } => (
                input,
                options,
                None,
                class,
                filter,
                case.sensitive(),
                Listing::Methods,
            ),
            Command::Get {
                input,
                at,
                raw,
                options,
            } => (
                input,
                options,
                Some((at, raw)),
                None,
                None,
                false,
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
                && !rxd_core::filter::Contains::new(&pattern.replace('.', "/"), case_sensitive)
                    .matches(&class.name)
            {
                continue;
            }
            if listing == Listing::Fields {
                for field in &class.fields {
                    if method_filter.as_ref().is_some_and(|pattern| {
                        !rxd_core::filter::Contains::new(pattern, case_sensitive)
                            .matches(&field.name)
                    }) {
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
                                // Modifiers come from the Java rendering (`get-class`),
                                // so a per-row copy would be the same fact twice.
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
                    if method_filter.as_ref().is_some_and(|pattern| {
                        !rxd_core::filter::Contains::new(pattern, case_sensitive)
                            .matches(&method.name)
                    }) {
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
                if method_filter.as_ref().is_some_and(|pattern| {
                    !rxd_core::filter::Contains::new(pattern, case_sensitive).matches(&method.name)
                }) {
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
