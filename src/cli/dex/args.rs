use super::super::default_threads;
use super::super::filter::CaseArgs;
use super::super::xref::{Anchor, Kind as XrefKind, Selector, Target, TargetArgs};
use anyhow::Result;
use clap::{Args, Subcommand};
use rxd_dex::query::{MemberQuery, Pattern, Query, format_class_name};
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Locate a class in an APK and decompile it.
    #[command(name = "get-class")]
    Getclass(GetClassArgs),
    /// List the entries an archive holds, in central-directory order.
    Entries(EntriesArgs),
    /// Write original entry bytes to --output, preserving them exactly.
    ///
    /// `--name` writes one entry to a file; `--all` writes every entry matching
    /// `--filter` (or every entry) under a destination directory. APK-native
    /// libraries are ordinary entries, so this is also how a `.so` is extracted
    /// before it is handed to a native analyzer.
    Extract(ExtractArgs),
    /// List every string in every root DEX of an archive.
    Strings(StringsArgs),
    /// Find code references across every DEX in an APK.
    Xref(XrefArgs),
    /// Decode and print AndroidManifest.xml from an APK.
    Manifest(ManifestArgs),
    /// List classes defined across every DEX in an APK.
    Classes(ClassesArgs),
    /// Decompile one method as Java source.
    ///
    /// `--at` takes the coordinate a `methods` row prints (`Lx/y;->name(proto)`), so a
    /// method found by `methods` or reported by `xref` renders without pulling its class.
    #[command(name = "get-method")]
    GetMethod(GetMethodArgs),
    /// List every field the archive's DEX files declare.
    Fields(FieldsArgs),
    /// List every method the archive's DEX files define.
    ///
    /// A method is reported once per definition; `method_ids` entries that are only
    /// referenced are not listed.
    Methods(MethodsArgs),
}

#[derive(Debug, Args)]
pub struct GetMethodArgs {
    pub apk_path: PathBuf,
    #[command(flatten)]
    pub selector: Selector,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[arg(long, alias = "thread", default_value_t = default_threads())]
    pub threads: usize,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
}

#[derive(Debug, Args)]
pub struct FieldsArgs {
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
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[arg(long, alias = "thread", default_value_t = default_threads())]
    pub threads: usize,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
}

#[derive(Debug, Args)]
pub struct MethodsArgs {
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
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[arg(long, alias = "thread", default_value_t = default_threads())]
    pub threads: usize,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
}

#[derive(Debug, Args)]
pub struct ClassesArgs {
    #[arg(long, alias = "thread", default_value_t = default_threads())]
    pub threads: usize,
    /// Only this class: `Lcom/foo/Bar;` or `com.foo.Bar`.
    #[arg(long)]
    pub class: Option<String>,
    ///  substring filter over Java-style class names. Folded unless `--case sensitive`.
    #[arg(short, long)]
    pub filter: Option<String>,
    #[command(flatten)]
    pub case: CaseArgs,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("index")
        .required(true)
        .multiple(false)
        .args(["method_index", "field_index"])
))]
pub struct MemberByIndexArgs {
    /// One or more code inputs - an APK, or bare DEX files - tried in this order.
    #[arg(long = "apk", required = true, num_args = 1.., value_name = "FILE")]
    pub apk: Vec<PathBuf>,
    /// Declaring class: `Lcom/foo/Bar;` or `com.foo.Bar`.
    #[arg(long)]
    pub descriptor: String,
    /// The index a runtime reports for a method: the `method_ids` index.
    #[arg(long)]
    pub method_index: Option<u32>,
    /// The index a runtime reports for a field: instance fields first, then statics
    /// (`ifields_` then `sfields_` order). Not the `field_ids` index - the row prints
    /// that one too, so the two can be told apart.
    #[arg(long)]
    pub field_index: Option<u32>,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[arg(long, alias = "thread", default_value_t = default_threads())]
    pub threads: usize,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
}

#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("selection")
        .required(true)
        .multiple(false)
        .args(["name", "all"])
))]
pub struct ExtractArgs {
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
    /// Destination file (--name) or directory (--all).
    #[arg(short, long)]
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
    /// Start at this match instead of the first, counted in the order they are printed: with
    /// `--limit` this is the other half of a page.
    #[arg(long)]
    pub offset: Option<usize>,
    #[arg(long, alias = "thread", default_value_t = default_threads())]
    pub threads: usize,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct EntriesArgs {
    ///  substring over the entry name. Folded unless `--case sensitive`.
    #[arg(short, long)]
    pub filter: Option<String>,
    #[command(flatten)]
    pub case: CaseArgs,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct ManifestArgs {
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct GetClassArgs {
    #[command(flatten)]
    pub selector: Selector,
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
    #[arg(long, alias = "thread", default_value_t = default_threads())]
    pub threads: usize,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct XrefArgs {
    /// Emit the analysis perf record as JSONL on stderr.
    #[arg(long)]
    pub perf: bool,
    #[arg(long, alias = "thread", default_value_t = default_threads())]
    pub threads: usize,
    #[command(flatten)]
    pub target: TargetArgs,
    pub apk_path: PathBuf,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

impl XrefArgs {
    /// The backend query a resolved target asks for.
    ///
    /// A coordinate becomes an exact identity: the class is normalized to the descriptor
    /// form the tables hold, and the descriptor selects one overload. A content search stays
    /// a substring. The two never mix in one query.
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
