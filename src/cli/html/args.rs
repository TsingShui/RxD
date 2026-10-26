use super::super::filter::CaseArgs;
use super::super::xref::Selector;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args)]
pub(crate) struct Common {
    /// One regular local HTML file.
    pub(super) input: PathBuf,
    #[cfg(any(feature = "backend-asar", feature = "backend-zip"))]
    #[command(flatten)]
    pub(super) container: super::super::input::EntryArgs,
    /// Write complete JSONL atomically instead of writing to stdout.
    #[arg(short, long)]
    pub(super) output: Option<PathBuf>,
    /// Enable backend diagnostic details on stderr.
    #[arg(long)]
    pub(super) debug: bool,
    /// Write timing/resource JSONL to stderr without changing the result payload.
    #[arg(long)]
    pub(super) perf: bool,
    // The shared budget table (rxd_core::Budget) supplies input, record, output and
    // depth ceilings; no per-backend knob exposes them.
}
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Enumerate lexical_naive_v1 records (not a DOM or browser-complete parse).
    #[command(
        name = "entries",
        after_long_help = "Examples:\nrxd html entries page.html --tag script\nrxd html entries page.html --attr 'name=AppPublic'\nOutput: element/text/comment/doctype JSONL; at is kind:start:end in UTF-8 byte offsets.\nCopy an element.at into --within; --depth is relative depth, including zero.\nNext: rxd html extract page.html --at '<element.at>' --content -o inline.js.\nLexical approximation only, not browser DOM reconstruction."
    )]
    Entries {
        #[command(flatten)]
        common: Common,
        /// Exact record kind.
        #[arg(long, value_parser = ["element", "text", "comment", "doctype"])]
        kind: Option<String>,
        /// Exact ASCII-case-folded element name.
        #[arg(long)]
        tag: Option<String>,
        /// Attribute presence or exact decoded value (NAME[=VALUE]).
        #[arg(long)]
        attr: Option<String>,
        /// Canonical element ID whose lexical subtree is selected.
        #[arg(long)]
        within: Option<String>,
        /// Maximum depth relative to --within (or top level), including zero.
        #[arg(long)]
        depth: Option<usize>,
        /// Substring over the element name or the text it holds, folded unless
        /// `--case sensitive`.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
    },
    /// List source-backed resources, inline code and unresolved references without fetching.
    #[command(
        after_long_help = "Examples:\nrxd html resources page.html --base-url https://example.org/page\nrxd html resources page.html --kind script --location inline --include-content\nOutput: resource JSONL; at is the owning element, detail.content is original inline text.\n--base-url is the final captured page URL; resolves relative references without fetching.\nExternal candidates are not automatically downloaded; hint/data are not executable scripts.\nNext: extract --at '<resource.at>' --content -o inline.html to keep the exact inline bytes."
    )]
    Resources {
        #[command(flatten)]
        common: Common,
        /// Final HTTP(S) document URL used to resolve relative references and <base href>.
        #[arg(long)]
        base_url: Option<String>,
        /// Exact resource kind, not a claim that the resource is loaded at runtime.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(rxd_html::RESOURCE_KINDS.iter().copied()))]
        kind: Option<String>,
        /// Inline bodies or external attribute references; external need not mean HTTP.
        #[arg(long, value_parser = ["inline", "external"])]
        location: Option<String>,
        /// Substring over the decoded reference, or the tag name for inline bodies.
        #[arg(short, long)]
        filter: Option<String>,
        #[command(flatten)]
        case: CaseArgs,
        /// Include exact inline source and its SHA-256; never fetch external URLs.
        #[arg(long)]
        include_content: bool,
    },
    /// Fetch original bytes from an exact catalog record.
    #[command(after_long_help = "Examples:\nrxd html entries page.html --tag script\nrxd html extract page.html --at '<element.at>' --content -o inline.js\nRequired: --at and -o FILE. Copy entries.at or resources.at, never an extracted-file offset.\nDefault extracts the whole node; --content extracts only an element's inner bytes.\nOutput: original bytes in FILE and one file receipt on stdout, with detail.origin.\nThen analyze FILE separately; its offsets are no longer page.html offsets.", mut_arg("at", |arg| arg.required(true).value_name("NODE_AT").help("Copy entries.at or resources.at from this HTML file, e.g. element:20:80")), mut_arg("output", |arg| arg.required(true).help("Write extracted original bytes to this file; stdout is a receipt")))]
    Extract {
        #[command(flatten)]
        common: Common,
        /// The coordinate an entries row prints.
        #[command(flatten)]
        selector: Selector,
        /// Extract only element content rather than its outer range.
        #[arg(long)]
        content: bool,
    },
}
impl Command {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Entries { .. } => "entries",
            Self::Resources { .. } => "resources",
            Self::Extract { .. } => "extract",
        }
    }
    pub(crate) fn perf(&self) -> bool {
        match self {
            Self::Entries { common, .. }
            | Self::Resources { common, .. }
            | Self::Extract { common, .. } => common.perf,
        }
    }
}
