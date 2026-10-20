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
| JS/JSX/TS/TSX | `rxd js` | `outline` (with `--kind`/`--name`), `find`, `identifiers`, `calls`, `members`, `locate`, `symbols` / lexical `refs --byte` / byte-exact `get --id` |
| Electron ASAR | `rxd asar` | `list` / extract a selected file, then use its code backend / `extract --file` |
| Local HTML document or fragment | `rxd html` | `list` (with `--kind`/`--tag`/`--attr`/`--within`/`--depth`) / `extract --id` a selected node, `--content` for inline content |
| Android APK or DEX | `rxd dex` (alias `apk`) | `classes`, `strings`, `entries`, `manifest` / `findrefs` / `get-class` |
| JVM class or ordinary JAR | `rxd jar` | `outline`, `strings` / `findrefs` / `get-method`, `get-class` |
| Managed .NET assembly | `rxd dotnet` | `outline`, `strings` / `refs` / `get-method`, `get-class` |
| Local EVM flat bytecode | `rxd evm` | `outline`, `strings`, `interface`, `storage` / `refs` / `get-function`, `get-block` |
| Binary WASM core module | `rxd wasm` | `outline`, `strings` / explicit function `refs --func` / `get-function` |
| CPython .pyc or versioned raw marshal | `rxd pyc` | `outline`, `strings` / explicit name or constant `refs` / `get-code`, whole-module `decompile` |
| Native binary (optional engine) | `rxd kuna` | native `functions` / engine-specific evidence / `decompile` |

Ordinary JVM class/JAR input goes directly to `rxd jar`, without Java, D8 or
conversion. A ZIP containing root DEX entries belongs to `rxd dex`. ASAR is a
container: extract only the selected entry and analyze that file, rather than
applying code-reference semantics to the container. HTML is a lexical container
(`lexical_naive_v1`, not a DOM or browser parse): `list` enumerates
`element`/`text`/`comment`/`doctype` records with content-bound IDs, `extract`
returns the exact original interval, and inline JS/TS is then handed to the code
backend with artifact-side offsets. PyInstaller/zipapp (`pyz`)
and binwalk integration are planned, not available commands.

## Consume results and preserve identity

- stdout contains payload; diagnostics and supported `--perf`/`--debug` output
  use stderr. Do not mix them. JSONL lists/searches/references have header →
  records → summary. Require successful exit and `summary.complete: true`; JS
  directory scans also have per-file summaries and a final `scan_summary`
  (files are scanned in path order, using `--threads` workers by default, and
  the stream is identical whatever the worker count).
  Backend-specific completeness, inference and reconstruction flags still apply.
- Schema v2 navigation (`id`, `source_id`, `parent_id`, `context_id`, `start`,
  `end`) is outside `detail`; format-specific fields are inside. Not every row
  has a usable ID or range. Parse actual responses, never manufacture IDs.
- Content-bound IDs must be reused with the same bytes and interpretation options.
  Changed input invalidates them. CLI inputs must remain fixed for the entire
  call. JS offsets are UTF-8 byte offsets, not characters or lines. Other offsets
  retain their backend meaning: JVM instruction offsets, .NET metadata tokens,
  DEX member indices/code units, WASM code-body offsets, PYC code-relative bytes,
  EVM decoded PCs versus original raw/hex carrier ranges. Do not interchange them.
- Narrow before FETCH. Inspect command-specific input, entity, work, output,
  depth and archive-inflation budgets with `--help`. Budget failure is a failure,
  not evidence of absence. Do not silently raise limits or discard partial output.
  Zero hits in a complete query are valid within that query's stated scope.
- Default JS `get` returns original bytes. Java/C#/C-like/Python reconstruction is
  an analysis representation, not original source or a universal semantic proof.
  Inspect reconstruction/fallback flags and original bytecode where available.
  Static references do not imply a complete runtime call graph.

Examples below use local input placeholders and IDs/offsets obtained from the
preceding response. Keep each input fixed and use its actual returned values.

## JavaScript: literal → local source → lexical references

```sh
rxd js find bundle.js --string '/api/login'
rxd js get bundle.js --id "$CONTEXT_ID" --json
rxd js identifiers bundle.js --name 'AIS3' --role reference
rxd js calls bundle.js --name 'fetch'
rxd js members bundle.js --name 'flag_correct'
rxd js symbols bundle.js --within "$ENTITY_ID"
rxd js refs bundle.js --byte "$SYMBOL_BYTE"
rxd js get bundle.js --id "$SYMBOL_ID"
```

A find match has no match ID: use its `context_id` for a local syntax fragment,
falling back to `parent_id` for the enclosing entity. `ENTITY_ID` is an entity
from outline or a match's parent; a fragment ID is not valid for `symbols --within`.
Use a symbol row's `start` for refs and its `id` for the canonical declaration.
Refs resolves lexical bindings within one file, not properties or cross-file
calls; `unresolved`/`dynamic` resolution remains explicit. For directory find,
route each hit with its header's `detail.path` into single-file commands.

The identifier/call/member queries are syntax-first locators. `identifiers`
returns binding/reference occurrences of a full lexical name (so `obj.foo` never
matches, but `obj[key]` matches `key`); each hit has `parent_id` and `context_id`
for FETCH. `calls` reports call sites of a name with
`call_kind: direct|member|new`, never a return value or call graph. `members`
reports static member/property keys and JSX attribute names, plus computed keys
whose key is a string literal or a constant expression the bounded evaluator can
prove (`member_kind: computed_key` with `key_resolution: literal|static`); a
dynamic `obj[expr]` key never matches. `outline --kind` takes one kind or a
comma-separated list, and `--name` filters entities exactly. `find
--resolve-static` additionally matches bounded AST constant evaluation; those
hits are `literal_kind: static_expression` with `detail.raw` and
`resolution: static`, and it never executes user-defined functions.

```sh
rxd asar entries app.asar
rxd asar extract app.asar --file main.js -o extracted-main.js
rxd js entities extracted-main.js
```

ASAR extract requires an output file and returns a receipt; it never runs the
entry or extracts the whole application implicitly.

HTML is a lexical container: `list` returns `element`/`text`/`comment`/`doctype`
records with content-bound IDs and decoded attribute values; `extract` copies the
exact original interval and returns a receipt whose `source_id` identifies the
artifact. Attribute values are decoded (not byte-exact), while extracted bytes
stay byte-exact. Inline JS/TS is handed to the JS backend with artifact-side
offsets — never apply container offsets to the extracted file.

```sh
rxd html entries page.html
rxd html entries page.html --tag script --attr type=module
rxd html extract page.html --id "$SCRIPT_ID" --content -o inline.js
rxd js entities inline.js
```

## Android and JVM: string → caller → selected Java

```sh
rxd dex strings app.apk --filter Authorization --limit 20
rxd dex findrefs app.apk string Authorization
rxd dex manifest app.apk
rxd jar strings library.jar --filter Authorization
rxd jar findrefs library.jar string Authorization
rxd jar get-method library.jar "$METHOD_ID" --bytecode
rxd jar get-method library.jar "$METHOD_ID"
rxd jar get-class library.jar "$CLASS_NAME"
```

DEX IDs/source IDs are null: route findrefs using `detail.class` (the caller)
and `detail.entry`; reuse the unchanged archive. DEX strings have global table
indices and no owning class. A duplicate class is an ambiguity, not a reason to
choose the first definition. DEX 041 may identify logical DEX members separately.
field_ids, while method indices are method_ids.
DEX `-o` also prints the same bytes to stdout; other backends have their own file
output contracts. In JVM findrefs take the caller `context_id` as `METHOD_ID`;
outline method IDs preserve overload descriptors and content identity. JVM
`--release` selects multi-release JAR versions. Java reconstruction may fail or
contain unsupported-method stubs; inspect bytecode when necessary.

## .NET: literal → owning method → IL/C#

```sh
rxd dotnet strings library.dll --contains Authorization
rxd dotnet refs library.dll --string Authorization
rxd dotnet get-method library.dll "$METHOD_ID" --il
rxd dotnet get-method library.dll "$METHOD_ID"
rxd dotnet get-class library.dll "$TYPE_ID" --il
```

Use returned owning method IDs and outline type IDs. Metadata tokens are scoped
to the module and cannot identify a member across assemblies. C# reconstruction
requires an available ILSpy NativeAOT engine (included in complete distributions;
optional in source builds). Engine absence or failure retains original IL and
explicit fallback information; do not report fallback IL as reconstructed C#.

## WASM: structure → explicit caller → selected function

```sh
rxd wasm functions module.wasm
rxd wasm strings module.wasm --contains hello
rxd wasm refs module.wasm --func "$FUNCTION_INDEX"
rxd wasm get-function module.wasm "$CALLER_ID"
rxd wasm get-function module.wasm "$CALLER_ID" --raw
```

Select the merged function index from outline, then use caller IDs from refs.
Strings inventory data/name sections; it does not prove string-to-code ownership.
Refs reports direct calls and `ref.func`; indirect calls remain unresolved.
Default FETCH attempts structured C-like reconstruction and falls back to a
complete instruction view for unsupported instructions/control flow. `--raw`
forces that view. WAT text, Component Model, linking and WASI execution are outside
this binary core-module analyzer.

## Python: constant → explicit reference → original code

```sh
rxd pyc strings module.pyc --contains Authorization
rxd pyc refs module.pyc --const-id "$CONSTANT_ID"
rxd pyc get-code module.pyc "$CODE_ID"
rxd pyc refs module.pyc --name token --within "$CODE_ID"
rxd pyc decompile module.pyc --json
```

Static navigation accepts the supported final CPython 3.8–3.13 formats, not every
Python implementation or prerelease. Raw marshal requires `--version X.Y`. Use
strings' occurrence `id`, then references' owning `context_id` for get-code.
Constant occurrences remain distinct even when values/containers are shared.
Outline/strings do not imply that instructions have been scanned. `get-code`
preserves instructions, caches, tables and location evidence; names are explicit
bytecode facts, not resolved duck-typed calls. Whole-module source reconstruction
is bounded and has independent completeness: inspect `detail.complete` in JSON;
plain source mode fails when recovery is incomplete.

## EVM: candidate literal → evidenced reference → region

```sh
rxd evm blocks runtime.hex --encoding hex
rxd evm strings runtime.hex --encoding hex --kind text --query Authorization
rxd evm refs runtime.hex --encoding hex --id "$LITERAL_ID"
rxd evm get-function runtime.hex "$CONTEXT_ID" --encoding hex --raw
rxd evm get-block runtime.hex "$BLOCK_ID" --encoding hex
rxd evm interface runtime.hex --encoding hex
```

Encoding is required (`raw` bytes or ASCII `hex`), never guessed. Search kinds
`text`, `bytes`, `number` have different matching semantics. Follow refs'
`context_kind`: function goes to get-function, block goes to get-block; null
contexts are not fetchable. Selectors, function boundaries, storage and text/data
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
native names/addresses and JSON contracts, not invented RxD entity IDs. MCP
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
