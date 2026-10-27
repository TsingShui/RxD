---
name: rxd
description: Analyze APK/DEX, JVM class/JAR, .NET IL, EVM bytecode, WebAssembly and CPython bytecode with the rxd CLI. Select a backend, locate structures or literals, trace explicit references and fetch selected original code or reconstruction; use optional Kuna for native binaries and optional MCP for typed tools.
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
| ASAR, ZIP, PyInstaller or wxapkg input | `rxd unpack` | `rxd unpack ARCHIVE_OR_ONEDIR -o DIR`; extracts every regular file, PYZ modules and reconstructed `.pyc` files; for one-dir, finds the launcher; for V1MMWX wxapkg, pass `--appid` |
| Android APK or DEX | `rxd rasc` | `classes`, `methods`, `fields`, `entries`, `strings`, `manifest` / `xref --at` or `-m PATTERN --kind string\|type\|method\|field` / `get-class`/`get-method --at` |
| JVM class or ordinary JAR | `rxd jar` | `classes`, `methods`, `fields`, `entries`, `strings` / `xref --at` or `-m … --kind` / `get-method`/`get-class --at`, `--raw` for original instructions |
| Managed .NET assembly | `rxd ilspy` | `classes`, `fields`, `methods`, `strings` / `xref --at` (token) or `-m … --kind string` / `get-method`/`get-class --at`, `--raw` for original IL |
| Local EVM flat bytecode | `rxd evm` | `interface`, `disassemble`, `strings`, `metadata` |
| Binary WASM core module | `rxd wasm` | `types`, `imports`, `exports`, `functions`, `tables`, `memories`, `globals`, `data`, `elements`, `strings` / `xref --at func:N` or `-m … --kind function` / `get-function --at` |
| CPython .pyc or versioned raw marshal | `rxd pyc` | `codes`, `strings` / `xref --at` (a `strings` row), `xref --kind name --at NAME/-m TEXT`, `get-code --at`, whole-module `decompile` |
| PUC Lua 5.1-5.5, LuaJIT 2.1 or Luau chunk | `rxd lua` | `codes`, `strings` / `instructions --at proto:N`, whole-chunk `decompile` |
| Native binary (optional engine) | `rxd kuna` (alias `rxd bin`) | native `functions` / engine-specific evidence / `decompile` |

Ordinary JVM class/JAR input goes directly to `rxd jar`, without Java, D8 or
conversion. A ZIP containing root DEX entries belongs to `rxd rasc`. ASAR, ZIP and
PyInstaller are opaque containers: `rxd unpack ARCHIVE -o DIR` extracts every
regular file into a new directory, then analyze those files with the matching
backend. PyInstaller one-file inputs additionally emit `PYZ.pyz_extracted`
modules with reconstructed `.pyc` headers. A PyInstaller one-dir root is accepted too:
its launcher's CArchive/PYZ is extracted; loose `_internal` dependencies stay in place.
wxapkg is unpacked from its index; encrypted `V1MMWX` PC/Mac packages need
`--appid` or a path component matching the appid. There is no entry selection or
in-memory archive analysis. AAR code normally lives in
`classes.jar` or `libs/*.jar`: unpack, then use `rxd jar`. Encrypted ASAR/ZIP/PyInstaller entries, links and automatic nested extraction
beyond PYZ are unsupported.

## Consume results and preserve identity

- stdout contains payload; diagnostics and the `--perf` record
  use stderr. Do not mix them. Listings, searches and reference results are
  Unix-table rows: one record per line, TAB-separated cells, no header and no
  summary. Each command's `--help` opens with its `Columns:` line; the first
  cell selects the row, text cells are JSON-quoted and `\N` means absent.
  Read columns by position (`cut -f2`, `awk -F'\t'`), not by guessing.
  Require exit 0; zero rows is a complete empty result. Backend-specific
  completeness and inference flags still apply.
- Fetch commands (`get-*`, `decompile`, `get-code`, `analyze`) return one JSON
  record with schema v2 navigation (`at`, `parent_at`, `context_at`,
  `source_id`, `start`, `end`) outside `detail` and format-specific fields
  inside. `at` is the copy-only coordinate a listing prints and the value `--at`
  accepts. Parse actual responses, never manufacture coordinates.
- A coordinate names a range in the input you are querying, not the file itself: reuse
  it only with the same bytes and interpretation options. A coordinate copied from
  another input is not checked; re-run the listing for the file you are fetching.
  CLI inputs must remain fixed for the entire call. Offsets retain their backend meaning: JVM instruction offsets, .NET metadata tokens,
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
- Java/C#/C-like/Python reconstruction is an analysis representation, not original source or a universal semantic proof.
  Inspect reconstruction/fallback flags and original bytecode where available.
  Static references do not imply a complete runtime call graph.

Examples below use local input placeholders and `at`/`context_at` coordinates or
offsets obtained from the preceding response. Keep each input fixed and use its actual returned values.

## Android and JVM: string → caller → selected Java

```sh
rxd rasc strings app.apk -f Authorization --limit 20
rxd rasc xref app.apk -m Authorization --kind string
rxd rasc xref app.apk --at "$METHOD_AT"
rxd rasc manifest app.apk
rxd rasc get-method app.apk --at "$METHOD_AT"
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
rxd ilspy strings library.dll -f Authorization
rxd ilspy xref library.dll -m Authorization --kind string
rxd ilspy xref library.dll --at "$METHOD_AT"
rxd ilspy methods library.dll
rxd ilspy get-method library.dll --at "$METHOD_AT" --raw
rxd ilspy get-method library.dll --at "$METHOD_AT"
rxd ilspy classes library.dll --class "$TYPE_NAME"
rxd ilspy get-class library.dll --at "$TYPE_AT" --raw
rxd ilspy xref library.dll --at "$TYPE_AT"
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

## Lua: prototypes, constants, instructions, source

```sh
rxd lua codes module.luac
rxd lua strings module.luac -f token
rxd lua instructions module.luac --at "$CODE_AT"
rxd lua decompile module.luac
```

The dialect (PUC Lua 5.1-5.5, LuaJIT 2.1, Luau) comes from the chunk header, so
there is no encoding flag. `codes.at` is `proto:N` in compiler order;
`strings.at` is `proto:N:const:I`, the pool slot the owning prototype loads, and
selects no fetch. `instructions` pairs chunk byte offsets with original opcodes.
Decompilation reconstructs the whole chunk through the pinned unluac engine and
is a representation, not the original source file.

## EVM: selectors, literals, compiler metadata

```sh
rxd evm interface runtime.hex --encoding hex
rxd evm disassemble runtime.hex --encoding hex
rxd evm strings runtime.hex --encoding hex --kind text --query Authorization
rxd evm metadata runtime.hex --encoding hex
```

Encoding is required (`raw` bytes or ASCII `hex`), never guessed. Search kinds
`text`, `bytes`, `number` have different matching semantics. `interface` lists
function candidates with their EVMole ABI fields, `disassemble` pairs decoded
bytecode offsets with opcodes, `metadata` decodes the terminal CBOR blob
(compiler, version, source hash). Selectors, function boundaries, storage and
text/data are inferred candidates: PUSH literal and contextual CODECOPY evidence
do not prove runtime strings or a verified source ABI. There is no decompiler
and no recovered source call graph; treat every row as evidence, not as Solidity.

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
