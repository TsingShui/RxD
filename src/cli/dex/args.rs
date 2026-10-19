use super::super::default_threads;
use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use rxd_dex::query::{ClassQuery, MemberQuery, Query, format_class_name, fuzzy_class_pattern};
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Locate a class in an APK and decompile it.
    #[command(name = "get-class")]
    Getclass(GetClassArgs),
    /// List the entries an archive holds, in central-directory order.
    Entries(EntriesArgs),
    /// List every string in every root DEX of an archive.
    Strings(StringsArgs),
    /// Find code references across every DEX in an APK.
    Xref(XrefArgs),
    /// Decode and print AndroidManifest.xml from an APK.
    Manifest(ManifestArgs),
    /// List classes defined across every DEX in an APK.
    Classes(ClassesArgs),
    /// List every method the archive's DEX files define.
    ///
    /// A method is reported once per definition; `method_ids` entries that are only
    /// referenced are not listed.
    Methods(MethodsArgs),
}

#[derive(Debug, Args)]
pub struct MethodsArgs {
    pub apk_path: PathBuf,
    /// Only list methods defined by this class: `Lcom/foo/Bar;` or `com.foo.Bar`.
    #[arg(long)]
    pub class: Option<String>,
    /// Case-insensitive substring filter over the method name.
    #[arg(short, long)]
    pub filter: Option<String>,
    /// Append each method's access flags to its record.
    #[arg(long)]
    pub with_flags: bool,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[arg(long, alias = "thread", default_value_t = default_threads())]
    pub threads: usize,
    /// Print phase timings on stderr.
    #[arg(long)]
    pub debug: bool,
}

#[derive(Debug, Args)]
pub struct ClassesArgs {
    #[arg(long, alias = "thread", default_value_t = default_threads())]
    pub threads: usize,
    /// Case-insensitive substring filter over Java-style class names.
    #[arg(short, long)]
    pub filter: Option<String>,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Print phase timings on stderr.
    #[arg(long)]
    pub debug: bool,
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
    /// Print phase timings on stderr.
    #[arg(long)]
    pub debug: bool,
}

#[derive(Debug, Args)]
pub struct StringsArgs {
    /// Only strings containing this text, ignoring case.
    #[arg(long)]
    pub filter: Option<String>,
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
    /// Print phase timings on stderr.
    #[arg(long)]
    pub debug: bool,
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct EntriesArgs {
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Print phase timings on stderr.
    #[arg(long)]
    pub debug: bool,
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct ManifestArgs {
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Print phase timings on stderr.
    #[arg(long)]
    pub debug: bool,
    pub apk_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct GetClassArgs {
    #[arg(long)]
    pub debug: bool,
    #[arg(long, alias = "thread", default_value_t = default_threads())]
    pub threads: usize,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    pub apk_path: PathBuf,
    pub dalvik_class: String,
}

#[derive(Debug, Args)]
pub struct XrefArgs {
    /// Print phase timings on stderr.
    #[arg(long)]
    pub debug: bool,
    #[arg(long, alias = "thread", default_value_t = default_threads())]
    pub threads: usize,
    pub apk_path: PathBuf,
    #[command(subcommand)]
    pub kind: FindKind,
}

impl XrefArgs {
    pub fn query(&self) -> Result<Query> {
        match &self.kind {
            FindKind::String(q) => Ok(Query::String(q.value.clone())),
            FindKind::Type(q) => Ok(Query::Type(q.value.clone())),
            FindKind::Method(q) => Ok(Query::Method(q.member_query("method")?)),
            FindKind::Field(q) => Ok(Query::Field(q.member_query("field")?)),
        }
    }

    pub fn query_json(&self) -> serde_json::Value {
        use serde_json::json;
        match &self.kind {
            FindKind::String(q) => json!({"kind":"string", "value":q.value}),
            FindKind::Type(q) => json!({"kind":"type", "value":q.value}),
            FindKind::Method(q) => {
                json!({"kind":"method", "name":q.name, "class":q.class_name, "fuzzy_class":q.fuzzy_class})
            }
            FindKind::Field(q) => {
                json!({"kind":"field", "name":q.name, "class":q.class_name, "fuzzy_class":q.fuzzy_class})
            }
        }
    }

    pub fn output_path(&self) -> Option<&Path> {
        match &self.kind {
            FindKind::String(q) | FindKind::Type(q) => q.output.as_deref(),
            FindKind::Method(q) | FindKind::Field(q) => q.output.as_deref(),
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum FindKind {
    /// Instructions loading a string literal containing the value (case-sensitive).
    String(ValueQuery),
    /// Instructions referencing a type whose name contains the value (case-sensitive).
    Type(ValueQuery),
    /// Call sites of a method, optionally restricted to an owner class.
    Method(MemberArgs),
    /// Access sites of a field, optionally restricted to an owner class.
    Field(MemberArgs),
}

#[derive(Debug, Args)]
pub struct ValueQuery {
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Literal substring to search for (case-sensitive).
    pub value: String,
}

#[derive(Debug, Args)]
pub struct MemberArgs {
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Member name to search for (case-sensitive exact match).
    pub name: Option<String>,
    /// Owner class, as `Lcom/foo/Bar;` or `com.foo.Bar` (exact unless `--fuzzy-class`).
    #[arg(long = "class")]
    pub class_name: Option<String>,
    /// Treat `--class` as a case-sensitive substring instead of an exact name.
    #[arg(long)]
    pub fuzzy_class: bool,
}

impl MemberArgs {
    fn member_query(&self, kind: &str) -> Result<MemberQuery> {
        let name = self
            .name
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let class = match self.class_name.as_deref().filter(|name| !name.is_empty()) {
            None => None,
            Some(name) if self.fuzzy_class => Some(ClassQuery::Fuzzy(fuzzy_class_pattern(name))),
            Some(name) => Some(ClassQuery::Exact(format_class_name(name)?)),
        };
        if name.is_none() && class.is_none() {
            bail!("{kind} query needs at least one of class or {kind} name");
        }
        Ok(MemberQuery { name, class })
    }
}

impl Command {
    /// Whether this invocation asked for `--debug` diagnostics.
    pub(crate) fn debug(&self) -> bool {
        match self {
            Self::Getclass(args) => args.debug,
            Self::Entries(args) => args.debug,
            Self::Strings(args) => args.debug,
            Self::Xref(args) => args.debug,
            Self::Manifest(args) => args.debug,
            Self::Classes(args) => args.debug,
            Self::Methods(args) => args.debug,
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Getclass(_) => "get-class",
            Self::Entries(_) => "entries",
            Self::Strings(_) => "strings",
            Self::Xref(_) => "xref",
            Self::Manifest(_) => "manifest",
            Self::Classes(_) => "classes",
            Self::Methods(_) => "methods",
        }
    }
}
