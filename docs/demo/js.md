# JavaScript examples

`rxd js` locates structures, searches literals and extracts source on demand; it never
executes input. It has nine commands:

| Command | Purpose | Default output |
| --- | --- | --- |
| `entities` | Every structure with containment; `-f` substring and exact `--kind`/`--name` | JSONL |
| `functions` / `classes` / `methods` | The same rows, one object kind group each | JSONL |
| `find` | Search decoded string literals and static template fragments; `--resolve-static` also matches bounded AST constant evaluation | JSONL |
| `identifiers` | Find lexical declarations/references by exact name | JSONL |
| `calls` | Find direct/member/`new` call sites by callee name | JSONL |
| `members` | Find static member names, property keys, JSX attribute names and statically resolvable computed keys | JSONL |
| `locate` | Find the innermost structure and ancestors at a byte offset | JSON |
| `get` | Extract source by structure/fragment ID, or the canonical declaration by symbol ID | Raw source |
| `xref --at` | Find the lexical binding, references and read/write roles at a name occurrence (`--byte` takes a raw offset instead) | JSONL |
| `symbols` | Discover declarations, external bindings and unresolved references in a region | JSONL |

structure listings and `find` accept a file or recursively scan a directory; `locate/get/xref/symbols/
identifiers/calls/members` accept one file. ASAR uses the separate
[`rxd asar` entry](asar.md) to extract a file, then these commands analyze it; identical
bytes and parse options keep the same content-bound IDs.

The backend performs **structural location**: literals, identifier names, call sites and
static member names map to content-bound AST structures, lexical relationships and
original source ranges. JS deobfuscation and string decryption are permanent non-goals;
obfuscated files are analyzed by their existing structure and extracted verbatim. Literal
escape decoding is parsing, not decryption. `--resolve-static` only evaluates closed AST
constant expressions; it does not execute user functions, parse string arrays or restore
dynamic property names.

Structured output follows [ADR 0003](../adr/0003-output-envelope.md),
`schema_version=2`: navigation outside, format fields under `detail`; header input size is
`input_bytes`, path is `detail.path`. Default `get` raw bytes are outside the envelope.

## Setup

Run from the repository root in one shell. Rust 1.97+ is required; `jq` is used to inspect
JSON and extract real IDs.

```sh
cargo build
RXD=./target/debug/rxd
FILE=tests/fixtures/example.js
DEMO=target/demo/js
mkdir -p "$DEMO"

"$RXD" js --help
"$RXD" js entities --help
"$RXD" js find --help
"$RXD" js identifiers --help
"$RXD" js calls --help
"$RXD" js members --help
"$RXD" js locate --help
"$RXD" js get --help
"$RXD" js xref --help
"$RXD" js symbols --help
```

The fixture contains `request`, an anonymous async arrow `send` inside it, object
methods/getters/setters and a `Client` class. Offsets/counts below correspond to this
fixed file; use actual query results for other inputs.

## 1. entities

```sh
"$RXD" js entities "$FILE" --jsonl -o "$DEMO/entities.jsonl"

jq 'select(.record == "entity") |
    {kind: .detail.kind, name: .detail.name, label: .detail.label, start, end, depth: .detail.depth, flags: .detail.flags}' "$DEMO/entities.jsonl"
```

`send` is an anonymous `arrow_function` with `label: "send"`, range `[66,129)`, absolute
depth 2 and `async: true`. The original entity also has `at`, `parent_at` and
`detail.label_origin`. Ranges are half-open original UTF-8 byte ranges; anonymous
functions have their own IDs. `name` is an explicit name; `label` can come from a variable
binding or property context.

The containment tree is:

```text
program
├── function request
│   └── async arrow send
├── object_method login
├── getter status
├── setter status
└── class Client
    ├── constructor
    ├── static async method create
    ├── generator method events
    └── arrow run
```

The CLI emits flat JSONL, not a printed tree. Ordinary variables, statement blocks and
objects are not separate entities; parent/child means code containment, not calls.

### Depth and subtree filters

```sh
# program and direct children; do not expand request/Client internals
"$RXD" js entities "$FILE" --depth 1

# Take request's real ID and show only its direct children
REQUEST_ID=$(jq -r 'select(.record == "entity" and .detail.name == "request") | .at' "$DEMO/entities.jsonl")
"$RXD" js entities "$FILE" --within "$REQUEST_ID" --depth 1
```

The second query returns only `send`, not `request` itself. Record `depth` is still
absolute from program (2 here). `--within` is single-file only and cannot be combined with
a directory or `-r`.

## 2. find

```sh
"$RXD" js find "$FILE" --string '/api/login' --jsonl -o "$DEMO/find.jsonl"
jq 'select(.record == "match")' "$DEMO/find.jsonl"
```

The fixture has two hits: the top-level `api` constant and `send`'s request. Use
`--resolve-static` for closed constant expressions:

```sh
"$RXD" js find "$FILE" --string '/api/login' --resolve-static --jsonl
```

Static-expression hits use `detail.literal_kind: static_expression`,
`detail.resolution: static` and `detail.raw`; ordinary literal hits leave these null.
Zero hits is success with `summary.records: 0`.

## 3. locate

```sh
"$RXD" js locate "$FILE" --byte 85 --json
```

`--byte` is an original UTF-8 byte offset in `[0, file_length)`. The innermost covering
structure wins; `detail.ancestors` is ordered inner to outer. A `rg --byte-offset
--only-matching --fixed-strings '/api/login' bundle.js` hit can be passed directly to
`locate`.

## 4. get

```sh
"$RXD" js get "$FILE" --at "$REQUEST_ID"
"$RXD" js get "$FILE" --at "$REQUEST_ID" --json
"$RXD" js get "$FILE" --at "$REQUEST_ID" -o "$DEMO/request.js"
```

Default output is exact original source bytes with no added newline or formatting.
`--json` returns a `record: source` envelope with source, entity metadata and original
range. Extract a parent to see closure context; a function fragment is not promised to run
independently.

## 5. xref

```sh
"$RXD" js xref "$FILE" --byte 43 --jsonl -o "$DEMO/xref.jsonl"
"$RXD" js xref "$FILE" --byte 43 --jsonl | jq 'select(.record == "reference")'
```

`--byte` must be inside a declaration identifier or identifier reference. Whitespace,
strings, comments, keywords, static property keys and `foo` in `obj.foo` return
`INVALID_TARGET`; `key` in `obj[key]` is a lexical reference. Header detail reports
`resolution: resolved|unresolved|dynamic`, the selected name/range, and the canonical
declaration. Reference detail has `read`, `write`, `member_write`, `type_reference` and
`value_reference`. `parent_at` routes to `get`; the declaration and initializer are not
reference records. `dynamic` means the file contains `with`/direct `eval` and is
conservatively unscanned.

## 6. symbols

```sh
"$RXD" js symbols "$FILE" --jsonl
"$RXD" js symbols "$FILE" --within "$REQUEST_ID" --jsonl
```

`symbols` emits `symbol` records (resolved bindings with canonical declaration IDs) and
`unresolved_reference` records. `relation: declared` means the binding is declared in the
selected region; `external` means a region reference to an outside declaration. Same-name
different bindings stay separate; imports resolve only to the local binding.

### Extract a canonical declaration by symbol ID

```sh
SYMBOL_ID=$(jq -r 'select(.record == "symbol" and .detail.name == "api") | .at' "$DEMO/symbols.jsonl")
"$RXD" js get "$FILE" --at "$SYMBOL_ID"
"$RXD" js get "$FILE" --at "$SYMBOL_ID" --json
```

The raw output is only the canonical declaration, not later redeclarations/assignments.
`--json` exposes the symbol, declaration kind/range/initializer and source; it never
claims runtime values or data-flow completeness. The symbol record's `start` can be passed
to `xref --at`.

### Extract a local fragment

Find, locate and xref results add `context_at` for the nearest supported syntax fragment
(variable declarator, assignment, return/throw, control statement, parameter, import,
etc.). `get` with that ID returns the exact fragment bytes; a variable declarator may omit
`const/let/var`, and an assignment may omit the statement semicolon. Functions/classes/
methods are context boundaries; entering them reselects an inner fragment. Use
`parent_at`/`entity.at` when no local fragment exists.

## Directories

`entities DIR` (or `functions`/`classes`/`methods`) and `find DIR --string TEXT` recurse into regular
`.js/.mjs/.cjs/.jsx/.ts/.mts/.cts/.tsx` files (case-sensitive), do not follow symlinks and
include hidden directories/`node_modules`. Results concatenate per-file header/records/
summary and end with `scan_summary` (detail `files`). Success needs exit 0 and the final
complete `scan_summary`; any file failure/resource overflow fails the whole call atomically.
`--threads` controls parallelism and in-flight parsed inputs (peak memory). Recursive
`--perf` adds a `discover` phase.

## Budgets and performance

Defaults: input bytes, entity count and AST depth are checked per file; records and output
bytes cover the whole invocation. Directory scans, records, semantic index nodes and AST
depth are bounded by `rxd_core::Budget`, which the CLI does not expose as flags. Syntax
errors fail atomically; there is no permissive mode. Raw source stdout has no summary;
exit status determines completeness. Ordinary representative queries follow the 150 ms
fresh-process rule; `benches/cli.py` measures release fresh processes with per-workload
samples, median/P95 and >150 ms share. Whole-function decompilation exceptions follow
[AGENTS.md](../../AGENTS.md#performance-requirements).

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend IDs, offsets, inference and degradation are in the
[JS design](../design/js-design.md#mcp-projection).
