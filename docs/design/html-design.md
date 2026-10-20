# HTML backend

Status: implemented.

`rxd html` is the Agent-facing lexical container entry for local HTML. It provides
`entries` and `extract`. It reads one bounded, valid-UTF-8 regular local file, enumerates
element/text/comment/doctype records in source order, and extracts exact byte ranges
by content-bound ID. `rxd-html` depends only on shared budgets, protocol and file
reading; it does not depend on `rxd-js` or Oxc, execute page scripts, load external
resources, or build a browser DOM. Inline JS/TS content is analyzed separately by
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
| `html entries FILE [-o CATALOG]` | header/records/summary JSONL, `backend: html`, `command: list`; `header.detail.analysis: lexical`. Lists the four lexical record kinds in source order; supports `--kind/--tag/--attr/--within/--depth`. `-o` writes the same bytes as stdout. |
| `html extract FILE --at AT -o OUTPUT [--content]` | `--at` and `-o` are required. The target stores original bytes (element outer range by default; content range with `--content`). Stdout is one JSON receipt, not the file content. |

Structured output follows [ADR 0003](../adr/0003-output-envelope.md),
`schema_version=2`. `entries` format-specific fields are under `detail`; the `extract`
receipt is a single `file` record with `backend`/`command`/`schema_version` and
`source_id` (artifact SHA-256), with receipt content under `detail`.

### Records and closed schema

Record kinds are `element`/`text`/`comment`/`doctype`; an end tag is not a fifth
record. Every data record carries the fixed nullable ADR 0003 navigation fields
`id, source_id, parent_id, context_id, start, end`; nullable fields are always present
and the field set never changes with input. IDs are
`rxd1:html:<lowercase sha256>:<kind>:<start>:<end>`, where start/end are canonical
decimal half-open UTF-8 byte ranges.

Element detail: `tag` (ASCII lowercase), `attrs` (ordered object array with `name` and
`value`, preserving duplicates), `self_closing`, `void`, `depth`, `end_tag`
(`explicit|missing|not_applicable`), `content_start`, `content_end`. Boolean and
explicit-empty attributes both use an empty value; browser "effective duplicate
attribute" semantics are not claimed. Text/comment/doctype detail: `value` (original
source slice) and `depth`. An element's `context_id` is its own ID; other nodes use
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

## Filters

`--kind` selects exactly one of element/text/comment/doctype. `--tag` is an ASCII
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

## Identity and extraction

ID validation is three layers: syntax (`rxd1`/`html`/64 lowercase hex/one of four
kinds/canonical decimal), then container hash, then exact catalog membership. A
syntactically valid ID with a different hash (foreign/stale) returns `SOURCE_MISMATCH`;
malformed, non-canonical or missing same-hash IDs return `INVALID_ID`. Bounds checks
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

CLI `extract` requires explicit `--at` and `-o`, one input, and forbids replacing the
input with the target (including path aliases). Host output paths are never derived
from element attributes. The receipt is one schema-2 `file` record: outer `backend:
html`, `command: extract`, `schema_version`, `source_id` (artifact SHA); `id`,
`parent_id`, `context_id` are null; `start` 0, `end` file_bytes. `detail` has
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

reading; 32-bit spans keep actual addressable content below 4 GiB), 100,000 records
(all catalog nodes before filtering, including extract's validation catalog), 32 MiB
output (complete JSONL for entries; raw artifact plus complete serialized receipt for
extract) and lexical nesting depth 512 (distinct from the `--depth` selector). All four
values live in `rxd_core::Budget`; the CLI exposes no budget flags. Node/depth limits are enforced during parsing, not by
unbounded accumulation and truncation. Exceeding a limit returns `RESOURCE_LIMIT` and
never silently truncates or skips nodes; arithmetic/range overflow is explicit.
Diagnostics go to stderr. Exit 0 means complete success (including zero rows); 1 means
input/ID/resource/I/O failure; 2 means command-line or query argument error (including
`INVALID_QUERY`).

## Relationship to JavaScript (ADR 0002)

Per [ADR 0002](../adr/0002-backend-capability-closure.md), HTML is a lexical container
format: it provides ENUMERATE (`entries`) and node FETCH (`extract`); `--attr` provides a
limited metadata SEARCH (exact decoded attribute value). General text/constant SEARCH
and code XREF are explicitly exempt because payload semantics belong to the extracted
content's code backend; this version does not parse external references or model a
browser runtime. HTML is not called a complete code-analysis backend, and no hidden
capability claim is attached to the container label.

Extracted artifacts have their own SHA-256 and offsets. The Agent chooses the syntax
for JS/TS-compatible content and passes it to `rxd js`; no container-to-JS offset
translation is introduced. Known JS artifacts remain analyzable; arbitrary script
content is not promised to parse as JS, and JSON/TS inputs are not misclassified. HTML
and JS backends are independent; HTML does not depend on JS.

## Performance diagnostics

`--perf` marks analysis and CLI records with `backend: html` and command `entries` or
`extract`. Stages are `read/hash/utf8/tokenize/query/serialize/write`; `tokenize` is
recorded by `Perf::measure("tokenize", ...)`. For extract, `analysis.output_bytes` is
artifact plus receipt and `CLI.output_bytes` is the stdout receipt; `list -o` reports
zero CLI output bytes. Extract `source_id` is the artifact identity; list `source_id`
is the container hash.

`benches/html.py` uses fixed authored pages (ordinary and synthetic large/markup-dense),
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

`html_entries`/`html_extract` reuse the same query and result construction. `html_entries`
projects header/records/summary, preserving CLI values, identity, order and
completeness (logical path restoration, not byte-identical JSONL). HTML capture is a
regular immutable file (path or file target); directories/bundles are rejected.
`html_extract` reuses backend validation/selection/receipt/artifact identity, keeps
`detail.path: null`, and adds `child_target`; `include_bytes` adds `bytes_base64` but
does not copy by default. The child target owns extracted bytes and artifact source_id
independently of the parent; target-origin retains container/node/range and the parent
handle when available.

CLI budgets map to MCP: `--max-input-bytes→input_bytes`, `--max-records→records`,
`--max-output-bytes→output_bytes`, `--max-depth→ast_depth` (HTML lexical nesting,
distinct from selection depth); lower-only server caps still apply. No caller-supplied
host write path is used. Shared snapshot, quota, cancellation and complete-response
rules are in the [MCP design](mcp-design.md); the decision is
[ADR 0007](../adr/0007-typed-mcp-service.md). CLI output and exit codes are unchanged
by the MCP projection.
