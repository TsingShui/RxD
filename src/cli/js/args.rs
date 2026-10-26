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
    /// Local UTF-8 JavaScript file; listings and name searches also accept directories.
    pub(super) input: PathBuf,
    #[cfg(any(feature = "backend-asar", feature = "backend-zip"))]
    #[command(flatten)]
    pub(super) container: super::super::input::EntryArgs,
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
    /// Copy an entity.at to select its descendants; single-file queries only.
    #[arg(long)]
    pub(super) within: Option<String>,
    /// Maximum containment depth, relative to --within or the file root; zero is valid.
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
    /// Also recover labels from proven literal-table lookups.
    #[arg(long)]
    pub(super) resolve_static: bool,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// List every structural entity, including the file root (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js entities bundle.js --name login\nrxd js entities bundle.js --within '<entity.at>' --depth 1\nOutput: entity JSONL, including program root; at/parent_at are file-scoped coordinates.\nNext: rxd js slice bundle.js --at '<entity.at>'.\nUse a known name/label or selected scope instead of a full-file inventory.\n--kind accepts comma-separated structural kinds; --name is exact, -f a substring.\n--within/--depth select containment, not calls. Directories scan files in path order."
    )]
    Entities {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        outline: Outline,
    },
    /// List function declarations, expressions, and arrows (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js functions bundle.js --name login\nrxd js functions bundle.js --within '<entity.at>' --depth 1\nOutput: entity JSONL restricted to functions/arrows. Copy at into slice.\n--name matches an exact name or context label; -f matches a substring of either.\nAnonymous helpers may have detail.label rather than detail.name.\nFollow a known helper by name instead of enumerating every function. A directory is scanned recursively."
    )]
    Functions {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        outline: Outline,
    },
    /// List classes, interfaces, enums, aliases, and namespaces (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js classes bundle.ts -f Client\nOutput: entity JSONL for class/type groups. Copy at into slice for original bytes.\nJSX/TSX need their file extension or --syntax override; no transpilation or runtime."
    )]
    Classes {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        outline: Outline,
    },
    /// List class and object members: methods, constructors, getters, setters (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js methods bundle.js -f login\nOutput: entity JSONL for methods/constructors/getters/setters. Copy at into slice.\nParent means syntactic containment, not a call target; --name is exact, -f a substring."
    )]
    Methods {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        outline: Outline,
    },
    /// List retained file and bundler module boundaries (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js modules bundle.js\nrxd js modules bundle.js --name 42\nOutput: module JSONL with retained module_id (null for the file root), registry_at, chunk_ids and import/export counts.\nCopy module.at into slice; --within selects a file-local structural range.\nWebpack factory invocation and chunk registration are syntax evidence; absent boundaries do not reconstruct original files.\nentry: true marks a direct initializer invocation, not a runtime call graph."
    )]
    Modules {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        names: Names,
        #[arg(long)]
        within: Option<String>,
    },
    /// List ESM, CommonJS and retained bundler import sites (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js imports entry.mjs\nrxd js imports bundle.js --within '<module.at>'\nOutput: import JSONL with source/expression, imported/local names, syntax and type_only.\n--name/-f match the imported name, falling back to source for a side-effect import.\nCopy at for a local binding, or context_at for the original import/loading site. Dynamic expressions stay explicit."
    )]
    Imports {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        names: Names,
        #[arg(long)]
        within: Option<String>,
    },
    /// List exported names, definitions and re-export sites (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js exports entry.mjs --name login\nrxd js exports src/ -f login\nOutput: export JSONL with exported/local names, source/imported re-export evidence, syntax and type_only.\n--name/-f match the exported name; at selects a lexical definition where available.\nUse context_at for an export site. A default expression is not a live alias of an identifier it reads."
    )]
    Exports {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        names: Names,
        #[arg(long)]
        within: Option<String>,
    },
    /// Enumerate string occurrences and optional proven expressions (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js strings bundle.js\nrxd js strings src/ --resolve-static\nOptional -f/--filter searches values; occurrences are not deduplicated.\n--resolution literal|static|decoder filters provenance; computed results still need --resolve-static.\n--within selects a real structure/fragment/binding declaration; filtering does not narrow input safety validation.\nOutput: the same match JSONL as find, in original byte order; use context_at with slice.\n--resolve-static includes bounded expressions composed from proven decoder calls, not a complete decrypted table."
    )]
    Strings {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        /// Optional substring of a string value; folded unless --case sensitive.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        /// Also enumerate bounded closed expressions and proven decoder values.
        #[arg(long)]
        resolve_static: bool,
        /// Filter provenance without implicitly enabling --resolve-static; literal includes template parts.
        #[arg(long,value_parser=["literal","static","decoder"])]
        resolution: Option<String>,
        /// Select an actual structure, fragment or binding declaration; single file only.
        #[arg(long)]
        within: Option<String>,
    },
    /// Find substrings in decoded JS literals (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js find bundle.js -f '/api/login'\nrxd js find bundle.js -f login --resolve-static\nRequired: non-empty -f/--filter. Searches decoded literals, not arbitrary source text.\nOutput: match JSONL; use context_at for a local syntax fragment, parent_at for its entity.\nNext: rxd js slice bundle.js --at '<match.context_at>'.\n--resolve-static folds bounded closed AST expressions and recognized literal-table decoders."
    )]
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
        /// Also match values proven by bounded side-effect-free evaluation and table decoders.
        #[arg(long)]
        resolve_static: bool,
    },
    /// List syntax-backed loading candidates, including unresolved expressions (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js resources bundle.js --kind request\nOutput: resource JSONL with known strings or unresolved loading expressions.\nUse context_at with slice; known text is not a resolved URL or permission to fetch.\n--resolve-bindings follows earlier unwritten local const strings only."
    )]
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
        /// Resolve supported literal-table decoder calls in loading expressions.
        #[arg(long)]
        resolve_static: bool,
        /// Restrict loading sites to this structure and its descendants; single file only.
        #[arg(long)]
        within: Option<String>,
    },
    /// List source-backed object entries, array elements, initializers and arguments.
    #[command(
        after_long_help = "Examples:\nrxd js values bundle.js --within '<entity.at>' --kind property\nrxd js values bundle.js --name key --kind initializer\nOutput: value JSONL for syntax-backed slots; inspect detail.value/resolution, not runtime values.\nUse a named slot to inspect a constant without fetching unrelated declarations.\n--within accepts a structure, fragment or symbol coordinate; use result context_at with slice."
    )]
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
        /// Resolve supported literal-table values and computed keys.
        #[arg(long)]
        resolve_static: bool,
    },
    /// List lexical identifier occurrences (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js identifiers bundle.js --name token --role reference\nOutput: identifier JSONL with occurrence ranges and local context_at.\nProperty foo in obj.foo is not a lexical identifier; obj[key] does reference key.\nNext: slice --at '<identifier.context_at>', or xref --byte '<identifier.start>'."
    )]
    Identifiers {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        names: Names,
        #[arg(long, value_enum, default_value = "all")]
        role: IdentifierRoleArg,
        /// Restrict occurrences to this structure and its descendants.
        #[arg(long)]
        within: Option<String>,
    },
    /// List statically named direct/member call and constructor sites (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js calls bundle.js --name fetch\nrxd js calls bundle.js --within '<entity.at>'\nOutput: call JSONL, direct/member/new sites. Static syntax only, not return values or a call graph.\nUse context_at with slice for the local call, parent_at for its enclosing structure.\nAfter FETCH, scope calls with --within and locate only the missing helper definitions.\n--resolve-static also names computed callee keys a literal-table decoder proves."
    )]
    Calls {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        names: Names,
        /// Restrict call sites to this structure and its descendants.
        #[arg(long)]
        within: Option<String>,
        /// Also name callee keys proven by a recognized literal-table decoder.
        #[arg(long)]
        resolve_static: bool,
    },
    /// Find static member names, property keys, and JSX attribute names (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js members bundle.js --name token --access write\nOutput: member JSONL for known keys/access sites; dynamic computed keys stay unresolved.\nreceiver is the original object text of a static path (window, this, a.b), else null.\n--access filters actual property reads/writes, not every declaration.\nNext: rxd js slice bundle.js --at '<member.context_at>'.\n--resolve-static also names computed keys a literal-table decoder proves."
    )]
    Members {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        names: Names,
        /// Select syntactic reads or writes; compound assignments and updates match both.
        #[arg(long, value_enum, default_value = "all")]
        access: MemberAccessArg,
        /// Restrict occurrences to this structure and its descendants.
        #[arg(long)]
        within: Option<String>,
        /// Also name keys proven by a recognized literal-table decoder.
        #[arg(long)]
        resolve_static: bool,
    },
    /// Locate the innermost structure covering a UTF-8 byte offset (JSON).
    #[command(
        after_long_help = "Examples:\nrxd js locate bundle.js --byte 120\nRequired: UTF-8 byte offset, not a character/line number; inside input, not EOF.\nOutput: one location JSON record with detail.entity and ancestors.\nNext: rxd js slice bundle.js --at '<detail.entity.at>'.\nUse actual byte offsets from query rows or rg -boF; numbers here are examples."
    )]
    Locate {
        #[command(flatten)]
        common: Common,
        /// Original UTF-8 byte offset from a result start or rg -boF; not a line number.
        #[arg(long)]
        byte: u64,
        #[arg(long)]
        json: bool,
    },
    /// List lexical references for a binding/reference (JSONL).
    #[command(after_long_help = "Examples:\nrxd js symbols bundle.js -f token\nrxd js xref bundle.js --at '<symbol.at>'\nrxd js xref bundle.js --byte 120\nrxd js xref bundle.js -m token --kind symbol\nRequired: exactly one of --at/--byte/--match; --match requires --kind symbol.\n--at copies symbols.at or a lexical identifier coordinate; --byte must be inside its name.\nOutput: reference JSONL; start/end are actual occurrences, context_at/parent_at route to slice.\nA file resolves lexical bindings; directory --match aggregates separate file-local results.\nrxd js xref src/ --file auth.mjs --at '<symbol.at>'\nDirectory --file follows proven relative ESM imports/re-exports, not same-name guesses.\nOnly exact ./ and ../ file paths are linked; packages, namespace properties and URL-like specifiers stay unlinked.\nMCP accepts a JS directory snapshot; retain it with rxd_open backend=js for repeated project queries.", group(clap::ArgGroup::new("anchor").required(true).args(["at", "byte", "filter"])))]
    Xref {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        /// On a directory, anchor --at/--byte in this relative file and follow exact relative ESM links.
        #[arg(long, conflicts_with = "filter")]
        file: Option<PathBuf>,
        #[command(flatten)]
        selector: super::super::xref::Selector,
        /// A byte offset to anchor on instead, including one inside a multi-byte identifier.
        #[arg(long, conflicts_with_all = ["at", "filter"])]
        byte: Option<u64>,
        /// Substring over binding names, folded unless `--case sensitive`.
        #[arg(short = 'm', long = "match", conflicts_with_all = ["at", "byte"], requires = "kind")]
        filter: Option<String>,
        /// Only `symbol`: the one reference kind JS has is a lexical binding.
        #[arg(long, value_name = "KIND", value_parser = ["symbol"], requires = "filter")]
        kind: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
    },
    /// Discover lexical bindings and unresolved references (JSONL).
    #[command(
        after_long_help = "Examples:\nrxd js symbols bundle.js --name token\nrxd js symbols bundle.js --within '<entity.at>'\nOutput: symbol and unresolved_reference JSONL; relation is declared or external.\nCopy symbol.at into xref for uses, or slice for its canonical declaration.\nUse this to follow a missing declaration rather than rereading the whole file.\nUnresolved references have no symbol.at; do not invent a binding for them.\n--within requires an entity coordinate, not a fragment coordinate."
    )]
    Symbols {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        scan: Scan,
        #[command(flatten)]
        names: Names,
        /// Include declarations and dependencies in this structure and its descendants.
        #[arg(long)]
        within: Option<String>,
    },
    /// Recover an obfuscator.io string table and literal decoder calls.
    #[command(
        after_long_help = "Examples:\nrxd js deobfuscate obfuscated.js\nrxd js deobfuscate obfuscated.js -o cleaned.js\nOutput: the original source with recovered literals; unresolved calls keep their setup.\nRecognises wrapped string-array factories, rotators and decoders called with literal arguments.\nDecoder setup runs in a bounded QuickJS runtime inside the analysis worker with no host I/O."
    )]
    Deobfuscate {
        #[command(flatten)]
        common: Common,
    },
    /// Slice the original bytes at a coordinate: a structure body, a symbol's declaration,
    /// or an explicit range.
    #[command(after_long_help = "Examples:\nrxd js functions bundle.js --name login\nrxd js slice bundle.js --at '<entity.at>'\nrxd js slice bundle.js --at '<match.context_at>' --json\nRequired: --at copied from this input. Structure: its body; fragment: local original syntax;\nsymbol: canonical declaration. Do not pass a raw byte offset, name or file hash.\nOutput: byte-exact source on stdout with no added newline. Use this default for code reading;\n--json adds a source record when metadata is needed; read detail.source.\nFollow missing helpers with functions/symbols --name, not a whole-file reread.\n-o atomically writes instead of stdout.", mut_arg("at", |arg| arg.required(true).value_name("SOURCE_AT").help("Copy entity.at, syntax context_at or symbol.at from this same file")))]
    Slice {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        selector: super::super::xref::Selector,
        /// Return one source JSON record instead of raw bytes; read detail.source.
        #[arg(long)]
        json: bool,
    },
}

impl Command {
    pub(super) fn into_query(self) -> Query {
        match self {
            Self::Entities { outline, .. } => Query::Outline {
                group: None,
                resolve_static: outline.resolve_static,
                within: outline.within,
                depth: outline.depth,
                kind: outline.kind,
                name: outline.name,
                contains: outline.filter,
                contains_case_sensitive: outline.case.sensitive(),
            },
            Self::Functions { outline, .. } => Query::Outline {
                group: Some("functions"),
                resolve_static: outline.resolve_static,
                within: outline.within,
                depth: outline.depth,
                kind: outline.kind,
                name: outline.name,
                contains: outline.filter,
                contains_case_sensitive: outline.case.sensitive(),
            },
            Self::Classes { outline, .. } => Query::Outline {
                group: Some("classes"),
                resolve_static: outline.resolve_static,
                within: outline.within,
                depth: outline.depth,
                kind: outline.kind,
                name: outline.name,
                contains: outline.filter,
                contains_case_sensitive: outline.case.sensitive(),
            },
            Self::Methods { outline, .. } => Query::Outline {
                group: Some("methods"),
                resolve_static: outline.resolve_static,
                within: outline.within,
                depth: outline.depth,
                kind: outline.kind,
                name: outline.name,
                contains: outline.filter,
                contains_case_sensitive: outline.case.sensitive(),
            },
            Self::Modules { names, within, .. } => Query::Modules(rxd_js::ModuleQuery {
                listing: rxd_js::ModuleListing::Modules,
                names: names.query(),
                within,
            }),
            Self::Imports { names, within, .. } => Query::Modules(rxd_js::ModuleQuery {
                listing: rxd_js::ModuleListing::Imports,
                names: names.query(),
                within,
            }),
            Self::Exports { names, within, .. } => Query::Modules(rxd_js::ModuleQuery {
                listing: rxd_js::ModuleListing::Exports,
                names: names.query(),
                within,
            }),
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
            Self::Strings {
                filter,
                case,
                resolve_static,
                resolution,
                within,
                ..
            } => Query::Strings {
                filter,
                case_sensitive: case.sensitive(),
                resolve_static,
                resolution: resolution.map(|r| match r.as_str() {
                    "literal" => rxd_js::StringResolution::Literal,
                    "static" => rxd_js::StringResolution::Static,
                    "decoder" => rxd_js::StringResolution::Decoder,
                    _ => unreachable!("validated resolution"),
                }),
                within,
            },
            Self::Resources {
                kind,
                resolution,
                filter,
                case,
                within,
                resolve_bindings,
                resolve_static,
                ..
            } => Query::Resources(rxd_js::ResourcesQuery {
                kind,
                resolution,
                filter,
                case_sensitive: case.sensitive(),
                within,
                resolve_bindings,
                resolve_static,
            }),
            Self::Values {
                within,
                kind,
                names,
                resolve_static,
                ..
            } => Query::Values(rxd_js::ValuesQuery {
                within,
                kind,
                names: names.query(),
                resolve_static,
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
            Self::Calls {
                names,
                within,
                resolve_static,
                ..
            } => Query::Calls {
                names: names.query(),
                within,
                resolve_static,
            },
            Self::Members {
                names,
                access,
                within,
                resolve_static,
                ..
            } => Query::Members {
                names: names.query(),
                access: match access {
                    MemberAccessArg::All => rxd_js::MemberAccess::All,
                    MemberAccessArg::Read => rxd_js::MemberAccess::Read,
                    MemberAccessArg::Write => rxd_js::MemberAccess::Write,
                },
                within,
                resolve_static,
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
            Self::Deobfuscate { .. } => unreachable!("deobfuscate bypasses the query catalog"),
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
            Self::Strings { .. } => "strings",
            Self::Modules { .. } => "modules",
            Self::Imports { .. } => "imports",
            Self::Exports { .. } => "exports",
            Self::Resources { .. } => "resources",
            Self::Values { .. } => "values",
            Self::Identifiers { .. } => "identifiers",
            Self::Calls { .. } => "calls",
            Self::Members { .. } => "members",
            Self::Locate { .. } => "locate",
            Self::Xref { .. } => "xref",
            Self::Symbols { .. } => "symbols",
            Self::Deobfuscate { .. } => "deobfuscate",
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
            | Self::Strings { scan, .. }
            | Self::Resources { scan, .. }
            | Self::Modules { scan, .. }
            | Self::Imports { scan, .. }
            | Self::Exports { scan, .. }
            | Self::Identifiers { scan, .. }
            | Self::Calls { scan, .. }
            | Self::Members { scan, .. }
            | Self::Symbols { scan, .. }
            | Self::Xref { scan, .. } => Some(scan),
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
            | Self::Strings { common, .. }
            | Self::Resources { common, .. }
            | Self::Modules { common, .. }
            | Self::Imports { common, .. }
            | Self::Exports { common, .. }
            | Self::Values { common, .. }
            | Self::Identifiers { common, .. }
            | Self::Calls { common, .. }
            | Self::Members { common, .. }
            | Self::Locate { common, .. }
            | Self::Xref { common, .. }
            | Self::Symbols { common, .. }
            | Self::Deobfuscate { common, .. }
            | Self::Slice { common, .. } => common,
        }
    }
}

impl Command {
    pub(crate) fn perf(&self) -> bool {
        self.common().perf
    }
}
