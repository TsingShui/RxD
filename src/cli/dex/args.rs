use super::super::default_threads;
use super::super::filter::CaseArgs;
use super::super::xref::{Anchor, Kind as XrefKind, Selector, Target, TargetArgs};
use anyhow::Result;
use clap::{Args, Subcommand};
use rxd_dex::query::{MemberQuery, Pattern, Query, format_class_name};
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Fetch one class as Java.
    #[command(name = "get-class", mut_arg("at", |arg| arg.required(true).value_name("CLASS_AT").help("Copy classes.at from this input, e.g. Lx/y;; not a method coordinate")), after_long_help = "Examples:\n  rxd rasc classes app.apk -f Main\n  rxd rasc get-class app.apk --at 'Lx/y;'\nUse actual classes.at. Output: reconstructed Java text.\nNo --raw flag. Unsupported methods may be stubs; inspect members and references.")]
    Getclass(GetClassArgs),
    /// List APK/ZIP entries (columns: name, uncompressed_size, compression).
    #[command(
        after_long_help = "Examples:\n  rxd rasc entries app.apk -f lib/arm64-v8a/\nOutput: entry JSONL with detail.name, compression and sizes. Copy detail.name into:\n  rxd rasc extract app.apk --name '<entry.detail.name>' -o target/library.so\nDuplicate entry names remain visible."
    )]
    Entries(EntriesArgs),
    /// Extract original entry bytes to -o.
    #[command(
        after_long_help = "Examples:\n  rxd rasc extract app.apk --name lib/arm64-v8a/libfoo.so -o target/libfoo.so\n  rxd rasc extract app.apk --all -f lib/arm64-v8a/ -o target/native\nRequired: exactly one of --name/--all, plus -o FILE/DIR. Copy entries.detail.name.\nOutput: original bytes at destination, file receipt JSONL on stdout.\nUnsafe/duplicate destinations and attempts to overwrite input are rejected."
    )]
    Extract(ExtractArgs),
    /// List DEX string literals (columns: at, value, entry).
    #[command(
        after_long_help = "Examples:\n  rxd rasc strings app.apk -f Authorization --limit 20\n  rxd rasc xref app.apk -m Authorization --kind string\nOutput: string JSONL; detail.value is text, detail.index is a DEX string index.\nString indices are not class/method coordinates. xref.context_at names a caller.\n--offset skips matching rows, --limit caps the page; omit both for full inventory."
    )]
    Strings(StringsArgs),
    /// Search code references across every DEX.
    #[command(
        after_long_help = "Examples:\n  rxd rasc xref app.apk --at 'Lx/y;->run()V'\n  rxd rasc xref app.apk -m Authorization --kind string\n  rxd rasc xref app.apk -m run --kind method --class x.y\nRequired: exactly one of --at/--match; --match requires --kind.\n--at copies classes.at/methods.at/fields.at; literal text needs --kind string.\n--class is one exact member owner, not a caller filter or substring.\nOutput: reference JSONL; context_at is caller, detail.target is target.\nNext: rxd rasc get-method app.apk --at '<reference.context_at>'."
    )]
    Xref(XrefArgs),
    /// Extract decoded AndroidManifest.xml.
    #[command(
        after_long_help = "Examples:\n  rxd rasc manifest app.apk -o target/AndroidManifest.xml\nInput must contain AndroidManifest.xml. Output is decoded XML, not JSONL.\nPlain XML remains original bytes; binary Android XML is decoded."
    )]
    Manifest(ManifestArgs),
    /// List classes across every DEX (columns: at).
    #[command(
        after_long_help = "Examples:\n  rxd rasc classes app.apk -f Main\n  rxd rasc classes app.apk --class com.example.Main\nOutput: class JSONL; at is Lcom/example/Main;.\nNext: rxd rasc get-class app.apk --at '<class.at>'.\n-f searches class names; --class selects one exact class."
    )]
    Classes(ClassesArgs),
    /// Fetch one method as Java.
    #[command(name = "get-method", mut_arg("at", |arg| arg.required(true).value_name("METHOD_AT").help("Copy methods.at or xref.context_at; include parameters and return type, e.g. Lx/y;->run()V")), after_long_help = "Examples:\n  rxd rasc methods app.apk -f run\n  rxd rasc get-method app.apk --at 'Lx/y;->run()V'\nUse actual methods.at or reference.context_at, not a name, index or file hash.\nThe prototype includes return type after ): ()V, (I)I, (I)[Ljava/lang/String;.\nOutput: reconstructed Java text. No --raw flag.")]
    GetMethod(GetMethodArgs),
    /// List fields across every DEX (columns: at).
    #[command(
        after_long_help = "Examples:\n  rxd rasc fields app.apk --class com.example.Main -f token\n  rxd rasc xref app.apk --at '<field.at>'\nOutput: field JSONL; at is Lx/y;->name:type. --name is exact, -f is a substring.\nFor declarations/layout use get-class with the owner classes.at."
    )]
    Fields(FieldsArgs),
    /// List defined methods (columns: at).
    #[command(
        after_long_help = "Examples:\n  rxd rasc methods app.apk --class com.example.Main -f run\n  rxd rasc methods app.apk --name run\nOutput: method JSONL; at is Lx/y;->name(parameters)return_type.\n--name retains exact-name overloads; -f searches names, --class selects one owner.\nNext: rxd rasc get-method app.apk --at '<method.at>', or xref --at for callers."
    )]
    Methods(MethodsArgs),
}

#[derive(Debug, Args)]
pub struct GetMethodArgs {
    /// One local APK/ZIP with root DEX entries, or a standalone DEX file.
    pub apk_path: PathBuf,
    #[command(flatten)]
    pub selector: Selector,
    /// Write the same payload to FILE and stdout; input and output must differ.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Parallel analysis workers; must be positive. One admitted input is held per worker.
    #[arg(long, alias = "thread", default_value_t = default_threads(), value_parser = super::super::positive)]
    pub threads: usize,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
}

#[derive(Debug, Args)]
pub struct FieldsArgs {
    /// One local APK/ZIP with root DEX entries, or a standalone DEX file.
    pub apk_path: PathBuf,
    /// Only list fields declared by this class: `Lcom/foo/Bar;` or `com.foo.Bar`.
    #[arg(long)]
    pub class: Option<String>,
    /// Exact field name; not a substring.
    #[arg(long)]
    pub name: Option<String>,
    ///  substring filter over the field name. Folded unless `--case sensitive`.
    #[arg(short, long)]
    pub filter: Option<String>,
    #[command(flatten)]
    pub case: CaseArgs,
    /// Write the same payload to FILE and stdout; input and output must differ.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Parallel analysis workers; must be positive. One admitted input is held per worker.
    #[arg(long, alias = "thread", default_value_t = default_threads(), value_parser = super::super::positive)]
    pub threads: usize,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
}

#[derive(Debug, Args)]
pub struct MethodsArgs {
    /// One local APK/ZIP with root DEX entries, or a standalone DEX file.
    pub apk_path: PathBuf,
    /// Only list methods defined by this class: `Lcom/foo/Bar;` or `com.foo.Bar`.
    #[arg(long)]
    pub class: Option<String>,
    /// Exact method name; not a substring.
    #[arg(long)]
    pub name: Option<String>,
    ///  substring filter over the method name. Folded unless `--case sensitive`.
    #[arg(short, long)]
    pub filter: Option<String>,
    #[command(flatten)]
    pub case: CaseArgs,
    /// Write the same payload to FILE and stdout; input and output must differ.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Parallel analysis workers; must be positive. One admitted input is held per worker.
    #[arg(long, alias = "thread", default_value_t = default_threads(), value_parser = super::super::positive)]
    pub threads: usize,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
}

#[derive(Debug, Args)]
pub struct ClassesArgs {
    /// Parallel analysis workers; must be positive. One admitted input is held per worker.
    #[arg(long, alias = "thread", default_value_t = default_threads(), value_parser = super::super::positive)]
    pub threads: usize,
    /// Only this class: `Lcom/foo/Bar;` or `com.foo.Bar`.
    #[arg(long)]
    pub class: Option<String>,
    ///  substring filter over Java-style class names. Folded unless `--case sensitive`.
    #[arg(short, long)]
    pub filter: Option<String>,
    #[command(flatten)]
    pub case: CaseArgs,
    /// Write the same payload to FILE and stdout; input and output must differ.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
    /// One local APK/ZIP with root DEX entries, or a standalone DEX file.
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("selection")
        .required(true)
        .multiple(false)
        .args(["name", "all"])
))]
pub struct ExtractArgs {
    /// One local APK/ZIP with root DEX entries, or a standalone DEX file.
    pub apk_path: PathBuf,
    /// Exact archive entry name; writes it to the --output file.
    #[arg(long)]
    pub name: Option<String>,
    /// Extract every entry matching --filter (or every entry when no filter is
    /// given); --output must be a directory.
    #[arg(long)]
    pub all: bool,
    ///  substring filter over entry names. Folded unless `--case sensitive`.
    #[arg(short, long)]
    pub filter: Option<String>,
    #[command(flatten)]
    pub case: CaseArgs,
    /// Required destination file (--name) or directory (--all).
    #[arg(short, long, required = true)]
    pub output: Option<PathBuf>,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
}

#[derive(Debug, Args)]
pub struct StringsArgs {
    /// Only strings containing this text. Folded unless `--case sensitive`.
    #[arg(short, long)]
    pub filter: Option<String>,
    #[command(flatten)]
    pub case: CaseArgs,
    /// At most this many strings, counted in the order they are printed.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Skip this many matches in output order; combine with --limit for pagination.
    #[arg(long)]
    pub offset: Option<usize>,
    /// Parallel analysis workers; must be positive. One admitted input is held per worker.
    #[arg(long, alias = "thread", default_value_t = default_threads(), value_parser = super::super::positive)]
    pub threads: usize,
    /// Write the same payload to FILE and stdout; input and output must differ.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
    /// One local APK/ZIP with root DEX entries, or a standalone DEX file.
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct EntriesArgs {
    ///  substring over the entry name. Folded unless `--case sensitive`.
    #[arg(short, long)]
    pub filter: Option<String>,
    #[command(flatten)]
    pub case: CaseArgs,
    /// Write the same payload to FILE and stdout; input and output must differ.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
    /// One local APK/ZIP with root DEX entries, or a standalone DEX file.
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct ManifestArgs {
    /// Write the same payload to FILE and stdout; input and output must differ.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
    /// One local APK/ZIP with root DEX entries, or a standalone DEX file.
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct GetClassArgs {
    #[command(flatten)]
    pub selector: Selector,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
    /// Parallel analysis workers; must be positive. One admitted input is held per worker.
    #[arg(long, alias = "thread", default_value_t = default_threads(), value_parser = super::super::positive)]
    pub threads: usize,
    /// Write the same payload to FILE and stdout; input and output must differ.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// One local APK/ZIP with root DEX entries, or a standalone DEX file.
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct XrefArgs {
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
    /// Parallel analysis workers; must be positive. One admitted input is held per worker.
    #[arg(long, alias = "thread", default_value_t = default_threads(), value_parser = super::super::positive)]
    pub threads: usize,
    #[command(flatten)]
    pub target: TargetArgs,
    /// One local APK/ZIP with root DEX entries, or a standalone DEX file.
    pub apk_path: PathBuf,
    /// Write the same payload to FILE and stdout; input and output must differ.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

impl XrefArgs {
    /// Normalize exact anchors to descriptors; keep content searches as substrings.
    pub fn query(&self, target: &Target) -> Result<Query> {
        target_query(target)
    }

    /// Resolve the anchor from its coordinate.
    pub fn target(&self) -> Result<Target> {
        self.target.resolve()
    }

    pub fn output_path(&self) -> Option<&Path> {
        self.output.as_deref()
    }
}

fn target_query(target: &Target) -> Result<Query> {
    let case = target.case_sensitive;
    let member =
        |name: &str, class: &str, descriptor: Option<&str>, exact: bool| -> Result<MemberQuery> {
            Ok(MemberQuery {
                name: Some(if exact {
                    Pattern::coordinate(name)
                } else {
                    Pattern::search(name, case)
                }),
                class: Some(if exact {
                    Pattern::coordinate(format_class_name(class)?)
                } else {
                    Pattern::search(format_class_name(class)?, case)
                }),
                descriptor: descriptor.map(str::to_owned),
            })
        };
    match (&target.anchor, target.filter.as_deref()) {
        (
            Some(Anchor::Method {
                class,
                name,
                descriptor,
            }),
            _,
        ) => Ok(Query::Method(member(name, class, Some(descriptor), true)?)),
        (
            Some(Anchor::Field {
                class,
                name,
                descriptor,
            }),
            _,
        ) => Ok(Query::Field(member(name, class, Some(descriptor), true)?)),
        (Some(Anchor::Type(name)), _) => {
            Ok(Query::Type(Pattern::coordinate(type_coordinate(name))))
        }
        (Some(Anchor::String(value)), _) => Ok(Query::String(Pattern::coordinate(value.clone()))),
        (None, Some(pattern)) => match target.kind {
            XrefKind::String => Ok(Query::String(Pattern::search(pattern, case))),
            // A descriptor never contains a dot, so a dot in a type pattern is a package
            // separator: `java.lang.String` must find `java/lang/String`.
            XrefKind::Type => Ok(Query::Type(Pattern::search(
                pattern.replace('.', "/"),
                case,
            ))),
            XrefKind::Method | XrefKind::Field => {
                let query = MemberQuery {
                    name: Some(Pattern::search(pattern, case)),
                    // `--class` names one class. A pattern over owners is `classes -f`.
                    class: target
                        .class
                        .as_deref()
                        .map(format_class_name)
                        .transpose()?
                        .map(Pattern::coordinate),
                    descriptor: None,
                };
                Ok(if target.kind == XrefKind::Method {
                    Query::Method(query)
                } else {
                    Query::Field(query)
                })
            }
        },
        (None, None) => unreachable!("resolve() rejects an empty target"),
    }
}

/// A type coordinate: a descriptor is used as written, a dotted name is normalized.
fn type_coordinate(coordinate: &str) -> String {
    if coordinate.starts_with('[') || (coordinate.starts_with('L') && coordinate.ends_with(';')) {
        coordinate.to_owned()
    } else {
        format_class_name(coordinate).unwrap_or_else(|_| coordinate.to_owned())
    }
}

impl Command {
    pub(crate) fn input(&self) -> &Path {
        match self {
            Self::Getclass(args) => &args.apk_path,
            Self::Entries(args) => &args.apk_path,
            Self::Extract(args) => &args.apk_path,
            Self::Strings(args) => &args.apk_path,
            Self::Xref(args) => &args.apk_path,
            Self::Manifest(args) => &args.apk_path,
            Self::Classes(args) => &args.apk_path,
            Self::Methods(args) => &args.apk_path,
            Self::Fields(args) => &args.apk_path,
            Self::GetMethod(args) => &args.apk_path,
        }
    }

    /// Whether this invocation asked for the perf record.
    pub(crate) fn perf(&self) -> bool {
        match self {
            Self::Getclass(args) => args.perf,
            Self::Entries(args) => args.perf,
            Self::Extract(args) => args.perf,
            Self::Strings(args) => args.perf,
            Self::Xref(args) => args.perf,
            Self::Manifest(args) => args.perf,
            Self::Classes(args) => args.perf,
            Self::Methods(args) => args.perf,
            Self::Fields(args) => args.perf,
            Self::GetMethod(args) => args.perf,
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Getclass(_) => "get-class",
            Self::Entries(_) => "entries",
            Self::Extract(_) => "extract",
            Self::Strings(_) => "strings",
            Self::Xref(_) => "xref",
            Self::Manifest(_) => "manifest",
            Self::Classes(_) => "classes",
            Self::Methods(_) => "methods",
            Self::Fields(_) => "fields",
            Self::GetMethod(_) => "get-method",
        }
    }
}
