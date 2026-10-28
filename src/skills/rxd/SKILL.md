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
| ASAR, ZIP/HAP, PyInstaller or wxapkg input | `rxd unpack` | `rxd unpack ARCHIVE_OR_ONEDIR -o DIR`; extracts every regular file, PYZ modules and reconstructed `.pyc` files; for one-dir, finds the launcher; for V1MMWX wxapkg, pass `--appid` |
| Dynamic Ark ABC 12.0.6.0 or HAP | `rxd ark` | `list record`, then `list method --at record:0x...`; `decompile --at record:0x...` composes the module and required definitions; `decompile --at method:0x...` reconstructs one function or async body. Methods appear as capture factories taking `{env, hole, receiver, newTarget, locals}`. Iterator semantics warnings appear at affected source operations. Unsupported constructs fail explicitly; add `--raw` for original instructions; HAP with multiple ABC files requires `--entry PATH` |
| Android APK or DEX | `rxd rasc` | `classes`, `methods`, `fields`, `entries`, `strings`, `manifest` / `xref --at` or `-m PATTERN --kind string\|type\|method\|field` / `get-class`/`get-method --at` |
| JVM class or ordinary JAR | `rxd jar` | `classes`, `methods`, `fields`, `entries`, `strings` / `xref --at` or `-m … --kind` / `get-method`/`get-class --at`, `--raw` for original instructions |
| Managed .NET assembly | `rxd ilspy` | `classes`, `fields`, `methods`, `strings` / `xref --at` (token) or `-m … --kind string` / `get-method`/`get-class --at`, `--raw` for original IL |
| Local EVM flat bytecode | `rxd evmole` | `interface`, `disassemble`, `strings`, `metadata` |
| Binary WASM core module | `rxd wasm` | `types`, `imports`, `exports`, `functions`, `tables`, `memories`, `globals`, `data`, `elements`, `strings` / `xref --at func:N` or `-m … --kind function` / `get-function --at` |
| CPython PYC, PyInstaller PYZ or versioned raw marshal | `rxd pyc` | `modules`, `codes --within MODULE`, `strings` / `xref --at` (a strings row), `xref --kind name --at NAME/-m TEXT` / `decompile --at` module or code; standalone PYC defaults to its whole module |
| Flutter Android `libapp.so` (Dart AOT, ELF ARM64) | `rxd dae` | `info`, `classes`, `functions --class`, `strings`, `xref --at str:0x...` / `decompile --at|--class|--lib` |
| PUC Lua 5.1-5.5, LuaJIT 2.1 or Luau chunk | `rxd unluac-rs` | `codes`, `strings`, `xref --at proto:N:const:I`, `instructions --at proto:N`, whole-chunk `decompile` |
| Native binary (optional engine) | `rxd kuna` (alias `rxd bin`) | native `functions` / engine-specific evidence / `decompile` |

Ordinary JVM class/JAR input goes directly to `rxd jar`, without Java, D8 or
conversion. A ZIP containing root DEX entries belongs to `rxd rasc`. ASAR, ZIP and
PyInstaller executables are opaque containers: `rxd unpack ARCHIVE -o DIR` extracts every
regular file into a new directory, then analyze those files with the matching
backend. HAP packages use the ZIP path: all entries, including ABC, resources and
native libraries, are extracted byte-exactly; the receipt format is `zip`.
PyInstaller one-file inputs additionally emit `PYZ.pyz_extracted`
modules with reconstructed `.pyc` headers. A PyInstaller one-dir root is accepted too:
its launcher's CArchive/PYZ is extracted; loose `_internal` dependencies stay in place.
A standalone PyInstaller `PYZ.pyz` can instead go directly to `rxd pyc` for
archive-wide queries and selected reconstruction without writing extracted files.
wxapkg is unpacked from its index; encrypted `V1MMWX` PC/Mac packages need
`--appid` or a path component matching the appid. `unpack` has no entry selection or
in-memory archive analysis. `rxd ark` can instead read the selected ABC directly
from a HAP without unpacking unrelated resources or native libraries. AAR code normally lives in
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
- Fetch commands print the selected unit: `get-*` and `decompile` print the
  reconstructed source text itself, and `--raw` prints original instructions as
  text lines. Evidence that is not source, such as class metadata under `--raw`,
  is one JSON record with schema v2 navigation (`at`,
  `parent_at`, `context_at`, `source_id`, `start`, `end`) outside `detail` and
  format-specific fields inside. `at` is the copy-only coordinate a listing
  prints and the value `--at` accepts. Parse actual responses, never manufacture
  coordinates.
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

`get-method` and `get-class` print the reconstructed Java itself; `--raw`
returns the original instructions instead. JVM method FETCH supports `--raw`;
DEX FETCH emits Java and has no `--raw` flag.
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

A `classes` row's first cell is its `at`; copy it to class FETCH or type XREF
without rewriting it as `type`. Select a real type first: unfiltered listings
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
function coordinate from `functions` (match the export/import label), then use the
caller coordinate in `xref`'s
`context_at`. Strings inventory data/name sections; it does not prove
string-to-code ownership. `xref` reports direct calls and `ref.func`; indirect
calls remain unresolved.
Default FETCH attempts structured C-like reconstruction and falls back to a
complete instruction view for unsupported instructions/control flow. `--raw`
forces that view. WAT text, Component Model, linking and WASI execution are outside
this binary core-module analyzer.

## Python: whole-PYZ search → selected module or code body

```sh
rxd pyc modules PYZ.pyz -f networking
rxd pyc codes PYZ.pyz --within "$MODULE_AT"
rxd pyc codes PYZ.pyz -f proxy
rxd pyc strings PYZ.pyz -f Authorization
rxd pyc xref PYZ.pyz --at "$CONSTANT_AT"
rxd pyc xref PYZ.pyz --kind name -m getproxies
rxd pyc xref PYZ.pyz --kind name -m proxy --within "$MODULE_OR_CODE_AT"
rxd pyc decompile PYZ.pyz --at "$MODULE_OR_CODE_AT"
rxd pyc decompile module.pyc
rxd pyc decompile module.pyc --at "$CODE_AT"
```

The same commands accept PYC and PyInstaller `PYZ\0`, detected by magic rather
than filename. ZIP zipapps are a different format. PYZ searches cover every
code-bearing member, not startup scripts/native libraries outside that archive.
No unpacked directory or persistent index is needed.

`modules` reads the PYZ directory without inflating members; its columns are
`at, kind`, with kind `module`, `package` or `namespace`. Namespace packages
have no executable body; data entries are excluded. For standalone PYC/raw marshal,
the single module row is `code:0`, without guessing a module name from its path.
`codes --within MODULE` parses only the exact selected module, including its root;
omit `--within` for whole-input search. `codes --at` selects just one row and is
exclusive with `--within`. Both listings support `-f` and `--case sensitive`.

PYZ module roots use the full original module name, such as
`yt_dlp.YoutubeDL`; nested code uses `yt_dlp.YoutubeDL::code:N`.
Constant occurrences use `MODULE::const:N:PATH`. Standalone PYC retains
`code:N` and `const:N:PATH`. Copy printed coordinates; code indices are local
to their module and remain unchanged by filtering.

A strings coordinate selects constant xref; equal strings in different modules
remain distinct. Name xref searches explicit imports/loads/attributes, not
resolved dynamic calls. `xref --within` accepts a whole-module coordinate or one
code coordinate. Reference `context_at` selects the caller for reconstruction.
PYZ reference `detail.entry` identifies its module; `start`/`end` refer to that
member's uncompressed marshal payload, not ranges in the compressed archive.

PYZ reconstruction requires `--at`. A module coordinate reconstructs that whole
module; a code coordinate reconstructs its complete execution body as a source
fragment. Enclosing bindings, defaults and decorators are outside a code-body
selection, not guessed wrappers. A standalone PYC without `--at` retains
whole-module reconstruction. Source/fallback labels use the listing coordinates.

Unsupported bodies retain labelled original-bytecode comments while healthy
siblings remain source. An unavailable enclosing binding leaves nested recovery
as isolated comments. Partial reconstruction has markers and a stderr diagnostic;
malformed input, incomplete reference scans and resource failures are explicit,
without partial success output. Navigation and reconstruction share embedded
magic/version rules; raw marshal still requires `--version X.Y`.

## Dart AOT (Flutter): snapshot info, functions, pseudocode

```sh
rxd dae info libapp.so
rxd dae classes libapp.so -f ClashConfig
rxd dae functions libapp.so --class 'class:0x8f5'
rxd dae functions libapp.so -f fl_clash
rxd dae strings libapp.so -f Authorization
rxd dae xref libapp.so --at 'str:0x6590'
rxd dae decompile libapp.so --lib 'fl_clash$models'
```

Input is the ELF ARM64 `libapp.so` from a Flutter Android release build; there is
no encoding flag and no SDK installation. `info` reports the fingerprint-detected
Dart profile and its verification status, which can be `unverified` — treat the
symbols as strong evidence, not as a verified source model. `functions` rows pair
a code entry offset with `library::class::method`; libraries use `$` separators,
so `--lib` and `-f` match that spelling. `strings` lists the object pool, and its
`at` is the pool offset `xref` consumes: `xref` decodes every function and reports
each instruction that loads the string, with the containing function in
`context_at`. Matches come from the ARM64 pool-load encoding, so only the hits
are decoded. `classes` names each type with `class:0x<id>` and its own method count;
`decompile` renders a whole library (`--lib PREFIX`), one class (`--class`), or
one function (`--at`), one `dart_library` record per rendered file; the output is machine-level pseudocode (registers, `mem()`
access, raw PC comments), not source Dart.

## Lua: prototypes, constants, instructions, source

```sh
rxd unluac-rs codes module.luac
rxd unluac-rs strings module.luac -f token
rxd unluac-rs xref module.luac --at "$CONST_AT"
rxd unluac-rs instructions module.luac --at "$CODE_AT"
rxd unluac-rs decompile module.luac
```

The dialect (PUC Lua 5.1-5.5, LuaJIT 2.1, Luau) comes from the chunk header, so
there is no encoding flag. `codes.at` is `proto:N` in compiler order;
`strings.at` is `proto:N:const:I`, the pool slot the owning prototype loads, and
selects `xref`, which reports the instructions naming that slot as
`proto:N:ins:M` with the owning prototype in `context_at`. `instructions` pairs
chunk byte offsets with original opcodes.
Decompilation reconstructs the whole chunk through the pinned unluac-rs engine and
is a representation, not the original source file.

## EVM: selectors, literals, compiler metadata

```sh
rxd evmole interface runtime.hex --encoding hex
rxd evmole disassemble runtime.hex --encoding hex
rxd evmole strings runtime.hex --encoding hex --kind text --query Authorization
rxd evmole metadata runtime.hex --encoding hex
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
consume `structuredContent` — `{text}` for printed source and table rows,
`{records: [...]}` for JSON streams, and the record itself for class metadata
under `--raw` — not a truncated text preview.

## Scope and maintenance

Standard literal/immediate decoding (escapes, LEB128, float/memory operands,
PUSH values) is parsing. Existing structured reconstruction (DEX/JVM→Java, IL→C#,
WASM→C-like, PYC→Python, selected EVM pseudo-code) remains subject to its declared
support and fallback contracts.

`rxd skill` prints this exact Markdown to stdout and writes no files; placing it
in an Agent skill directory is the caller's job.
