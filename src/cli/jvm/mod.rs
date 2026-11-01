use std::path::PathBuf;

use clap::{Args, Subcommand};
use rxd_core::Result;
use rxd_jvm::{Input, Limits};

use super::filter::CaseArgs;
use super::xref::{Anchor, Kind as XrefKind, Selector, Target, TargetArgs};

#[derive(Subcommand)]
pub enum Command {
    /// List class metadata (columns: at).
    #[command(
        long_about = "Columns: at\nList class metadata recursively across embedded JAR/ZIP/AAR archives. Nested coordinates prefix the class with archive paths separated by !. One # column header precedes the data rows.",
        after_long_help = "Examples:\nrxd jar classes library.jar -f Client\nOutput: class coordinates after # at; copy one into get-class for Java.\n--class is an exact internal/dotted/descriptor name; -f is a substring."
    )]
    Classes {
        /// One local .class, JAR, ZIP or AAR; nested archives are scanned recursively.
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
    /// List JAR/ZIP entries (columns: name, uncompressed_size, compression).
    #[command(
        long_about = "Columns: name, uncompressed_size, compression\nList archive entries recursively, using ! between archive paths. One # column header precedes the data rows.",
        after_long_help = "Examples:\nrxd jar entries library.jar -f META-INF\nOutput: TAB-separated name, uncompressed_size, compression. Entries are archive metadata,\nnot member coordinates. --release N controls multi-release class selection."
    )]
    Entries {
        /// One local .class, JAR, ZIP or AAR; nested archives are scanned recursively.
        input: PathBuf,
        ///  substring over the entry name. Folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        #[command(flatten)]
        options: Options,
    },
    /// List fields (columns: at).
    #[command(
        long_about = "Columns: at\nList fields. One # column header precedes the data rows.",
        after_long_help = "Examples:\nrxd jar fields library.jar --class x.y -f token\nOutput: field coordinates after # at; e.g. Lx/y;->name:type. --name is exact, -f a substring.\nNext: rxd jar xref library.jar --at '<field.at>'."
    )]
    Fields {
        /// One local .class, JAR, ZIP or AAR; nested archives are scanned recursively.
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
    /// List methods (columns: at).
    #[command(
        long_about = "Columns: at\nList methods. One # column header precedes the data rows.",
        after_long_help = "Examples:\nrxd jar methods library.jar --class x.y -f run\nOutput: method coordinates after # at; e.g. Lx/y;->name(parameters)return_type.\n--name retains exact-name overloads. Next: get-method --at '<method.at>'."
    )]
    Methods {
        /// One local .class, JAR, ZIP or AAR; nested archives are scanned recursively.
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
    #[command(name = "get-method", after_long_help = "Examples:\nrxd jar methods library.jar -f run\nrxd jar get-method library.jar --at 'Lx/y;->run()V' --raw\nCopy methods.at or xref.context verbatim; include parameters AND return type.\nOutput: reconstructed Java text; --raw prints the original JVM instructions,\nbyte offsets and exception handlers as text.", mut_arg("at", |arg| arg.required(true).value_name("METHOD_AT").help("Copy methods.at or xref.context: JVM descriptor including return type, e.g. Lx/y;->run()V")))]
    Get {
        /// One local .class, JAR, ZIP or AAR; nested archives are scanned recursively.
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
    #[command(name = "get-class", after_long_help = "Examples:\nrxd jar classes library.jar -f Client\nrxd jar get-class library.jar --at 'Lx/y;'\nCopy classes.at, not a method coordinate. Output: reconstructed Java text.\nWhole-class rendering may contain unsupported-method stubs; inspect method --raw separately.", mut_arg("at", |arg| arg.required(true).value_name("CLASS_AT").help("Copy classes.at from this input, e.g. Lx/y;; do not pass a method coordinate")))]
    GetClass {
        /// One local .class, JAR, ZIP or AAR; nested archives are scanned recursively.
        input: PathBuf,
        #[command(flatten)]
        selector: Selector,
        #[command(flatten)]
        options: Options,
    },
    /// List constant-pool string literals (columns: at, value).
    #[command(
        long_about = "Columns: at, value\nList constant-pool string literals. One # column header precedes the data rows.",
        after_long_help = "Examples:\nrxd jar strings library.jar -f Authorization\nrxd jar xref library.jar -m Authorization --kind string\nOutput: TAB-separated at, value from CONSTANT_String literals, not every constant-pool Utf8 name.\nPool indices are not methods. xref.context routes to get-method for the caller."
    )]
    Strings {
        /// One local .class, JAR, ZIP or AAR; nested archives are scanned recursively.
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
        long_about = "Columns: entry, context, match\nList explicit references as TAB-separated rows after one # column header. Successful zero-hit queries retain the header.",
        after_long_help = "Examples:\nrxd jar xref library.jar --at 'Lx/y;->run()V'\nrxd jar xref library.jar -m Authorization --kind string\nrxd jar xref library.jar -m append --kind method --class java.lang.StringBuilder\nRequired: exactly one of --at/--match; --match requires --kind.\nCoordinates copy classes.at/methods.at/fields.at; literal --at needs --kind string.\n--class is one exact target owner, not caller filtering.\nOutput: TAB-separated entry, context, match. entry is the class archive path (\\N for standalone class); context is the referencing method; match is the actual target.\nNext: rxd jar get-method library.jar --at '<xref.context>' --raw."
    )]
    Xref {
        /// One local .class, JAR, ZIP or AAR; nested archives are scanned recursively.
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
        let class = class.rsplit_once('!').map_or(class, |(_, local)| local);
        self.owned.push(rxd_jvm::normalize_class_name(class));
        self.owned.last().expect("just pushed")
    }

    /// The descriptor spelling a type row prints (`Lcom/google/gson/Gson;`).
    fn descriptor(&mut self, class: &str) -> &str {
        let class = class.rsplit_once('!').map_or(class, |(_, local)| local);
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
        // Reconstruction is the answer: print the source, never wrap it in a record.
        super::write_output(source.as_bytes(), options.output.as_deref(), sink)?;
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
        let wanted = class.as_deref();
        let needle = filter
            .as_deref()
            .map(|needle| rxd_core::filter::Contains::new(needle, case.sensitive()));
        let mut output = super::table::header(&["at", "value"]).into_bytes();
        for selected in &input.classes {
            if wanted
                .as_ref()
                .is_some_and(|name| !Input::matches_class(selected, name))
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
                let mut rows_first = true;
                let utf16_units: Option<Vec<u16>> = (!lossless).then(|| value.as_utf16().to_vec());
                let _ = (&utf16_units, &selected.class.name);
                let quoted = serde_json::to_string(&text)
                    .map_err(|error| anyhow::anyhow!("INVALID_INPUT: {error}"))?;
                append_row(&mut output, &mut rows_first, &format!("{index}\t{quoted}"))?;
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
        let owner = match target.anchor.as_ref() {
            Some(Anchor::Method { class, .. } | Anchor::Field { class, .. }) => {
                Some(class.as_str())
            }
            Some(Anchor::Type(class)) => Some(class.as_str()),
            _ => None,
        };
        if let Some(owner) = owner.filter(|owner| owner.contains('!')) {
            input.class_by_name(owner)?;
        }
        let mut names = Normalized { owned: Vec::new() };
        let query = ref_query(&target, &mut names);
        let hits = rxd_jvm::find_refs(&input, &query, limits)?;
        // Missing anchors are NOT_FOUND, not empty reference lists.
        if query.is_anchor() && !rxd_jvm::resolves(&input, &query) {
            anyhow::bail!("{}", query.anchor_not_found());
        }
        use super::table::{self, Cell};
        use std::io::Write as _;
        let mut output = rxd_core::output::Output::new(sink.remaining());
        output.write_all(table::header(&["entry", "context", "match"]).as_bytes())?;
        for hit in &hits {
            let row = table::line(&[
                hit.entry.as_deref().map_or(Cell::Null, Cell::Text),
                Cell::Atom(&hit.method_at),
                Cell::Text(&hit.target),
            ])?;
            output.write_all(row.as_bytes())?;
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
        let mut output =
            super::table::header(&["name", "uncompressed_size", "compression"]).into_bytes();
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
            use super::table::{self, Cell};
            output.extend_from_slice(
                table::line(&[
                    Cell::Text(&entry.name),
                    Cell::Atom(&entry.uncompressed_size.to_string()),
                    Cell::Text(compression),
                ])?
                .as_bytes(),
            );
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
    let scoped = request
        .as_ref()
        .and_then(|(selector, _)| selector.at.as_deref())
        .and_then(|at| at.split_once("->"))
        .map(|(class, _)| class)
        .or(class_filter.as_deref());
    let input = match scoped {
        Some(class) => Input::read_class_scoped(&path, limits, options.release, class)?,
        None => None,
    }
    .map_or_else(|| Input::read(&path, limits, options.release), Ok)?;
    let mut output = Vec::new();
    let mut rows_first = true;
    if let Some((selector, raw)) = request {
        let at = selector.resolve()?;
        if raw {
            let (selected, index) = input.method_by_at(&at)?;
            let method = &selected.class.methods[index];
            let code = method
                .code
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("INVALID_QUERY: method has no Code attribute"))?;
            // Original instructions are the answer: one line per instruction,
            // with the code attributes as comment lines around them.
            let mut text = String::new();
            text.push_str(&format!(
                "// max_stack={} max_locals={} handlers={}\n",
                code.max_stack,
                code.max_locals,
                code.handlers.len()
            ));
            for instruction in rxd_jvm::bytecode(&selected.class, method)? {
                text.push_str(&instruction.byte_offset.to_string());
                text.push('\t');
                text.push_str(instruction.mnemonic);
                for operand in &instruction.operands {
                    text.push(' ');
                    text.push_str(&operand.to_string());
                }
                if !instruction.targets.is_empty() {
                    text.push_str(" ->");
                    for target in &instruction.targets {
                        text.push_str(&format!(" {target}"));
                    }
                }
                text.push('\n');
            }
            for handler in &code.handlers {
                text.push_str(&format!(
                    "// handler start={} end={} target={} catch={}\n",
                    handler.start,
                    handler.end,
                    handler.target,
                    handler.catch_type.as_deref().unwrap_or("any")
                ));
            }
            super::write_output(text.as_bytes(), options.output.as_deref(), sink)?;
        } else {
            let source = rxd_jvm::render_method(&input, &at, limits)?;
            super::write_output(source.as_bytes(), options.output.as_deref(), sink)?;
        }
    } else {
        output.extend_from_slice(super::table::header(&["at"]).as_bytes());
        let class_name = class_filter.as_deref();
        let selected_classes: Vec<_> = input
            .classes
            .iter()
            .filter(|item| {
                class_name
                    .as_ref()
                    .is_none_or(|name| Input::matches_class(item, name))
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
                    let at = format!(
                        "{}->{}:{}",
                        Input::class_at(selected),
                        field.name,
                        field.descriptor
                    );
                    append_row(&mut output, &mut rows_first, &at)?;
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
                    let at = Input::method_at(selected, index);
                    append_row(&mut output, &mut rows_first, &at)?;
                }
                continue;
            }
            append_row(&mut output, &mut rows_first, &Input::class_at(selected))?;
        }
    }
    super::write_output(&output, options.output.as_deref(), sink)?;
    Ok(())
}

fn append_row(output: &mut Vec<u8>, first: &mut bool, row: &str) -> anyhow::Result<()> {
    let _ = &first;
    output.extend_from_slice(row.as_bytes());
    output.push(b'\n');
    *first = false;
    Ok(())
}
