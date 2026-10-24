# JavaScript backend

Status: implemented.

`rxd js` uses Rust and Oxc for compact bundles and ordinary JavaScript files without
source maps. It provides `entities`/`functions`/`classes`/`methods`, `find`,
`resources`, `values`, `identifiers`, `calls`, `members`,
`locate`, `slice`, `xref --at`/`--byte` and `symbols`. The backend maps literals, identifiers,
call sites and static member names to content-bound AST structures, lexical
relationships and original source ranges. It never executes input, performs
deobfuscation or string decryption, restores obfuscator intent, or promises runtime
semantics.

ASAR extraction is handled by `rxd asar`; see [ASAR](asar-design.md). HTML resource
classification and inline source handoff use `rxd html resources`; see
[HTML](html-design.md#resource-candidates) and the [JavaScript examples](../demo/js.md).

## Scope

The backend handles local valid-UTF-8 JS, JSX, TS, TSX and TypeScript declaration
files, both script and ESM. The four structure listings, `find` and `resources`
recurse into directories; files are
analyzed independently and cross-file modules/bindings are not resolved. Original
source retains type annotations, JSX, modifiers and declaration syntax; it is never
compiled, transformed or type-checked as a project.

Structural records cover the program root; function declarations/expressions and
arrow functions; class declarations/expressions; methods, constructors, getters and
setters; object-literal methods/getters/setters; and TS type aliases, interfaces,
enums, namespaces, external modules, global declarations and interface method
signatures. A method produces one method record, not a duplicate function record.
Ordinary statement blocks and object literals produce no structural record.
`parent_at` is the nearest containing structure, not a full lexical scope tree. Arrow
functions in class fields are function records whose containing structure is the class.

Deobfuscation and string decryption are permanent non-goals, including control-flow
recovery. Obfuscated input is still searched for readable literals and extracted
byte-exactly; standard JS escape decoding is parsing and is used by `find`. The first
version does not execute input, recover original modules, perform program slicing or
build a complete call graph. Bundler wrapper functions are ordinary functions; module
boundary detection may be added separately later.

## Agent workflow

A typical task locates an API and extracts its implementation:

1. `find` the API string; the result includes the literal position and containing
   structure.
2. `slice` the target function's original source.
3. Read the function and identify outer closures or helpers.
4. `slice` the parent structure, or `locate` another position by known byte offset.
5. `symbols --within` lists declarations, external bindings and unresolved references
   with exact declaration offsets.
6. `xref --at` (or `--byte`) follows an explicit binding and returns declaration/reference
   structures, reducing same-name false matches.

For an existing `rg` hit, `locate` can be used directly. `slice` on a function preserves
its access to outer variables; the fragment may not run independently. `parent_at`
provides the closure context.

## HTML → JavaScript → resource acquisition

Keep deterministic local analysis separate from network acquisition. For a captured
HTML page, start with one source-backed catalog rather than treating every script tag,
link or string ending in `.js` as a download. This route needs only a CLI build with
`--no-default-features --features backend-js,backend-html`; neither JVM nor MCP is
required:

```sh
rxd html resources page.html --base-url 'https://example.org/app/' -o resources.jsonl
rxd html resources page.html --kind script --location inline --include-content \
  -o inline.jsonl
```

The document URL must be the capture's **final** URL after redirects. HTML resolves
relative references and the first applicable lexical `base[href]`, distinguishes
classic/module JS from import maps/data blocks, separates asset/preload references
from connection hints, and retains load-condition evidence and unresolved references.
It does not fetch, parse CSS dependencies, expand import maps or simulate a browser.
An absolute resolved URL is still only a candidate; the Agent enforces permitted
origins/schemes, credentials policy, redirects, MIME, size and content identity when
using curl. Do not forward a page's cookies to CDN/other origins by default.

`--include-content` returns each non-empty inline body's exact original text and
`detail.content_source_id`. Decode the JSON string and write its UTF-8 bytes unchanged;
name the artifact from the content hash, never an input URL or attribute. Preserve
the container header's `source_id`, row `at`, and `content_start/content_end` as
provenance. Use `.cjs` for classic scripts and `.mjs` for modules so JS auto mode
preserves their intended source type. This materializes many inline scripts from one
listing rather than issuing one extract query per script; `extract --at --content`
remains available for a single artifact. The JS source ID is the body
hash, and JS offsets start at zero, not at the HTML offset. Import maps and other
data blocks are deliberately not selected by `--kind script`.

On downloaded JS, `resources` inventories recognized loading sites rather than all
strings that resemble filenames. `--resolution unresolved` keeps dynamic expressions
visible, with directly sliceable contexts. For example, on the saved login bundle,
`resources --kind attribute -f 'c.p'` selects the actual script-src assignment and its
complete chunk-name/hash construction; it does not execute or expand that function.
Follow public-path setup with `members --name p --access write` and `slice` its
`context_at`. Follow observed configuration reads back into
`html entries --attr` (for example an `AppPublic` meta element). Use
`find --resolve-static` for bounded literal construction and `symbols/xref` only
when lexical binding identity matters. Chunk filename/hash tables and CSS loading
conditions still require Agent interpretation; no general Webpack resolver or
automatic runtime dependency closure is claimed. Record source identities,
coordinates, configuration inputs, conditions and unresolved expressions for every
derived edge, then fetch only the reviewed finite URL set.

The saved anonymous Feishu login snapshot (92,565 bytes, not authenticated Drive)
yields 2 external JS + 2 external CSS, 4 non-empty inline JS + 1 inline CSS,
4 connection hints and one data-URI icon; one empty inline script is counted but
omitted. The four inline scripts total 5,346 bytes and can be materialized together
and scanned as one directory. The CLI-only walkthrough also fetched the four direct
resources again with curl (4,436,195 bytes, HTTP/MIME/size/SHA-256 checked), parsed
both fresh JS bundles, and reduced the public-path query to two directly sliceable
writes plus one HTML `AppPublic` metadata match. All 54 local JS artifacts, including
the previously captured chunks, parsed successfully. No browser, target-code
execution, credentials or business-interface requests were used. These are HTML
declarations and reviewed static evidence, not proof of runtime reachability.
Neither a complete lexical catalog nor successful acquisition of the reviewed URLs
is page coverage.

## Commands

```sh
rxd js entities bundle.js
rxd js functions bundle.js -f check
rxd js classes bundle.js
rxd js methods bundle.js
rxd js entities bundle.js --within '<entity-id>' --depth 1
rxd js entities bundle.js --kind object_method
rxd js entities bundle.js --name checkFlag
rxd js find bundle.js -f '/api/login'
rxd js find bundle.js -f 'setInterval' --resolve-static
rxd js resources bundle.js
rxd js resources bundle.js --kind attribute --resolution unresolved
rxd js resources assets/ --kind module --threads 2
rxd js identifiers bundle.js --name '_0x4e3653'
rxd js identifiers bundle.js --name '_0x4e3653' --role reference --within '<entity-id>'
rxd js calls bundle.js --name 'AIS3'
rxd js calls bundle.js -f 'load'
rxd js values bundle.js --kind property --name hashes
rxd js members bundle.js --name 'checkFlag'
rxd js members bundle.js --name 'p' --access write
rxd js locate bundle.js --byte 183240 --json
rxd js xref bundle.js --byte 183240
rxd js symbols bundle.js --within '<entity-id>'
rxd js slice bundle.js --at '<entity>'
rxd js slice bundle.js --at '<symbol>' --json
rxd js slice bundle.js --at '<parent>' --json
```

| Command | Behavior | Default output |
| --- | --- | --- |
| `entities` | Every structural entity, including the file root; ranges, names, parent relations, `-f` substring, exact `--kind`/`--name`. | JSONL |
| `functions` | The same rows restricted to function declarations, expressions and arrows. | JSONL |
| `classes` | The same rows restricted to classes, interfaces, enums, aliases and namespaces. | JSONL |
| `methods` | The same rows restricted to class/object members: methods, constructors, getters, setters. | JSONL |
| `find -f PATTERN` | Literal substring over decoded string values, folded unless `--case sensitive`; returns containing structures. `--resolve-static` also matches bounded AST constant evaluation. | JSONL |
| `resources` | Syntax-backed module/worker/request/URL/attribute candidates, optionally bounded local const strings; accepts files or directories. | JSONL |
| `values` | Named object/array slots, variable initializers, call arguments and assignments, with source-backed values and explicit unknowns; single-file, optional `--within`. | JSONL |
| `identifiers` | Lexical identifier occurrences (declaration or reference), optional own-name filters. | JSONL |
| `calls` | Statically named call sites (direct, member or `new`); optional own-callee `--name`/`-f`. | JSONL |
| `members` | Static member names, property keys, JSX attribute names and statically resolvable computed keys; `--access read\|write` narrows property accesses. | JSONL |
| `locate` | Innermost structure and ancestors containing a byte position. | JSON |
| `slice` | Validate a structure, symbol or fragment ID and extract original source. | Raw bytes |
| `xref --at` | Single-file lexical binding and same-binding references at a name occurrence. `--byte` anchors on a raw offset instead, including one inside a multi-byte identifier. | JSONL |
| `symbols` | Lexical bindings and unresolved references in a file or selected structure. | JSONL |

`entities` returns the program and all descendants by default. `--within` returns
descendants but not the selected structure itself. `--depth 1` limits to direct
children; no depth means all descendants; `--depth 0` returns none. Record `depth` is
always absolute from program; filter depth is relative to the selected root. Filters
preserve original parent IDs. `--kind` matches exact kinds separated by commas;
`--name` matches exact `name` or context `label` case-sensitively. Both can combine
with `--within`/`--depth`.

`find -f` includes string literals and static template fragments. Ordinary
strings are decoded before matching while original spans are retained. Template
fragments match independently and are marked `dynamic`; concatenation is not inferred
across interpolations or fragments. Empty queries are argument errors; there are no
regex or implicit fuzzy matches. Unpaired UTF-16 surrogates remain valid JSON escapes
and are never replaced, preserving search semantics.

`--resolve-static` additionally evaluates closed AST constant expressions within
fixed step/depth/value budgets. Hits set `detail.literal_kind: static_expression`,
`detail.resolution: static` and `detail.raw` to the full original expression; ordinary
literal hits leave these null. Supported evaluation covers literals, string/template
concatenation, standard conversions such as `String.fromCharCode`/`fromCodePoint`,
a small set of string methods (`slice`, `substring`, `substr`, `charAt`,
ASCII-only `toUpperCase`/`toLowerCase`), static array/string indexes and numeric/bitwise
operations, with all arguments statically evaluable. All strings, indexes and method
bounds use JS UTF-16 code units, including isolated surrogates. `fromCharCode`
uses ToUint16, while `fromCodePoint` validates integral code points through U+10FFFF
(including surrogate code points). Slice relative offsets, substring bound swapping
and substr length semantics are distinct. Invalid code points, unsupported receivers
and unsupported coercions produce no derived hit. Numeric formatting uses the pinned
Oxc ECMAScript formatter; numeric string coercion is limited to ASCII decimal and
Infinity forms. Step/depth limits cover quasi/array elements, and the 64 KiB value
limit covers intermediate and aggregate values before concatenation/collection.

This optional evaluator assumes standard, unmodified intrinsic operations. A bounded
syntax prepass disables helper calls for any visible `String` binding, direct eval/with,
or String/global-object uses outside the supported direct helpers. This intentionally
includes conservative false negatives and does not prove external prototypes unmodified.
Literal/operator-only folding remains available when helpers are disabled.
It does not read bindings, execute user functions, interpret string tables or rewrite
source. It is not a deobfuscator. The option is off by default; one-level foldability
guards avoid repeatedly walking impossible operands.

`locate` uses original UTF-8 byte offsets in half-open `[start, end)`; `--byte` must be
less than the file length, and an empty file has no locatable position. The innermost
covering structure wins; ancestors are ordered inner to outer. The program covers
`[0, file_length)`.

### Shared listing contract

As in DEX/JAR/.NET, the listing itself comes first; selectors only narrow it.
`identifiers/calls/members/symbols/values` accept optional `--name` (exact,
case-sensitive), `-f/--filter` (substring, folded by default) and `--case sensitive`.
Selectors AND together; omitting them lists every row in that command's declared
profile. Empty exact names are errors; an empty substring matches all. Filters search
only the row's own label, never its owner or a second payload field. Unicode folding
uses the shared lowercase rule, preserving supplementary characters and lone UTF-16
surrogates instead of replacing them.

`--within` is a source-region restriction, not a second spelling of identity
selection. `slice --at` reads an identified source unit; `xref --at` anchors a binding
query. A coordinate is a span in the current input, with no hash-then-reopen race. JS
retains native source spans and lexical identities rather than inventing bytecode tokens.

`identifiers`, `calls` and `members` are single-file syntax-level location queries.
They do not copy `find`'s literal semantics or infer bindings. Calls list statically
named callees, not every dynamic call. Their filters use identifier name, callee name
and member name respectively; no regex/fuzzy mode.

- `identifiers` emits `record: identifier` with `detail.role` (`declaration` or
  `reference`) and `detail.syntax_kind` (`binding_identifier` or
  `identifier_reference`). `--role` accepts `all|declaration|reference`. Only real
  lexical names match: `foo` in `obj.foo` is not lexical, while `key` in `obj[key]`
  is. `parent_at` is the containing structure; `context_at` is the local fragment.
- `calls` emits `record: call` with `callee`, `call_kind` (`direct`/`member`/`new`),
  `optional` and `argument_count`. It answers only where a name is called, not return
  values, call graphs, cross-file resolution or alias propagation. Static string
  computed names (`obj["load"]()`, `obj["lo" + "ad"]()`) use the same bounded key
  projection as `members`; dynamic keys (`obj[key]()`) still produce no hit. Transparent
  TS/parenthesis wrappers are supported. `optional` includes optional syntax along the
  callee path, not in arguments; it is not a proof of runtime short-circuit behavior.
  `context_at` routes directly to the nearest
  supported original syntax fragment (for example the assignment that contains a call).
- `members` emits `record: member` with `member_kind` (`static_member`,
  `property_key`, `object_method_key`, `class_field_key`, `class_method_key`,
  `jsx_attribute_name`, `computed_key`) and `key_resolution` (`literal` or `static`
  for computed keys, null otherwise). Computed keys match only string literals or
  statically foldable operator/index expressions; helper calls are never evaluated
  to infer a key. The same rule covers object/class computed declaration keys.
  Variable keys and numeric indexes are handled by
  `identifiers`/normal syntax, not as property names. This is a syntactic property
  position, not runtime object/prototype/type resolution. `context_at` routes to the
  local fragment without requiring a separate `locate` or extracting the whole owner.
  `detail.read/write` are booleans for member accesses and null for declaration/property/
  JSX keys. `--access all|read|write` defaults to all, preserving every existing match;
  read/write filters exclude keys whose access flags are null. Simple assignment,
  destructuring targets, for-in/of targets and `delete` are writes; compound/logical
  assignment and increment/decrement are both reads and writes, matching either filter.
  Only the actual target is written: `obj.p.child = rhs` and `obj[key.p] = rhs` read `p`.
  Parentheses and TypeScript assertion/non-null wrappers do not change the target.
  These are syntactic operations, not proof of execution, a particular receiver, getter/
  setter behavior or runtime success. Definitions are not mislabeled as property writes.

All three obey the record and output-byte budgets from `rxd_core::Budget`, sort by `(start, end)` and keep
the same coordinate scheme. They have no own ID; `parent_at` points at an extractable
structure. Their `context_at` is null at context boundaries with no local fragment (for
example a shorthand arrow); use `parent_at` in that case. Context tracking is an AST-depth
stack, not a whole-file fragment index or semantic pass.

For a loader/public-path task, start with `members --name p --access write`, then
`slice` each returned `context_at`. Follow the literal/configuration evidence into HTML
metadata as needed. Use `symbols/xref` only when an actual lexical binding needs tracing;
these queries do not resolve a property receiver or expand a chunk map automatically.

`slice` accepts structure IDs, symbol IDs and context fragment IDs and emits the exact
original range without reformatting or extra newlines. `--json` wraps source and
structure metadata in one JSON object. Every command supports `-o FILE` with bytes
identical to stdout. Diagnostics go to stderr; `--perf` reports phase timings as JSONL. Each
invocation is independent, with no implicit background service or persistent index.

## Structured value slots

`values FILE [--within COORD] [--kind KIND] [--name NAME] [-f LABEL] [--case sensitive]`
lists syntax-backed value slots. It is single-file, with
`analysis: syntax` and `value_profile: syntax_value_slots_v1`. It does not evaluate
objects, invoke getters/functions, expand spreads, decode JSON strings as objects,
or determine final property values.

Kinds are `initializer` (variable declarators), `property` (object-literal entries,
including methods/accessors/spreads), `element` (array entries including holes),
`argument` (calls, constructors and dynamic import source/options), and `assignment`.
All matching nested slots are included in source order; this is not a shallow object
view. Duplicate keys, source ordering, shorthand, computed keys, holes and spreads
remain separate. `__proto__` is not converted into an ordinary JSON property or
applied to an object.

The slot's `detail.name` is its property key, binding pattern, assignment target or
zero-based syntactic index for arguments/elements. Unknown computed keys and spreads
use their original expression as the label. `-f` searches only this label, not values
or owner names; `--kind` narrows the same listing. An index after a spread is not a
runtime position. Key resolution is literal/static/unresolved; only closed string
expressions are folded for computed keys.

`--within` restricts listing to an existing structure, context fragment or canonical
symbol's declaration, including contained slots. Nonexistent coordinates fail, never
silently produce an empty listing. Symbol scopes use the existing bounded semantic
index; other scopes are parser-only. The command does not overload `--at`
as a scope selector: read an identified source context with `slice` instead.

Rows have null `at`, source-backed `start/end`, containing `parent_at` and sliceable
`context_at` where available. Detail fields are
`kind, name, syntax_kind, owner_start, owner_end, index, computed, spread, key_start,
key_end, key_resolution, value`. Owner ranges identify the original object, array,
call, declarator or assignment, not runtime object identities.

The nested `value` always has `start, end, kind, resolution, value, source,
unresolved_reason`. Literal primitive strings/numbers/booleans/null are decoded;
closed string concatenations/templates can be static. Nonfinite numeric literals
are unresolved. Objects/arrays have `resolution: container`, not a fabricated JSON
value; holes have `resolution: hole`, distinct from null. Other expressions retain
exact source and an unresolved reason. Getters/methods, spreads and compound
assignments are explicitly unresolved. Resolved values/containers do not duplicate
their source text; their original spans remain available. Strings preserve UTF-16.

Collection uses the selected work/metadata envelope; complete serialization uses
its separate output envelope (ADR 0016), with atomic failure rather than truncation.
`complete` means all selected source slots were returned, not that the object's
runtime value or dependency graph is known.

## Resource-reference query

`resources FILE|DIR` is a parser-only projection with
`header.detail.analysis: syntax` and `resource_profile: syntax_load_sites_v1`.
It lists syntactic reference **candidates**, not proven calls to built-ins, successful
loads, a browser dependency graph or page coverage. No network, package resolver,
import-map expansion or input-code execution is involved. Default queries are parser-only;
`--resolve-bindings` optionally enables the narrowly bounded lexical projection below.

Supported sites, with their exact `--kind` groups:

| Kind | Supported syntax |
| --- | --- |
| `module` | import declarations, named/star re-exports, dynamic `import()`, direct `require()`, `module.require()`, `require.resolve()`, and TS external import-equals |
| `script` | `importScripts()`, one candidate per supplied argument |
| `worker` | `new Worker()`, `new SharedWorker()`, `navigator.serviceWorker.register()` |
| `request` | `fetch()`, `navigator.sendBeacon()`, `new WebSocket()`, `new EventSource()` |
| `url` | `new URL(value[, base])`, which constructs a value and does not itself load it |
| `attribute` | Direct member assignments to lowercase `src/href`, and member `setAttribute("src"|"href", value)` calls |

Web API names may have one explicit `window/self/globalThis` prefix, with static
dot or string-literal computed member names. Other receiver aliases, arbitrary
`object.fetch()` methods and dynamic computed names are not guessed. Matched names
can still be shadowed: use lexical queries if binding identity matters.
`src/href` receivers are **not** proven DOM elements, and `href` can mean navigation
rather than an asset. Destructuring/updates, JSX props, object-literal props, XHR,
`eval`, tagged templates, TS import types and bundler-specific loader APIs are not
in this profile. The existing generic `members/calls/find` queries remain useful
outside it. `require.resolve` is a resolution hint, not a module execution claim.

### Values and filters

Each candidate retains a target expression even when its value cannot be proved.
`detail.name` is the decoded target string when known, otherwise the exact original
target expression. `-f/--filter` searches this one label, folding case by default;
`--case sensitive` disables folding. `--kind` is exact, and
`--resolution resolved|unresolved` selects known versus unproved **strings**, not
whether a URL has been resolved. `resolved` includes literal, folded and opt-in binding-derived values.
All filters are ANDed. `--within` uses a real structure coordinate and selects
contained loading sites, including nested structures; it requires a single file.

Resolution is `literal|static|binding|unresolved` (`binding` requires opt-in). String literals and interpolation-free
templates are literal. Closed string-only `+` and template interpolation are folded
automatically; transparent parentheses/TS assertion wrappers do not change values.
Folding operates on exact JS UTF-16 code units, including lone surrogates. It does
not evaluate calls (including apparently pure helpers), coerce non-string values or
interpret tables. By default it does not follow variables either. A function call,
numeric interpolation or runtime public path remains unresolved.

Optional folding has a per-expression depth bound of 32, 512 expression/quasi steps
and at most 32,768 UTF-16 units (64 KiB) per intermediate/result value. Literal decoding
remains bounded by the original input, not this derivation limit. An unavailable
derivation retains the whole candidate with `unresolved_reason:
not_closed_string|fold_limit`, rather than silently dropping it. Core input/AST/output
failures still fail the whole query. Compound/logical assignments retain their RHS
as unresolved `compound_assignment`, never mislabeling a suffix/default as the
complete assigned URL. Spread arguments retain their original spelling and
`spread_argument`, never treating the spread operand as one URL.

### Optional local const strings

`--resolve-bindings` builds Oxc's existing node-budgeted lexical index and echoes
`resolve_bindings: true`; header `binding_profile` becomes
`local_const_strings_v1` (null otherwise). Site recognition remains syntax-based:
this does not prove that a named web API is a built-in.

Only simple `const name = expression` bindings with no lexical write references,
in the **same lexical scope** as each reference, with the initializer ending before
that reference, are followed. Initializers must themselves close to strings using
the existing literal/concatenation/template rules. No let/var/import/parameter/
destructuring bindings, closure or outer-scope captures, object/array property reads,
calls, control-flow interpretation or cross-file resolution. Direct eval/with makes
binding derivation unavailable for the whole file. Source ordering and read-only
bindings are not proofs of reachability or successful initialization.

Success has `resolution: binding` and a bounded `bindings` list of
`{at, initializer_start, initializer_end}` evidence; each `at` is the real canonical
symbol coordinate accepted by `slice`. Non-binding results have an empty list.
Explicit URL base expressions use the same projection. SetAttribute name recognition
stays syntax-local. Failure retains the original expression; additional reasons are
`binding_unavailable, not_local_const, binding_scope_boundary,
binding_before_initializer, dynamic_scope`. The same depth/step/value ceilings
bound the whole derivation, not each binding independently. No speculative values
are returned when a derivation stops.

### Records and URL context

Each `resource` has null `at`, its target's original UTF-8 `start/end`,
the containing structure's `parent_at`, and a local `context_at` accepted by
`slice --at`. Use `parent_at` when no local fragment exists. Named re-exports use
`fragment.export_from_declaration`. No synthetic resource coordinate or fabricated
byte range is introduced.

Fixed detail fields:

- `kind/syntax_kind/name/resolution/unresolved_reason`.
- `site_start/site_end`: the whole import, call, constructor or assignment.
- `argument_index`: zero-based **syntactic** argument position, null for declarations
  and assignments; spread arguments do not establish runtime argument positions.
- `spread/optional/type_only`: syntax facts, not reachability. `type_only` records
  the declaration-level TS import/export modifier, not compiler erasure inferred from
  individual specifiers. `optional` reflects the matched call/member path, not all
  surrounding conditions.
- `attribute/operator`: matched src/href and assignment operator, null otherwise.
- `bindings`: local const evidence, empty unless binding derivation succeeds.
- `base`: `{kind, expression}`. Kind is `module|commonjs|worker|environment|
  explicit|import_meta_url|none|unknown`; an explicit constructor base expression
  carries `name/resolution/unresolved_reason/start/end`, otherwise expression is null.

`base` records the context needed to interpret the value; it is **not an absolute
URL**. Browser environment references can depend on document/worker state, while
module references need their referrer and possibly import maps, and CommonJS needs
host/package resolution. No URL is inferred from a local artifact path or one global
base switch. `new URL("./worker.js", import.meta.url)` identifies the module-relative
base explicitly. Nested `new Worker(new URL(...))` retains separate Worker and URL
records, with spans and shared context linking them; no constructor is evaluated.

The query echoes `kind/resolution/filter/case/within/resolve_bindings`. Summary detail counts
selected `resolved/unresolved` candidates. Ordering is target start/end, then site
start and syntactic argument index. Directory results retain each file's header and
identity under the existing atomic scan contract. Completion means all selected
sites in this profile were delivered, even when values remain unresolved.

A literal string is not necessarily a valid URL, a network address or an authorized
download. Request records can name business APIs; module names can be packages;
attribute writes can navigate or use embedded data. **Do not pipe these labels
directly to curl.** The Agent must interpret the base and context, review unresolved
expressions and conditions, then apply the existing scheme/origin/credential/MIME
policy to a finite asset list.

## Recursive scan

`rxd js entities DIR`, `find DIR -f TEXT` and `resources DIR` recursively scan regular
`.js/.mjs/.cjs/.jsx/.ts/.mts/.cts/.tsx` files (case-sensitive), do not follow symlinks,
and do not apply ignore rules (hidden directories and `node_modules` are included). A
symlinked directory entry is not scanned. Files sort by path; with multiple files each
gets a worker and results are reassembled in path order, so output and budgets match a
serial run byte-for-byte. A single file does not create a thread pool.

`--threads` (default logical CPUs) controls both parallelism and in-flight parsed
inputs, and therefore peak memory: each worker holds at most one input and AST. Serial
and parallel `--perf` counters and phase totals agree (summed in one process; peak RSS
is the maximum). `--depth`/`--kind`/`--name` apply per file; `--within` is single-file
and returns `INVALID_QUERY` (exit 2) for directory input, including empty directories.
`locate/slice/xref/symbols/identifiers/calls/members/values` remain single-file.

Regular-file input keeps its single-file output and does not restrict extension. A
directory is a scan and a file is one input: there is no flag to choose, so the same
input always produces the same stream. A listing is JSONL because it is a stream, so
there is no `--jsonl` switch spelling out the default.

Recursive JSONL concatenates each file's header/results/summary and appends a final
`scan_summary` with `record, schema_version=2, backend, command, records, complete,
detail`, where `detail` is `files`. Records belong to the preceding header's
`detail.path/source_id`; identical content at different paths is emitted separately.
Success requires exit 0 and a final complete `scan_summary`; empty directories still
return it. Any file failure or resource limit fails the whole invocation atomically,
exposing no partial stdout and preserving the `-o` target.

Admission, syntax depth and applicable work/storage/output envelopes are checked
per file using ADR 0016. Ordinary CLI directory scans have no fixed file-count or
aggregate output quota. Directory nesting and worker concurrency remain bounded;
explicit service discovery quotas remain separate.
`--perf` accumulates counters/timings and adds a `discover` phase with `source_id:
null`. Each file is read once into a fixed buffer and parser state is released after
processing; no cross-file index is kept.

## Syntax and TS/JSX boundary

`--syntax auto|js|jsx|ts|tsx` and `--source-type auto|script|module` are independent.
Auto syntax uses case-sensitive extensions: `.jsx`→JSX, `.ts/.mts/.cts`→TS,
`.tsx`→TSX, other single files→standard JS; `.d.ts/.d.mts/.d.cts` are declaration
files. An explicit syntax overrides the filename (for extensionless TSX) and never
retries after parse failure. Standard `.js/.mjs/.cjs` remain non-JSX; a `.js` file
containing JSX must use `--syntax jsx`. Directory discovery only covers the eight
extensions and does not follow links; `--syntax` applies to every file.

`header.detail` and JSON `detail` always include `syntax` and `declaration_file`;
extended syntax sets `jsx|ts|tsx` and a boolean, standard JS sets null. `source_type`
is the final script/module mode. Source identity is based on original bytes; entity/
symbol/fragment IDs validate real nodes and are deterministic for the same content,
parse mode, syntax and schema. If copying to another extension changes auto syntax,
the caller must retain the original syntax/mode explicitly.

`find` marks JSX quoted attributes as `literal_kind: jsx_attribute` and JSX child text
as `jsx_text`. Known XML/HTML named and numeric entities are decoded with Oxc's rules;
unknown/invalid entities remain literal. Backslashes inside JSX quotes are not JS
escapes, and React text whitespace is not collapsed; original whitespace/newlines and
spans are retained. JS strings inside JSX expression containers decode normally. JSX
element/fragment/opening/closing/attribute/spread/expression/text nodes can be extracted
via `context_at`; they are not structure entities.

JSX component names and member-tag object bases use Oxc `IdentifierReference`; ordinary
tags, attribute names and static member names are not lexical references. Paired
opening/closing tags may each contribute a syntactic reference, not two runtime calls.
React component types, prop types and dynamic members are not resolved.

TS string literal types and template type static fragments are `type_string`/
`type_template`; template types do not infer concatenation across interpolations.
Type declaration names/hierarchy support source navigation; enum initializers remain
original expressions without evaluation. Enum members are bindings in their
initializer scope, and identifiers or static string/template member names can be
xref/slice source bindings; ordinary JS/JSX static property names remain
`NOT_FOUND`.

TS/TSX symbol records and slice symbol metadata add `space: type|value|type_value`,
meaning the lexical uses Oxc permits, not runtime existence or inferred type. Refs
references and symbols unresolved references add `type_reference`/`value_reference`
booleans; `typeof value` is a type use, which is not a runtime read. Ordinary JS/JSX
always has these fields as null. Missing host/module types may return unresolved rather
than guessing; these are not tsc results.

TS merged/redeclared symbols keep Oxc's canonical first identifier; declaration/value/
type facts are not textually merged into a new type. With multiple declarations,
`symbols.detail`, resolved `xref` header detail and `slice` symbol detail expose
`declaration_ids` in original order as structure/fragment IDs. `slice(symbol)` stdout
still extracts only the canonical declaration; the Agent can expand other
declarations/overloads. Metadata declaration IDs and top-level records share the
the record budget check before collection; summary/perf records only count top-level
results. A forged canonical symbol range from a later declaration is rejected.

`slice(symbol)` also covers type aliases, interfaces, enums, namespaces, import-equals
declarations, type parameters and enum members. Type parameters include original
constraint/default source; an enum member can provide its original initializer span.
TS type/namespace structures reset local fragment boundaries; property/call/construct/
index/function/constructor signatures, type parameters, enum members, import-equals
and export assignments can be extracted verbatim. Declaration functions and abstract
methods without bodies still emit their real syntactic range. Type identity,
references, spaces and declaration merging follow pinned single-file Oxc semantics;
cross-module resolution, alias propagation, inference, compilation and runtime
identity are out of scope.

## Identity and names

Each invocation first obtains one fixed source buffer; parsing, searching and
extraction all use it. It never parses one version and reopens another for extraction.

`source_id` is the original file's identity digest, and it lives in the stream header only.
An entity coordinate is the span and its kind:

```text
<kind>:<start>:<end>          e.g. function_declaration:26:149
```

The hash is not repeated inside ids: a coordinate names a range, not a file, so the same
coordinate means the same range in any input while the header says which input the rows
describe. That is also why there is no stale-coordinate error on a rewritten file.
`slice` and `entities --within`
validate kind and real structure boundaries; a syntactically valid range without a
matching entity is still an error. Paths are display metadata, not identity.

Object/class method ranges cover the complete member (modifiers, key, body); function
expressions cover the expression; function declarations cover the declaration node;
program covers the whole input. Exact spans come from the corresponding Oxc nodes and
are fixed by tests. A method and its underlying function never produce two entities on
the same span.

Records distinguish explicit names from context labels:

- `name`: declaration name, named function expression name, or a determinate member
  key; anonymous functions use null.
- `label`: Agent-facing context such as `a` in `const a = () => ...`.
- `label_origin`: `declared`, `binding`, `property`, `assignment` or `anonymous`.

A label is not a unique function name or runtime identity. Dynamic computed keys keep
the key expression range and a null name; expression text is never used to guess the
final property value.

Input is never pre-formatted, CRLF-converted or BOM-stripped, so offsets can extract
directly. If the parser needs adaptation, the adapter preserves or explicitly maps
spans and never silently moves offsets.

## Structured output (schema_version=2)

All structured output follows [ADR 0003](../adr/0003-output-envelope.md) and shared
constructors, with no legacy flat shim. Single-file structure/find/xref/symbols JSONL
is header → zero or more records → summary. Success requires exit 0 and
`summary.complete=true`; budget failures return no partial result.

The header contains `record, backend, command, source_id, input_bytes, schema_version,
query, detail`. `input_bytes` replaces `file_bytes`. Query echoes: entities/functions/classes/methods
`within, depth, kind, name`; find `filter, case, resolve_static`; identifiers
`name, filter, case, role, within`; calls `name, filter, case, within`; members
`name, filter, case, access, within`; resources
`kind, resolution, filter, case, within, resolve_bindings`; values `within, kind, name, filter, case`;
xref echoes `at` or `match` plus `kind, case`; symbols `name, filter, case, within`.

Header detail fields:

| Command | Fields |
| --- | --- |
| entities/functions/classes/methods, find | `path, root_id, source_type, analysis, syntax, declaration_file, origin` |
| resources | above plus `resource_profile, binding_profile` |
| values | above plus `value_profile` |
| xref | above plus `resolution, selected, declaration, declaration_ids` |
| symbols | above plus `resolution, within_id` |

`analysis` is `lexical` for xref/symbols, `syntax` for resources/values, and null otherwise.
Resource binding derivation is independently identified by `binding_profile`. `source_type` is the final
script/module mode. `root_id` always points at program (`parent_id: null`,
`detail.depth: 0`). `origin` is the container adapter's
`container, format, entry` or null for a direct file; an entry path is the
archive path, while identity and offsets use the entry's original bytes. Refs
`selected` is `name, role, start, end, entity_id, context_id, type_reference,
value_reference`; `declaration` is `start, end, entity_id` or null. These header
objects are not independent records. Summary is `record, records, complete, detail`
with `detail: {}` except resources' selected resolved/unresolved counts.

Data records always contain `record, at, source_id, parent_at, context_at, start, end,
detail`; all keys are present and inapplicable values are null. `source_id` is null (the
header carries the input identity). Entity and symbol coordinates carry no hash, so the
same coordinate in two inputs is the same position, not the same bytes;
match/reference/unresolved records have `at: null` and use `parent_at` for the containing
structure, which can be passed to `slice`. `symbol.parent_at` is the canonical
declaration's containing structure. Entity `parent_at` keeps the structural parent. Match/reference
`context_at` is the local fragment; entity/symbol/unresolved use null. Start/end are
original half-open UTF-8 byte ranges.

Fixed detail fields:

| Record | Fields |
| --- | --- |
| entity | `kind, name, label, label_origin, depth, flags, key_span` |
| match | `literal_kind, value, dynamic, resolution, raw` |
| identifier | `name, role, syntax_kind` |
| call | `callee, call_kind, optional, argument_count` |
| member | `name, member_kind, key_resolution, read, write` |
| resource | `kind, syntax_kind, name, resolution, unresolved_reason, site_start, site_end, argument_index, spread, optional, type_only, attribute, operator, base, bindings` |
| value | `kind, name, syntax_kind, owner_start, owner_end, index, computed, spread, key_start, key_end, key_resolution, value` |
| reference | `read, write, member_write, type_reference, value_reference` |
| symbol | `name, relation, space, declaration_ids` |
| unresolved_reference | `name, type_reference, value_reference` |
| locate | `complete, entity, ancestors, syntax, declaration_file, origin` |
| source (`slice --json`) | `complete, representation, source, entity, symbol, declaration, fragment, syntax, declaration_file, origin` |
| binding (embedded in slice) | `name, space, declaration_ids` |
| fragment (embedded in slice) | `kind, entity_id` |

`flags` always contains `async, generator, static`. `key_span` is `start, end` or null.
`space`, `declaration_ids` and type/value booleans are null when inapplicable. Match
`resolution`/`raw` are non-null only for `literal_kind: static_expression`. Identifier/
call/member have no own ID; identifier/call/member carry `context_at` when a local
fragment exists, otherwise null. Names/labels/values preserve JS UTF-16 code units; unpaired
surrogates remain valid JSON escapes and are never lossy-converted.

`xref` takes one of three entries: `--at COORD` (a `symbols`/`identifiers` row's
coordinate), `--byte OFFSET` (a position, possibly inside a multi-byte identifier), or
`-f PATTERN --kind symbol` (every binding whose name matches, one `reference` row per
target and site, each row naming its own `detail.target`).

`locate` returns one `record=locate`; `slice --json` returns one `record=source`; neither
has header/summary and both add `backend=js, command, schema_version=2` outside.
`locate` outer id/parent_id/start/end come from the innermost entity and `context_at`
is the local fragment; `detail.entity`/`ancestors` are complete entity bodies ordered
inner to outer. `slice` outer id is the requested target, start/end the extracted range,
context_id null; structure targets use the structural parent, symbol targets the
declaration's containing structure, fragment targets the outer fragment.
`detail.representation: original` and `detail.source` are byte-exact. Only the
requested entity/symbol/fragment is non-null. The symbol `binding` record uses the
canonical declaration identifier range; `declaration` is typed (`kind, start, end,
variable_kind, initializer`) and not enveloped; initializer is `start, end` or null.
The fragment parent is the outer fragment, while `fragment.detail.entity_id` is the
containing structure (a different relationship). Default `slice` raw bytes stay outside
the envelope and are never modified.

Entities sort by start ascending, end descending, kind; matches by original start;
references by `(start, end)`. Identical input/arguments produce identical output;
timing diagnostics never enter payload.

## Implementation

```text
fixed source buffer
  → UTF-8 and input-mode checks
  → Oxc parse
  → structural traversal and query collection
  → filter, sort and budget checks
  → JSONL or source output
```

Oxc Parser provides a Rust JS/TS parser with spanned AST. Using Oxc does not inherit
its published speed claims; rxd also reads, hashes, traverses and serializes and must
measure itself.

The AST lives in an arena for the request lifetime. One traversal maintains a
containing-structure stack and builds the entity table while collecting string
positions as needed. `slice` still parses and validates the real structure, canonical
binding or fragment; it does not slice by ID alone. After validation it extracts from
the fixed buffer without a source printer or print-only AST references.

`--source-type auto|script|module`: auto selects module for `.mjs/.mts`, script for
`.cjs/.cts`, and Oxc unambiguous mode otherwise, returning the final mode. Script
top-level `return` and host extensions are rejected unless a later explicit option is
added.

## Lexical references

`xref FILE --byte N` uses Oxc Semantic binding/reference analysis on demand. It is
single-file only, with no `--id`, `--within` or recursion. `symbols` implements symbol
discovery and `symbol:<start>:<end>` coordinates; `xref --id` and cross-file bindings are not
implemented.

`N` must be less than the original length and inside a declaration identifier or
identifier-reference span; it may be inside a multibyte character or escaped
identifier. Whitespace, strings, comments, keywords, static property keys and `foo` in
`obj.foo` return `NOT_FOUND`; object/destructuring shorthand that is both binding
and reference is selectable. `key` in `obj[key]` is a normal lexical reference.

Refs header detail adds `analysis: lexical`, `resolution`, `selected`, `declaration`:

- `selected` has decoded `name`, `role: declaration|reference`, original `start/end`,
  `entity_id`, `context_at`.
- `resolution: resolved` means a canonical declaration exists in this file;
  `declaration` has its identifier `start/end/entity_id`. Repeated `var` declarations
  still point at the same binding; the canonical position is Oxc's first declaration,
  not every redeclaration. The declaration and initializer are not reference records.
- `resolution: unresolved` means no resolvable lexical declaration; `declaration` is
  null and there are zero references. Unknown globals are not merged by text, and host
  or imported module definitions are not searched; an import alias resolves only to the
  local import binding.
- `resolution: dynamic` means the file contains `with` or a direct `eval(...)` call;
  conservatively the whole file returns dynamic, `declaration: null`, and no
  references. This includes unreachable or shadowed eval calls; it is not a runtime
  impact judgment and eval arguments are never executed/parsed.

Each `record: reference` has `start/end/parent_id/context_id` and detail booleans
`read`, `write`, `member_write`. Simple assignment is write; compound assignment and
increment are read+write; member assignment/update/delete makes the object basis a
read+member_write, not a variable reassignment. References include closure captures but
exclude same-name different-binding positions. No call relation, alias propagation or
dynamic property inference is claimed.

All positions are half-open original identifier ranges, not whole declarations/
statements. `parent_at` (still called `entity_id` inside selected/declaration) is the
innermost extracted structure, not a lexical scope ID; block scopes may share one
structure. Summary records count reference records only; `complete: true` means the
lexical query completed, not runtime reference completeness. Unused resolved
declarations, unresolved and dynamic queries can all succeed with zero records.

`xref`/`symbols` and slice symbol paths enable the semantic pass as needed and return its
diagnostics as `INVALID_INPUT`; structure listings/find/locate and slice structure paths stay
parser-only. Structural traversal checks entity and AST-depth budgets first; semantic
nodes above the selected `semantic_nodes` envelope (input-scaled for CLI, fixed
for explicit service/default library callers) return `RESOURCE_LIMIT` before
index allocation. Reference records are budgeted before collecting positions; output
bytes include header/summary. The node budget is not a parser/peak-RSS limit. Perf
adds a semantic phase and semantic_nodes/symbols/references counts only on those paths.

## Symbol discovery

`symbols FILE [--within ENTITY_ID]` emits single-file JSONL. Default selection is
program; `--within` validates a real structure ID/content identity and selects its
complete original range, including its own declaration identifiers, parameters and all
nested structures. This is a source-region filter, not a "visible symbols in this
lexical scope" query; external bindings not used in the region are not returned, and
class/method keys and parameter defaults are analyzed in their original range. It is
single-file only, with no recursion or depth option. Optional `--name`/`-f` match
the symbol/unresolved-reference's own name, preserving binding identity and same-name
separate scopes.

This command builds the same Oxc Semantic index on demand. Header detail adds
`analysis: lexical`, `within_id` (program ID by default) and `resolution:
static|dynamic`. A whole file containing `with` or direct `eval(...)` returns dynamic
and zero records even if the eval is outside the selected region; the rule matches
xref. Static means static lexical analysis, but unresolved names may remain.

Two record kinds:

- `record: symbol`: one per resolved binding; `id/start/end/parent_id` outside,
  detail `name/relation/space/declaration_ids`. `name` is decoded; start/end are the
  canonical declaration identifier's original half-open UTF-8 range; `parent_at` is
  its innermost containing structure. Repeated `var` declarations merge into one
  binding with the xref canonical position. `relation: declared` means at least one
  declaration identifier is in the region; otherwise `external` means a region
  reference to a binding declared outside. External positions still point at the
  original declaration. Same-name different bindings stay separate; imports point only
  at the local import binding.
- `record: unresolved_reference`: one per unresolved identifier reference, with
  `start/end/parent_id` and detail `name/type_reference/value_reference`; no symbol ID,
  no text merging, no host/dynamic guessing. Static property keys, strings, labels,
  `this` and implicit bindings have no declaration record; shorthand/computed real
  bindings/references are handled normally.

Symbol coordinates are `symbol:<canonical-start>:<canonical-end>`, not Oxc internal
ordinals. They are stable for the same content, parse options and schema
across calls, paths and within selections, but not across incompatible versions or
different source content. Structure and symbol IDs have different target semantics:
structure/symbols `--within` reject symbol IDs; xref does not accept an id selector; slice accepts
symbol IDs and extracts the canonical declaration; slice accepts parent_id for the
containing structure. A symbol record's `at` can be passed directly to `xref --at`, and its
`start` to `xref --byte`.

Both record kinds mix in ascending `(start, end)`. Summary records count symbols plus
unresolved references; complete means the query completed, not resolution/runtime
completeness. Record budgets are checked before collecting identities/positions;
output bytes include header/summary. Semantic node admission follows ADR 0016;
structural depth, input and atomic-output contracts remain. Perf adds the semantic phase/counts only for xref/symbols/slice-symbol.

## Declaration extraction by binding

`slice FILE --at SYMBOL_AT [--json]` accepts a canonical symbol ID. On the same fixed
buffer, after entity/depth checks, it builds a node-budgeted semantic index and
validates symbol kind, exact canonical identifier range and real binding. Reference,
later redeclaration, non-canonical numeric encoding and arbitrary valid slices return
`INVALID_QUERY`. Structure coordinates keep the existing parser-only path.

The original extraction range is chosen from the canonical declaration identifier's
AST ancestors:

| Source | `declaration.kind` | Range |
| --- | --- | --- |
| variable declaration | `variable_declarator` | One declarator: full binding pattern and initializer; excludes `const/let/var/using` prefix, sibling declarators and the statement semicolon. |
| ordinary parameter | `parameter` | One complete parameter, including destructuring and top-level default. |
| rest parameter | `rest_parameter` | Complete rest parameter, including ellipsis and binding pattern. |
| catch parameter | `catch_parameter` | Complete catch binding pattern, excluding the catch body. |
| import binding | `import_declaration` | Whole import declaration, retaining local alias and module specifier; the imported module is not read. |
| function/class name | `function_declaration/function_expression/class_declaration/class_expression` | Complete function/class node; does not expand to an outer assignment or export wrapper. |

Default stdout and `-o` are the raw UTF-8 bytes of that range, without prefix,
parentheses or newline and without rewriting into standalone code. Destructuring
bindings may extract the same complete pattern/initializer rather than one inferred
value. Repeated declarations extract only Oxc's canonical first declaration; no later
declaration, assignment or call is appended. `for-in/of` variables return the
declarator, not the iteration input. Output expresses source facts; it never executes,
evaluates or propagates aliases.

Symbol-path `--json` returns `record=source` with `complete/representation: original`
and `symbol`, `declaration`, `source`, not a structural entity:

- symbol: `id/start/end/parent_id` outside, detail `name/space/declaration_ids`;
  start/end are the canonical binding identifier range; parent_id can expand the
  containing structure.
- declaration: `kind/start/end/variable_kind/initializer`; start/end are the actual
  extraction range, separate from symbol identity. `variable_kind` is
  `var/let/const/using/await using` only for variable declarations, otherwise null.
  `initializer` is the full variable initializer or top-level parameter default
  `start/end`, null if absent; nested destructuring defaults stay only in source.
- source is byte-identical to the declaration range. Complete means this declaration
  was extracted, not runtime values, all redeclarations or data-flow completeness. A
  valid real binding can be extracted from a file containing eval/with as syntax only;
  dynamic binding is not recovered. The dynamic symbols query contract is unchanged.

Common input, depth, output and atomic-failure contracts apply. Only symbol IDs
build the semantic index, using the selected input-scaled/fixed node envelope;
structure IDs do not. Perf records is 1; symbol paths include the
semantic phase and whole-file index counts.

## Local syntax fragments

Each find match, locate object and xref selected/reference adds
`context_id: string|null` for local source around the hit. Structural parent IDs/
ranges/order keep their meaning. Refs `declaration` still means the canonical binding
identifier; `context_at` is the selected/reference location and never replaces binding
identity. Symbols output is unchanged.

`context_at` is `fragment.<kind>:<start>:<end>`, a real AST fragment identity, not an
arbitrary byte slice. The nearest
supported syntactic node is selected:

- variable declaration/declarator; assignment/update expression; ordinary data object
  property/class field.
- parameter/rest/catch parameter; import/export named/default/all declaration.
- expression/return/throw statement; if/switch/switch case/for/for-in/for-of/while/
  do-while/try/with/labeled/break/continue/debugger/empty statement.

Program, functions, arrow functions, classes, class/object methods, getters/setters
are context boundaries; entering one reselects an inner fragment and never reuses an
outer variable initializer or call expression. Ordinary blocks produce no fragment.
Bare directives, function/class names and arrow shorthand without an inner node return
`context_id: null`; the Agent can use parent_id/entity id for the full structure.
Control-statement conditions/discriminants may associate the whole statement. Fragments
are not promised small or complete runtime context.

`slice FILE --at CONTEXT_AT [--json]` re-parses the same fixed input, checks entity/AST
depth budgets and validates kind and real AST range. Missing nodes, wrong kinds, forged
ranges and non-canonical numeric encoding return `INVALID_QUERY`. It uses parser
traversal only, no semantic index; the semantic-node ceiling does not apply.
Outline/symbols `--within` still accept only structure IDs. Symbol IDs remain separate.

Default slice emits raw fragment bytes without prefix/parentheses/newline. A variable
declarator may omit `const/let/var`; an assignment expression may omit the statement
semicolon; a template hit is associated with the complete enclosing fragment.
Destructuring and dynamic property expressions retain raw source; no value or call
target is inferred.

Fragment-path `--json` returns `record=source` with `complete/representation:
original`, `fragment` and `source`. Fragment `id/start/end/parent_id` outside, detail
`kind/entity_id`: start/end are the extracted range; entity_id is the containing
structure; parent_id is the nearest outer supported fragment within the same context
boundary, or null, and can be expanded by slice. For example object property → single
declarator → whole variable declaration; return/throw can expand to an outer if/loop.
Source is byte-exact; complete means extraction succeeded, not business meaning or
runtime completeness.

Find/locate/get fragment paths keep only the current AST-depth context stack and one
requested fragment/match ID; no whole-file fragment table is created. Refs uses the
built semantic node ancestors and the same selection rule; unresolved/dynamic selected
results may still provide a syntactic fragment but do not claim a resolved binding.
Fragments do not add structure entity records or change the entity ceiling. Existing records,
output budgets, directory atomic failure and `-o` contracts remain; serializing
`context_at` is charged to existing output budgets and release benchmarks.

## Performance and validation

Performance work covers process startup, input read/hash, parse/traverse, and
serialization/source output. The native JS path avoids generic compilation,
transformation and project generation. Structural results retain only spans,
relationships and small labels; source is read on demand.

Single-file and recursive scans stay serial in output; file-level parallelism is a
later measured change that must preserve deterministic output. Re-parsing per call is
included in benchmarks; AST-resident query speed is never presented as CLI speed.

Release benchmarks cover about 1 MiB, 10 MiB and 50 MiB authored or authorized real
bundles, recording bytes, structures, build, hardware, tool versions and hashes.
Comparisons include:

| Scenario | Baseline | Metrics |
| --- | --- | --- |
| String entry lookup | `rg -boF` vs `rxd js find` | End-to-end latency and output differences |
| Function structure | Equivalent Oxc/Babel traversal script | Latency, peak RSS, structural accuracy |
| Post-hit extraction | Text search/manual boundaries vs `find → get` | Complete function extraction accuracy and total time |
| Multi-turn Agent analysis | `rg` + file reads, formatted search, `rxd` | Answer/evidence correctness, tool turns, tokens and total time |

Fresh-process and repeated calls are reported separately with cache conditions; OS
file-cache gains are not parser-cache gains. Each sample reports median, tail latency,
peak RSS and output bytes; redirection and real serialization stay in the timing.

Performance is an acceptance requirement: ordinary representative release queries must
complete within 150 ms in most cases. Per-workload samples, median, P95 and >150 ms share
are reported; unrelated workloads are never pooled or padded with tiny fixtures. The
measurement is fresh-process external wall time including startup, worker init,
directory discovery, read/hash, parse, traversal, serialization and output writes; an
AST-resident timing or one `--perf` phase cannot substitute. Hardware, input size,
fresh-process and cache conditions are explicit. Large inputs/directories need
separate scope, measurements and approval; results are never truncated or ID/resource
checks weakened to meet the gate.

### Property-navigation measurement (2026-10-20)

The captured public Feishu sign-in loader (1,278,289 bytes, SHA-256
`635f2f259e0c1ac2facc0a329a8cab4ccaae08d297fa1b86ea1ac78077258357`) and vendor
bundle (2,632,211 bytes, SHA-256
`93fa1a5a1a9a22c39e9e4aacef5e5031142139ca77c81062f336d177636b59f3`) were measured
with release `backend-js,backend-html,backend-jvm`, before at `23d091e` and after the
access/context change. Apple M3 Pro, 36 GiB, 12 logical CPUs, macOS 26.5.1 arm64;
warm OS file cache, two warm-ups followed by 30 samples per workload, interleaved
before/after, no concurrent build and no CPU isolation. Each sample starts fresh CLI/
worker processes and includes parsing, serialization and atomic `-o` writes.

| Query | Before median / P95 ms | After median / P95 ms | After rows |
| --- | --- | --- | --- |
| loader `members --name p` | 29.81 / 32.61 | 31.06 / 33.41 | 38 |
| loader `members --name p --access write` | not available | 31.12 / 32.93 | 2 |
| vendor `members --name src` | 59.25 / 62.56 | 63.55 / 65.68 | 47 |
| vendor `members --name src --access write` | not available | 62.80 / 65.74 | 13 |
| loader `calls --name querySelector` | 27.84 / 30.24 | 29.64 / 32.93 | 13 |
| vendor `calls --name fetch` | 54.80 / 59.67 | 59.34 / 63.95 | 19 |

Every measured workload/version had 0/30 samples above 150 ms. These are scoped
observations, not a general speedup: tracking contexts adds roughly 1–5 ms here and
unfiltered streams grow. The useful gain is selection/navigation: the loader's `.p`
search goes from 38 candidates / 8,664 output bytes to 2 writes / 1,078 bytes, with
local assignment coordinates ready for `slice`. No target JavaScript was executed;
this is not a browser or resource-coverage measurement. Raw samples, binary identities
and the runner are under ignored `target/bench/js-navigation/`; website bundles are not
repository test fixtures.

If repeated parsing dominates, compare batched queries, an explicit disk index and a
resident session before choosing; startup cost and applicability remain part of the
comparison, and resident results never replace standalone CLI acceptance. Any cache
must validate source content, schema, parser version and parse options. IDs are not
reused across incompatible versions. The first version has no disk cache.

## Errors and resources

Input is untrusted. Ordinary CLI work, retained name/value metadata, semantic nodes
and bounded serialization grow with admitted input under
[ADR 0016](../adr/0016-input-scaled-cli-resources.md), instead of fixed total quotas.
Existing AST and closed-fold recursion/steps/value-expansion guards remain; MCP and
explicit default library limits retain their fixed policy. These envelopes are not
peak-memory guarantees, and none is replaced with an unlimited sentinel.

Syntax errors fail atomically and emit no recovered partial structures. There is no
permissive mode. Parser recursion depth, stack use and cancellation need separate
verification; hostile-depth input is not promised until adequate protection or
isolation exists. `slice` is subject to the same parse depth and output checks.

| Code | Condition |
| --- | --- |
| `INVALID_QUERY` | Malformed coordinate, byte position out of range, contradictory flags |
| `NOT_FOUND` | A well-formed coordinate that names no structure, fragment or binding |
| `INVALID_INPUT` | Invalid UTF-8, unsupported input mode, parser error or semantic diagnostic |
| `RESOURCE_LIMIT` | Input/index/output budget exceeded |
| `IO_ERROR` | Read or write failure |

Codes are ADR 0015's closed set. Exit 0 means complete success including zero results;
2 means the query was wrong and can be corrected; 1 means the input, a budget, the
environment or an invariant failed. Stderr diagnostics include a stable code and
understandable reason. Ctrl-C returns non-zero and stops output; cancellation is not
success.

JSONL determines matches and checks the total output budget before writing; resource
limits never silently truncate. An I/O error during writing may leave partial bytes;
consumers detect failure by non-zero exit or missing summary. `-o FILE` writes a
same-directory temporary file then replaces the target, preserving the old content on
failure. Raw source stdout has no summary; exit status determines completeness.

Tests use handwritten small JS fixtures covering anonymous/named/arrow/nested
functions, closures, object/class methods, constructors, getters/setters, async and
generators, and confirm methods do not duplicate function records. Boundaries cover
Unicode identifiers, multibyte text, unpaired surrogate escapes, CRLF, BOM, template
interpolation, string escapes, dynamic keys, empty files, invalid offsets and syntax
errors. Every original span must extract the exact code; `slice --json` source must be
byte-exact.

Identity tests cover same-name functions, identical content at different paths, changed
content at the same path, forged ranges, wrong kinds and incompatible ID versions.
Filter tests cover `--within`, depth, references when a parent is filtered, and
deterministic ordering. Output tests cover stdout/stderr separation, JSONL header/
summary, empty-result exit, budget failure, write failure, raw bytes and `-o`
consistency. Resource tests include deep nesting and many entities.

Agent acceptance tasks locate an API request function, explain error-trigger
conditions, trace an outer closure variable, and distinguish same-name bindings in
different scopes, checking answers and evidence independently. Shorter output with
lower task correctness is not acceptance.

## MCP projection

`js_entities/functions/classes/methods/find/identifiers/calls/members/locate/xref/symbols/get` reuse existing
implementations. JSONL becomes header/records/summary; recursive directories keep
`files` and `scan_summary` (only for directory-capable listings and `find`); get uses the
original source record's `detail.source` with source/CRLF unchanged. Isolated UTF-16
surrogate decoded values are represented losslessly as `{utf16_units: [...]}` in
structuredContent, while ordinary strings stay strings; CLI JSON escaping is unchanged.
A directory target must declare `backend: js`, pin the file set and obey file/discovery/
depth budgets. `identifiers`/`calls`/`members`, like `locate/get/xref/symbols`, are
single-file only.

Shared snapshot, quota, cancellation and complete-response rules are in the
[MCP design](mcp-design.md); the decision is [ADR 0007](../adr/0007-typed-mcp-service.md).
CLI output and exit codes are unchanged by the MCP projection.
