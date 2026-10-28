# Repository Guidelines

## Project Structure & Module Organization

`rxd` is an Agent-oriented, multi-format analysis CLI. The Rust backends cover
Android APK/DEX (`rxd rasc`), JVM class/JAR (`rxd jar`), .NET IL (`rxd ilspy`),
WebAssembly (`rxd wasm`), CPython bytecode (`rxd pyc`), Lua/LuaJIT/Luau
bytecode (`rxd unluac-rs`) and native EVMole EVM bytecode (`rxd evmole`). Outer command names credit the engines a backend wraps;
crates, modules, features and `backend` record labels keep their original names. CPython bytecode has bounded embedded source reconstruction
and static navigation using the embedded parser's recognized formats. The same
`rxd pyc` queries read PyInstaller PYZ directly, with module-qualified coordinates
and module/code-body reconstruction; standalone PYC remains supported.
`modules` reads the PYZ directory; `codes --within MODULE` parses only that module. Kuna has an optional directly linked
CLI adapter and bundled specs. ASAR, ZIP (including HAP) and PyInstaller archives
are unpacked into a directory. `rxd ark` can also read a selected dynamic ABC
directly from HAP; it does not analyze the other archive payloads.

The current layout is:

- `src/main.rs`: entry point and isolated analysis worker process.
- `crates/rxd-kuna-cli/`: generated library adapter for the unchanged Kuna CLI.
  Use `scripts/format-workspace.py`; `cargo fmt --all` also traverses upstream
  path dependencies.
- `src/query/`: CLI-reflected typed query catalog and backend projections;
  `src/cli/output.rs`: explicit stdout/buffer output sink.
  `src/mcp/`: optional stdio protocol, immutable targets and
  disposable analysis-worker scheduling.
- `src/cli/mod.rs`: top-level dispatch; `src/cli/dex/` (`rasc`), `src/cli/jvm/`
  (`jar`), `src/cli/dotnet/` (`ilspy`), `src/cli/kuna/`, `src/cli/evm/`,
  `src/cli/wasm/`, `src/cli/lua/`, `src/cli/flutter/`, and `src/cli/pyc/`: peer
  command adapters.
  `src/cli/table.rs` renders the shared TAB-separated listing rows.
  `src/cli/unpack.rs` performs whole-archive ASAR/ZIP/HAP/PyInstaller extraction.
  DEX preserves streaming and exit-code contracts.
- `crates/rxd-core/`: content identity, output protocols, resource budgets,
  diagnostics, and phase timings (`src/perf.rs`).
- `crates/rxd-ark/`: dynamic Ark ABC 12.0.6.0 parsing and navigation, semantic IR,
  value/control analyses and direct Oxc AST emission. Class/super, generator,
  spread and enumeration bridges mark approximations in source; no old-engine fallback.
- `crates/rxd-asar/`: ASAR index and whole-archive file extraction.
- `crates/rxd-zip/`: ZIP-family (including HAP) index and whole-archive
  Stored/Deflate extraction with CRC, length, duplicate-path and path-safety validation.
- `crates/rxd-pyinstaller/`: PyInstaller CArchive/PYZ extraction with reconstructed
  `.pyc` headers and immutable PYZ indexing/member reads shared with `rxd pyc`.
- `crates/rxd-wxapkg/`: wxapkg index extraction and V1MMWX appid decryption.
- `crates/rxd-dex/`: APK/DEX analysis behind `rxd rasc`, Manifest and Java
  emission; the shared typed service is provided by `src/mcp/`.
- `crates/rxd-jvm/`: class/JAR parsing and JVM stack front end; lifts bytecode
  into the vendored radguard backend IR and renders Java through it.
- `crates/rxd-dotnet/`: managed-assembly metadata and original IL through the
  pinned `cildec` ECMA-335 decoder, and C# reconstruction via the ILSpy NativeAOT
  shared library.
- `crates/rxd-evm/`: pinned EVMole Rust analysis behind `rxd evmole`, explicit
  raw/hex inputs, selector candidates, disassembly, PUSH literals and terminal
  CBOR metadata only; no CFG, storage or decompilation surface.
- `crates/rxd-wasm/`: WASM module structure, instruction decode, explicit
  reference scanning, and self-built structured C-like decompilation; uses a
  pinned wasmparser (`REFERENCE.md`).
- `crates/rxd-flutter/`: Dart AOT snapshot navigation and pseudocode through the
  vendored `dae-rs` analyzer (`third_party/dae/`); listings read the library,
  function and pool tables, `xref` decodes instructions to find pool loads, and
  `decompile` runs the upstream renderer.
- `crates/rxd-lua/`: bounded Lua/LuaJIT/Luau chunk parsing and whole-chunk
  reconstruction through the pinned `unluac` crate (`unluac-rs`); listings read the proto tree
  without running the decompiler.
- `crates/rxd-pyc/`: bounded PYC/marshal parsing, code/constant identities,
  explicit references and whole-module pycdc reconstruction with function-local
  bytecode fallbacks; `third_party/pycdc/` retains provenance and local safety
  patches.
- Maintained third-party dependency forks and source snapshots (radguard,
  axmldecoder, pycdc) live under the top-level `third_party/`.
- Arxd is maintained in the standalone private repository `TsingShui/Arxd`;
  its former App sources are not part of this repository.
- `tests/fixtures/` and `tests/cli.rs`: handcrafted inputs and CLI contract
  tests; testing guidance is in `tests/AGENTS.md`.
- `tests/corpus/`: pinned production releases, obfuscated samples, PyInstaller, ZIP and
  wxapkg fixtures, independent parser expectations, and the WABT wasm-decompile differential suite;
  provenance and generation instructions are in `tests/corpus/README.md` and
  per-backend corpus READMEs.
- `benches/`: end-to-end benchmarks (`mcp.py`)
  the pinned ILSpy build in `benches/ilspy/`, and measurement/perf contracts in
  `benches/README.md`. The per-backend runners written for the earlier CLI
  generation were removed; a backend that needs one gets it written against the
  current surface (`entries`/`classes`/`codes`, `--at`, `-f`/`--case`,
  `--name`).
- `docs/`: user-facing `install.md`, `ci.md` and `mcp.md`, plus `docs/agents/`
  working agreements. Backend usage lives in `rxd --help` and the embedded skill;
  there are no per-backend example documents to keep in sync.
- `site/`: static GitHub Pages site (`index.html`, `examples.html`,
  `arxd.html`, `roadmap.html`, `changelog.html`, and `zh/` counterparts) served
  by `.github/workflows/pages.yml`; no build step.

Use the implementation, CLI help and focused tests as the source of truth.
Keep that help and the embedded skill current when a command changes; separate
design or decision documents are not required.

Build binaries, benchmark reports, demo outputs and native runtime bundles belong
under ignored `target/`, not in source or documentation directories. No compiled
ILSpy runtime is checked in; desktop engines are built per target and Android App
engines are produced from source by `devtools/android-ilspy/`.

## Build, Test, and Development Commands

- `cargo build`: compile the complete development binary with default
  `all-feature` (all backend namespaces and MCP; Kuna and desktop ILSpy remain
  explicitly optional). Initialize `third_party/kuna` for Cargo path
  resolution. No ILSpy runtime is checked in: run
  `python3 scripts/build-ilspy.py <rid>` and set `RXD_ILSPY_EMBED_RS` to its
  `embed.rs` when building `bundled-ilspy`. Android App engines follow
  `devtools/android-ilspy/`. Use `--no-default-features` for reduced builds.
- `cargo test --test cli`: run all-backend CLI E2E for shared product changes,
  not every commit. During backend development prefer the real focused CLI,
  for example
  `cargo test -p rxd --no-default-features --features backend-dotnet --test cli dotnet::`.
- `cargo test --release --test cli`: verify shared/release behavior; use the
  same backend feature/filter for backend-only milestones.
- `python3 scripts/ci-scope.py --base HEAD`: inspect the conservative
  changed-path CI scope (includes local tracked/untracked changes). See
  `docs/agents/testing.md` for scoped gates and validation reuse.
- `cargo test --features mcp --test cli`: include real stdio MCP contracts;
  distributed builds use `mcp,bundled-kuna,bundled-ilspy` with a native
  producer `RXD_ILSPY_EMBED_RS` and the pinned Kuna submodule.
- `python3 scripts/format-workspace.py --check`: check Rust formatting.
- `cargo clippy --workspace --all-targets -- -D warnings`: check lint warnings.
- `cargo build --release`: build for performance measurements.
- `python3 benches/cli.py`: build release and benchmark the production/obfuscated
  corpus.

On the shared interactive workstation, default local agent runs to
`CARGO_BUILD_JOBS=2` and `RUST_TEST_THREADS=2`, without overlapping build/test/lint/
benchmark jobs. Clean up owned disposable processes and rebuildable duplicate
caches after milestones while retaining source, corpus and verification evidence;
see `docs/agents/testing.md`. These controls do not add backend query quotas.

## First Principle: Fast Is All You Need

When the tool is fast enough, intelligence emerges. rxd supplies fast, precise
primitives; reasoning and judgment belong to the Agent. Cheap calls let the
Agent probe, query in parallel, and verify repeatedly. Intelligence comes from
many cheap iterations, not from one heavy call.

This principle ranks above every other guideline. When designs conflict, prefer
the one that lets the Agent reach a correct answer fastest. That means low
per-call latency (see Performance Requirements), few tool calls, and low token
cost. Never trade correctness, completeness, identity validation, or resource
safety for speed.

## Agent-Only Design Principle

All designs target Agents exclusively. Human reading needs are not requirements
and must not influence design decisions. This applies to every backend, CLI
interface, output protocol, diagnostic, example, and future integration.

Optimize for Agent task correctness, precise navigation and extraction,
deterministic machine-consumable output, low token and tool-call cost, and
latency. Do not add formatting, terminal presentation, colors, visual trees, or
alternate output modes merely for human readability. Metadata and labels must
serve Agent queries and interpretation, not presentation aesthetics; original
source stays byte-exact. Evaluate proposals using Agent workflows and
measurable contracts, never whether output is pleasant for a person to read.

## Performance Requirements

The 150 ms acceptance gate applies only to decompilation/analysis backend
operations, including navigation, search, reference, extraction and
reconstruction queries. Non-backend commands have no 150 ms requirement:
installation/update, skill printing,
configuration/preview, help/version, standalone MCP startup/discovery/target
lifecycle, maintenance, and build/test/release orchestration are exempt.
Classify the operation, not its namespace: a backend query invoked through MCP
retains its backend contract, while starting the service does not.

Do not require numeric latency exceptions, repeated timing batches or release
benchmarks solely to change non-backend commands. Their measurements are
optional unless a concrete performance regression warrants investigation.
Functional E2E, correctness, completeness, identity validation, explicit
resource budgets, timeouts/cancellation and security requirements remain
mandatory.

- Within the backend scope above, most ordinary, representative CLI queries
  must complete within **150 ms** in release builds. Reject designs that exceed
  150 ms in most common cases; performance is an acceptance requirement, not an
  optional optimization.
- Measure end-to-end wall time with a fresh CLI process, including startup,
  worker initialization, directory discovery where applicable, input reading,
  analysis, serialization, and output writes. In-memory parser timings or
  warmed resident sessions cannot substitute for CLI measurements.
- Report per-workload samples, median, P95, the proportion exceeding 150 ms,
  input sizes, hardware, and cache conditions. Use representative production
  inputs; do not hide slow workloads by pooling many tiny fixtures or averaging
  unrelated commands.
- Special workloads require documented scope, measurements, and an approved
  numeric bound. Do not silently relax the requirement or sacrifice
  correctness, completeness, identity validation, or resource safety to meet
  it. Use release benchmarks for latency acceptance, not timing assertions in
  ordinary E2E tests.
- Whole-function/whole-class/whole-module decompilation may exceed 150 ms on
  only two grounds:
  - **Third-party engine**: the time is spent inside an integrated, unmodified
    pinned upstream decompiler that rxd does not control (currently the ILSpy
    NativeAOT library). Source forks under `third_party/` (radguard, pycdc) are
    rxd-owned code and never qualify. rxd's own surrounding work (startup, input
    reading, identity validation, selection, marshalling, serialization) still
    follows the 150 ms rule; prove it with the engine's `--perf` stage.
  - **Inherently large target**: the selected function, class, or module is
    genuinely large, and the cost scales with that one target. A small target
    must not become slow, and a large input must not make small targets slow.
    Prove it with target scale (backend-native bytecode/instruction size, member
    count, and output bytes) plus an ordinary small target from the same input
    measured within 150 ms.
- An eligible ground is not an approval. Each exception records an explicit
  scope (input hash, selected target), release measurements, and its own numeric
  bound; there is no blanket ceiling. Slowness in rxd-owned code with neither
  ground is a defect. Non-decompilation special workloads use the general
  special-workload rule above, not these grounds.

`rxd evmole` is capability-first and has no numeric latency gate. Preserve
correctness, identities and resource budgets; EVM measurements are
informational unless a concrete regression is being investigated. It exposes
only `interface`, `disassemble`, `strings` and `metadata`: commands whose facts
the bytecode settles, never a control-flow graph or a recovered source ABI.

## Query Surface

Keep listing filters consistent through `rxd_core::filter`: case-folded substring
matching by default, with `--case sensitive` for exact case. Use the printed `at`
coordinate to select a unit and xref's `context` to follow a reference to its owner.
Coordinates are file-scoped; re-list when the input changes.

Every table starts with one `# ` column header, even for successful empty results.
Data rows have TAB-separated cells, no envelope and no `columns`/`rows` document.
Text cells are JSON-quoted and `\N` marks an absent value. The header counts
toward output bytes, not data rows. References use `entry, context, match` on
DEX/JVM/PYC and `context, match` on single-file backends. Container-capable
backends keep a null entry cell for standalone inputs. Do not expand these rows
with instruction detail or JSON envelopes; retain actual matched targets and full
owner coordinates. Kuna retains its upstream output contracts. Each command's long help opens with its own `Columns:`
line and the root help states the format once, so a shape change updates both.
Source reconstruction prints the source itself, never a record: `decompile`
commands emit one labelled block per unit, and `get-method`/`get-class`/
`get-function` print the reconstructed Java, C# or C-like text. `--raw` prints the
original instructions as text lines too. Only evidence that is not source
(such as class metadata under `--raw`) stays a JSON record. Do not add presentation columns,
padding or per-backend dialects.

`xref` is one command, not per-kind subcommands. Preserve argument errors for
contradictory kinds and distinguish a missing coordinate from zero search hits.
Exact options and output fields belong to the implementation, CLI help and tests.
Absent and null navigation values are equivalent. `--raw` selects original
instructions on backends that also offer reconstruction. Keep CLI help, MCP
descriptions and the embedded skill focused on usage, not repeated safety disclaimers.

## Coding Style & Naming Conventions

Use `rustfmt`, four-space indentation, `snake_case` functions/modules, and
`UpperCamelCase` types. Keep parsing within backend crates. Preserve original
UTF-8 spans, deterministic ordering, and each backend's tested output contracts.
Write diagnostics to stderr.
Keep each comment block at most two lines, including doc comments. Delete redundant
comments; retain concise contracts, non-obvious invariants and safety conditions.

### Command names, crate names and backend labels

The user-facing command credits the engine that does the work; everything
inside keeps the neutral format name. Naming a command after its engine keeps
the attribution where a user sees it, and keeping the internal name format
based stops a crate, feature or record label from changing when an engine is
swapped or added.

| Outer command | Engine or author | Crate / module / feature | `backend` label |
|---|---|---|---|
| `rxd rasc` | Rascal (APK/DEX) | `rxd-dex`, `src/cli/dex`, `backend-dex` | `dex` |
| `rxd jar` | JAR format, no engine | `rxd-jvm`, `src/cli/jvm`, `backend-jvm` | `jvm` |
| `rxd ilspy` | ILSpy | `rxd-dotnet`, `src/cli/dotnet`, `backend-dotnet` | `dotnet` |
| `rxd unluac-rs` | unluac | `rxd-lua`, `src/cli/lua`, `backend-lua` | `lua` |
| `rxd dae` | dae-rs | `rxd-flutter`, `src/cli/flutter`, `backend-flutter` | `flutter` |
| `rxd ark` | RxD dynamic Ark ABC implementation | `rxd-ark`, `src/cli/abc`, `backend-abc` | `abc` |
| `rxd pyc` | CPython format, no engine | `rxd-pyc`, `src/cli/pyc`, `backend-pyc` | `pyc` |
| `rxd wasm` | WASM format, no engine | `rxd-wasm`, `src/cli/wasm`, `backend-wasm` | `wasm` |
| `rxd evmole` | EVM format, no engine | `rxd-evm`, `src/cli/evm`, `backend-evm` | `evm` |

Apply the same rule to a new backend: pick the outer command from the engine
when one exists, keep the crate/module/feature/label on the format, and never
rename internals to match the command or add hidden aliases for the old name.

## Testing Guidelines

**Prioritize end-to-end tests at the affected observable seam, not the largest
test suite.** Backend algorithms start with the relevant public package tests;
observable backend milestones use the corresponding feature-scoped real CLI E2E
in debug/release. Shared product changes use all-backend gates.
Documentation/project skills and maintenance scripts do not trigger Rust builds
or CLI tests unless they change embedded product bytes or build behavior. A
merge alone does not require rerunning unchanged, already validated content.
Follow the scope and evidence/reuse rules in `docs/agents/testing.md`; see
`tests/AGENTS.md` for test design. Preserve coverage of extraction, Unicode,
IDs, budgets, completeness, perf and atomic failures.

Prefer pinned real production artifacts and reproducible obfuscator outputs for
representative scenarios. Keep small fixtures for boundary diagnosis. Preserve
provenance, licenses, versions, seeds and hashes; use independent parser
expectations and keep ordinary tests offline.

Be conservative about unit tests: add one only for a concrete, important
regression that E2E cannot reasonably cover, with minimal setup and maintenance.
Do not mirror implementation details, chase coverage targets, or restructure
code just to enable tests. When focused E2E checks provide sufficient
confidence, proceed with the implementation.

## Commit & Pull Request Guidelines

- Mark automatically generated commits with `[AUTOMATED]` at the start of the
  subject.
- Write a concise English imperative describing observable behavior, for
  example `[AUTOMATED] Add CLI benchmarks and phase timings`. Conventional
  Commit prefixes are optional.
- Commit completed milestones; keep unrelated changes separate and exclude
  build artifacts.
- Explain the problem, resulting behavior, and validation in the body. Include
  runnable examples and release measurements for performance claims. Never
  invent evidence or issue references.
- Before commits, pass the checks applicable to the changed scope in
  `docs/agents/testing.md`: relevant formatting/lint and tests, not
  unconditional workspace Clippy or full debug/release E2E. Record exactly what
  ran or which matching evidence was reused. Backend-analysis runtime changes
  still need relevant release benchmarks, with inputs/cache conditions and
  measurement limits. Full release and cross-platform packaging gates remain
  mandatory before publication.
- Keep PR bodies short: **The problem**, **The fix**, **The tests**. Lead bug
  fixes with a runnable repro and actual wrong output; link issues when
  available.

## Structured Decompilation

Structured decompilation (DEX→Java, JVM→Java, IL→C#, WASM→language-neutral
C-like) recovers existing structured control flow and expressions.
Whole-function/whole-class decompilation follows the two decompilation exception
grounds in Performance Requirements. Standard literal/immediate decoding
(WASM LEB128, floats, memarg; etc.) is parsing. Obfuscated inputs remain useful
for testing original structure queries and byte-exact extraction.

## Input Safety

Treat analyzed files as untrusted data. Never execute input code or instantiate
input modules. Parse and extract from one fixed source buffer, validate
coordinates and byte ranges against that input, and enforce explicit resource budgets.

Ordinary CLI metadata/index admission grows with input instead of fixed total
quotas. Work, instruction count, individual expanded-entry bytes and serialization
have no input-derived quotas; allocation failures remain explicit. Preserve format/address validation, recursion/cycle/expansion guards
and explicit failure; never substitute an unlimited sentinel. MCP uses the same
input-scaled analysis policy and adds configurable service-wide snapshot/in-flight
byte admission, worker/thread concurrency, deadlines and Native worker RSS protection.
It has no separate file/entry/target counts or target TTL. Storage envelopes are not
hard RSS/time limits.

## Agent skills

### Upstream maintenance

Use the project skill `.agents/skills/check-upstream/SKILL.md` to check upstream
updates and assess maintained forks/Rust adaptations. Start with
`python3 scripts/check-upstream.py`; see `devtools/upstream/README.md` for scope,
statuses and separate review cursors. Discovery never changes build pins, and
upstream changes do not imply that all of them should be ported.

### Issue tracker

Issues and specs live in GitHub Issues for `TsingShui/RxD`. Use `gh` with an
explicit `--repo TsingShui/RxD`; pull requests are not a request surface.
Publish or comment only when authorized.

### Domain vocabulary

Use root `GLOSSARY.md` for domain vocabulary. Follow the Agent-only design principle.
