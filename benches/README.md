# CLI benchmarks and performance instrumentation

**Scope:** the 150 ms gate applies only to existing decompilation/analysis backend
queries. Installation, configuration, `mcp install`, skill management, help/version,
standalone MCP discovery/lifecycle, maintenance and build/test/release orchestration are
not gated, and code changes there do not require latency benchmarks or numeric
exceptions. Runners and historical timings may remain as optional diagnostics; their
`>150ms` counts are not acceptance failures. Functional, safety, completeness and
resource-budget checks are never exempt. See `AGENTS.md` for the authoritative scope.

The root Agent-skill installer is measured with
`python3 benches/skill.py --binary /path/to/release/rxd --samples 20 --output target/benchmarks/skill`.
The runner does not build; the caller supplies and freezes the release executable. It
measures first install, replacement of synthetic old content, repeated identical
content, print and `all` four-target install in isolated HOME/CODEX_HOME, including
fresh processes, complete stdout/stderr and real file writes. Target setup is outside
timing; OS caches are not cleared. Raw samples, median, nearest-rank P95, >150 ms share,
skill bytes/hashes, executable/embedded-source identity, hardware and cache conditions
are saved. Installer measurements do not imply analysis-backend acceptance.

Benchmarks use Python 3.9+ standard library and start the real CLI. Input JavaScript is
parsed, never executed; no unit tests or in-memory AST microbenchmarks are substitutes.

## Running

```sh
# Build release --locked, then use the pinned production/obfuscated corpus.
python3 benches/cli.py

# JSX/TS/TSX and real published declaration files.
python3 benches/cli.py --corpus tests/corpus/typed/manifest.json --samples 20

# One obfuscated case.
python3 benches/cli.py --case application-rc4

# Authored size tests (supplementary).
python3 benches/cli.py --sizes 1 --syntax tsx --samples 20
python3 benches/cli.py --sizes 1 10 50

# Quick validation; use more samples for a report.
python3 benches/cli.py --sizes 1 --samples 3 --profile-samples 1

# Save a comparable independent run.
python3 benches/cli.py --samples 20 --output target/benchmarks/baseline

# Authorized real bundle; the query must hit an extractable structure.
python3 benches/cli.py --input /path/to/bundle.js --needle '/api/login' \
  --output target/benchmarks/real-bundle
```

The default corpus is `tests/corpus/manifest.json`: pinned jQuery/Lodash/Vue
production files plus minified, renamed/escaped, control-flow/string-array and RC4
samples, each queried with its actual string scenario. `--corpus PATH` replaces the
manifest, `--case NAME` selects a scenario and repeatable `--input` supplies files.
`--corpus`, `--input` and `--sizes` are mutually exclusive. `--syntax auto|js|jsx|ts|tsx`
overrides syntax for all CLI queries; authored inputs then include type annotations or
JSX, and the report records the override. Provenance, licenses and independent Acorn
expectations are in [tests/corpus/README.md](../tests/corpus/README.md).

Authored size inputs use ~2 KiB functions with long strings, nested arrows and locals;
only one function contains the target marker. They test scale and repeated parse cost,
not real business bundles. Reports save generator version, exact bytes, structure count
and SHA-256.

By default each scenario records one initial sample, then one warmup and seven measured
samples, plus three `--perf` samples. Every call creates a fresh CLI/analysis process.
Generated files and prepared IDs warm the file cache; the script does not clear OS
caches, and the initial sample is not a cold-file-cache claim. P95 uses nearest rank;
below 20 samples it equals the maximum.

Coverage includes `outline`, `find`, `locate`, raw `get`, single-file `refs --byte`,
`symbols`, `symbols --within`, raw/JSON symbol declaration get and its containing
structure, raw/JSON local-fragment get, find → context get, applicable `find → get`,
and `rg -boF`. Preparation validates real IDs and original fragments; outline/find/
locate check completeness, structure counts or entity identity; later samples check
output consistency. Full workflows include ID decoding, routing and two process calls,
and are also reported as sums of command time. RC4 scenarios expect zero plaintext hits
and mark find→get not applicable; structural queries use independent entity
expectations and are not described as recovering business functions. `rg` searches raw
text and returns offsets; `find` searches decoded literals and returns structures; they
are not equivalent queries.

`refs_byte` uses independent Acorn identifier positions/references from the adjacent
`refs-expectations.json` and checks resolved state, name, ranges and read/write flags;
authored inputs use generator-known local parameters. Custom inputs without refs
expectations mark the scenario not applicable rather than guessing. Typed manifests use
independent Babel expectations for TS type/value reference flags; an unused parameter
in a declaration file may legitimately return zero and must not be interpreted as a
type-check result. `symbols` checks independently selected declarations/names;
`symbols --within` uses actual extracted structure IDs and checks JSONL completeness,
order, identity uniqueness and repeat consistency, not a general symbol oracle.

Input/record/output/depth budget overflow keeps a failure record and returns non-zero;
limits are never silently raised. `--max-input-bytes`, `--max-entities`,
`--max-records`, `--max-output-bytes`, `--max-ast-depth` and `--max-semantic-nodes`
(default 1,000,000, passed to refs/symbols/get; get only indexes symbol paths) are
reported. Each call has a 120-second timeout (`--timeout`).

`get_symbol/get_symbol_json/get_symbol_parent` use independent Acorn declarations from
`definitions-expectations.json`: raw symbol, declarator, kind and initializer ranges must
match exactly. Authored inputs use a generator-known marker; custom inputs without
expectations are not applicable. Reports record declaration bytes, containing-structure
bytes and reduction ratio. The JS production sample deliberately chooses the largest
containing/declaration ratio among unique 32–2048-byte bindings to demonstrate extraction
benefit; it is not an average task or token claim. Typed manifests use the first
initialized identifier declaration; without one the scenario is not applicable. All get
paths still parse, validate and output completely without a resident index.

`get_context/get_context_json/find_context_get` use independent Acorn local ancestor
ranges/parents from `contexts-expectations.json`, choosing the first literal hit;
zero-hit corpora use independent parameter positions only for get_context and do not run
find_context_get or recover encrypted literals. Authored inputs use a known marker's
declarator and variable declaration; custom inputs only check returned span/byte
consistency and the report states the verification source. Workloads are timed
separately; the full find_context_get includes two fresh processes, ID routing and real
output. Reports also list fragment and containing-structure raw bytes for each selected
sample, not average task/token benefit.

## 150 ms design gate

PYC decompilation: `python3 benches/pyc.py --input /path/to/module.pyc` (default
`--command decompile`). Static code navigation uses `--command outline`, constant search
`--command strings`, and literal/hex filters with repeated
`--query-arg=--contains --query-arg=URL`. Name references use `--command refs
--query-arg=--name --query-arg=node`; constant XREF uses `--command refs
--query-arg=--const-id --query-arg="$CONST_ID"` where CONST_ID comes from the same
input's strings response. The strings→refs→get-code workflow is timed separately
including three fresh processes, JSON parsing and returned-ID routing. `--input` is
repeatable and `--source` records origin. Each file is prepared once, then 20 fresh
CLI/worker samples are checked for completeness/output consistency. Raw samples,
median/P95, >150 ms share, input/output hashes, hardware and cache conditions go to
`target/benchmarks/pyc/results.json`. Fixtures are regression diagnostics only;
acceptance uses sourced real modules and never treats pycdc upstream measurements as
rxd results.

JVM: `python3 benches/jvm.py` measures pinned Gson 2.11.0 and Rhino 1.7.15 JARs after
`cargo build --release`; the script does not install/run Java tools. Each workload has 30
fresh CLI/worker samples covering whole-JAR outline, selected-method Java/original
bytecode and real atomic file output. Reports, samples, input/binary hashes, hardware and
cache conditions go to ignored `target/benchmarks/jvm/`; samples are never pooled across
workloads.

Within backend scope, ordinary representative release CLI queries must complete within
**150 ms** in most cases. This is complete fresh-process wall time: process startup,
worker initialization, reading, analysis, serialization and real output, plus directory
discovery and the whole scan for directory queries. Report per-workload raw samples,
median, P95 and >150 ms share with hardware, input size and cache conditions. Never hide
slow queries with tiny samples or cross-scenario averages, and never treat authored
inputs as production. Special cases need explicit scope, measurements and approval;
decompilation exceptions follow the two grounds in
[AGENTS.md](../AGENTS.md#performance-requirements).

Scripts save raw samples, median/P95 and per-scenario
`over_150ms_count/over_150ms_proportion`; Markdown lists both count and share. The
threshold is `samples[].duration_ns > 150000000`. Exit 0 means the run and contract
checks succeeded, **not** that 150 ms acceptance passed. `--timeout` is an execution
guard, not the design threshold; ordinary functional E2E adds no timing assertions.

## Reports

Default output is ignored `target/benchmarks/latest/`:

- `results.json`: raw samples, phase statistics, failure diagnostics, argv, tool
  versions, platform/CPU, start/end load average, binary/Cargo.lock hashes, Git HEAD and
  worktree state.
- `results.md`: latency, output size, memory and phase tables.
- `synthetic-*.js`: replayable inputs.

Headline latency includes real serialization and temporary-file output with
`--perf` off. Phase diagnostics are sampled separately and never pooled with
headline samples. `--binary PATH` skips the build and marks the report `custom`; the
caller must verify provenance. Preparation, input generation, result checks, hashing and
report generation are outside single-command wall time; full workflows include routing
and output capture.

On macOS/Linux, `wait4` records the largest **single-process RSS high-water mark** in the
waited tree, not the sum of simultaneously resident parent/children. Separate perf data
gives CLI and analysis RSS but they must not be added. Other platforms report null.
Units are bytes: Linux `ru_maxrss` is KiB; current Darwin XNU uses
`resident_size_max` bytes.

## Perf contract

```sh
target/release/rxd js find tests/fixtures/example.js --string '/api/login' \
  --perf > result.jsonl 2> perf.jsonl
```

`--perf` writes two JSONL records to stderr, `scope: analysis` and `scope: cli`, with
independent `perf_version: 1`. It never changes query stdout, schema, ordering or exit
code and are the only timing channel. Ordinary errors also emit perf; abnormal process
termination may leave only the CLI record; argument-parse failures and help/version emit
none.

Each record has backend, command, pid, status, error_code, `stages_ns`, `total_ns`,
`peak_rss_bytes` and counters. Analysis also reports source_id and effective resource
budgets.

| Analysis stage | Included cost |
| --- | --- |
| read | Open, file type/size checks, fixed-buffer read |
| hash | SHA-256 and content identity encoding |
| utf8 | UTF-8 validation |
| parse | Mode selection, arena creation, Oxc parse |
| visit | Entities, names, parent/child relations, find literal matching |
| semantic | Only refs/symbols/get symbol paths: node precheck, scope bindings/reference index; no CFG/JSDoc/class table |
| query | ID validation, structure/binding selection, filtering, sorting, ancestor collection |
| serialize | JSON/JSONL serialization; raw get copies the source buffer |
| write | Analysis stdout, or temporary file plus atomic replacement |

CLI stages are `args`, `spawn_wait` and `forward`. `spawn_wait` includes child
initialization, analysis, pipe collection, diagnostics and exit; it is not added to
analysis total. Analysis total includes cleanup and costs outside named stages; CLI total
runs from main entry to forwarding completion. Neither includes its own perf
serialization/printing or pre-main process start; external wall time covers the whole
command.

Counters `records` excludes JSONL header/summary; refs counts reference records;
symbols counts symbol plus unresolved_reference. Only refs/symbols/get symbol paths fill
semantic_nodes/semantic_symbols/semantic_references, representing whole-file index
counts, not references to the selected binding; other commands use null and no semantic
stage. With `-o FILE`, analysis output_bytes is the file content and CLI output_bytes is
0. Unknown counters are null on failure. EVM default get-function adds nullable
reconstructed_instructions/blocks/contexts, reconstruction_values/work,
structured_contexts/edges and structuring_work; other backends/commands/raw use null.
`reconstruct` accumulates lower/render; `structure` is separate and non-overlapping. PYC
decompile fills counters.work/ast_depth and resource_limits; other backends null.
Instrumentation is off by default and reads no clock per node/record.

## ASAR

```sh
python3 -B benches/asar.py --samples 20 --cpu-model 'Apple M3 Pro' --output target/benchmarks/asar
```

Builds release by default. It uses pinned jQuery/Lodash/Vue production JS plus explicitly
artificial binary bytes to generate an ordinary ASAR and one with 100 fake 1 GiB entries.
Inflated entries follow asarmor `createBloatPatch` offset/size shape without payloads and
do not run the protection tool or sample. It measures list and extract of each original
file, checks all index entries/range_valid/range_error, byte-identical output, receipt
SHA-256 and atomic write, and measures extract → js find (two fresh processes, receipt
decode/routing) against independent expectations.

Each workload is validated/warmed once, then 20 samples with instrumentation off; three
`--perf` samples are separate. Real atomic file writes and stdout receipts count in child
wall time; checks run after timing. Generated/prepared files warm OS caches; caches are
not cleared. Reports keep raw samples, median/P95, >150 ms share, input/output bytes,
origin, hashes, hardware and cache conditions. `--binary` skips the build and marks
custom; `--cpu-model` records a separately verified host CPU. Outputs go to ignored
`target/benchmarks/asar/` with app.asar, bloat.asar, prepared files, results.json and
results.md. The corpus is an artificial container, not a real protected application;
real apps, very large archives, installers and other packing schemes need separate
validation.

ASAR perf uses backend `asar`: container handles the header, read/hash the target bytes,
serialize the metadata, write the atomic file plus receipt. Extract
analysis.output_bytes includes file plus receipt; CLI.output_bytes is receipt only;
`list -o` reports zero CLI output. input_bytes excludes the index header. There is no
AST/semantic/directory-recursion stage.

## HTML

```sh
python3 -B benches/html.py --samples 20 --cpu-model 'Apple M3 Pro' --output target/benchmarks/html
```

Builds release by default; `--binary` skips and marks custom. Inputs include authored
ordinary pages (document, entities/duplicate attributes, inline script), generated
large/markup-dense pages (20,000 flat elements, 500 levels), and pinned real npm pages
when `tests/corpus/node_modules` is installed (`tslib` 2.8.1, `terser` 5.51.2
`tools/props.html`, `string-template` 1.0.0, `md5` 2.3.0; versions from
`tests/corpus/package-lock.json`, per-file SHA-256), labeled corpus/authored/synthetic.
`node_modules` is gitignored (`npm ci`); when absent the script records a skip and tests
only authored/synthetic pages. The repository embeds no production HTML and uses no
network; pages are not executed and external resources are not loaded. It measures list
and small-script `--content` extract, checks header identity, lexical_profile, closed
record fields, ID/container hash, receipt SHA-256 and atomic write, and measures
list → returned ID → extract → rxd js outline (three fresh processes, ID routing, one
atomic write).

Each workload is validated/warmed once, then 20 samples with instrumentation off; three
`--perf` samples are separate. Reports keep raw samples, median/nearest-rank P95,
>150 ms share, input/output bytes, origin, hashes, hardware and cache conditions.
Ordinary representative list/extract must meet the 150 ms fresh-process requirement;
tokenizer microbenchmarks are not acceptance. Outputs go to ignored
`target/benchmarks/html/` with pages, prepared files, results.json and results.md. Pinned
npm pages are real package files but not a complete production site; authored/synthetic
pages are not real protected markup.

HTML perf uses backend `html` with read/hash/utf8/tokenize/query/serialize/write.
Extract analysis.output_bytes includes artifact plus receipt; CLI.output_bytes is the
receipt; `list -o` reports zero CLI output.

## DEX

```sh
python3 -B benches/dex.py --input /authorized/app.apk --class com.example.Main ...
```

Uses explicitly supplied authorized APK/DEX inputs, fresh CLI processes and per-query
samples, median/P95, >150 ms share, input hashes/sizes, hardware and cache conditions.
It covers ordinary index/reference/manifest/member queries and whole-class
decompilation separately. rasc schema-v1 byte comparisons are not used for schema-v2
JSON. Whole-class decompilation exceptions need a scoped input hash, selected target,
release measurements and approval; ordinary index/reference queries are not exempt.

## .NET

Build the pinned NativeAOT engine with `benches/ilspy/build-native.sh` (or use a verified
`prebuilt/ilspy` bundle), then `cargo build --release --features csharp-decompiler`.
Cargo never calls dotnet. Set `RXD_REFDIR` to trusted .NET 10.0.12 framework metadata
when using an external library; see [ilspy/README.md](ilspy/README.md). Current local
measurements are macOS-only.

- `benches/dotnet.py` / `benches/dotnet_ilspy.py` measure the actual CLI and pinned
  v11.1.0.9782 engine with at least 20 fresh CLI/worker processes per workload. They
  check original IL/EH, retain C# and syntax-sugar evidence, and record
  median/P95/>150 ms, input/output sizes, input/binary/dylib hashes, hardware and cache
  conditions. WindowsBase Accessibility cross-assembly degradation is a correctness
  scenario, not C# latency evidence.
- `benches/dotnet_strings.py` validates complete string navigation against independent
  `#US`/PEReader expectations, unfiltered and filtered, with stdout/atomic-file variants
  and a separate three-process strings → method/class IL workflow.
- `benches/dotnet_refs.py` uses an independent selective PEReader method/field/type
  operand oracle for raw and semantic XREF, stdout and atomic files, with nonzero
  complete site sets and schema-v2 output.
- `benches/dotnet_coverage.py` samples methods at even MethodDef-token intervals and
  records C# success/degradation reasons; success rate is not semantic correctness.

Reports pin binary/input identity, every sample, median/P95/>150 ms, sizes, hardware and
warm-cache conditions. Whole-class decompilation exceptions follow the two grounds in
[AGENTS.md](../AGENTS.md#performance-requirements); the exact scope and bound must be
recorded. Expected fallback and resource failures never count as successful latency
evidence.

## PYC

`benches/pyc.py` covers decompile/outline/strings/name refs/constant refs/get-code with
repeatable `--input` and `--query-arg`. `benches/pyc_versions.py` consumes the pinned
six-version compile-only stdlib pack, audits independent source/PYC/raw/marshal oracle
hashes, and checks original instructions/operands/cache/line/exception evidence per code
body. It measures outline/strings/name refs/returned-ID small-target FETCH and existing
decompile, with at least 20 fresh CLI/worker samples. `benches/pyc_closure.py` covers all
30 actual stdlib modules, full/selected scopes and three returned-ID workflows, and
requires a frozen release binary plus build-provenance JSON. `--smoke` tests one
email-policy per format and is explicitly not full acceptance. Complete response token
accounting requires a separately pinned tiktoken 0.12.0 environment and `o200k_base`
cache; `--tokenizer-provenance` records package/encoding/cache hashes. Token accounting
is not a Codex tokenizer or billing claim.

Every workload keeps raw samples, median/nearest-rank P95, >150 ms count/share,
input/output/binary hashes, hardware/cache conditions, target scales/perf and tokens.
Command and workflow timings are separate; workflow totals add process launches, JSON
routing and residual harness cost with no aggregate numeric bound. Failures/slow samples
return non-zero while other rows continue; known incomplete source retains its own
stdout/stderr/exit semantics and never counts as successful complete recovery.

## MCP

`benches/mcp.py` measures frozen executables through actual CLI processes and a
standard-library stdio client at the same public seam as MCP E2E. It never imports or
executes analyzed inputs. Supply build-provenance JSON whose `binary_sha256` matches the
executable; record source revision/manifest, Cargo features, Rust/profile, engine/spec
hashes and explicit native/reference paths. Binaries are copied before sampling and
their identities, provenance, hardware/cache/load conditions and raw complete
stdout/stderr/JSON-RPC artifacts are saved under ignored `target/`.

```sh
python3 benches/mcp.py --binary /path/to/frozen-release/rxd \
  --build-info /path/to/frozen-release/build-info.json \
  --baseline /path/to/frozen-prior-release/rxd \
  --corpus-root "$PWD" --dotnet-input /path/to/pinned/Newtonsoft.Json.dll \
  --include-csharp --samples 20 --cpu-model 'Apple M3 Pro' \
  --memory-bytes 38654705664 \
  --conditions 'Warm OS files/engine cache after preflight; no cache eviction; no CPU isolation; other builds stopped.' \
  --output target/benchmarks/mcp-release
```

The built-in selection pins production jQuery/Lodash/Vue, Gson/Rhino and one actual
`email.parser` module from each final CPython release. A supplied Newtonsoft.Json
13.0.3 DLL adds independent PEReader string navigation; `--include-csharp` requires
successful native C# for the small JsonReader TokenType getter and verifies original
IL/EH. Missing engine fallback is a retained failure. Additional actual modules/queries,
public authorized APK/WASM artifacts and native Kuna are supplied through repeated
`--plan` JSON; no private APK or native binary is auto-discovered. Plans declare name,
backend, input, sha256, synthetic flag, provenance, oracle, query, workflow and source
scale. Workflow steps select IDs from actual returned results; plans are hashed.

Every workload has at least 20 retained samples for fresh CLI, server
startup+initialization, complete typed capability discovery (`tools_list`), first
direct-path query, and actual fresh start+initialize+discovery+first-query wall time,
plus open and repeated queries against one captured target. Discovery samples retain the
complete actual `tools/list` response and exact wire bytes. `workflow_fresh_mcp_total`
includes a new server process, initialization, discovery, route selection, open, actual
returned-ID query chain and close. `workflow_target_total` is the resident
already-initialized/opened workflow. Reports use `benchmark_version: 2`; older reports
that omitted discovery remain historical subtotals, not full fresh Agent cost. CLI
verification stays outside the timer and must match the same preflight oracle.

Concurrency uses an explicit request count (default two, matching worker slots), reports
whole-batch wall time and every response, and retains admission failures. A nominal 10 ms
`ps` sampler records server+descendant tree RSS on Unix as a sampled observation, not an
exact high-water mark or hard bound. Windows RSS is unavailable, never zero; sampling
load is disclosed. Complete MCP response bytes include structuredContent, JSON
compatibility copy and protocol framing; backend-only serializer timings cannot
substitute.

Failures/timeouts retain artifacts and return non-zero. `>150 ms` samples remain in raw
statistics but do not change runner exit status: **a successful run is not automatic
latency acceptance**. Inspect per-workload median, nearest-rank P95 and >150 ms share
without pooling unrelated inputs. Opening a large archive/directory is a distinct
capture workload; existing CLI exemptions grant no implicit MCP capture/workflow
exception. Synthetic ASAR/DEX/WABT modules stay labeled and cannot replace production
measurements. EVM remains informational with no numeric gate.

## EVM

`benches/evm.py` uses two immutable release executables and records final source
SHA-256 manifest, tracked diff, binary/Cargo.lock/Kuna pin, input/output hashes,
hardware, load, cache, argv, per-sample wall time, median/P95/>150 ms share and separate
perf stages/counters. Every command/workflow starts a fresh CLI/worker; workflow totals
include ID parsing, routing, evidence checks and the sum of command wall time. EVM is
informational only with no numeric gate.

```sh
python3 benches/evm.py --binary target/benchmarks/evm-final/rxd-final \
  --baseline target/evm-baseline/rxd-baseline --samples 20 \
  --output target/benchmarks/evm-final
```

The runner does not compile/deploy/execute input; it reads only the pinned corpus.
Authored viaIR/legacy inputs are synthetic solc 0.8.26 compiler output; OpenZeppelin is
a pinned 5.1.0 published library-source runtime and is not deployed mainnet bytecode.
It checks seven native EVMole cases × three samples × raw/hex exact exit/stdout/stderr,
actual returned-ID outline/SEARCH/XREF/default FETCH workflows, and that unrelated
padding does not change selected raw instructions/reconstruction or selected
reconstruction/structuring counters. Whole-input navigation and selected reconstruction
costs are recorded separately.
