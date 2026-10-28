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
| ASAR, ZIP/HAP, PyInstaller or wxapkg input | `rxd unpack` | `rxd unpack ARCHIVE_OR_ONEDIR -o DIR [--threads N]`; extracts every regular file, PYZ modules and reconstructed `.pyc` files; for one-dir, finds the launcher; for V1MMWX wxapkg, pass `--appid` |
| Dynamic Ark ABC or HAP | `rxd ark` | `export INPUT -o DIR` writes the whole HAP tree plus one JS file per record for `rg`; `decompile INPUT` reconstructs the selected whole ABC; multiple records or non-scalar module names use scoped, namespaced exports. `list record`, then `list method` (all records by default, `--at record:0x...` to scope); `strings`/`xref` scan decoded string operands. `decompile --at record:0x...` composes one original module, and `--at method:0x...` selects one function. Methods use capture factories with `{env, hole, receiver, newTarget, locals, home, function}`. Semantic approximations and unrecovered structure are marked in source; add `--raw` for original instructions. HAP with multiple ABC files requires `--entry PATH` |
| Android APK or DEX | `rxd rasc` | `classes`, `methods`, `fields`, `entries`, `strings`, `manifest` / `xref --at` or `-m PATTERN --kind string\|type\|method\|field` / `get-class`/`get-method --at` |
| JVM class or ordinary JAR | `rxd jar` | `classes`, `methods`, `fields`, `entries`, `strings` / `xref --at` or `-m … --kind` / `get-method`/`get-class --at`, `--raw` for original instructions |
| Managed .NET assembly | `rxd ilspy` | `classes`, `fields`, `methods`, `strings` / `xref --at` (token) or `-m … --kind string` / `get-method`/`get-class --at`, `--raw` for original IL |
| Local EVM flat bytecode | `rxd evmole` | `interface`, `disassemble`, `strings`, `metadata` |
| Binary WASM core module | `rxd wasm` | `types`, `imports`, `exports`, `functions`, `tables`, `memories`, `globals`, `data`, `elements`, `strings` / `xref --at func:N` or `-m … --kind function` / `get-function --at` |
| CPython PYC, PyInstaller PYZ or versioned raw marshal | `rxd pyc` | `modules`, `codes --within MODULE`, `strings` / `xref --at` (a strings row), `xref --kind name --at NAME/-m TEXT` / `decompile` (a PYZ without `--at` writes one source file per code-bearing module with `-o DIR`) or `decompile --at` one module/code; standalone PYC defaults to its whole module |
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
from a HAP without unpacking unrelated resources or native libraries, or
`rxd ark export` can extract the complete HAP tree and add reconstructed record files. AAR code normally lives in
`classes.jar` or `libs/*.jar`: unpack, then use `rxd jar`. Encrypted ASAR/ZIP/PyInstaller entries, links and automatic nested extraction
beyond PYZ are unsupported.

## Consume results and preserve identity

- stdout contains payload; diagnostics and the `--perf` record
  use stderr. Do not mix them. Every table begins with one `# ` column-header
  line, then TAB-separated data rows. An empty result retains the header.
  Each table command's `--help` opens with its `Columns:` line. Listing data
  starts with its coordinate; xref routing uses the context column below.
  Text cells are JSON-quoted and `\N` means absent.
  Skip the first line and read columns by position (`tail -n +2 | cut -f2`),
  not by guessing. Kuna keeps its native formatting.
  Require exit 0; zero data rows is a complete empty result. Backend-specific
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

Reference rows are minimal: `rasc`, `jar` and `pyc` emit `entry, context, match`;
their standalone inputs retain the entry cell as `\N`. `ilspy`, `wasm`,
`unluac-rs` and `dae` emit `context, match`. Entry paths/module
names are JSON-quoted text. Context is the complete owner coordinate; match is
the actual operand target, not the search pattern. PYC constant/Lua queries
retain the selected constant coordinate as match; ILSpy and WASM retain target
tokens/coordinates. Repeated sites can produce identical rows; no new deduplication
is implied by omitting instruction details. No JSON envelope is emitted.
The first line is `# entry<TAB>context<TAB>match` or `# context<TAB>match`.
Skip it before reading data (for example, `tail -n +2 | cut -f2`); a successful
zero-hit query contains only that header, and errors still emit no partial payload.
Kuna keeps its upstream `xrefs` formatting and `--json` option; it is not converted.

Examples below use local input placeholders and listing `at` / reference `context`
coordinates or offsets obtained from the preceding response. Keep each input fixed and use its actual returned values.

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
DEX xref rows retain `entry` (which DEX), `context` (the referencing member),
and `match` (the target). Reuse the unchanged input. DEX strings have per-DEX
table indices and no owning class. A duplicate class is an ambiguity, not a reason to
choose the first definition. DEX 041 may identify logical DEX members separately.
DEX `-o` also prints the same bytes to stdout; other backends have their own file
output contracts. JVM xref puts the class archive path in `entry` and the caller
in `context` (usable as `--at`); method coordinates preserve overload descriptors. JVM
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

Use `strings.context_at` or `xref.context` for the owning method, and
`classes.at` for a type; .NET coordinates are bare metadata tokens (`0x06000025`).
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
rxd pyc decompile PYZ.pyz -o recovered/
rxd pyc decompile PYZ.pyz --at "$MODULE_OR_CODE_AT"
rxd pyc decompile module.pyc
rxd pyc decompile module.pyc --at "$CODE_AT"
```

`decompile` without `--at` reconstructs every code-bearing module. With `-o DIR` it
writes one source file per module in the original package layout
(`json/__init__.py`, `json/decoder.py`) plus `manifest.jsonl`; without `-o` it prints
concatenated `# ===== <module> =====` blocks. A module whose decompiler panics becomes
a labelled `PYC_FALLBACK` file/block and is named on stderr; other failures abort
atomically. Redirect `-o` and search with `rg` for a
whole-archive text pass. `--at` still selects one module or code body.

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
code coordinate. Reference `context` selects the code body for reconstruction.
PYZ reference `entry` identifies its module. Name matches remain original names;
constant matches remain exact module-qualified constant coordinates.

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

## Ark ABC/HAP: record → method → JS, or an export tree for text search

```sh
rxd ark list record app.hap --entry ets/modules.abc
rxd ark list method app.hap --entry ets/modules.abc -f token
rxd ark list method app.hap --entry ets/modules.abc --at "record:0x1234"
rxd ark strings app.hap --entry ets/modules.abc -f Authorization
rxd ark xref app.hap --entry ets/modules.abc -m Authorization
rxd ark xref app.hap --entry ets/modules.abc --at "<strings.at>"
rxd ark decompile app.hap --entry ets/modules.abc --at "method:0x5678"
rxd ark decompile app.hap --entry ets/modules.abc --at "record:0x1234" --raw
rxd ark export app.hap -o recovered
rxd ark export app.hap -o sources --extract none
rg -n -C 3 'Authorization' recovered/decompiled
```

`list method` searches every record unless `--at` names one, and prints the owning
`record` column. `strings` lists `str:0x...` values referenced by decoded instruction
operands; `xref` takes one `strings.at` or `-m TEXT` and returns `context, match` rows
(method and string coordinate), one per method/string pair. These scans decode every
method, run in parallel by default (`--threads`), and reject an `--at` string that no
decoded instruction references. They index explicit string operands only, not module
requests or literal-array entries.

`decompile` without `--at` returns the whole ABC; `--raw` returns instructions.
`--extract all` (default) writes `extracted/` with every original HAP file at its
archive path, `abc` writes only `.abc` entries, and `none` writes no originals; ABC
reconstruction overlaps the remaining extraction. Sources go under
`decompiled/<abc path>/0x<record>.js`, with `manifest.jsonl` beside them. Each record
file starts with `// record:0x... "<name>"` and one `// method:0x... "<name>"` per
coded method, then an ordinary JS module holding every coded method, including methods
no initializer reaches. `rg` therefore resolves a name to its coordinate directly.
Manifest record rows carry the
`record:0x...` coordinate, original name, coded-method count, warning codes and
relative file; the ABC row carries its SHA-256. Route a hit back to `--at` on the
same input. The export is atomic and refuses an existing directory. `--threads N`
reconstructs records in parallel with forked parsers; output does not depend on
thread count. All ABC entries export by default and `--entry` narrows to one.

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
