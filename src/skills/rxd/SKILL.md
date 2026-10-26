---
name: rxd
description: Analyze JavaScript, ASAR, ZIP-family archives, HTML, APK/DEX, JVM class/JAR, .NET IL, EVM bytecode, WebAssembly and CPython bytecode with the rxd CLI. Select a backend, locate structures or literals, trace explicit references and fetch selected original code or reconstruction; use optional Kuna for native binaries and optional MCP for typed tools.
---

# RxD

RxD is an Agent-oriented multi-format static analysis CLI. It supplies precise,
bounded queries; reasoning belongs to the Agent. Choose the input format
explicitly, then locate → associate
→ fetch only the relevant unit. Start with `rxd --help` and the selected command's
`--help`; feature availability and exact options belong to the installed binary.

## Select the backend

| Input | Entry | Locate / associate / fetch |
| --- | --- | --- |
| JS/JSX/TS/TSX | `rxd js` | `entities`/`functions`/`classes`/`methods` (with `-f`/`--kind`/`--name`), `strings`/`find`, `identifiers`, `calls`, `members`, `locate`, `symbols`, `modules`/`imports`/`exports` / lexical `xref --at` (or `--byte`) / byte-exact `slice --at` |
| Electron ASAR | `rxd asar` | `entries` / JS, HTML and WASM queries accept `rxd BACKEND COMMAND app.asar --entry ENTRY_PATH`; other backends use `extract --at` first |
| ZIP/AAR/VSIX/XPI/wheel/NuGet | `rxd zip` | `entries` / `extract --at`; JS/HTML/WASM also accept `--entry` with a printed `zip:N` coordinate or unique exact path |
| Local HTML document or fragment | `rxd html` | `entries` (with `-f`/`--kind`/`--tag`/`--attr`/`--within`/`--depth`) / `extract --at` a selected node, `--content` for inline content |
| Android APK or DEX | `rxd dex` (alias `apk`) | `classes`, `methods`, `fields`, `entries`, `strings`, `manifest` / `xref --at` or `-m PATTERN --kind string\|type\|method\|field` / `get-class`/`get-method --at` |
| JVM class or ordinary JAR | `rxd jar` | `classes`, `methods`, `fields`, `entries`, `strings` / `xref --at` or `-m … --kind` / `get-method`/`get-class --at`, `--raw` for original instructions |
| Managed .NET assembly | `rxd dotnet` | `classes`, `fields`, `methods`, `strings` / `xref --at` (token) or `-m … --kind string` / `get-method`/`get-class --at`, `--raw` for original IL |
| Local EVM flat bytecode | `rxd evm` | `blocks`, `interface`, `strings`, `storage` / `xref --at` or `-m … --kind KIND` (function, block or literal) / `get-function`/`get-block --at` |
| Binary WASM core module | `rxd wasm` | `types`, `imports`, `exports`, `functions`, `tables`, `memories`, `globals`, `data`, `elements`, `strings` / `xref --at func:N` or `-m … --kind function` / `get-function --at` |
| CPython .pyc or versioned raw marshal | `rxd pyc` | `codes`, `strings` / `xref --at` (a `strings` row), `xref --kind name --at NAME/-m TEXT`, `get-code --at`, whole-module `decompile` |
| Native binary (optional engine) | `rxd kuna` (alias `rxd bin`) | native `functions` / engine-specific evidence / `decompile` |

Ordinary JVM class/JAR input goes directly to `rxd jar`, without Java, D8 or
conversion. A ZIP containing root DEX entries belongs to `rxd dex`. ASAR and ZIP
are containers: with the container and requested backend enabled, use
`rxd BACKEND COMMAND ARCHIVE --entry ENTRY` with backend `js`, `html` or `wasm`
to analyze one entry in memory. ASAR uses an exact path; ZIP accepts `zip entries.at`
(`zip:N`, required for duplicate paths) or a unique exact path. `--at` and byte
offsets select inside the entry; JS auto syntax uses the selected filename, not
its coordinate. Other backends use explicit extraction. AAR code normally lives
in `classes.jar` or `libs/*.jar`: extract the JAR, then use `rxd jar`. ZIP supports
Stored/Deflate and ZIP64, not encrypted entries, links or automatic nested recursion.
HTML resources are not automatically followed into other entries. Containers have no code-reference semantics. HTML is a lexical container
(`lexical_naive_v1`, not a DOM or browser parse): `entries` enumerates
`element`/`text`/`comment`/`doctype` records with copy-only coordinates, `extract`
returns the exact original interval, and inline JS/TS is then handed to the code
backend with artifact-side offsets. Unprefixed ZIP-based zipapps can be enumerated
with `rxd zip`; PyInstaller PYZ/CArchive and executable-prefixed ZIP handling remain planned.

## Consume results and preserve identity

- stdout contains payload; diagnostics and the `--perf` record
  use stderr. Do not mix them. JSONL lists/searches/references are data rows
  only: no header and no summary. Require exit 0 and parse the rows; zero rows is
  a complete empty result. JS directory scans open each file with a `file` data
  record and are scanned in path order, using `--threads` workers by default.
  Backend-specific completeness, inference and reconstruction flags still apply.
- Schema v2 navigation (`at`, `parent_at`, `context_at`, `source_id`, `start`,
  `end`) is outside `detail`; format-specific fields are inside. `at` is the
  copy-only coordinate a row prints and the value `--at` accepts; a stream row
  does not repeat a content hash. Not every row has a usable coordinate or range.
  Parse actual responses, never manufacture coordinates.
- A coordinate names a range in the input you are querying, not the file itself: reuse
  it only with the same bytes and interpretation options. A coordinate copied from
  another input is not checked; re-run the listing for the file you are fetching.
  CLI inputs must remain fixed for the entire call. JS offsets are UTF-8 byte offsets, not characters or lines. Other offsets
  retain their backend meaning: JVM instruction offsets, .NET metadata tokens,
  DEX member indices/code units, WASM code-body offsets, PYC code-relative bytes,
  EVM decoded PCs versus original raw/hex carrier ranges. Do not interchange them.
- Narrow before FETCH with selectors (`-f`, `--kind`, `--class`, `--within`,
  `--depth`). Ordinary CLI metadata/index bounds grow with admitted input. Work,
  instruction counts, expanded-entry bytes and output have no input-derived quotas. Format, recursion and
  expansion guards remain. MCP shares that analysis policy; its adjustable
  service-wide storage/concurrency/deadline protections are listed by
  `rxd mcp --help`, not per-query `limits`. Backend CLI queries have no
  `--max-*` flags. Resource failure is not evidence of absence, and
  partial output is never discarded silently.
  Zero hits in a complete query are valid within that query's stated scope.
- Default JS `slice` returns original bytes. Java/C#/C-like/Python reconstruction is
  an analysis representation, not original source or a universal semantic proof.
  Inspect reconstruction/fallback flags and original bytecode where available.
  Static references do not imply a complete runtime call graph.

Examples below use local input placeholders and `at`/`context_at` coordinates or
offsets obtained from the preceding response. Keep each input fixed and use its actual returned values.

## JavaScript: choose a locator, fetch only missing evidence

Read a short, readily understandable file directly when that already answers the
question. Use RxD for missing structure, exact ranges, bindings or selected archive
entries; do not require a listing/FETCH ceremony for every file. Consult the
selected command's help only when its options are unknown, not at every step.

Start from evidence already available, not an unfiltered function inventory:

| Existing clue | First query |
| --- | --- |
| Function or assigned helper label | `functions bundle.js --name NAME` |
| Named call or constructor | `calls bundle.js --name NAME` |
| Property/key write | `members bundle.js --name KEY --access write` |
| Literal text such as a path | `find bundle.js -f TEXT` |
| Binding or constant name | `symbols bundle.js --name NAME` |
| Existing UTF-8 byte position | `locate bundle.js --byte BYTE` |

Prefix each query above with `rxd js`. Use `-f` for a substring when the exact
name is unknown. If names repeat, distinguish their parent/scope rather than
silently taking the first row. Full inventories remain appropriate when the task
actually asks for all structures; filtering must not stand in for completeness.

```sh
rxd js calls bundle.js --name decrypt
rxd js slice bundle.js --at "$CONTEXT_AT"
rxd js calls bundle.js --within "$ENTITY_AT"
rxd js symbols bundle.js --within "$ENTITY_AT"
rxd js functions bundle.js --name "$HELPER_NAME"
rxd js slice bundle.js --at "$HELPER_AT"
rxd js values bundle.js --name key --kind initializer
rxd js xref bundle.js --at "$SYMBOL_AT"
rxd js slice bundle.js --at "$SYMBOL_AT"
```

Copy a hit's `context_at` for local syntax or `parent_at` for its owner;
`ENTITY_AT` is a printed structure coordinate. Default `slice` already returns
byte-exact code: omit `--json` when only reading source, and request it when range
or representation metadata is needed. Preserve that evidence instead of fetching
it again or rereading the whole file as confirmation.

After FETCH, follow only missing dependencies: scope `calls`/`symbols`/`values`
with `--within`, then locate a named helper/constant in its defining scope. A
callee's definition can be outside its caller's containment range. Use named
`values` slots for small constants; use symbol rows to establish a binding exists
without fetching a huge literal initializer. Unresolved references do not supply
a declaration coordinate. For assigned/property helpers, `functions --name`
also matches context labels; member keys are not lexical bindings.

A filtered locator and dependent FETCH can run in one shell/Python tool call:
parse JSONL, select the intended row, then pass its printed coordinate as a
separate argument. Keep ambiguity and errors explicit. This avoids a model
round trip and exposing intermediate inventories without changing CLI results.

`strings` enumerates the same match records as `find` without requiring a needle;
optional `-f` filters values. `--resolution literal|static|decoder` filters
provenance without enabling recovery: use `--resolve-static` for computed rows.
`literal` selects the existing null-resolution literals/template parts without
changing their output fields. `--within` selects a real structure, fragment or
binding declaration in one file; zero rows differs from a missing coordinate.
Filters narrow returned occurrences, not whole-input parsing or safety proofs.
It preserves occurrences, including duplicate/empty literals and dynamic template parts, rather than deduplicating a string pool or
claiming a complete decrypted table. `--resolve-static` adds proved expressions.
A row's `detail.value` is the JavaScript string value: exactly one escape layer is
decoded, so a nested `eval`/`Function` payload still contains its own escapes.
A filter for the payload's decoded text then returns zero rows; that is a scope
limit, not evidence of absence. Unescape the value mechanically, keep the derived
bytes as a separate file, and analyze that file with its own coordinates.

A find/strings match has no coordinate of its own: use its `context_at` for a local syntax
fragment, falling back to `parent_at` for the enclosing entity. `ENTITY_AT` is a
row from `entities` or a match's `parent_at`; a fragment coordinate is not valid
for `symbols --within`. A symbol row's `at` anchors `xref`; its `start` byte is the
`--byte` spelling when the anchor is a raw position. `xref` resolves lexical bindings
within a file; unresolved names and dynamic `with`/`eval` produce no reference rows.
Directory searches (`calls`, `members`, `identifiers`, `symbols`, `xref --match`,
`modules`, `imports`, `exports`, and the structure/literal listings) aggregate
independent file-local results. Route each hit through the enclosing `file`
record's `detail.path`; same names in different files are not a binding relation.
`--within` requires a single file.

```sh
rxd js modules bundle.js
rxd js imports bundle.js --within "$MODULE_AT"
rxd js exports entry.mjs --name login
rxd js calls src/ --name fetch
rxd js xref src/ -m login --kind symbol
rxd js xref src/ --file "$RELATIVE_FILE" --at "$SYMBOL_AT"
```

`modules` reports the file plus retained webpack factory/registration boundaries;
`module_id` (null for the file root), `registry_at`, `chunk_ids`, counts and direct initializer `entry`
evidence describe the artifact, not a recovered original file tree. Its `at` is
sliceable original syntax. `entry` requires a nonescaping registry and a direct
first dispatch after closed data initialization; writes, opaque effects, cached
or indirect dispatch leave it absent without discarding retained boundaries.
`imports` exposes source/expression, imported/local
aliases and `type_only`; `--name`/`-f` match the imported name (source for side-effect
imports). `exports` filters exported names and exposes definitions or re-export
sites. Follow an available `at` to a local binding and `context_at` to the site.

Directory `xref --file` anchors a symbol/identifier coordinate or `--byte` in the
relative discovered file and joins only proven ESM bindings through exact `./`
and `../` file paths. Aliases and named/star re-exports may link; ambiguity,
missing targets and unsupported relationships stay unlinked. Default function/
class declarations and live export specifiers can link; a default expression
snapshot is not an alias of the identifier it reads. No package/extension guessing,
namespace-property inference or URL-like specifier resolution is provided.
`target_path` plus `detail.target` identifies the canonical binding; `link: esm`
marks cross-file references. MCP can capture a JS directory snapshot; open it with
`rxd_open` and `backend: js` to reuse identical files across project queries.

The identifier/call/member queries are syntax-first locators. `identifiers`
returns binding/reference occurrences of a full lexical name (so `obj.foo` never
matches, but `obj[key]` matches `key`); each hit has `parent_at` and `context_at`
for FETCH. `calls` reports call sites of a name with
`call_kind: direct|member|new`, never a return value or call graph. `members`
reports static member/property keys and JSX attribute names, plus computed keys
whose key is a string literal or a constant expression the bounded evaluator can
prove (`member_kind: computed_key` with `key_resolution: literal|static`); a
dynamic `obj[expr]` key never matches. A member row's `receiver` gives the
original object text and byte range (`{start, end, text}`) when the object path
is an identifier, `this`, or a chain of static or string-literal keys; it is null
for call results, variable keys and other dynamic objects. Compare receivers to
distinguish `window.key` from `document.key`; this is the syntactic receiver, not
runtime identity, and key-only rows (object/class/JSX) never carry one. `entities --kind` takes one kind or a
comma-separated list, `--name` filters exactly, and `-f` is a substring over the
name/label; `functions`, `classes` and `methods` are the same
rows restricted to one kind group. `find
--resolve-static` additionally matches bounded AST constant evaluation; those
hits are `literal_kind: static_expression` with `detail.raw` and
`resolution: static`, or `decoder` when composition uses proven decoder values.
Existing bounded concatenation, template, indexing and string operations compose
those values; this does not follow factory returns or interpret arbitrary calls.
`charCodeAt` reads UTF-16 units; `codePointAt` combines valid surrogate pairs.
Out-of-range results preserve NaN/undefined semantics in enclosing expressions.
`join` accepts closed temporary arrays of primitive values/holes, not external
mutable arrays, object elements or nested-array coercion.

`--resolve-static` shares one decoder proof across `strings`, `find`, `calls`, `members`,
`values`, `resources` and structure labels. Supported shapes use a closed string
array factory, a lexical decoder with a bounded integral numeric index and at
most one numeric offset, and optionally one uniquely solved cached-table rotation.
Unwritten initialized `const` aliases are followed only after their activation/order
is proven; captured declarations must not be read or escape before initialization. Plain tables and direct
standard `atob` table decoding are supported; `atob` yields Latin-1 code units,
not UTF-8. RC4 and arbitrary custom decoder bodies are unsupported. Rotation bootstrap is
restricted to pure declarations/closed values and known decoder calls; opaque
bootstrap effects, runtime imports and evaluator/reflection/constructor escapes
leave intrinsic-dependent recovery unresolved. Computed reads outside the proven
decoder/initializer need closed or recovered keys; opaque reads remain unsupported.

Recovered find hits have `literal_kind: decoded`, `resolution: decoder` and the
original call in `raw`; calls/members expose the proven key. Values use
`detail.value.resolution: decoder` for values and `key_resolution: decoder` for
computed keys; resources use `resolution: decoder`. Opt-in entity
labels use `label_origin: decoder` without changing declared `name`, coordinates
or source bytes. Mutated/escaping tables or decoders, unsafe initialization,
string/fractional or opaque indices, exported/ambient table escapes, prototype or
intrinsic mutation, ambiguous rotations and partially
invalid encoded tables stay unresolved. These are syntax/semantic proofs, not
sample fingerprints or runtime execution.

```sh
rxd asar entries app.asar -f main
rxd asar extract app.asar --at main.js -o extracted-main.js
rxd js entities extracted-main.js
```

ASAR extract requires an output file and returns a receipt; it does not extract
the whole application implicitly.

```sh
rxd zip entries extension.vsix -f main
rxd js entities extension.vsix --entry extension/dist/main.js
rxd zip entries sdk.aar -f jar
rxd zip extract sdk.aar --at classes.jar -o target/classes.jar
rxd jar classes target/classes.jar
```

MCP uses `zip_extract` → child target → backend query, not CLI `--entry`.

HTML is a lexical container: `entries` returns `element`/`text`/`comment`/`doctype`
records with copy-only coordinates and decoded attribute values; `extract` copies
the exact original interval and returns a receipt whose `source_id` identifies the
artifact. Attribute values are decoded (not byte-exact), while extracted bytes
stay byte-exact. Inline JS/TS is handed to the JS backend with artifact-side
offsets — never apply container offsets to the extracted file.

```sh
rxd html entries page.html
rxd html entries page.html --tag script --attr type=module
rxd html extract page.html --at "$SCRIPT_AT" --content -o inline.js
rxd js entities inline.js
```

## Android and JVM: string → caller → selected Java

```sh
rxd dex strings app.apk -f Authorization --limit 20
rxd dex xref app.apk -m Authorization --kind string
rxd dex xref app.apk --at "$METHOD_AT"
rxd dex manifest app.apk
rxd dex get-method app.apk --at "$METHOD_AT"
rxd jar strings library.jar -f Authorization
rxd jar xref library.jar -m Authorization --kind string
rxd jar xref library.jar --at "$METHOD_AT"
rxd jar get-method library.jar --at "$METHOD_AT" --raw
rxd jar get-method library.jar --at "$METHOD_AT"
rxd jar get-class library.jar --at "$CLASS_AT"
```

Every `get-class`/`get-method` answers with one JSON record (`class_body` /
`method_body`) whose `detail.code` is the source. JVM method FETCH supports
`--raw` for original instructions; DEX FETCH emits Java and has no `--raw` flag.
DEX records carry no per-row `source_id`: route `xref` rows with their
`context_at` (the caller) and `detail.entry` (which DEX); reuse the unchanged
archive. DEX strings have global table
indices and no owning class. A duplicate class is an ambiguity, not a reason to
choose the first definition. DEX 041 may identify logical DEX members separately.
DEX `-o` also prints the same bytes to stdout; other backends have their own file
output contracts. In JVM `xref` rows put the caller in `context_at` (usable as
`--at`) and the reference site's `byte_offset`/`mnemonic` in `detail`; method
coordinates preserve overload descriptors. JVM
`--release` selects multi-release JAR versions. Java reconstruction may fail or
contain unsupported-method stubs; inspect bytecode when necessary.

## .NET: literal → owning method → IL/C#

```sh
rxd dotnet strings library.dll -f Authorization
rxd dotnet xref library.dll -m Authorization --kind string
rxd dotnet xref library.dll --at "$METHOD_AT"
rxd dotnet methods library.dll
rxd dotnet get-method library.dll --at "$METHOD_AT" --raw
rxd dotnet get-method library.dll --at "$METHOD_AT"
rxd dotnet classes library.dll --class "$TYPE_NAME"
rxd dotnet get-class library.dll --at "$TYPE_AT" --raw
rxd dotnet xref library.dll --at "$TYPE_AT"
```

A `classes` row carries `record: class`; copy its `at` to class FETCH or type
XREF without rewriting it as `type`. Select a real type first: unfiltered listings
can start with the non-fetchable `<Module>` row.

Use the `context_at`/`parent_at` a `strings` or `xref` row prints, and the `at` a
`classes` row prints; `.NET` coordinates are bare metadata tokens (`0x06000025`).
Metadata tokens are scoped to the module and cannot identify a member across
assemblies. A .NET query validates the rows it reads, so unrelated corruption does not
fail it; a row that is itself unreadable fails when queried. C# reconstruction
requires an available ILSpy NativeAOT engine (included in complete distributions;
optional in source builds). Engine absence or failure retains original IL and
explicit fallback information; do not report fallback IL as reconstructed C#.

## WASM: structure → explicit caller → selected function

```sh
rxd wasm functions module.wasm -f hello
rxd wasm strings module.wasm -f hello
rxd wasm xref module.wasm --at "$FUNCTION_AT"
rxd wasm xref module.wasm --kind function -m hello
rxd wasm get-function module.wasm --at "$CALLER_AT"
rxd wasm get-function module.wasm --at "$CALLER_AT" --raw
```

Each listing emits one kind only: `types`, `imports`, `exports`, `functions`,
`tables`, `memories`, `globals`, `data`, `elements`, `strings`. Select the merged
function coordinate from `functions` (or `detail.target_at` from exports/imports),
then use the caller coordinate in `xref`'s
`context_at`. Strings inventory data/name sections; it does not prove
string-to-code ownership. `xref` reports direct calls and `ref.func`; indirect
calls remain unresolved.
Default FETCH attempts structured C-like reconstruction and falls back to a
complete instruction view for unsupported instructions/control flow. `--raw`
forces that view. WAT text, Component Model, linking and WASI execution are outside
this binary core-module analyzer.

## Python: constant → explicit reference → original code

```sh
rxd pyc codes module.pyc -f token
rxd pyc strings module.pyc -f Authorization
rxd pyc xref module.pyc --at "$CONSTANT_AT"
rxd pyc get-code module.pyc --at "$CODE_AT"
rxd pyc xref module.pyc --kind name --at token --within "$CODE_AT"
rxd pyc xref module.pyc --kind name -m token
rxd pyc decompile module.pyc --json
```

Static navigation accepts the supported final CPython 3.8–3.13 formats, not every
Python implementation or prerelease. Raw marshal requires `--version X.Y`. Use a
strings row's `at`, then the owning `context_at` from `xref` for `get-code`.
Constant occurrences remain distinct even when values/containers are shared.
`codes`/`strings` do not imply that instructions have been scanned. `get-code`
preserves instructions, caches, tables and location evidence; names are explicit
bytecode facts, not resolved duck-typed calls. Whole-module source reconstruction
is bounded and has independent completeness: inspect `detail.complete` in JSON;
plain source mode fails when recovery is incomplete.

## EVM: candidate literal → evidenced reference → region

```sh
rxd evm blocks runtime.hex --encoding hex
rxd evm interface runtime.hex --encoding hex
rxd evm strings runtime.hex --encoding hex --kind text --query Authorization
rxd evm xref runtime.hex --encoding hex --at "$LITERAL_AT"
rxd evm xref runtime.hex --encoding hex -m 1ba64846 --kind function
rxd evm get-function runtime.hex --at "$CONTEXT_AT" --encoding hex --raw
rxd evm get-block runtime.hex --at "$BLOCK_AT" --encoding hex
```

Encoding is required (`raw` bytes or ASCII `hex`), never guessed. Search kinds
`text`, `bytes`, `number` have different matching semantics. `blocks` lists
file-scoped basic-block coordinates, `interface` lists function candidates with their
EVMole ABI fields. Follow `xref` rows' `context_at`: a function coordinate goes to
`get-function`, a block coordinate to `get-block`; null contexts are not fetchable. Selectors, function boundaries, storage and text/data
are inferred candidates. PUSH literal and contextual CODECOPY evidence do not
prove runtime strings, source ABI or complete ownership/call graphs. Delivery,
discovery, ownership and reconstruction completeness are independent. Default
get-function attempts bounded pseudo-code reconstruction with explicit raw
fallback; `--raw` requests original selected regions.

## Optional Kuna and MCP

```sh
rxd kuna --help
rxd kuna functions ./native-app --summary --json
rxd kuna decompile ./native-app main --json
rxd mcp
```

Kuna requires the optional linked engine feature with bundled specs; those builds
can also use an explicitly configured external engine. Unavailable builds report
that requirement. Use its
native names/addresses and JSON contracts, not invented RxD coordinates. MCP
requires an `mcp` build and exposes all tools supported by that build; complete
single-file distributions include MCP, Kuna and ILSpy. Discover `tools/list` and
actual capability/engine availability before calling typed tools. Normal calls
use exactly one `input: {path}` or `input: {target_id}`. Local input paths must be
absolute. All paths are allowed by default; repeat `--allow-path` at startup to
restrict access to selected files/directories. `rxd_open` captures immutable bytes; use the returned target
handle for related calls and `rxd_close` afterward. Targets fix input bytes but do
not cache parser state. Backend tools retain original identity, reference and
fallback semantics. Path grants and worker/capture/deadline/full-response budgets apply;
consume `structuredContent` (`{records: [...]}` for streams), not a truncated text preview.

## Scope and maintenance

Standard literal/immediate decoding (escapes, LEB128, float/memory operands,
PUSH values) is parsing. Existing structured reconstruction (DEX/JVM→Java, IL→C#,
WASM→C-like, PYC→Python, selected EVM pseudo-code) remains subject to its declared
support and fallback contracts.

`rxd skill` prints this exact Markdown to stdout and writes no files; placing it
in an Agent skill directory is the caller's job.
