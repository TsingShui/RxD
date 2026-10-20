# Repository Guidelines

## Project Structure & Module Organization

`rxd` is an Agent-oriented, multi-format analysis CLI. Read the relevant
`docs/design/*-design.md` before changing a backend's behavior. The Rust
backends cover JavaScript (Oxc), ASAR, HTML, Android APK/DEX, JVM class/JAR,
.NET IL, WebAssembly, and native EVMole EVM bytecode analysis. CPython bytecode
has bounded embedded source reconstruction and static navigation for CPython
3.8–3.13. Kuna has an optional directly linked CLI adapter and bundled specs
(ADR 0006). PyInstaller/zipapp containers and binwalk integration are planned.

The current layout is:

- `src/main.rs`: entry point and isolated analysis worker process.
- `crates/rxd-kuna-cli/`: generated library adapter for the unchanged Kuna CLI.
  Use `scripts/format-workspace.py`; `cargo fmt --all` also traverses upstream
  path dependencies.
- `src/query/`: shared typed query catalog, bounded result sink and backend
  projections; `src/mcp/`: optional stdio protocol, immutable targets and
  disposable analysis-worker scheduling (ADR 0007).
- `src/cli/mod.rs`: top-level dispatch; `src/cli/js/`, `src/cli/asar/`,
  `src/cli/dex/`, `src/cli/jvm/`, `src/cli/dotnet/`, `src/cli/kuna/`,
  `src/cli/evm/`, `src/cli/wasm/`, and `src/cli/pyc/`: peer command adapters.
  JavaScript discovery is in `src/cli/js/recursive.rs`. DEX preserves streaming
  and exit-code contracts.
- `crates/rxd-core/`: content identity, output protocols, resource budgets,
  diagnostics, and phase timings (`src/perf.rs`).
- `crates/rxd-js/`: parsing (`src/lib.rs`), structure extraction
  (`src/structure.rs`), decoded literals (`src/strings.rs`), on-demand lexical
  references (`src/refs.rs`), and original source extraction.
- `crates/rxd-asar/`: ASAR index and on-demand raw file extraction.
- `crates/rxd-dex/`: APK/DEX analysis, Manifest and Java emission. DEX CLI
  details are in `docs/design/dex-design.md`; the shared typed service is
  provided by `src/mcp/`.
- `crates/rxd-jvm/`: class/JAR parsing and JVM stack front end; lifts bytecode
  into the vendored radguard backend IR and renders Java through it.
- `crates/rxd-dotnet/`: managed-assembly metadata, IL and C# reconstruction via
  the ILSpy NativeAOT shared library (see
  `docs/adr/0009-dotnet-ilspy-v11.md`). Its pinned validation fork lives in
  `crates/rxd-dotnet/vendor/`.
- `crates/rxd-evm/`: pinned EVMole Rust analysis, explicit raw/hex inputs,
  inferred interfaces/storage/CFG and metadata; design and navigation gaps in
  `docs/design/evm-design.md`.
- `crates/rxd-wasm/`: WASM module structure, instruction decode, explicit
  reference scanning, and self-built structured C-like decompilation; uses a
  pinned wasmparser (`REFERENCE.md`).
- `crates/rxd-pyc/`: bounded PYC/marshal parsing, code/constant identities,
  original instruction FETCH, explicit references and embedded pycdc source
  reconstruction; `third_party/pycdc/` retains provenance and local safety
  patches.
- Maintained third-party dependency forks and source snapshots (radguard,
  axmldecoder, pycdc) live under the top-level `third_party/`.
- `apps/arxd/` is archived: Arxd is maintained in the standalone private
  repository `TsingShui/Arxd`. Do not make product changes here.
- `tests/fixtures/` and `tests/cli.rs`: handcrafted inputs and CLI contract
  tests; testing guidance is in `tests/AGENTS.md`.
- `tests/corpus/`: pinned production releases, obfuscated samples, independent
  parser expectations, and the WABT wasm-decompile differential suite;
  provenance and generation instructions are in `tests/corpus/README.md` and
  per-backend corpus READMEs.
- `benches/`: per-backend end-to-end benchmarks (`cli.py`, `dex.py`,
  `dotnet.py`, `jvm.py`, `asar.py`, …) and measurement/perf contracts in
  `benches/README.md`.
- `docs/design/`: one `*-design.md` per backend plus the app, MCP and
  external-tools contracts, scope, command semantics, output contracts,
  resource constraints, and validation.
- `docs/design/mcp-design.md`: transport, snapshot and scheduling contracts.
- `docs/demo/`: one `*.md` of runnable examples per backend.
- `site/`: static GitHub Pages site (`index.html`, `examples.html`,
  `arxd.html`, `roadmap.html`, `changelog.html`, and `zh/` counterparts) served
  by `.github/workflows/pages.yml`; no build step.

Keep each backend's design changes in its `docs/design/*-design.md` and update
affected examples in the matching `docs/demo/*.md`. Keep cross-backend
architecture in this file and `docs/adr/`; there is no separate root-level
design document.

Build binaries, benchmark reports, and demo outputs belong under ignored
`target/`, not in source or documentation directories. Curated, verified
runtime bundles under `prebuilt/` are the only exception; keep their manifests,
source/package identities and licenses, but never SDKs, caches or
intermediates. See `prebuilt/README.md`.

## Build, Test, and Development Commands

- `cargo build`: compile the complete development binary with default
  `all-feature` (all backend namespaces, MCP and embedded ILSpy; Kuna remains
  explicitly optional). Initialize `third_party/kuna` for Cargo path
  resolution; curated ILSpy bundles need no SDK or preparation env. Use
  `--no-default-features` for reduced builds.
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
  distributed builds use `mcp,bundled-kuna,bundled-ilspy`.
- `python3 scripts/format-workspace.py --check`: check Rust formatting.
- `cargo clippy --workspace --all-targets -- -D warnings`: check lint warnings.
- `cargo build --release`: build for performance measurements.
- `python3 benches/cli.py`: build release and benchmark the production/obfuscated
  corpus.

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
installation/update, `mcp install`, skill installation/printing,
configuration/preview, help/version, standalone MCP startup/discovery/target
lifecycle, maintenance, and build/test/release orchestration are exempt.
Classify the operation, not its namespace: a backend query invoked through MCP
retains its backend contract, while registering MCP does not.

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

`rxd evm` is capability-first and has no numeric latency gate. Preserve
correctness, identities and resource budgets; EVM measurements are
informational unless a concrete regression is being investigated.

## Query Surface

Every listing command takes `-f/--filter`: a case-insensitive substring over the value
the row is named by (class, method or field name, or the literal). Backend-specific
exact selectors sit next to it and narrow the same listing (`--class`, `--type`,
`--method`), and `--at` is the only way to select a row that already exists. Two
spellings for the same question are a defect, not a preference.

Original instructions versus reconstructed source is one flag, `--raw`, on every
backend that can emit both; `--bytecode`, `--il` and future spellings are not separate
flags.

## Coding Style & Naming Conventions

Use `rustfmt`, four-space indentation, `snake_case` functions/modules, and
`UpperCamelCase` types. Keep parsing within backend crates. Preserve original
UTF-8 spans, deterministic ordering, and each backend's
`docs/design/*-design.md` output contracts. Write diagnostics to stderr.

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

## Deobfuscation Scope

Deobfuscation and string decryption are permanently out of scope for every
backend. For JavaScript this covers obfuscator transforms and string decryption;
for WASM and Python it covers control-flow deflattening and deobfuscation. Never
implement or plan them, or list them as pending capabilities. Obfuscated inputs
remain useful for testing original structure queries and byte-exact extraction.
Standard literal/immediate decoding (JS escapes; WASM LEB128, floats, memarg;
etc.) is parsing, not decryption.

Structured decompilation (DEX→Java, JVM→Java, IL→C#, WASM→language-neutral
C-like) is an established capability, not deobfuscation: it recovers
already-structured control flow and expressions, never unscrambles deliberately
obfuscated code. Whole-function/whole-class decompilation follows the two
decompilation exception grounds in Performance Requirements.

## Input Safety

Treat analyzed files as untrusted data. Never execute input code or instantiate
input modules. Parse and extract from one fixed source buffer, validate
content-bound IDs, and enforce explicit resource budgets.

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

### Domain docs

Use root `GLOSSARY.md` and `docs/adr/`; read the relevant decision records before
changing architecture. Follow the Agent-only design principle.
