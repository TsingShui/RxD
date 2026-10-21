# HTML backend

Status: implemented.

`rxd html` is the Agent-facing lexical container entry for local HTML. It provides
`entries`, `resources` and `extract`. It reads one bounded, valid-UTF-8 regular local
file, enumerates lexical nodes and source-backed resource candidates, and extracts
exact byte ranges by content-bound ID. Resource URL resolution uses a pinned local URL
parser, not a network client. It does not depend on `rxd-js` or Oxc, execute page
scripts, load external resources, or build a browser DOM. Inline JS/TS content is analyzed separately by
`rxd js`. See the [HTML examples](../demo/html.md).

## Inputs and identity

The input is one regular local HTML file checked by content (fragments are accepted;
no doctype or `.html` extension is required). Directories, bundles and network inputs
are rejected. The entire file is read into one fixed buffer and SHA-256 becomes the
container identity. Every record ID binds that hash and a half-open UTF-8 byte range.
This entry does not discover directories, widen JS input scope, or repair JS directory
scans; HTML files are always handled by `rxd html`.

## Commands

| Command | Output and behavior |
| --- | --- |
| `html entries FILE [-f TEXT] [-o CATALOG]` | header/records/summary JSONL, `backend: html`, `command: entries`; `header.detail.analysis: lexical`. Lists the four lexical record kinds in source order; supports `--kind/--tag/--attr/--within/--depth`. `-o` writes the same bytes as stdout. |
| `html resources FILE [--base-url URL] [-f TEXT] [-o CATALOG]` | header/resource/summary JSONL; classifies inline bodies and external references. `--kind`, `--location inline\|external`, and optional `--include-content` narrow or enrich the same listing. No requests or script execution. |
| `html extract FILE (--at AT \| --from JSONL) -o OUTPUT [--content]` | One selector and `-o` are required. The target stores original bytes (element outer range by default; content range with `--content`). Stdout is one JSON receipt, not the file content. |

Structured output follows [ADR 0003](../adr/0003-output-envelope.md),
`schema_version=2`. `entries` format-specific fields are under `detail`; the `extract`
receipt is a single `file` record with `backend`/`command`/`schema_version` and
`source_id` (artifact SHA-256), with receipt content under `detail`.

### Records and closed schema

For `entries`, record kinds are `element`/`text`/`comment`/`doctype`; an end tag is not a fifth
record. Every data record carries the ADR 0003 navigation fields (`at, source_id, parent_at,
context_at, start, end`); a field with no value is `null` or absent, and the field set
never changes with input. Printed coordinates are `kind:start:end`; source identity
lives in the header. The full content-bound spelling
`rxd1:html:<lowercase sha256>:<kind>:<start>:<end>` remains accepted. Start/end are
canonical decimal half-open UTF-8 byte ranges.

Element detail: `tag` (ASCII lowercase), `attrs` (ordered object array with `name` and
`value`, preserving duplicates), `self_closing`, `void`, `depth`, `end_tag`
(`explicit|missing|not_applicable`), `content_start`, `content_end`. Boolean and
explicit-empty attributes both use an empty value; browser "effective duplicate
attribute" semantics are not claimed. Text/comment/doctype detail: `value` (original
source slice) and `depth`. An element's `context_at` is its own coordinate; other nodes use
the nearest parent element (top level: null). Every record ID is directly extractable.

Header detail: `path`, `analysis: lexical`, `parser: html5gum`,
`parser_version: 0.8.4`, `lexical_profile: lexical_naive_v1`. `query` always has
`kind/tag/attr/depth/within` (null when absent); tag and attribute names use their
canonical spelling, attribute query values are echoed verbatim.

Summary detail: `nodes/elements/texts/comments/doctypes/tokenizer_errors/
unmatched_end_tags/missing_end_tags`. Kind counts and `nodes` describe the entire
parsed catalog; outer `records` counts emitted selected rows, distinguishing filters
from parse scope. Errors outside the selection are not dropped.

## Lexical model and recovery

Profile `lexical_naive_v1` is not a DOM or browser parser. `noscript` assumes
scripting is enabled; DOM insertion modes, optional end tags, template insertion,
foreign namespaces, CDATA and integration points are not implemented. State switching
is a documented approximation, not browser semantics. In foreign content an error
state may consume markup after the foreign subtree; catalog completeness is only
relative to the declared lexical model.

The tokenizer is MIT-licensed `html5gum =0.8.4` with the tree-builder feature disabled;
a callback emitter preserves ordered duplicate attributes and byte spans. Its
documented naive next-state mapping has a verified typo: `noframe` (singular) must not
select RawText; `noframes` (plural) must. A private forwarding emitter fixes the
spelling and keeps `script→ScriptData`, `title/textarea→RCDATA`,
`style/iframe/xmp/noembed/noscript→RawText` and `plaintext→PlainText`. This local
adapter is not a fork or upgrade; version, license, fix and provenance are in
`crates/rxd-html/REFERENCE.md`.

Attribute values returned are tokenizer-decoded (entities, CR/CRLF normalization,
NUL replacement) and need not equal original bytes; values must be correct and decoded
length is never used to infer the original span. There is no raw/decoded dual mode and
no attribute raw-span guarantee. Text/comment/doctype values and all extracted bytes
are slices of the original fixed buffer, with no entity decoding or newline conversion.

Deterministic pairing and recovery:

- No synthetic document record. Top-level records have depth 0 and parent null; a
  record's parent is the innermost open lexical element at creation; child depth is
  parent depth + 1. Filters do not change these relationships.
- An element stays pending until its start tag is complete; no element or attribute is
  published early. EOF with a pending start fails `INVALID_INPUT`; content boundaries
  are never fabricated from partial events.
- A completed non-void start tag creates and pushes an element. A slash is only a
  lexical flag: `<div/>` still pushes. Standard HTML void names are interpreted without
  namespaces; void elements cover only the completed start tag, have an empty content
  range, use `end_tag: not_applicable`, and are not pushed. `self_closing` and `void`
  are independent.
- An end tag finds the nearest same-name element on the stack and closes intervening
  unmatched elements as `missing` at the end-tag start (outer and content end there);
  the matched element closes as `explicit` (content ends at the tag start, outer
  includes the tag). Unmatched end tags create no node and count in
  `unmatched_end_tags`. Attribute events on end tags never attach to an unrelated open
  element. At EOF, remaining completed non-void elements close as `missing` with
  content/outer ending at EOF.
- Optional end tags are not implemented: for example a repeated `li` start nests
  lexically until a matching or ancestor close. `explicit` does not by itself imply
  browser-equivalent range reliability.
- Recoverable tokenizer syntax errors follow the fixed tokenizer and count in
  `tokenizer_errors`; missing/unmatched are counted separately. Accepted NUL is not
  globally fatal and raw bytes remain extractable. Reader failure, invalid span/UTF-8
  boundaries, pending-start EOF and impossible internal ranges fail before output.
- Records sort by source start, then kind/end as fixed tie-breakers; canonical IDs are
  asserted unique and merge/sort order is deterministic.
- `complete` means every record selected from the bounded lexical catalog was
  delivered; it does not mean every source byte has an independent record or every DOM
  element was discovered.

Boundary fixtures live in `tests/fixtures/html/`: `foreign.html` (SVG script without a
closing script; naive ScriptData consumes `</svg><p>later</p>` as text and `p` is absent
from the catalog), `template.html` (`tr` remains a lexical child of `template`, not a
browser template-content fragment), and `contexts.html` (textarea/title/style/noscript/
noframes/plaintext contexts and the unknown `noframe`).

## Entry filters

For `entries`, `--kind` selects exactly one of element/text/comment/doctype. `--tag` is an ASCII
case-insensitive full-name match on elements only. `--attr NAME[=VALUE]` applies to
elements only; the first `=` separates name and value. Without `=` it checks presence;
with `=` it compares the reported decoded UTF-8 value byte-for-byte. Query values are
not entity-decoded or HTML-parsed again: stored `/a&amp;b` reports `/a&b`, query with
`/a&b`. Any duplicate attribute may match. Each filter option appears at most once;
there are no regexes, substrings, selector languages or implicit MIME classification.
All filters are ANDed; a valid but empty intersection returns a complete zero-match
result. An invalid kind or empty/invalid attribute name is an argument/query error, not
a silent zero.

`--within` accepts only a validated element ID, includes that element and its lexical
descendants, and fails other kinds with `INVALID_QUERY`. `--depth N` selects relative
depth ≤ N; without `--within` the top level is depth 0; N=0 is valid. Filtering
preserves original depth and parent/context IDs even when ancestors are filtered out.

## Resource candidates

`resources` projects the same fixed lexical catalog under
`resource_profile: resource_candidates_v1`. It is not a browser dependency graph.
The supported source forms are:

- `script`: distinguish classic JavaScript, modules, import maps, speculation rules
  and data blocks using HTML `type`/`language` rules. The JavaScript MIME essence
  check accepts ASCII whitespace around an explicit MIME type, but not parameters;
  `type="module "` and `type="text/javascript; charset=utf-8"` are data blocks.
  Empty/absent type defaults follow HTML rules; JSON/TS/other data types are not JS.
  Data blocks ignore `src`. External import maps/speculation rules retain an
  unresolved `unsupported_script_src` reference rather than a fetch URL.
- `style`: absent/empty type or `text/css` is `style`; other types are `data`.
- `link href`: stylesheet, modulepreload, preload (classified by `as`), icon and
  manifest; preconnect/dns-prefetch are `hint`, not asset downloads. Recognized
  relation tokens are emitted once each, in their source order.
- `img src`, image-input `src`, audio/video/source `src`, video `poster`,
  track `src`, iframe `src`, embed `src` and object `data`.
  `srcset` on img/source and preload `imagesrcset` remain a single unresolved
  `compound` value; no comma splitting or responsive-image selection is guessed.
  An iframe with `srcdoc` reports that decoded value as unresolved `embedded_html`
  instead of its overridden `src`. It is not recursively parsed.

The kind enum is `script|style|importmap|data|image|font|media|document|hint|other`.
Non-empty inline bodies are emitted; zero-byte bodies are omitted and counted.
Whitespace-only bodies remain non-empty. External means an attribute reference,
**not** a network request: embedded HTML, compound references, `data:`, `file:`
and other schemes are not converted into HTTP downloads.

`--base-url` supplies the captured document's final absolute HTTP(S) URL, without
credentials, at most 64 KiB. Never infer it from the local file path. The first
lexical `base[href]` outside a lexical template wins; its value resolves against
the supplied document URL. An invalid base or a data/javascript base falls back to
the document URL, not a later base. Without a usable base, relative references stay
unresolved while absolute URLs remain usable. URL parsing/normalization is WHATWG
`url =2.5.8`; values are tokenizer-decoded before resolution. CSP, DOM insertion,
script mutations, namespace behavior and runtime timing are not simulated.

`--kind` and `--location` are exact selectors. `-f/--filter` matches
`detail.name`: the decoded attribute value for external references, or the tag
name for an inline body. Shared case folding is the default; `--case sensitive`
disables it. Filters are ANDed and do not change base selection or source identity.

Each `resource` row has the owning element's `at/context_at/start/end` and its
lexical `parent_at`. These are element coordinates accepted directly by
`extract --at`, not new resource IDs. Multiple relations on one element can share
a coordinate. `extract --from` accepts resource streams too; use the ordinary
catalog without included content when selecting one body. Detail has a closed schema:

- `kind/name/tag/location/relation/attribute`; attribute is null for inline bodies.
- `url`: normalized absolute URL or null. `resolution`: `absolute|base|empty|
  invalid|unresolved|compound|embedded_html|unsupported_script_src`, or null for
  inline bodies. A resolved URL is neither a permission to fetch nor proof of a
  runtime request; the Agent applies scheme/origin/authorization policy.
- `script_type`: `classic|module|importmap|speculationrules|data`, or null where
  inapplicable; modulepreload uses `module`. `end_tag` preserves lexical recovery.
- `content_start/content_end/content_bytes`: original inline range and size, null
  for external references. `content/content_source_id` are null by default.
  `--include-content` supplies the exact original inline UTF-8 slice as JSON text
  plus its independent SHA-256. JSON decoding followed by UTF-8 encoding restores
  the bytes without entity decoding, BOM removal or newline conversion.
- `conditions`: decoded nullable `type/language/rel/as/media/crossorigin/integrity/
  referrerpolicy/fetchpriority`, and boolean attribute-presence flags
  `async/defer/nomodule/disabled`, plus lexical `in_template`. These are evidence,
  not evaluated loading conditions, CORS decisions or integrity checks.

Header detail extends the lexical parser fields with `resource_profile`,
`duplicate_attributes: first` and `base`
(`document_url/url/href/at/status`). Base status is `document|element|missing|
unresolved|invalid_fallback|disallowed_scheme_fallback`. Attribute projections use
the first duplicate; `entries` still preserves every attribute. Query echoes
`base_url/kind/location/filter/case/include_content`.

Summary detail contains total pre-filter `resources/empty_inline`, selected
`inline/external/unresolved` counts, and the entire catalog's three recovery
counters. Rows follow source order. `complete` means all selected candidates in
this explicit profile were delivered, **not page coverage**. CSS imports/URLs,
style attributes, event handlers, nested srcdoc, SVG references, navigation links,
JS-created URLs, import-map expansion and responsive-image selection are outside
this profile. Neither successful downloads nor these counts supply a browser
resource-coverage denominator.

## Identity and extraction

ID validation is three layers: syntax (`rxd1`/`html`/64 lowercase hex/one of four
kinds/canonical decimal), then container hash, then exact catalog membership. A
syntactically valid ID with a different hash (foreign/stale) returns `SOURCE_MISMATCH`;
malformed, non-canonical or missing same-hash IDs return `INVALID_QUERY`. Bounds checks
never replace membership lookup. Content ranges derive from the matched element, never
from invented IDs or caller ranges.

By default `extract` accepts all four kinds: elements select the entire outer range
(including missing-recovery ranges); others select their complete record range.
`--content` is element-only (other kinds return `INVALID_QUERY`) and selects the
content range. Empty content returns `NOT_FOUND` with no artifact, independent of `src`
or JS-compatible type; non-empty content is extracted directly without execution
claims. Extracted bytes are the selected half-open container range with independent
SHA-256; no entity decode, newline conversion, BOM removal, pretty-printing or syntax
insertion.

CLI `extract` requires `--at` or `--from` plus `-o`, one input, and forbids replacing
the input with the target (including path aliases). `--from` compares the stream's
content identity to the same frozen source buffer used for extraction, not merely
its length or a separate read; same-size stale streams return `SOURCE_MISMATCH`. Host output paths are never derived
from element attributes. The receipt is one schema-2 `file` record: outer `backend:
html`, `command: extract`, `schema_version`, `source_id` (artifact SHA); `at`,
`parent_at`, `context_at` are null; `start` 0, `end` file_bytes. `detail` has
`complete`, `path`, `file_bytes`, `representation: original`, `part` (outer|content)
and `origin` (container/format: html/source_id, node_id/kind/part/start/end). Parent
node ID appears only in `origin`, never in artifact navigation; artifact-side and
container-side offsets are strictly separate.

## Safety and read boundary

Only one bounded input buffer is read; input is never executed, external resources are
never loaded, and `src`/CSS/imports are not followed. Parsing, identity validation,
byte selection, artifact identity and receipt serialization finish before commit;
commit uses an existing same-directory temporary file and atomic replacement. A
pre-commit failure preserves the old target, produces no final artifact and emits no
payload; an output/commit failure leaves no success receipt. If stdout transfer fails
after replacement, the output file may exist and a non-zero exit is not complete
success; `entries` stdout may also be partial, and complete success still requires a
complete summary plus exit 0. Multi-node extraction, directory write transactions and
persistent parse state are not implemented.

## Resource budgets

Input admission follows [ADR 0016](../adr/0016-input-scaled-cli-resources.md): actual regular-file length plus
the existing 32-bit span guard, not a fixed 8 GiB quota. There is no catalog-record
ceiling (including extract's validation catalog) or CLI output ceiling (complete
JSONL, or raw artifact plus receipt); lexical nesting is limited to 512, distinct
from the `--depth` selector. The explicitly supplied document base URL is additionally
limited to 64 KiB. Inline bodies and attribute values come from the bounded source
buffer. The CLI exposes no budget knobs; nesting is checked during parsing, not by
truncating an already accumulated catalog. Exceeding a limit returns `RESOURCE_LIMIT` and
never silently truncates or skips nodes; arithmetic/range overflow is explicit.
Diagnostics go to stderr. Exit 0 means complete success (including zero rows); 1 means
input/ID/resource/I/O failure; 2 means command-line or query argument error (including
`INVALID_QUERY`).

## Relationship to JavaScript (ADR 0002)

Per [ADR 0002](../adr/0002-backend-capability-closure.md), HTML is a lexical container
format: it provides ENUMERATE (`entries/resources`) and node FETCH (`extract`);
`--attr` provides exact decoded attribute SEARCH and resource `-f` searches the
reference label. General code text/constant SEARCH and code XREF are explicitly
exempt because payload semantics belong to the extracted content's code backend.
Resource classification and URL resolution do not model a browser runtime. HTML is not called a complete code-analysis backend, and no hidden
capability claim is attached to the container label.

Extracted artifacts have their own SHA-256 and offsets. The Agent chooses the syntax
for JS/TS-compatible content and passes it to `rxd js`; no container-to-JS offset
translation is introduced. Known JS artifacts remain analyzable; arbitrary script
content is not promised to parse as JS, and JSON/TS inputs are not misclassified. HTML
and JS backends are independent; HTML does not depend on JS.

## Performance diagnostics

`--perf` marks analysis and CLI records with `backend: html` and command `entries`,
`resources` or `extract`. Stages are `read/hash/utf8/tokenize/query/serialize/write`; `tokenize` is
recorded by `Perf::measure("tokenize", ...)`. For extract, `analysis.output_bytes` is
artifact plus receipt and `CLI.output_bytes` is the stdout receipt; `list -o` reports
zero CLI output bytes. Extract `source_id` is the artifact identity; list `source_id`
is the container hash.

### Resource CLI measurement

A CLI-only release build (`--no-default-features --features backend-js,backend-html`)
was measured on the saved anonymous Feishu login HTML: 92,565 bytes, SHA-256
`e410ca06f6f119ea1d21ab217a37039ba76ce886034ceabe1ccab27069075ad7`.
Hardware: Apple M3 Pro, 36 GiB RAM, macOS 26.5.1 arm64. Each workload used 30 fresh
CLI/analysis-worker processes after two warm-ups, warm OS file/binary caches, real
atomic output writes, and nearest-rank P95. No workloads were pooled.

| Workload | Median ms | P95 ms | >150 ms | Output bytes |
| --- | ---: | ---: | ---: | ---: |
| Resource catalog with document URL | 13.10 | 15.96 | 0/30 (0%) | 14,852 |
| Inline JS with original content | 9.05 | 9.90 | 0/30 (0%) | 9,570 |
| All candidates with inline content | 9.81 | 11.24 | 0/30 (0%) | 104,041 |
| Extract one body via resource stream | 9.18 | 11.15 | 0/30 (0%) | 4,308 + stdout receipt |

Samples, commands, binary hash and separate phase profiles are under ignored
`target/bench/html-resources/`. These separate batches are not a speedup comparison.
Scope is one production capture and these four CLI operations, not arbitrary HTML or
authenticated Drive; the optional npm HTML corpus was absent and was not downloaded.

The HTML runner used fixed authored pages (ordinary and synthetic large/markup-dense),
plus pinned real npm pages when `tests/corpus/node_modules` exists (`tslib` 2.8.1,
`terser` 5.51.2 `tools/props.html`, `string-template` 1.0.0, `md5` 2.3.0; versions from
`tests/corpus/package-lock.json`, per-file SHA-256), labeled corpus/authored/synthetic.
It measures list, small-script extract and list → extract → `rxd js` with fresh
CLI/worker processes and real atomic writes, saving samples, median/P95, >150 ms share,
origin, hashes, hardware and cache conditions. `node_modules` is gitignored; when
absent the script records a skip and tests only authored/synthetic pages. The repository
does not embed production HTML or use the network. Ordinary representative list/extract
queries follow the 150 ms fresh-process requirement; tokenizer microbenchmarks do not
replace acceptance. Markup-dense non-decompilation workloads have no automatic
exception; any exception needs explicit scope, measurements and approval.

## MCP projection

`html_entries`/`html_resources`/`html_extract` reuse the same query and result
construction. The listings project header/records/summary, preserving CLI values,
identity, order and completeness (logical path restoration, not byte-identical JSONL). HTML capture is a
regular immutable file (path or file target); directories/bundles are rejected.
`html_resources` reflects `base_url/kind/location/filter/case/include_content` from
the CLI. Included inline content does not create child targets or write host files;
use `html_extract` on the returned element coordinate for an owned child artifact.
`html_extract` reuses backend validation/selection/receipt/artifact identity, keeps
`detail.path: null`, and adds `child_target`; `include_bytes` adds `bytes_base64` but
does not copy by default. The child target owns extracted bytes and artifact source_id
independently of the parent; target-origin retains container/node/range and the parent
handle when available.

Shared budgets map to MCP: `Budget::input_bytes→input_bytes`, `Budget::records→records`,
`Budget::output_bytes→output_bytes`, `Budget::ast_depth→ast_depth` (HTML lexical nesting,
distinct from selection depth); lower-only server caps still apply. No caller-supplied
host write path is used. Shared snapshot, quota, cancellation and complete-response
rules are in the [MCP design](mcp-design.md); the decision is
[ADR 0007](../adr/0007-typed-mcp-service.md). CLI output and exit codes are unchanged
by the MCP projection.
