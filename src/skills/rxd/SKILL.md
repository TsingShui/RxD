---
name: rxd
description: Analyze JavaScript, ASAR, HTML, APK/DEX, JVM class/JAR, .NET IL, EVM bytecode, WebAssembly and CPython bytecode with the rxd CLI. Select a backend, locate structures or literals, trace explicit references and fetch selected original code or reconstruction; use optional Kuna for native binaries and optional MCP for typed tools.
---

# RxD

RxD is an Agent-oriented multi-format static analysis CLI. It supplies precise,
bounded queries; reasoning belongs to the Agent. Never execute analyzed input or
instantiate a module. Choose the input format explicitly, then locate → associate
→ fetch only the relevant unit. Start with `rxd --help` and the selected command's
`--help`; feature availability and exact options belong to the installed binary.

## Select the backend

| Input | Entry | Locate / associate / fetch |
| --- | --- | --- |
| JS/JSX/TS/TSX | `rxd js` | `entities`/`functions`/`classes`/`methods` (with `-f`/`--kind`/`--name`), `find`, `identifiers`, `calls`, `members`, `locate`, `symbols` / lexical `xref --at` (or `--byte`) / byte-exact `get --at` |
| Electron ASAR | `rxd asar` | `entries` / extract a selected file with `extract --at`, then use its code backend |
| Local HTML document or fragment | `rxd html` | `entries` (with `-f`/`--kind`/`--tag`/`--attr`/`--within`/`--depth`) / `extract --at` a selected node, `--content` for inline content |
| Android APK or DEX | `rxd dex` (alias `apk`) | `classes`, `methods`, `fields`, `entries`, `strings`, `manifest` / `xref --at` or `-f PATTERN --kind string\|type\|method\|field` / `get-class`/`get-method --at` |
| JVM class or ordinary JAR | `rxd jar` | `classes`, `methods`, `fields`, `entries`, `strings` / `xref --at` or `-f … --kind` / `get-method`/`get-class --at`, `--raw` for original instructions |
| Managed .NET assembly | `rxd dotnet` | `classes`, `fields`, `methods`, `strings` / `xref --at` (token) or `-f … --kind string` / `get-method`/`get-class --at`, `--raw` for original IL |
| Local EVM flat bytecode | `rxd evm` | `blocks`, `interface`, `strings`, `storage` / `xref` / `get-function`/`get-block --at` |
| Binary WASM core module | `rxd wasm` | `types`, `imports`, `exports`, `functions`, `tables`, `memories`, `globals`, `data`, `elements`, `strings` / `xref --at func:N` / `get-function --at` |
| CPython .pyc or versioned raw marshal | `rxd pyc` | `codes`, `strings` / `xref --at` (a `strings` row), `xref --kind name --at NAME/-f TEXT`, `get-code --at`, whole-module `decompile` |
| Native binary (optional engine) | `rxd kuna` (alias `rxd bin`) | native `functions` / engine-specific evidence / `decompile` |

Ordinary JVM class/JAR input goes directly to `rxd jar`, without Java, D8 or
conversion. A ZIP containing root DEX entries belongs to `rxd dex`. ASAR is a
container: extract only the selected entry and analyze that file, rather than
applying code-reference semantics to the container. HTML is a lexical container
(`lexical_naive_v1`, not a DOM or browser parse): `entries` enumerates
`element`/`text`/`comment`/`doctype` records with copy-only coordinates, `extract`
returns the exact original interval, and inline JS/TS is then handed to the code
backend with artifact-side offsets. PyInstaller/zipapp (`pyz`)
and binwalk integration are planned, not available commands.

## Consume results and preserve identity

- stdout contains payload; diagnostics and the `--perf` record
  use stderr. Do not mix them. JSONL lists/searches/references have header →
  records → summary. Require successful exit and `summary.complete: true`; JS
  directory scans also have per-file summaries and a final `scan_summary`
  (files are scanned in path order, using `--threads` workers by default, and
  the stream is identical whatever the worker count).
  Backend-specific completeness, inference and reconstruction flags still apply.
- Schema v2 navigation (`at`, `parent_at`, `context_at`, `source_id`, `start`,
  `end`) is outside `detail`; format-specific fields are inside. `at` is the
  copy-only coordinate a row prints and the value `--at` accepts; `source_id` is
  set only on the stream header. Not every row has a usable coordinate or range.
  Parse actual responses, never manufacture coordinates.
- A coordinate names a range in the input you are querying, not the file itself: reuse
  it only with the same bytes and interpretation options. A coordinate copied from
  another input is not checked; re-run the listing for the file you are fetching.
  CLI inputs must remain fixed for the entire call. JS offsets are UTF-8 byte offsets, not characters or lines. Other offsets
  retain their backend meaning: JVM instruction offsets, .NET metadata tokens,
  DEX member indices/code units, WASM code-body offsets, PYC code-relative bytes,
  EVM decoded PCs versus original raw/hex carrier ranges. Do not interchange them.
- Narrow before FETCH with selectors (`-f`, `--kind`, `--class`, `--within`,
  `--depth`). Ordinary CLI work, metadata and serialization bounds grow with the
  admitted input, rather than using fixed total quotas. Format, recursion and
  expansion guards remain; MCP retains independent fixed service quotas. There is
  no `--max-*` flag to tune. Resource failure is not evidence of absence, and
  partial output is never discarded silently.
  Zero hits in a complete query are valid within that query's stated scope.
- Default JS `slice` returns original bytes. Java/C#/C-like/Python reconstruction is
  an analysis representation, not original source or a universal semantic proof.
  Inspect reconstruction/fallback flags and original bytecode where available.
  Static references do not imply a complete runtime call graph.

Examples below use local input placeholders and `at`/`context_at` coordinates or
offsets obtained from the preceding response. Keep each input fixed and use its actual returned values.

## JavaScript: literal → local source → lexical references

```sh
rxd js entities bundle.js -f login
rxd js find bundle.js -f '/api/login'
rxd js slice bundle.js --at "$CONTEXT_AT" --json
rxd js identifiers bundle.js --name 'AIS3' --role reference
rxd js calls bundle.js --name 'fetch'
rxd js members bundle.js --name 'flag_correct'
rxd js symbols bundle.js --within "$ENTITY_AT"
rxd js xref bundle.js --at "$SYMBOL_AT"
rxd js xref bundle.js --byte "$SYMBOL_BYTE"   # a raw offset, e.g. inside a multi-byte name
rxd js slice bundle.js --at "$SYMBOL_AT"
```

A find match has no coordinate of its own: use its `context_at` for a local syntax
fragment, falling back to `parent_at` for the enclosing entity. `ENTITY_AT` is a
row from `entities` or a match's `parent_at`; a fragment coordinate is not valid
for `symbols --within`. A symbol row's `at` anchors `xref`; its `start` byte is the
`--byte` spelling when the anchor is a raw position. `xref` resolves lexical bindings
within one file,
not properties or cross-file calls; `unresolved`/`dynamic` resolution remains
explicit. For directory find,
route each hit with its header's `detail.path` into single-file commands.

The identifier/call/member queries are syntax-first locators. `identifiers`
returns binding/reference occurrences of a full lexical name (so `obj.foo` never
matches, but `obj[key]` matches `key`); each hit has `parent_at` and `context_at`
for FETCH. `calls` reports call sites of a name with
`call_kind: direct|member|new`, never a return value or call graph. `members`
reports static member/property keys and JSX attribute names, plus computed keys
whose key is a string literal or a constant expression the bounded evaluator can
prove (`member_kind: computed_key` with `key_resolution: literal|static`); a
dynamic `obj[expr]` key never matches. `entities --kind` takes one kind or a
comma-separated list, `--name` filters exactly, and `-f` is a substring by default
substring over the name/label; `functions`, `classes` and `methods` are the same
rows restricted to one kind group. `find
--resolve-static` additionally matches bounded AST constant evaluation; those
hits are `literal_kind: static_expression` with `detail.raw` and
`resolution: static`, and it never executes user-defined functions.

```sh
rxd asar entries app.asar -f main
rxd asar extract app.asar --at main.js -o extracted-main.js
rxd js entities extracted-main.js
```

ASAR extract requires an output file and returns a receipt; it never runs the
entry or extracts the whole application implicitly.

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
rxd dex xref app.apk -f Authorization --kind string
rxd dex xref app.apk --at "$METHOD_AT"
rxd dex manifest app.apk
rxd dex get-method app.apk --at "$METHOD_AT"
rxd jar strings library.jar -f Authorization
rxd jar xref library.jar -f Authorization --kind string
rxd jar xref library.jar --at "$METHOD_AT"
rxd jar get-method library.jar --at "$METHOD_AT" --raw
rxd jar get-method library.jar --at "$METHOD_AT"
rxd jar get-class library.jar --at "$CLASS_AT"
```

Every `get-class`/`get-method` answers with one JSON record (`class_body` /
`method_body`) whose `detail.code` is the source; `--raw` keeps the record and
switches `detail.representation` to the original instructions. DEX records carry no
per-row `source_id`: route `xref` rows with their
`context_at` (the caller) and `detail.entry` (which DEX); reuse the unchanged
archive. DEX strings have global table
indices and no owning class. A duplicate class is an ambiguity, not a reason to
choose the first definition. DEX 041 may identify logical DEX members separately.
field_ids, while method indices are method_ids.
DEX `-o` also prints the same bytes to stdout; other backends have their own file
output contracts. In JVM `xref` rows put the caller in `context_at` (usable as
`--at`) and the reference site's `byte_offset`/`mnemonic` in `detail`; method
coordinates preserve overload descriptors. JVM
`--release` selects multi-release JAR versions. Java reconstruction may fail or
contain unsupported-method stubs; inspect bytecode when necessary.

## .NET: literal → owning method → IL/C#

```sh
rxd dotnet strings library.dll -f Authorization
rxd dotnet xref library.dll -f Authorization --kind string
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
rxd wasm xref module.wasm --kind function -f hello
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
rxd pyc xref module.pyc --kind name -f token
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
rxd evm get-function runtime.hex --at "$CONTEXT_AT" --encoding hex --raw
rxd evm get-block runtime.hex --at "$BLOCK_AT" --encoding hex
```

Encoding is required (`raw` bytes or ASCII `hex`), never guessed. Search kinds
`text`, `bytes`, `number` have different matching semantics. `blocks` lists
content-bound basic blocks, `interface` lists content-bound functions with their
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
rxd mcp serve --root /workspace --backends js
```

Kuna requires the optional linked engine feature with bundled specs; those builds
can also use an explicitly configured external engine. Unavailable builds report
that requirement. Use its
native names/addresses and JSON contracts, not invented RxD coordinates. MCP
requires an `mcp` build; complete single-file distributions include MCP, Kuna and
ILSpy. Discover `tools/list` and actual capability/engine availability before
calling typed tools. Normal calls use exactly one `input: {path}` or
`input: {target_id}`. `rxd_open` captures immutable bytes; use the returned target
handle for related calls and `rxd_close` afterward. Targets fix input bytes but do
not cache parser state. Backend tools retain original identity, reference and
fallback semantics. Roots and worker/capture/deadline/full-response budgets apply;
consume structuredContent and complete summaries, not a truncated text preview.

## Scope and maintenance

Deobfuscation, control-flow deflattening and string decryption are permanently
excluded for every backend. Standard literal/immediate decoding (escapes, LEB128,
float/memory operands, PUSH values) is parsing. Existing structured reconstruction
(DEX/JVM→Java, IL→C#, WASM→C-like, PYC→Python, selected EVM pseudo-code) is not
obfuscation reversal. Never run inputs to fill static-analysis gaps.

Install/update with `rxd skill install -a pi`, `-a codex`, `-a claude-code`,
`-a agents`, multiple agents or `-a all`. Without `-a`, detected Pi/Codex/Claude
user directories are selected; otherwise use the cross-agent directory. Codex
honors nonempty `CODEX_HOME`, else `~/.codex`. Use `--dir DIR` for a custom skills
root or `--print` for exact text; neither combines with explicit agent targets,
and print never installs files. Re-running reports installed/updated/unchanged;
identical content is not rewritten.
