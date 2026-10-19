use super::super::{default_threads, input_bytes, positive};
use clap::{Args, Subcommand, ValueEnum};
use rxd_core::Limits;
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
    /// Input byte cap in bytes (1 through 8589934592); 8 GiB when omitted.
    #[arg(long, value_parser = input_bytes)]
    pub(super) max_input_bytes: Option<usize>,
    #[arg(long, default_value_t = 100_000, value_parser = positive)]
    pub(super) max_entities: usize,
    #[arg(long, default_value_t = 100_000, value_parser = positive)]
    pub(super) max_records: usize,
    #[arg(long, default_value_t = 32 * 1024 * 1024, value_parser = positive)]
    pub(super) max_output_bytes: usize,
    /// AST node nesting budget; checked after parsing.
    #[arg(long, default_value_t = 512, value_parser = positive)]
    pub(super) max_ast_depth: usize,
}

#[derive(Args)]
pub(crate) struct Scan {
    /// Compatibility flag: directories are scanned recursively by default.
    #[arg(short = 'r', long)]
    pub(super) recursive: bool,
    /// Maximum JavaScript files in a recursive scan.
    #[arg(long, default_value_t = 10_000, value_parser = positive)]
    pub(super) max_files: usize,
    /// Workers for a directory scan; one parsed input per worker is held at a time.
    #[arg(long, alias = "thread", default_value_t = default_threads(), value_parser = positive)]
    pub(super) threads: usize,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// List program, functions, classes, and methods (JSONL).
    Outline {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[arg(long, conflicts_with = "recursive")]
        within: Option<String>,
        #[arg(long)]
        depth: Option<usize>,
        /// Keep only entities of this exact structural kind.
        #[arg(long)]
        kind: Option<String>,
        /// Keep only entities whose name or context label equals this value.
        #[arg(long)]
        name: Option<String>,
        /// Explicitly select the default JSONL format.
        #[arg(long)]
        jsonl: bool,
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
        /// Bound the semantic index before allocation.
        #[arg(long, default_value_t = 1_000_000, value_parser = positive)]
        max_semantic_nodes: usize,
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
        #[arg(long, default_value_t = 1_000_000, value_parser = positive)]
        max_semantic_nodes: usize,
        #[arg(long)]
        jsonl: bool,
    },
    /// Extract original structure source or a symbol's source declaration.
    Get {
        #[command(flatten)]
        common: Common,
        #[arg(long)]
        id: String,
        /// Bound semantic analysis for symbol IDs; structure IDs do not build it.
        #[arg(long, default_value_t = 1_000_000, value_parser = positive)]
        max_semantic_nodes: usize,
        #[arg(long)]
        json: bool,
    },
}

impl Command {
    pub(super) fn semantic_nodes(&self) -> usize {
        match self {
            Self::Xref {
                max_semantic_nodes, ..
            }
            | Self::Symbols {
                max_semantic_nodes, ..
            }
            | Self::Get {
                max_semantic_nodes, ..
            } => *max_semantic_nodes,
            _ => Limits::default().semantic_nodes,
        }
    }
    pub(super) fn into_query(self) -> Query {
        match self {
            Self::Outline {
                within,
                depth,
                kind,
                name,
                ..
            } => Query::Outline {
                within,
                depth,
                kind,
                name,
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
            Self::Get { id, json, .. } => Query::Get { id, json },
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Outline { .. } => "outline",
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
            Self::Outline { scan, .. } | Self::Find { scan, .. } => Some(scan),
            _ => None,
        }
    }
    pub(super) fn common(&self) -> &Common {
        match self {
            Self::Outline { common, .. }
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
