# JavaScript examples

Ordinary CLI work, semantic indexing, retained name/value metadata and bounded
output scale with admitted input. Directory scans derive limits per file; lexical
and static-fold recursion/expansion guards remain.
See [ADR 0016](../adr/0016-input-scaled-cli-resources.md); MCP service quotas
remain independent and unchanged.

`rxd js` locates structures, searches literals and extracts source on demand; it never
executes input. Its query surface is:

| Command | Purpose | Default output |
| --- | --- | --- |
| `entities` | Every structure with containment; `-f` substring and exact `--kind`/`--name` | JSONL |
| `functions` / `classes` / `methods` | The same rows, one object kind group each | JSONL |
| `find` | Search decoded string literals and static template fragments; `--resolve-static` also matches bounded AST constant evaluation | JSONL |
| `resources` | Find syntactic loading candidates, known strings and unresolved expressions with extraction contexts | JSONL |
| `identifiers` | Find lexical declarations/references by exact name | JSONL |
| `calls` | Find direct/member/`new` call sites by callee name | JSONL |
| `members` | Find static member names and keys; `--access read\|write` narrows actual property access sites | JSONL |
| `locate` | Find the innermost structure and ancestors at a byte offset | JSON |
| `slice` | Extract source by structure/fragment ID, or the canonical declaration by symbol ID | Raw source |
| `xref --at` | Find the lexical binding, references and read/write roles at a name occurrence (`--byte` takes a raw offset instead) | JSONL |
| `symbols` | Discover declarations, external bindings and unresolved references in a region | JSONL |

Structure listings, `find` and `resources` accept a file or scan a directory; `locate/slice/xref/symbols/
identifiers/calls/members` accept one file. ASAR uses the separate
[`rxd asar` entry](asar.md) to extract a file, then these commands analyze it; identical
bytes and parse options keep the same content-bound IDs. For a local HTML capture,
use [`rxd html resources`](html.md#resource-discovery-and-batch-inline-handoff):
classify URLs/preloads/hints and batch-return original inline scripts with
`--kind script --location inline --include-content`. Save classic bodies as `.cjs`
and modules as `.mjs`, retain the HTML origin and body hash, then scan their directory
with `js entities`. JSON data blocks and import maps are not JS scripts. Network
acquisition remains an explicit, policy-checked Agent + curl step; see the
[HTML → JS workflow](../design/js-design.md#html--javascript--resource-acquisition).

The backend performs **structural location**: literals, identifier names, call sites and
static member names map to content-bound AST structures, lexical relationships and
original source ranges. JS deobfuscation and string decryption are permanent non-goals;
obfuscated files are analyzed by their existing structure and extracted verbatim. Literal
escape decoding is parsing, not decryption. `--resolve-static` only evaluates closed AST
constant expressions; it does not execute user functions, parse string arrays or restore
dynamic property names.

Structured output follows [ADR 0003](../adr/0003-output-envelope.md),
`schema_version=2`: navigation outside, format fields under `detail`; header input size is
`input_bytes`, path is `detail.path`. Default `slice` raw bytes are outside the envelope.

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
"$RXD" js resources --help
"$RXD" js identifiers --help
"$RXD" js calls --help
"$RXD" js members --help
"$RXD" js locate --help
"$RXD" js slice --help
"$RXD" js xref --help
"$RXD" js symbols --help
```

The fixture contains `request`, an anonymous async arrow `send` inside it, object
methods/getters/setters and a `Client` class. Offsets/counts below correspond to this
fixed file; use actual query results for other inputs.

## 1. entities

```sh
"$RXD" js entities "$FILE" -o "$DEMO/entities.jsonl"

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
"$RXD" js find "$FILE" -f '/api/login' -o "$DEMO/find.jsonl"
jq 'select(.record == "match")' "$DEMO/find.jsonl"
```

The fixture has two hits: the top-level `api` constant and `send`'s request. Use
`--resolve-static` for closed constant expressions:

```sh
"$RXD" js find "$FILE" -f '/api/login' --resolve-static
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

## 4. slice

```sh
"$RXD" js slice "$FILE" --at "$REQUEST_ID"
"$RXD" js slice "$FILE" --at "$REQUEST_ID" --json
"$RXD" js slice "$FILE" --at "$REQUEST_ID" -o "$DEMO/request.js"
```

Default output is exact original source bytes with no added newline or formatting.
`--json` returns a `record: source` envelope with source, entity metadata and original
range. Extract a parent to see closure context; a function fragment is not promised to run
independently.

## 5. xref

```sh
"$RXD" js xref "$FILE" --byte 43 -o "$DEMO/xref.jsonl"
"$RXD" js xref "$FILE" --byte 43 | jq 'select(.record == "reference")'
```

`--byte` must be inside a declaration identifier or identifier reference. Whitespace,
strings, comments, keywords, static property keys and `foo` in `obj.foo` return
`NOT_FOUND`; `key` in `obj[key]` is a lexical reference. Header detail reports
`resolution: resolved|unresolved|dynamic`, the selected name/range, and the canonical
declaration. Reference detail has `read`, `write`, `member_write`, `type_reference` and
`value_reference`. `parent_at` routes to `slice`; the declaration and initializer are not
reference records. `dynamic` means the file contains `with`/direct `eval` and is
conservatively unscanned.

## 6. symbols

```sh
"$RXD" js symbols "$FILE"
"$RXD" js symbols "$FILE" --within "$REQUEST_ID"
"$RXD" js symbols "$FILE" -f api
```

`symbols` emits `symbol` records (resolved bindings with canonical declaration IDs) and
`unresolved_reference` records. `relation: declared` means the binding is declared in the
selected region; `external` means a region reference to an outside declaration. Same-name
different bindings stay separate; imports resolve only to the local binding.

### Extract a canonical declaration by symbol ID

```sh
SYMBOL_ID=$(jq -r 'select(.record == "symbol" and .detail.name == "api") | .at' "$DEMO/symbols.jsonl")
"$RXD" js slice "$FILE" --at "$SYMBOL_ID"
"$RXD" js slice "$FILE" --at "$SYMBOL_ID" --json
```

The raw output is only the canonical declaration, not later redeclarations/assignments.
`--json` exposes the symbol, declaration kind/range/initializer and source; it never
claims runtime values or data-flow completeness. The symbol record's `start` can be passed
to `xref --at`.

### Extract a local fragment

Find, resources, identifiers, calls, members, locate and xref results add `context_at` for the nearest supported syntax fragment
(variable declarator, assignment, return/throw, control statement, parameter, import,
etc.). `slice` with that ID returns the exact fragment bytes; a variable declarator may omit
`const/let/var`, and an assignment may omit the statement semicolon. Functions/classes/
methods are context boundaries; entering them reselects an inner fragment. Use
`parent_at`/`entity.at` when no local fragment exists.

## Locate property writes and loading calls

When a minified loader uses a short property such as `.p`, inspecting every read is
unnecessary. Filter writes first, then fetch the local assignment rather than its whole
containing function:

```sh
printf '%s\n' \
  'runtime.p="";' \
  'runtime.p=document.querySelector("meta[name=AppPublic]").content;' \
  'const chunk=runtime.p+"static/login/chunk/";' > "$DEMO/loader.js"

"$RXD" js members "$DEMO/loader.js" --name p --access write -o "$DEMO/writes.jsonl"
# Two writes, not the third occurrence that only reads runtime.p.
CONTEXT=$(jq -sr 'map(select(.record == "member")) | last | .context_at' "$DEMO/writes.jsonl")
"$RXD" js slice "$DEMO/loader.js" --at "$CONTEXT"

"$RXD" js calls "$DEMO/loader.js" --name querySelector
```

The final call hit carries the same assignment `context_at`. Follow its `AppPublic`
clue with `rxd html entries page.html --tag meta --attr name=AppPublic`; obtaining the
configuration does not execute the loader or prove a particular resource is requested.
No HTML/JS backend dependency or automatic network fetching is introduced.

`members --access all` is the default. `--access read` and `write` both include compound
assignments and updates; simple assignment, destructuring/for-in/of targets and deletion
are writes. A target's object and computed-key expressions are still reads. Member
records expose `detail.read/write`; object/class declaration and JSX keys leave both
null and are excluded by read/write filters. `context_at` may be null at a function/
class context boundary; fall back to `parent_at`. Use lexical `symbols/xref` when the next
question is about a binding, not as a mandatory step for every property hit.

## Follow resource-reference sites

The following is written as source data only; rxd never loads the imported files or
runs the code:

```sh
printf '%s\n' \
  'import "./boot.js";' \
  'export { thing } from "./shared.mjs";' \
  'import(`./chunks/${"panel"}.mjs`);' \
  'const worker = new Worker(new URL("./worker.mjs", import.meta.url));' \
  'script.src = runtime.publicPath + chunkName(id);' \
  'script.setAttribute("src", "/static/" + "entry.js");' \
  'script.src += "?v=1";' > "$DEMO/resources.mjs"

"$RXD" js resources "$DEMO/resources.mjs" -o "$DEMO/resources.jsonl"
"$RXD" js resources "$DEMO/resources.mjs" --kind module --resolution resolved
"$RXD" js resources "$DEMO/resources.mjs" --kind attribute --resolution unresolved \
  -o "$DEMO/dynamic-loads.jsonl"
AT=$(jq -sr 'map(select(.record == "resource")) | first | .context_at' "$DEMO/dynamic-loads.jsonl")
"$RXD" js slice "$DEMO/resources.mjs" --at "$AT"
```

The module rows name `./boot.js`, `./shared.mjs` and the folded
`./chunks/panel.mjs`. The setter folds to `/static/entry.js`. The runtime public
path and compound assignment remain unresolved; `"?v=1"` is not misreported as
the complete URL. The nested Worker/URL forms produce separate evidence records;
the URL's base is `import_meta_url`, not the local file directory.

Use `--kind module|script|worker|request|url|attribute`, or `-f` over the
known string/original unresolved expression. A resolved string can still be a
package name, API endpoint, navigation URL or invalid URL, and named APIs can be
shadowed. Source positions and contexts are evidence to review, not download
instructions.

On the captured login bundle, this directly finds its dynamic chunk-src assignment
rather than all strings ending in .js:

```sh
# Replace login.js with the captured local bundle; no network access occurs.
"$RXD" js resources login.js --kind attribute -f 'c.p'
# Use its context_at with slice, then follow the observed public-path setup.
"$RXD" js members login.js --name p --access write
```

There is no automatic chunk-table expansion or runtime execution. Preserve the
input identity and review configuration, filename tables and loading conditions
before constructing a finite curl candidate list.

## Structured values and local constants

`values` lists named data slots without executing expressions or converting objects
into JSON. As with other name-based listings, `-f` matches the **slot name**, not
its value; `--name` is exact. `--within` limits a source region, whereas
`slice --at/--from` reads an identified source unit:

```sh
printf '%s\n' \
  'const config = {hashes:{home:"a1",guide:"b2"},flags:[true,null,,...rest]};' \
  'loader["load"]("/static/entry.js");' > "$DEMO/values.mjs"
"$RXD" js values "$DEMO/values.mjs" --kind initializer --name config \
  -o "$DEMO/config-values.jsonl"
CONTEXT=$(jq -r 'select(.record == "value") | .context_at' "$DEMO/config-values.jsonl")
"$RXD" js values "$DEMO/values.mjs" --within "$CONTEXT" --kind property -f home
"$RXD" js values "$DEMO/values.mjs" --kind argument
"$RXD" js slice "$DEMO/values.mjs" --at "$CONTEXT"
```

The `home` row has a literal string value `a1`, its original spans and a sliceable
property context. Scope coordinates can also be structures or canonical symbols.
No scope filter accepts a fabricated range.

Property rows retain duplicate keys, source order, computed-key uncertainty and
spreads. Array holes differ from null; argument/element indexes are syntactic,
especially after spreads. Containers have child slots, not an invented final value.
A `JSON.parse("...")` argument exposes the decoded string; the call is not executed
and its result remains unresolved.

For a local source such as:

```js
const base = "/assets/";
const name = "entry.js";
import(base + name);
```

```sh
rxd js resources entry.mjs --resolve-bindings
```

returns `/assets/entry.js` with `resolution: binding` and the contributing canonical
symbol coordinates. Without that option the expression stays unresolved. Only
earlier, unwritten, simple const-string initializers in the same lexical scope are
followed; outer-scope captures, mutable values, object reads and calls remain
unresolved. This does not prove the load executes or that the URL is authorized.

`calls --name load` recognizes both `loader.load()` and
`loader["load"]()`, including bounded static string keys and transparent TS wrappers.
It does not resolve a variable key or infer the receiver's runtime identity.

`find --resolve-static` preserves UTF-16 code units: `"😀x"[1]` is a low surrogate,
not `x`; `String.fromCharCode(0x1f600)` is U+F600, not an emoji. Invalid code points
or unsupported conversions remain unproved. Intrinsic helper folding assumes standard
unmodified built-ins, not runtime prototype analysis. Visible String shadowing,
direct eval/with or unsupported String/global-object use disables helper folding
conservatively; operator-only folding remains available.

## Consistent listing selectors

`identifiers/calls/members/symbols/values` allow listing without a name. They share
optional `-f/--filter`, `--case sensitive` and exact `--name`; filters AND together
and never search owner names or arbitrary payload fields:

```sh
"$RXD" js calls "$DEMO/values.mjs" -f LOAD
"$RXD" js calls "$DEMO/values.mjs" -f LOAD --case sensitive
"$RXD" js calls "$DEMO/values.mjs" --name load
"$RXD" js members "$DEMO/values.mjs" -f hash
```

The first call search matches `load`; the second does not. A dynamic callee such as
`loader[key]()` is outside the named-call listing, not inferred from spelling.

## Directories

`entities DIR` (or `functions`/`classes`/`methods`), `find DIR -f TEXT` and
`resources DIR` recurse into regular
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
