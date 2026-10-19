# Kuna and external tools

Status: Kuna is linked through a generated library entry; binwalk remains planned.

## Kuna build and distribution

Per [ADR 0006](../adr/0006-linked-kuna-cli.md), `third_party/kuna` is a pinned Git
submodule whose source is never edited. `crates/rxd-kuna-cli/build.rs` validates the
gitlink and a clean source tree, then generates a callable entry point and a few
adapter files in Cargo's `OUT_DIR`. Upstream parsing, loading, analysis,
decompilation, output and task scheduling are used directly; commands are not
reimplemented one by one. The entry accepts argv and returns the original exit code.
Persistent workers restart rxd with the `kuna` prefix. Single-function text uses a
cleanable thread-local stdout capture, without an external Kuna process. Generated
adapters require unique matches at key fragments; upstream entry changes fail the
build explicitly.

`python3 scripts/build-bundled.py` builds slacomp, compiles the specs for every
architecture, then builds the complete rxd. Kuna machine code is linked into rxd;
spec data and LICENSE/NOTICE remain deterministic gzip/tar payloads. Compilation uses
`opt-level=3`, Thin LTO, one codegen unit and stripped symbols. `decomp_dbg` is not
built or packaged; slacomp is build-time only. Runtime needs no Rust, Git, Python or a
separate Kuna installation.

The default Cargo build does not compile Kuna, but Cargo needs the submodule
initialized to resolve the optional path dependency. Format with
`python3 scripts/format-workspace.py --check`; do not use `cargo fmt --all`, which
traverses upstream path dependencies.

## Command contract

```sh
rxd kuna --help
rxd kuna --version
rxd kuna functions ./app --summary --json
rxd kuna decompile ./app main --json
rxd kuna decompile ./app main
rxd kuna decompile-all ./app --json --jobs 2
rxd kuna decompile-project ./app --stream --jobs 2 -o ./app.kuna
rxd kuna catalog --json --option loweredswitch
```

The rxd-side adapter is `src/cli/kuna/mod.rs`; `src/main.rs` dispatches to it before
the generic clap/analysis worker and calls `rxd_kuna_cli::run`. Kuna keeps its UTF-8
argument model. JSON is not wrapped in the rxd envelope or generic IDs; file and
streaming indices keep their original meaning. Parallel workers are restarted rxd
processes entering the same internal Kuna CLI, preserving upstream budgets, stdin
protocol and parent-exit detection.

The reduced adapter remains: default catalog text, JSON and `check` use the engine's
own metadata and support option/tier filters. Single-function output without
`--json` still uses the JSON analysis strategy and extracts `functions[].code`; it
does not promise console-text equivalence. `--raw`, `--regions` and `--kassert`
fail explicitly on this JSON path; an explicit external engine keeps native behavior.
`decompile-all` and `decompile-project` run natively; project does not accept
`--json`. Agents must inspect each function's error; overall exit 0 is not per-function
success. `kuna test` and `kuna specs` need separately supplied helper programs; their
presence in native help does not mean they are packaged.

## Resource cache

Commands that need specs extract the pure resource archive into the private cache
`rxd-kuna/<archive-sha256>`. Help, version, docs and the default catalog do not create
it; analysis sets the specs path before any upstream thread starts. First extraction
checks the archive and every file's SHA-256, length, relative path and type, then
publishes atomically from a temporary directory. The cache contains no `kuna`,
`decomp_dbg`, `slacomp` or catalog JSON. Later opens validate the completion marker;
spec files use the same private-cache trust boundary as the previous version. ILSpy
uses a separate namespace and validates its cached library hash. Corrupt or
incomplete caches fail explicitly; deleting the version directory allows rebuild.

## Validation and size

Validation covers standalone rxd execution, native JSON/text and exit-code
compatibility, concurrent resource preparation, self-restarting parallel workers,
streaming project export, and version/catalog isolation. Compare against the pinned
original Kuna and the previous rxd for output and fresh-process latency. Size
comparisons fix Rust/profile/platform, the Kuna gitlink and embedded ILSpy bytes, and
record the executable, outer archive and resource cache separately. macOS arm64 is
the locally measured platform; cross-platform CI remains a separate gate.

## Offline native Frida hook

```sh
rxd hook-gen frida native INPUT TARGET [--lang js|ts] [--module NAME | --main-module]
```

`TARGET` is a function name or an explicit `0xHEX` **static virtual address**, never a
file offset or runtime absolute pointer. The default module is inferred from the
input; libraries use the basename, executables use the main-module route. Renamed or
special inputs need an explicit module. Duplicate names with different entries/modes
fail; arbitrary interior instructions are not declared functions. There is no Java
bridge, `--import`, `Java.perform`, automatic module watcher or pointer dereference.

Supported: linked little-endian ELF32/64, PE32/PE32+, thin Mach-O 32/64, with
ia32/x64/ARM32/AArch64 and consistent address width. ELF accepts OSABI 0/3/97 and
declares a Linux runtime assumption, using header-backed PT_LOAD image/load-bias with
page congruence checks. PE uses the actual preferred base/RVA and preserves Thumb
mode for THUMB/ARMNT. Mach-O excludes `__PAGEZERO`, converts `LC_MAIN` entryoff
through segments, and preserves `LC_UNIXTHREAD` VMA. ARM mapping-symbol/entry LSB,
Mach-O Thumb flags and the selected Kuna TMode are preserved; conflicts/alignment
fail. Relocatable, archive/universal, big-endian, unsupported architecture/subtype
(including arm64e/PAC), and inputs without a supported header mapping fail explicitly.
Not every accepted format/architecture/ABI combination has an independent fixture;
this is not a universal platform-coverage claim.

Offsets are hex strings. Runtime validates arch/pointer size/platform/page/module/range
before `Interceptor.attach`. It does not validate live module content hashes; the caller
must match the correct version.

Existing text symbols, PE exports and real image entries can produce cheap JS/raw TS
without an engine. Entry discovery uses an optional Kuna exact executable-entry
inventory. A name miss in the cheap index also triggers on-demand discovery, including
default JS; missing engine/resources is not a confirmed NotFound. Kuna's
`sub_ADDRESS` placeholder is valid only when the address belongs to the actual
executable-entry inventory; mapped interior addresses are not treated as functions.

With prepared specs, `bundled-kuna` TS requests only the selected function's structured
prototype/type/storage information; it does not decompile the whole library or parse
rendered C to determine identity. Public modules pass options.specs_dir explicitly,
with no process-global environment change; the root reuses the verified resource cache
when needed. Missing engine/resources for required analysis is `EngineUnavailable`;
entries or arguments are never fabricated. Embedded DWARF is debug-derived, Kuna
recovery is inferred, and absent information is unknown.

TS always has Frida callback API types. With trusted storage/model info it generates
named locals with original names, C types and provenance. It supports the first six GP
SysV x64 arguments, the first four Windows x64, arm64 x0–x7, Linux ARM32 r0–r3,
ia32 cdecl pointer-sized stack slots and corresponding GP returns. Signed/unsigned
8/16/32 use real truncation/sign extension; 64-bit targets keep Int64/UInt64
precision; pointers stay NativePointer. It never automatically reads memory, strings
or structs. Float/vector/aggregate/variadic, split/indirect/hidden storage and
unsupported model/stack/register/width are explicitly raw/unavailable; casts never
pretend a pointer is a number/struct and args are never fabricated.

Input is captured once. Kuna's path-only API uses a private snapshot and never reopens
the original input or absorbs external PDB/sidecars; non-empty `kuna_pdb_path` /
`kuna_fid_db` overrides are rejected. Preflight bounds compressed debug-section
expansion, mapping/work/entry ceilings; selected analysis receives internal deadlines
and instruction caps. Bootstrap/listing are cooperative checks only and cannot
preempt a blocking upstream call or enforce a kernel RSS ceiling. The upstream CLI and
gitlink are unchanged (ADR 0006), and native SEARCH/XREF closure is not added.

Module/target/attach begin/success and setup catch/rethrow are generated statements;
they do not prove live attach or ABI correctness. Shared budgets, exit codes and
module-driver gates are in [devtools/hook-gen/README.md](../../devtools/hook-gen/README.md).

## binwalk plan

binwalk will be integrated separately and does not inherit Kuna's packaging or adapter
policy.

## binwalk semantic boundary

The first version keeps signature scanning, include/exclude, native JSON logs, explicit
carve/extract, and recursion options; scanning never enables extraction, recursion or
entropy automatically. Agents that need extraction provide native arguments and an
output directory explicitly.

binwalk v3 `--log -` emits a native JSON array; use `--quiet` to keep ordinary output
out of it. rxd does not convert it to JS/ASAR JSONL. File logs use the tool's native
behavior; do not assume overwrite or atomic replacement.

A signature hit is a tool identification result, not proof that extraction succeeded,
that content is trustworthy, or that the format is complete. binwalk may estimate an
unknown length to the next signature or EOF; log sizes are not always exact file
boundaries. Native UUIDs are not wrapped as rxd content-bound IDs.

Extraction may invoke external tools; their availability, return values and output
files are part of the extraction contract. A zero binwalk exit does not replace
per-extraction success checks. Native carve is not decompression; they are not merged
into a vague "extracted" state.

## MCP projection

Only bundled-Kuna builds expose `kuna_functions` (complete native function JSON) and
`kuna_decompile` (single-function native JSON). Fixed upstream fields and per-function
errors are preserved; no rxd envelope or entity IDs are added, and SEARCH/XREF or all
CLI modes are not claimed. Tool parameters are typed with no arbitrary
argv/env/output; the linked dispatcher runs once in an isolated worker under bounded
capture, IPC, complete-response and deadline limits. The upstream submodule is not
modified; specs are prepared on demand.

The Kuna MCP RSS guard samples the actual child process identity about every 5 ms.
`--max-native-worker-bytes` defaults to and is capped at 268,435,456 (256 MiB) and may
only be lowered at startup. Exceeding it stops/reaps the native worker and returns
`RESOURCE_LIMIT`; this is not an instantaneous kernel hard limit and does not affect JS
workers. Single-function calls use exactly one of `name` or hex-string `address`; the
returned `address_hex` can be passed back without 64-bit precision loss in JS numbers.
Duplicate names preserve complete native candidates; a native instruction-ceiling
truncation is an explicit failure, not successful source.

The MCP native entry fixes `KUNA_DECODE_JOBS=1` inside the isolated process so an
inherited override cannot bypass the service thread budget; ordinary CLI environment
configuration is preserved. Actual process thread peaks and upstream admitted-lane
diagnostics are verified separately for both entry points.

Cold preparation of the shared resource cache keeps archive SHA-256, exact per-file
length/content SHA-256, file count, relative paths and regular-file checks. Every
successfully written chunk participates in verification; after all pass, a complete
marker is written and the cache is published atomically, using a platform-detected
SHA-256 backend. A publisher does not reopen a library it just verified; existing and
concurrent-winner caches still verify actual library/executable bytes, and unverified
or incomplete resources are never loaded.

Shared snapshot, quota, cancellation and complete-response rules are in the
[MCP design](mcp-design.md); the decision is [ADR 0007](../adr/0007-typed-mcp-service.md).
CLI output and exit codes are unchanged by the MCP projection.
