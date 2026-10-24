use super::super::filter::CaseArgs;
use super::super::{default_threads, positive};
use clap::{Args, Subcommand, ValueEnum};
use rxd_js::{Query, RefsTarget};
use std::path::PathBuf;
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Mode {
    Auto,
    Script,
    Module,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum SyntaxArg {
    Auto,
    Js,
    Jsx,
    Ts,
    Tsx,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum IdentifierRoleArg {
    All,
    Declaration,
    Reference,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum MemberAccessArg {
    All,
    Read,
    Write,
}

#[derive(Args)]
pub(crate) struct Common {
    /// Local UTF-8 JavaScript file; structure/find/resources listings also accept directories.
    pub(super) input: PathBuf,
    #[arg(long, value_enum, default_value = "auto")]
    pub(super) source_type: Mode,
    /// Select JS/JSX/TS/TSX independently of script/module mode.
    #[arg(long, value_enum, default_value = "auto")]
    pub(super) syntax: SyntaxArg,
    /// Atomically replace this output file after a successful query.
    #[arg(short, long)]
    pub(super) output: Option<PathBuf>,
    /// Emit versioned phase timings and counters as JSONL on stderr.
    #[arg(long)]
    pub(super) perf: bool,
    // Every ceiling comes from rxd_core::Budget; the CLI exposes no knob for one.
}

#[derive(Args)]
pub(crate) struct Scan {
    /// Workers for a directory scan; one parsed input per worker is held at a time.
    #[arg(long, alias = "thread", default_value_t = default_threads(), value_parser = positive)]
    pub(super) threads: usize,
}

/// Own-name selectors shared by syntax and lexical listings.
#[derive(Args)]
pub(crate) struct Names {
    /// Match this exact name (case-sensitive), independently of the substring filter.
    #[arg(long)]
    name: Option<String>,
    /// Substring of the row's own name; folded unless --case sensitive.
    #[arg(short, long)]
    filter: Option<String>,
    #[command(flatten)]
    case: CaseArgs,
}
impl Names {
    fn query(self) -> rxd_js::NameQuery {
        rxd_js::NameQuery {
            name: self.name,
            filter: self.filter,
            case_sensitive: self.case.sensitive(),
        }
    }
}

/// Selectors shared by the grouped structure listings.
#[derive(Args)]
pub(crate) struct Outline {
    #[arg(long)]
    pub(super) within: Option<String>,
    #[arg(long)]
    pub(super) depth: Option<usize>,
    /// Keep only entities of this exact structural kind (comma-separated list allowed).
    #[arg(long)]
    pub(super) kind: Option<String>,
    /// Keep only entities whose name or context label equals this value.
    #[arg(long)]
    pub(super) name: Option<String>,
    /// Substring over the entity name or context label, folded unless `--case sensitive`.
    #[arg(short, long)]
    pub(super) filter: Option<String>,
    #[command(flatten)]
    pub(super) case: CaseArgs,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// List every structural entity, including the file root (JSONL).
    Entities {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        outline: Outline,
    },
    /// List function declarations, expressions, and arrows (JSONL).
    Functions {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        outline: Outline,
    },
    /// List classes, interfaces, enums, aliases, and namespaces (JSONL).
    Classes {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        outline: Outline,
    },
    /// List class and object members: methods, constructors, getters, setters (JSONL).
    Methods {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        outline: Outline,
    },
    /// Find substrings in decoded JS literals (JSONL).
    Find {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        /// Substring of a decoded literal value; folded unless `--case sensitive`.
        #[arg(short, long)]
        filter: String,
        #[command(flatten)]
        case: CaseArgs,
        /// Also match values proven by bounded, side-effect-free AST evaluation.
        #[arg(long)]
        resolve_static: bool,
    },
    /// List syntax-backed loading candidates, including unresolved expressions (JSONL).
    Resources {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        /// Select one source role; these are candidates, not a runtime dependency graph.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(rxd_js::RESOURCE_KINDS.iter().copied()))]
        kind: Option<String>,
        /// Resolved means a known string, not a resolved URL or permission to fetch it.
        #[arg(long, value_parser = ["resolved", "unresolved"])]
        resolution: Option<String>,
        /// Substring over the decoded reference, or original expression when unresolved.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        /// Follow earlier, unwritten const strings in the same lexical scope; no calls or object reads.
        #[arg(long)]
        resolve_bindings: bool,
        /// Restrict loading sites to this structure and its descendants; single file only.
        #[arg(long)]
        within: Option<String>,
    },
    /// List source-backed object entries, array elements, initializers and arguments.
    Values {
        #[command(flatten)]
        common: Common,
        /// Scope to a real structure, source fragment or binding declaration.
        #[arg(long)]
        within: Option<String>,
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(rxd_js::VALUE_KINDS.iter().copied()))]
        kind: Option<String>,
        /// The row's own name is its property/binding, target or syntactic index.
        #[command(flatten)]
        names: Names,
    },
    /// List lexical identifier occurrences (JSONL).
    Identifiers {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        names: Names,
        #[arg(long, value_enum, default_value = "all")]
        role: IdentifierRoleArg,
        /// Restrict occurrences to this structure and its descendants.
        #[arg(long)]
        within: Option<String>,
    },
    /// List statically named direct/member call and constructor sites (JSONL).
    Calls {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        names: Names,
        /// Restrict call sites to this structure and its descendants.
        #[arg(long)]
        within: Option<String>,
    },
    /// Find static member names, property keys, and JSX attribute names (JSONL).
    Members {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        names: Names,
        /// Select syntactic reads or writes; compound assignments and updates match both.
        #[arg(long, value_enum, default_value = "all")]
        access: MemberAccessArg,
        /// Restrict occurrences to this structure and its descendants.
        #[arg(long)]
        within: Option<String>,
    },
    /// Locate the innermost structure covering a UTF-8 byte offset (JSON).
    Locate {
        #[command(flatten)]
        common: Common,
        #[arg(long)]
        byte: u64,
        #[arg(long)]
        json: bool,
    },
    /// List lexical references for a binding/reference (JSONL).
    Xref {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        selector: super::super::xref::Selector,
        /// A byte offset to anchor on instead, including one inside a multi-byte identifier.
        #[arg(long, conflicts_with_all = ["at", "filter"])]
        byte: Option<u64>,
        /// Substring over binding names, folded unless `--case sensitive`.
        #[arg(short, long, conflicts_with_all = ["at", "byte"], requires = "kind")]
        filter: Option<String>,
        /// Only `symbol`: the one reference kind JS has is a lexical binding.
        #[arg(long, value_name = "KIND", requires = "filter")]
        kind: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
    },
    /// Discover lexical bindings and unresolved references (JSONL).
    Symbols {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        names: Names,
        /// Include declarations and dependencies in this structure and its descendants.
        #[arg(long)]
        within: Option<String>,
    },
    /// Slice the original bytes at a coordinate: a structure body, a symbol's declaration,
    /// or an explicit range.
    Slice {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        selector: super::super::xref::Selector,
        /// Bound semantic analysis for symbol IDs; structure IDs do not build it.
        #[arg(long)]
        json: bool,
    },
}

impl Command {
    pub(super) fn into_query(self) -> Query {
        match self {
            Self::Entities { outline, .. } => Query::Outline {
                group: None,
                within: outline.within,
                depth: outline.depth,
                kind: outline.kind,
                name: outline.name,
                contains: outline.filter,
                contains_case_sensitive: outline.case.sensitive(),
            },
            Self::Functions { outline, .. } => Query::Outline {
                group: Some("functions"),
                within: outline.within,
                depth: outline.depth,
                kind: outline.kind,
                name: outline.name,
                contains: outline.filter,
                contains_case_sensitive: outline.case.sensitive(),
            },
            Self::Classes { outline, .. } => Query::Outline {
                group: Some("classes"),
                within: outline.within,
                depth: outline.depth,
                kind: outline.kind,
                name: outline.name,
                contains: outline.filter,
                contains_case_sensitive: outline.case.sensitive(),
            },
            Self::Methods { outline, .. } => Query::Outline {
                group: Some("methods"),
                within: outline.within,
                depth: outline.depth,
                kind: outline.kind,
                name: outline.name,
                contains: outline.filter,
                contains_case_sensitive: outline.case.sensitive(),
            },
            Self::Find {
                filter,
                case,
                resolve_static,
                ..
            } => Query::Find {
                filter,
                case_sensitive: case.sensitive(),
                resolve_static,
            },
            Self::Resources {
                kind,
                resolution,
                filter,
                case,
                within,
                resolve_bindings,
                ..
            } => Query::Resources(rxd_js::ResourcesQuery {
                kind,
                resolution,
                filter,
                case_sensitive: case.sensitive(),
                within,
                resolve_bindings,
            }),
            Self::Values {
                within,
                kind,
                names,
                ..
            } => Query::Values(rxd_js::ValuesQuery {
                within,
                kind,
                names: names.query(),
            }),
            Self::Identifiers {
                names,
                role,
                within,
                ..
            } => Query::Identifiers {
                names: names.query(),
                within,
                role: match role {
                    IdentifierRoleArg::All => rxd_js::IdentifierRole::All,
                    IdentifierRoleArg::Declaration => rxd_js::IdentifierRole::Declaration,
                    IdentifierRoleArg::Reference => rxd_js::IdentifierRole::Reference,
                },
            },
            Self::Calls { names, within, .. } => Query::Calls {
                names: names.query(),
                within,
            },
            Self::Members {
                names,
                access,
                within,
                ..
            } => Query::Members {
                names: names.query(),
                access: match access {
                    MemberAccessArg::All => rxd_js::MemberAccess::All,
                    MemberAccessArg::Read => rxd_js::MemberAccess::Read,
                    MemberAccessArg::Write => rxd_js::MemberAccess::Write,
                },
                within,
            },
            Self::Locate { byte, .. } => Query::Locate { byte },
            Self::Xref { byte, .. } => Query::Refs {
                target: match byte {
                    Some(byte) => RefsTarget::Byte(byte),
                    None => unreachable!("xref needs a resolved selector or a byte offset"),
                },
            },
            Self::Symbols { names, within, .. } => Query::Symbols {
                names: names.query(),
                within,
            },
            Self::Slice { .. } => unreachable!("slice needs the resolved coordinate"),
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Entities { .. } => "entities",
            Self::Functions { .. } => "functions",
            Self::Classes { .. } => "classes",
            Self::Methods { .. } => "methods",
            Self::Find { .. } => "find",
            Self::Resources { .. } => "resources",
            Self::Values { .. } => "values",
            Self::Identifiers { .. } => "identifiers",
            Self::Calls { .. } => "calls",
            Self::Members { .. } => "members",
            Self::Locate { .. } => "locate",
            Self::Xref { .. } => "xref",
            Self::Symbols { .. } => "symbols",
            Self::Slice { .. } => "slice",
        }
    }
    pub(super) fn scan(&self) -> Option<&Scan> {
        match self {
            Self::Entities { scan, .. }
            | Self::Functions { scan, .. }
            | Self::Classes { scan, .. }
            | Self::Methods { scan, .. }
            | Self::Find { scan, .. }
            | Self::Resources { scan, .. } => Some(scan),
            _ => None,
        }
    }
    pub(super) fn common(&self) -> &Common {
        match self {
            Self::Entities { common, .. }
            | Self::Functions { common, .. }
            | Self::Classes { common, .. }
            | Self::Methods { common, .. }
            | Self::Find { common, .. }
            | Self::Resources { common, .. }
            | Self::Values { common, .. }
            | Self::Identifiers { common, .. }
            | Self::Calls { common, .. }
            | Self::Members { common, .. }
            | Self::Locate { common, .. }
            | Self::Xref { common, .. }
            | Self::Symbols { common, .. }
            | Self::Slice { common, .. } => common,
        }
    }
}

impl Command {
    pub(crate) fn perf(&self) -> bool {
        self.common().perf
    }
}
