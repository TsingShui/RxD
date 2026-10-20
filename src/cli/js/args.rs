use super::super::{default_threads, positive};
use clap::{Args, Subcommand, ValueEnum};
use rxd_js::Query;
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

#[derive(Args)]
pub(crate) struct Common {
    /// Local UTF-8 JavaScript file; outline/find also accept directories.
    pub(super) input: PathBuf,
    #[arg(long, value_enum, default_value = "auto")]
    pub(super) source_type: Mode,
    /// Select JS/JSX/TS/TSX independently of script/module mode.
    #[arg(long, value_enum, default_value = "auto")]
    pub(super) syntax: SyntaxArg,
    /// Atomically replace this output file after a successful query.
    #[arg(short, long)]
    pub(super) output: Option<PathBuf>,
    /// Print stage timings to stderr.
    #[arg(long)]
    pub(super) debug: bool,
    /// Emit versioned phase timings and counters as JSONL on stderr.
    #[arg(long)]
    pub(super) perf: bool,
    // Every ceiling comes from rxd_core::Budget; the CLI exposes no knob for one.
}

#[derive(Args)]
pub(crate) struct Scan {
    /// Compatibility flag: directories are scanned recursively by default.
    #[arg(short = 'r', long)]
    pub(super) recursive: bool,
    /// Workers for a directory scan; one parsed input per worker is held at a time.
    #[arg(long, alias = "thread", default_value_t = default_threads(), value_parser = positive)]
    pub(super) threads: usize,
}

/// Selectors shared by the grouped structure listings.
#[derive(Args)]
pub(crate) struct Outline {
    #[arg(long, conflicts_with = "recursive")]
    pub(super) within: Option<String>,
    #[arg(long)]
    pub(super) depth: Option<usize>,
    /// Keep only entities of this exact structural kind (comma-separated list allowed).
    #[arg(long)]
    pub(super) kind: Option<String>,
    /// Keep only entities whose name or context label equals this value.
    #[arg(long)]
    pub(super) name: Option<String>,
    /// Case-insensitive substring over the entity name or context label.
    #[arg(short, long)]
    pub(super) filter: Option<String>,
    /// Explicitly select the default JSONL format.
    #[arg(long)]
    pub(super) jsonl: bool,
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
    /// Find case-sensitive substrings in decoded JS literals (JSONL).
    Find {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[arg(long)]
        string: String,
        /// Also match values proven by bounded, side-effect-free AST evaluation.
        #[arg(long)]
        resolve_static: bool,
        #[arg(long)]
        jsonl: bool,
    },
    /// Find exact lexical identifier occurrences (JSONL).
    Identifiers {
        #[command(flatten)]
        common: Common,
        #[arg(long)]
        name: String,
        #[arg(long, value_enum, default_value = "all")]
        role: IdentifierRoleArg,
        /// Restrict occurrences to this structure and its descendants.
        #[arg(long)]
        within: Option<String>,
        #[arg(long)]
        jsonl: bool,
    },
    /// Find call sites of a direct or member callee name (JSONL).
    Calls {
        #[command(flatten)]
        common: Common,
        #[arg(long)]
        name: String,
        /// Restrict call sites to this structure and its descendants.
        #[arg(long)]
        within: Option<String>,
        #[arg(long)]
        jsonl: bool,
    },
    /// Find static member names, property keys, and JSX attribute names (JSONL).
    Members {
        #[command(flatten)]
        common: Common,
        #[arg(long)]
        name: String,
        /// Restrict occurrences to this structure and its descendants.
        #[arg(long)]
        within: Option<String>,
        #[arg(long)]
        jsonl: bool,
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
    /// List lexical references for a binding/reference at a UTF-8 byte offset (JSONL).
    Xref {
        #[command(flatten)]
        common: Common,
        #[arg(long)]
        byte: u64,
        #[arg(long)]
        jsonl: bool,
    },
    /// Discover lexical bindings and unresolved references (JSONL).
    Symbols {
        #[command(flatten)]
        common: Common,
        /// Include declarations and dependencies in this structure and its descendants.
        #[arg(long)]
        within: Option<String>,
        #[arg(long)]
        jsonl: bool,
    },
    /// Extract original structure source or a symbol's source declaration.
    Get {
        #[command(flatten)]
        common: Common,
        /// The `at` a structure or symbol row prints.
        #[arg(long)]
        at: String,
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
            },
            Self::Functions { outline, .. } => Query::Outline {
                group: Some("functions"),
                within: outline.within,
                depth: outline.depth,
                kind: outline.kind,
                name: outline.name,
                contains: outline.filter,
            },
            Self::Classes { outline, .. } => Query::Outline {
                group: Some("classes"),
                within: outline.within,
                depth: outline.depth,
                kind: outline.kind,
                name: outline.name,
                contains: outline.filter,
            },
            Self::Methods { outline, .. } => Query::Outline {
                group: Some("methods"),
                within: outline.within,
                depth: outline.depth,
                kind: outline.kind,
                name: outline.name,
                contains: outline.filter,
            },
            Self::Find {
                string,
                resolve_static,
                ..
            } => Query::Find {
                string,
                resolve_static,
            },
            Self::Identifiers {
                name, role, within, ..
            } => Query::Identifiers {
                name,
                within,
                role: match role {
                    IdentifierRoleArg::All => rxd_js::IdentifierRole::All,
                    IdentifierRoleArg::Declaration => rxd_js::IdentifierRole::Declaration,
                    IdentifierRoleArg::Reference => rxd_js::IdentifierRole::Reference,
                },
            },
            Self::Calls { name, within, .. } => Query::Calls { name, within },
            Self::Members { name, within, .. } => Query::Members { name, within },
            Self::Locate { byte, .. } => Query::Locate { byte },
            Self::Xref { byte, .. } => Query::Refs { byte },
            Self::Symbols { within, .. } => Query::Symbols { within },
            Self::Get { at, json, .. } => Query::Get { id: at, json },
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Entities { .. } => "entities",
            Self::Functions { .. } => "functions",
            Self::Classes { .. } => "classes",
            Self::Methods { .. } => "methods",
            Self::Find { .. } => "find",
            Self::Identifiers { .. } => "identifiers",
            Self::Calls { .. } => "calls",
            Self::Members { .. } => "members",
            Self::Locate { .. } => "locate",
            Self::Xref { .. } => "xref",
            Self::Symbols { .. } => "symbols",
            Self::Get { .. } => "get",
        }
    }
    pub(super) fn scan(&self) -> Option<&Scan> {
        match self {
            Self::Entities { scan, .. }
            | Self::Functions { scan, .. }
            | Self::Classes { scan, .. }
            | Self::Methods { scan, .. }
            | Self::Find { scan, .. } => Some(scan),
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
            | Self::Identifiers { common, .. }
            | Self::Calls { common, .. }
            | Self::Members { common, .. }
            | Self::Locate { common, .. }
            | Self::Xref { common, .. }
            | Self::Symbols { common, .. }
            | Self::Get { common, .. } => common,
        }
    }
}

impl Command {
    pub(crate) fn debug(&self) -> bool {
        self.common().debug
    }
    pub(crate) fn perf(&self) -> bool {
        self.common().perf
    }
}
