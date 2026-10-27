use std::path::PathBuf;

use clap::{Args, Subcommand};
use rxd_core::Result;
use rxd_core::envelope::{RecordMeta, SCHEMA_VERSION, record};
use rxd_jvm::{Input, Limits};
use serde_json::json;

use super::filter::CaseArgs;
use super::xref::{Anchor, Kind as XrefKind, Selector, Target, TargetArgs};

#[derive(Subcommand)]
pub enum Command {
    /// List class metadata as JSONL.
    #[command(
        after_long_help = "Examples:\nrxd jar classes library.jar -f Client\nOutput: class JSONL; at is Lx/y;. Copy at into get-class for Java.\n--class is an exact internal/dotted/descriptor name; -f is a substring."
    )]
    Classes {
        /// One regular local .class or JAR/ZIP; nested archives are not scanned.
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
    /// List JAR/ZIP entries in central-directory order.
    #[command(
        after_long_help = "Examples:\nrxd jar entries library.jar -f META-INF\nOutput: entry JSONL with names, compression and sizes. Entries are archive metadata,\nnot member coordinates. --release N controls multi-release class selection."
    )]
    Entries {
        /// One regular local .class or JAR/ZIP; nested archives are not scanned.
        input: PathBuf,
        ///  substring over the entry name. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// List fields as JSONL.
    #[command(
        after_long_help = "Examples:\nrxd jar fields library.jar --class x.y -f token\nOutput: field JSONL; at is Lx/y;->name:type. --name is exact, -f a substring.\nNext: rxd jar xref library.jar --at '<field.at>'."
    )]
    Fields {
        /// One regular local .class or JAR/ZIP; nested archives are not scanned.
        input: PathBuf,
        /// Only list fields of this class: internal, dotted or descriptor name.
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
    /// List methods as JSONL.
    #[command(
        after_long_help = "Examples:\nrxd jar methods library.jar --class x.y -f run\nOutput: method JSONL; at is Lx/y;->name(parameters)return_type.\n--name retains exact-name overloads. Next: get-method --at '<method.at>'."
    )]
    Methods {
        /// One regular local .class or JAR/ZIP; nested archives are not scanned.
        input: PathBuf,
        /// Only list methods of this class: internal, dotted or descriptor name.
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
    /// Fetch one method as Java or JVM instructions.
    #[command(name = "get-method", after_long_help = "Examples:\nrxd jar methods library.jar -f run\nrxd jar get-method library.jar --at 'Lx/y;->run()V' --raw\nCopy methods.at or xref.context_at verbatim; include parameters AND return type.\nOutput: one method_body JSON record. Default detail.code is reconstructed Java;\n--raw returns original JVM instructions, byte offsets and exception handlers.", mut_arg("at", |arg| arg.required(true).value_name("METHOD_AT").help("Copy methods.at or xref.context_at: JVM descriptor including return type, e.g. Lx/y;->run()V")))]
    Get {
        /// One regular local .class or JAR/ZIP; nested archives are not scanned.
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        /// Print the original instruction listing instead of reconstructed source.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        options: Options,
    },
    /// Fetch one whole class as Java.
    #[command(name = "get-class", after_long_help = "Examples:\nrxd jar classes library.jar -f Client\nrxd jar get-class library.jar --at 'Lx/y;'\nCopy classes.at, not a method coordinate. Output: one class_body record, detail.code is Java.\nWhole-class rendering may contain unsupported-method stubs; inspect method --raw separately.", mut_arg("at", |arg| arg.required(true).value_name("CLASS_AT").help("Copy classes.at from this input, e.g. Lx/y;; do not pass a method coordinate")))]
    GetClass {
        /// One regular local .class or JAR/ZIP; nested archives are not scanned.
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        #[command(flatten)]
        options: Options,
    },
    /// List constant-pool string literals as JSONL.
    #[command(
        after_long_help = "Examples:\nrxd jar strings library.jar -f Authorization\nrxd jar xref library.jar -m Authorization --kind string\nOutput: string JSONL from CONSTANT_String literals, not every constant-pool Utf8 name.\nPool indices are not methods. xref.context_at routes to get-method for the caller."
    )]
    Strings {
        /// One regular local .class or JAR/ZIP; nested archives are not scanned.
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
    /// Search references to a string, type, method or field.
    #[command(
        after_long_help = "Examples:\nrxd jar xref library.jar --at 'Lx/y;->run()V'\nrxd jar xref library.jar -m Authorization --kind string\nrxd jar xref library.jar -m append --kind method --class java.lang.StringBuilder\nRequired: exactly one of --at/--match; --match requires --kind.\nCoordinates copy classes.at/methods.at/fields.at; literal --at needs --kind string.\n--class is one exact target owner, not caller filtering.\nOutput: reference JSONL; context_at names caller, detail.byte_offset/mnemonic the use.\nNext: rxd jar get-method library.jar --at '<reference.context_at>' --raw."
    )]
    Xref {
        /// One regular local .class or JAR/ZIP; nested archives are not scanned.
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
    /// Atomically write the result to FILE instead of stdout; never overwrite input.
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    perf: bool,
}

impl Options {
    fn limits(&self, input: &std::path::Path, policy: rxd_core::BudgetPolicy) -> Result<Limits> {
        Ok(Limits::from(policy.for_file(input)?))
    }
}

/// Which enumeration a listing command performs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listing {
    Classes,
    Fields,
    Methods,
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

/// Normalize owner spellings before building the backend reference selector.
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
                // `--class` names one class. A pattern over owners is `classes -f`.
                let class = target
                    .class
                    .as_deref()
                    .map(|class| RefPattern::coordinate(names.class(class)));
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

pub fn execute(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    sink: &mut super::Output<'_>,
) -> Result<()> {
    let emit_perf = command.perf();
    let name = command.name();
    let mut perf = rxd_core::perf::Perf::new(emit_perf, "analysis", "jvm", name);
    let result = perf.measure("query", || execute_inner(command, policy, sink));
    perf.finish(result.as_ref().err().map(|_| "IO_ERROR"));
    if emit_perf {
        perf.emit();
    }
    result.map_err(super::classify)
}

fn execute_inner(
    command: Command,
    policy: rxd_core::BudgetPolicy,
    sink: &mut super::Output<'_>,
) -> anyhow::Result<()> {
    if let Command::GetClass {
        input,
        selector,
        options,
    } = command
    {
        let limits = options.limits(&input, policy)?;
        // A plain `--at` fetch parses only the named class; unusual archive layouts keep
        // the full reader.
        let scoped = match &selector.at {
            Some(at) => Input::read_class_scoped(&input, limits, options.release, at)?,
            None => None,
        };
        let input = match scoped {
            Some(input) => input,
            None => Input::read(&input, limits, options.release)?,
        };
        let coordinate = selector.resolve()?;
        let source = rxd_jvm::render_class(&input, &coordinate, limits)?;
        // One FETCH is one record: the source is a field of it, not the whole answer, so a
        // consumer parses the same envelope as every other command.
        let mut body = record(
            "class_body",
            RecordMeta {
                at: Some(coordinate.as_str()),
                ..RecordMeta::default()
            },
            json!({
                "representation": "java",
                "decompiled": true,
                "complete": true,
                "code": source,
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        body["backend"] = json!("jvm");
        body["command"] = json!("get-class");
        body["schema_version"] = json!(SCHEMA_VERSION);
        body["source_id"] = json!(input.source_id);
        let mut output = Vec::new();
        append(&mut output, &body)?;
        super::write_output(&output, options.output.as_deref(), sink)?;
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
        let limits = options.limits(&input, policy)?;
        let input = Input::read(&input, limits, options.release)?;
        let wanted = class.as_deref().map(rxd_jvm::normalize_class_name);
        let needle = filter
            .as_deref()
            .map(|needle| rxd_core::filter::Contains::new(needle, case.sensitive()));
        let mut output = Vec::new();
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
                let utf16_units: Option<Vec<u16>> = (!lossless).then(|| value.as_utf16().to_vec());
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
                            "utf16_units":utf16_units,
                        })
                        .as_object()
                        .unwrap()
                        .clone(),
                    ),
                )?;
            }
        }
        super::write_output(&output, options.output.as_deref(), sink)?;
        return Ok(());
    }
    if let Command::Xref {
        input,
        target,
        options,
    } = command
    {
        let limits = options.limits(&input, policy)?;
        let input = Input::read(&input, limits, options.release)?;
        let target = target.resolve().map_err(super::classify)?;
        let mut names = Normalized { owned: Vec::new() };
        let query = ref_query(&target, &mut names);
        let hits = rxd_jvm::find_refs(&input, &query, limits)?;
        // Missing anchors are NOT_FOUND, not empty reference lists.
        if query.is_anchor() && !rxd_jvm::resolves(&input, &query) {
            anyhow::bail!("{}", query.anchor_not_found());
        }
        let mut output = Vec::new();
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

                        "byte_offset": hit.byte_offset,
                        "mnemonic": hit.mnemonic,
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )?;
        }
        super::write_output(&output, options.output.as_deref(), sink)?;
        return Ok(());
    }
    if let Command::Entries {
        input,
        options,
        filter,
        case,
    } = command
    {
        let limits = options.limits(&input, policy)?;
        let (_, _, entries) = Input::read_entries(&input, limits, options.release)?;
        let mut output = Vec::new();
        let pattern = filter.map(|value| rxd_core::filter::Contains::new(&value, case.sensitive()));
        for entry in &entries {
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
            )?;
        }
        super::write_output(&output, options.output.as_deref(), sink)?;
        return Ok(());
    }
    let (path, options, request, class_filter, exact_name, method_filter, case_sensitive, listing) =
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
                None,
                filter,
                case.sensitive(),
                Listing::Classes,
            ),
            Command::Fields {
                input,
                options,
                class,
                name,
                filter,
                case,
            } => (
                input,
                options,
                None,
                class,
                name,
                filter,
                case.sensitive(),
                Listing::Fields,
            ),
            Command::Methods {
                input,
                options,
                class,
                name,
                filter,
                case,
            } => (
                input,
                options,
                None,
                class,
                name,
                filter,
                case.sensitive(),
                Listing::Methods,
            ),
            Command::Get {
                input,
                selector,
                raw,
                options,
            } => (
                input,
                options,
                Some((selector, raw)),
                None,
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
    let limits = options.limits(&path, policy)?;
    let input = Input::read(&path, limits, options.release)?;
    let mut output = Vec::new();
    if let Some((selector, raw)) = request {
        let at = selector.resolve()?;
        if raw {
            let (selected, index) = input.method_by_at(&at)?;
            let method = &selected.class.methods[index];
            let code = method
                .code
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("INVALID_QUERY: method has no Code attribute"))?;
            let mut bytecode = record(
                "method_body",
                RecordMeta {
                    at: Some(Input::method_at(selected, index).as_str()),
                    start: Some(code.range.start),
                    end: Some(code.range.end),
                    ..RecordMeta::default()
                },
                json!({
                    "representation":"bytecode",
                    "decompiled":false,
                    "code":serde_json::Value::Null,
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
            bytecode["source_id"] = json!(input.source_id);
            append(&mut output, &bytecode)?;
        } else {
            let source = rxd_jvm::render_method(&input, &at, limits)?;
            let mut body = record(
                "method_body",
                RecordMeta {
                    at: Some(at.as_str()),
                    ..RecordMeta::default()
                },
                json!({
                    "representation": "java",
                    "decompiled": true,
                    "complete": true,
                    "code": source,
                    "instructions": serde_json::Value::Null,
                })
                .as_object()
                .unwrap()
                .clone(),
            );
            body["backend"] = json!("jvm");
            body["command"] = json!("get-method");
            body["schema_version"] = json!(SCHEMA_VERSION);
            body["source_id"] = json!(input.source_id);
            append(&mut output, &body)?;
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
        let exact_name = exact_name.as_deref();
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
                    if exact_name.is_some_and(|wanted| field.name != wanted) {
                        continue;
                    }
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
                    )?;
                }
                continue;
            }
            if listing == Listing::Methods {
                for (index, method) in class.methods.iter().enumerate() {
                    if exact_name.is_some_and(|wanted| method.name != wanted) {
                        continue;
                    }
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

                                "signature":method.signature,
                                "exceptions":method.exceptions,
                            })
                            .as_object()
                            .unwrap()
                            .clone(),
                        ),
                    )?;
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
                        // A default parent or an empty interface list is not evidence, and
                        // printing it would put the same two bytes on most of a class index.
                        "super":class.super_name.as_deref().filter(|name| *name != "java/lang/Object"),
                        "interfaces":(!class.interfaces.is_empty()).then_some(&class.interfaces),
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
            )?;
        }
    }
    super::write_output(&output, options.output.as_deref(), sink)?;
    Ok(())
}

fn append(output: &mut Vec<u8>, record: &serde_json::Value) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *output, record)
        .map_err(|error| anyhow::anyhow!("INVALID_INPUT: {error}"))?;
    output.push(b'\n');
    Ok(())
}
