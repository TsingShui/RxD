# Ark semantic-IR validation

`rxd ark` and its reflected MCP tools use **`rxd-ark`**. Older milestone evidence
below predates the production cutover. Approximations carry source-local warnings;
input-safety failures remain explicit errors. All pinned dynamic opcode encodings
now have reconstruction paths; this is not a universal runtime-equivalence claim.

## Searchable export, method search and string references

`rxd ark export INPUT -o DIR` writes the original container tree, one standalone JS
module per code-bearing record, and a JSONL manifest; `--extract all|abc|none` selects
the original tree, `--threads` reconstructs records in parallel, `--entry` narrows a
HAP to one ABC, and each file opens with its record and method name/coordinate header.
ABC entries are read from the file index first and reconstruct while the remaining
extraction runs (at most four I/O workers). Batch export is a special workload, not an
ordinary 150 ms query. Measured on Darwin arm64 release builds, 12 logical CPUs, warm
caches, whole command including extraction, reconstruction, writes and manifest:
ClashBox HAP (107,951,762 bytes, 152 archived files) produced 376 record files, 14,082
coded methods, 23,880,029 source bytes and a 303-record warning set in 0.19–0.25 s wall
for `all` and 0.20–0.30 s for `abc`/`none`; the deflated Harflix HAP took 0.10–0.11 s
for `all` and 0.040–0.045 s for `abc`/`none`. The edgemusic HAP (2 ABCs, 322 records, 6,758 methods)
produced byte-identical trees at `--threads 1` and `--threads 8`. Copi's
`record:0x8220` exported 36 coded methods, including methods that
`decompile --at record:0x8220` does not reach, so export does not silently drop
unreferenced code. All 698 record files (322 edgemusic, 376 ClashBox) parsed with
Babel with no unbound generated identifiers; 303 ClashBox records carried warnings,
matching the earlier 303 warned record outputs. After stripping the name header,
ClashBox `record:0x283a8` and Copi `record:0x2290` bodies were byte-identical to
`decompile --at record`. All extract modes emitted identical `decompiled/` trees and
manifests; `abc` extracted only the selected ABCs and `none` wrote no `extracted/`.
Extracted ClashBox files matched Python `zipfile` payload hashes; `--entry` with
`--extract abc` wrote only that ABC. The ClashBox tree had recursive SHA-256
`63e0b8c4…` and its ABC SHA-256 matches the fixed input. Existing directories and a
bare ABC with `--entry` fail with `INVALID_QUERY`; a missing entry fails with
`NOT_FOUND` and removes the new directory. Corrupting one byte of a Stored or Deflate
entry failed with `INVALID_INPUT: ZIP entry CRC mismatch` for every extract mode and
worker count, leaving no partial directory.

`rxd unpack` now accepts `--threads` and decodes Stored/Deflate entries from their
declared spans with libdeflater (≤64 MiB compressed; larger entries stream). ClashBox
extraction took 97–125 ms at 1 worker, 71–86 ms at 4 and 99–103 ms at 12; the deflated
Harflix HAP took 109–119 ms, 90 ms and 52 ms (previously 151 ms read plus 48 ms write).
Tree hashes matched across worker counts; all nine Unix unpack E2E checks passed with
the default parallel path, including duplicate, unsafe-path, link and pinned
production archives. The default all-backend CLI suite (debug) reported 71 passed and
5 failures identical at the parent commit `415d4e2` (Flutter help columns, two PYC
cases, a .NET resource-policy case and a WASM coordinate case); none exercises
`rxd-zip`, unpack or Ark, so they are unrelated pre-existing failures.

`list method` without `--at` searches every record and reports the owning `record`
column; ClashBox lists 14,082 rows and amcl 21,841 rows in 40–70 ms. `strings` lists
`str:0x...` values named by decoded instruction operands, and `xref` returns one
`context, match` row per method/string pair. Both decode all methods and scan in
parallel with `Abc::fork()`: forks share the immutable indexes and input slice while
caches stay per worker, so parallel scans do not re-parse or copy the ABC. Worker
setup must stay O(indexes): rayon's `map_init` invokes its initializer per split, not
per thread. Before `fork`, the export initializer re-parsed the 7.2 MB ABC hundreds of
times (6.0–6.6 CPU-seconds for ClashBox); explicit per-chunk forks reduced that to
0.55–0.59 CPU-seconds, while serial cost (0.41 CPU-seconds) is unchanged. ClashBox
(7.2 MB, 8,640 strings) took 52 ms for `strings` and 51–52 ms for `xref` at 12
workers (96 ms and 92 ms at `--threads 1`). The largest measured input, amcl
(7,028,500 bytes, 21,841 methods), took 77 ms for `strings` and 64–70 ms for `xref`
at 12 workers; serial values were 151 ms and 139–146 ms. Every tested query stays
under 150 ms in the default parallel configuration; `--threads 1` can exceed it on
that largest input. Exact-set checks against a raw-instruction scan matched `xref`
for `--at` and `-m` on Copi (741 strings) and ClashBox (8,640 strings), HAP and bare
ABC produced identical bytes, real MCP `abc_strings`/`abc_xref` results matched CLI
bytes, and an unreferenced `--at` coordinate fails with `NOT_FOUND`.

HAP queries now use the same file-backed pattern as `rasc`: index the archive and
inflate only the selected ABC. `ark list record` on the 107,951,762-byte ClashBox HAP
fell from 121,208,832 to 11,190,272 bytes peak RSS (bare ABC: 12,910,592); the worker
read stage dropped from 12.2 ms to 0, and analysis total from 30.5 ms to 23.2 ms. HAP
and bare-ABC bytes remained identical for `list record`, `list method`, `strings`,
`xref` and `decompile`; a symlinked HAP still produced identical output through the
fallback, and ambiguous/missing-entry errors were unchanged. `rxd zip unpack` and Ark
export already read archives from the file rather than a whole-buffer copy; the
ClashBox HAP is entirely Stored, so inflate speed is not the export bottleneck.

No new Rust tests were added; CLI help, the embedded skill and MCP documentation
describe the CLI-only export and the search commands. Evidence is under ignored
`target/verification/ark/export-test/` and `target/verification/ark/ark-search/`.

## Whole ABC reconstruction

`rxd ark decompile INPUT` emits every coded method in each code-bearing record;
multiple records use scoped imports and namespaced exports with explicit warnings.
A single ordinary record keeps its existing module form. Non-scalar module names
use UTF-16-encoded aliases and warned namespace-property snapshots. Raw whole-ABC
fetch, exact HAP entry selection, and the same optional selector through MCP are
available without changing record/method coordinates.

Static inspection covered 17 production ABC entries: 2,024 records and 73,879 coded
methods all appeared once in their record section. Babel parsed each section; cross-
section bindings and export names were checked for collisions. Debug outputs matched
final release hashes. The original ClashBox sample contained 376 sections and 14,082
methods with the same static checks, and its final release output matched the inspected
source. Separate whole-program Babel parses, CLI/MCP parity, output-budget checks,
late-error atomicity, orphan methods and non-scalar names were also checked. No analyzed
or reconstructed code was executed; this does not establish runtime equivalence.
Performance measurement remains paused. Evidence is under ignored
`target/verification/ark/whole-abc/`.

At the operator's request, the 16 Ark CLI tests and one Ark MCP test referenced in
historical milestones below were removed; no new Rust tests were added. The unused
MCP comparison and schema helpers were removed as requested. Native MCP scenarios
still check schema presence but no longer validate output against that schema. Strict
Clippy passed for the default and Ark/MCP all-target configurations; the optional
bundled-Kuna compile check used a synthetic embed and reports two upstream warnings.

## Independent GitHub applications and version expansion

Fourteen independently sourced releases contain 17 ABC entries: 12.0.2.0,
12.0.6.0, 13.0.1.0 and 24.0.0.0. Exact release URLs, package/ABC hashes and
selection details are in ignored `target/verification/ark/github-generalization/`.
All 17 entries now admit and reconstruct **73,879/73,879 coded methods** and
**2,024/2,024 source records**; 176 additional metadata records have no module
initializer. Babel parsed all emitted methods and records, found no unbound
`__ark_`/`_ark_capture_` names, and per-method source warning kinds matched typed
warnings. 11,614 method queries and 1,693 source records contain warnings. These
are full static coverage counts for this corpus, not runtime-equivalence evidence.

The global class list no longer requires every record to also occur in its region's
reference index. Header literal-array indexes can be present or reserved. The
line-index table range is validated, and each method's referenced debug target is
checked when its instructions are requested; unrelated methods and unused stale
debug entries are not traversed for that query.
The three apps with out-of-range *unreferenced* debug entries have no bad referenced
entries. On a modified MeshArc file, a damaged debug target in method B leaves an
unrelated method A in the same record byte-identical for source and raw queries;
method listing succeeds, while requesting B fails atomically. Writable implicit
registers now use initialized local shadows, while VM `ldthis` keeps its
separate frame receiver and `ldnewtarget` reads its actual register. FinVideo's
five formerly panicking records and all 4,486 methods now emit source.

The newer reference ISA has 332 encodings: its 323 earlier encoding signatures
and operand formats are unchanged, and nine were added. Named calls preserve the
callee, receiver, arguments and diagnostic name with a warned JS bridge; forward-all
super calls retain the actual argument list; sendable local module access carries a
source-local timing/sharing warning. The 332 individual encoding paths generated
and fetched raw/source successfully and parsed statically. Unknown or upstream-
incompatible version labels are not refused merely for their version; they carry
`ARK_VERSION_SEMANTICS` in source. Unknown operations, unsupported static formats,
invalid references and resource exhaustion remain explicit errors.

The original 12.0.6.0 ClashBox fixed sample remains 14,082/14,082 successful methods
and 376/376 source records; every method source is byte-identical to the previous
milestone. A second full sweep of all 17 new ABCs matched the first emitted sources
byte-for-byte. Eighty-five selected list/source/raw cases matched in 340 debug/release
CLI processes across original HAP and bare ABC; 34 real MCP source/raw results matched
CLI, and multi-ABC selection remained explicit. Existing focused Ark CLI/MCP checks
passed in debug and release (17 each); package tests (zero package/doc tests), strict
package and product-bin Clippy, opcode generation, formatting and whitespace checks
passed. Reduced-feature test builds retain existing unused-helper warnings.

No analyzed or emitted code was executed. Performance measurements remain paused,
and no new Rust tests were added. Detailed results and scripts are under ignored
`target/verification/ark/version-broadening/`.

## Fixed input and observed coverage (historical baseline)

ClashBox v1.7.4 `ets/modules.abc`, 7,203,484 bytes, SHA-256
`a2046b75ebd368e0599a50b2b5e5c6dab8070007364af5a09d67d267582c197c`.
The enclosing HAP is 107,951,762 bytes, SHA-256
`9977a4b9e40c96e2fcb7831340f43829a25f95c5aa5ffe7addb811992cfdfd0e`.

Dev-profile full-input scan:

| Observation | Result |
| --- | ---: |
| Records admitted | 385 |
| Methods with code | 14,082 |
| Successful method reconstructions | 14,082 |
| Explicit method failures | 0 |
| Successful module records | 376 / 376 |
| Independent Babel method parses / failures | 14,082 / 0 |
| Independent Babel record parses / failures | 376 / 0 |
| Method queries / record outputs with any source warning | 1,412 / 303 |
| Emitted method-source bytes, including required nested definitions and warnings | 65,250,568 |

Nine auxiliary records have no module initializer. Before the best-effort expansion,
the 826 method failures had these first errors; all now produce source, without
calling the removed engine or substituting empty bodies:

| First unsupported operation | Method queries |
| --- | ---: |
| `defineclasswithbuffer` | 233 |
| `throw.ifsupernotcorrectcall` | 229 |
| `supercallthisrange` | 190 |
| `starrayspread` | 114 |
| `creategeneratorobj` | 35 |
| `supercallspread` | 9 |
| for-in normalization | 11 |
| `ldsuperbyname` | 5 |

`method:0x6af27` (`SandboxFinder.handleRequest`) now emits 528 bytes. Its source
retains two distinct `requestHandler` reads, an explicit call receiver and await.

No input or reconstructed code was executed. Coverage, decoding, static
inspection and parseability are not a runtime-equivalence proof. Static source
comparisons do not prove general semantics. Performance measurement and new
tests were intentionally not added; the operator requested real-sample
implementation work instead.

## Complete 12.0.6.0 opcode surface (historical)

All **323 encodings / 259 names** in the retained dynamic 12.0.6.0 ISA now have
lowering paths. BigInt/regex, rest/template/iterator operations, private fields,
sendable/lazy bindings, legacy register forms and async generators are represented
by typed IR and directly built Oxc ASTs. Canonical async normalization remains the
preferred path; a rejected optimization now retains explicit promise/resume state
with `ARK_ASYNC_FRAME` instead of refusing the method. Unclassified function kinds,
multiple generator identities, extra comparison branches and typed/overlapping
catch regions each have explicit source warnings describing their chosen model.
Private/sendable/legacy support does not use a generic opcode-dispatch placeholder.

A real-process per-encoding scan independently generated checked ABC containers,
verified each first raw opcode name, and obtained source with empty stderr for all
323. Babel parsed all of them and found no unbound generated identifiers. Fourteen
additional combinations exercised private initialization/access, sendable layouts,
rest/template operations, async-generator yield/await, noncanonical async frames,
multiple identities, unknown function kinds, arrow rest arguments, colliding and
non-scalar global names, explicit legacy definition environments, overlapping typed
catches and accessor literals. All produced source and expected warning markers;
all parsed with no unbound generated identifiers. These inert fixtures were not
executed; they validate reconstruction paths, not runtime outcomes or all possible
opcode combinations.

The full fixed production scan still reconstructs 14,082 methods and 376 source
records. Every source parsed and its source-warning kind set matched the typed
warning vector. 13,780 method sources were byte-identical to the preceding expansion;
302 changed with generalized class literal metadata, generator bridges or retained
runtime notification semantics. No successful target disappeared.

The current output agreement is the shared format merged at `fa2a5fd`: listings
start with one `# ` TAB-column header even for successful empty results; text cells
are JSON-quoted. Source and raw FETCH stay plain, with approximation warnings only
as source comments. Ark continues using `src/cli/table.rs` without a private format.
74 selected CLI results, including ordinary/empty record/method tables and source/raw
results, matched between debug/release and ABC/HAP; 21 real MCP results matched CLI.

No new Rust tests were added. Existing assertions expecting now-supported inputs to
fail were updated; checksum/index/region/MUTF8/HAP safety and atomic errors remain
unchanged. The latest focused CLI/MCP suite has 17 checks after the separate shared
output-format milestone, and passes in debug and release. Reduced-feature test
builds emit existing unused-helper warnings. Package Clippy and product-bin Clippy
pass with `-D warnings`; a broader all-targets attempt stopped on the unrelated
unused `failure` import in `tests/dex/mod.rs` from the shared output-format change.
That unrelated file was not changed here. Formatting and whitespace checks pass.

Evidence and generators are retained under ignored
`target/verification/ark/all-opcodes/`. At that milestone static typed-array/static
ABC formats, other versions, invalid references, cyclic expansion and explicit
resource failures remained outside successful reconstruction. Reflection, host lexical globals, VM sendability
and rare coroutine/iterator semantics remain warned approximations. Nothing here
claims exact behavior for every valid program. Analyzed and reconstructed code was
not executed, and performance measurement remains paused.

## Best-effort class, generator and iteration expansion (historical)

The operator requested reconstruction in the new engine, including imperfect
structure, with approximation warnings at the affected source sites. Class shells
are native Oxc class ASTs; bodies retain explicit constructor/home-object contexts.
Generators retain bytecode resume dispatch in a native suspended frame with an
explicit next/throw/return driver. Spread preserves target writes and insertion
indices; for-in uses suspended native enumeration. State dispatch is retained
rather than guessing an unproven high-level structure. None is an empty placeholder.

The final full-input audit restored all 826 previously failing methods and 254
records. Babel parsed every source; scope traversal found no unbound generated
identifiers, and source warning kinds matched the typed diagnostic vector. Static
checks confirmed that warnings precede affected operations. Removing warning lines
and the added home-object context field made all 13,256 previously successful
method sources byte-identical to the cutover baseline.

An independent read of the pinned instruction traces and class-literal bytes
matched all 351 class definitions (constructor, lengths, member names/identities,
instance/static partition), 3,519 method definitions, 2,658 accessor definitions,
141 array spreads, 48 apply calls, 166 synchronous-generator suspensions and nine
property-enumeration creations in their root method outputs. The 653 suspensions
in async functions remain normalized into await by the existing async front end.
These counts and syntax checks do not prove runtime equivalence.

Source warnings explicitly cover VM iteration fast paths, enumeration timing and
mutation, external class markers, method constructibility/reflection/error details,
generator identity and borrowed methods, and unrecovered original control structure.
Unknown instructions/literal layouts, invalid input and exhausted budgets still fail;
100% coverage here is for this fixed sample, not every dynamic ABC program.

Validation on Darwin arm64, Rust/Cargo 1.98.0, with two build/test threads:

- Existing feature-scoped Ark CLI/MCP checks passed in debug and release: 18 each.
  Only obsolete own-property refusal expectations were changed; no tests were added.
- 56 real CLI queries matched across debug/release and ABC/HAP for selected classes,
  derived constructors, generators, spread, nested for-in and an unaffected async
  method; seven real MCP source results matched CLI bytes. Source stderr was empty;
  raw output contained no warnings. Input-safety checks remained passing.
- Package/doc-test commands passed (zero package/doc tests); strict default-feature
  `cargo clippy -p rxd-ark -p rxd --all-targets -- -D warnings`, formatting and
  whitespace checks passed. Reduced-feature CLI builds retain existing unused
  helper warnings, not new product warnings.

Evidence and independent scripts: ignored `target/verification/ark/coverage-next/`.
No analyzed input or reconstructed program was executed. Performance measurement
remains paused; there is no new latency claim or approval.

## Production cutover (historical)

The old crate was removed; `backend-abc` now selects `rxd-ark`. CLI and MCP use
identical source bytes, including an `ARK_ITERATOR_SEMANTICS` JS comment directly
before each affected iterator operation. Raw instructions have no source warnings.
The typed library warning remains available; selected source queries do not emit
warnings on stderr. Existing CLI checks were updated for the new emitter without
adding new tests.

The cutover debug audit produced 13,256 successful methods and 122 records. Its
success and failure identities matched the preceding iterator milestone. Babel parsed all 13,256 method sources
and 122 record sources; static traversal found 312 and 61 annotated iterator
operations respectively, with each warning immediately before its call. Removing
only the warning comment lines made every output byte-identical to that baseline.
Additional static fixture checks covered direct return, local write and unused
iterator results. The final audit after the async admission change was byte-identical
to the verified cutover scan. These are not runtime-equivalence proofs.

Real CLI checks compared bare ABC and HAP listings, selected source/raw output,
atomic `-o` success/failure, and nested unsupported failure with empty stdout.
MCP source and raw output for selected ABC/HAP targets matched CLI bytes; nested
failure retained its method ID.
During migration an existing fixture exposed a branch into the return following
`asyncfunctionresolve`, bypassing promise resolution. The new engine rejects that
branch, other unpaired async returns and handlers entering protocol interiors.
The fixture now fails with empty stdout and `DECOMPILE_FAILED` at PC 3.

On Darwin arm64 with Rust/Cargo 1.98.0, `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2`:

- `cargo test -p rxd-ark` passed (no package or doc tests).
- `cargo test -p rxd --no-default-features --features backend-abc,mcp --test cli abc::`
  and the same command with `--release` passed: 18 checks each, including MCP.
  The reduced-feature build emitted existing unused-test-helper warnings.
- `cargo clippy -p rxd-ark -p rxd --all-targets -- -D warnings`, workspace
  formatting, `git diff --check` and the opcode generator check passed.
- `cargo check -p rxd --no-default-features` passed with existing dead-code warnings.

Evidence is under ignored `target/verification/ark/cutover-*`. No analyzed or
reconstructed code was executed and performance measurement remains paused.

## Standard JavaScript iterator milestone (historical)

At that milestone, `getiterator` followed the selected standard JavaScript semantics: read the
iterator property once, call it with its original receiver, then require an object
or function result. Runtime-specific errors, implementations and mutable globals
can differ; this is not a claim of exact behavior across every Ark VM. Each
successful method or record reconstruction whose source includes the operation
returns the typed `ARK_ITERATOR_SEMANTICS` warning with its source. The audit CLI
writes one warning line to stderr per selected query, without modifying source
stdout. Its full-sample mode writes one warning for the whole invocation and
records affected method IDs in JSONL and record offsets in the summary.

Against the preceding strict-failure milestone, all 195 method queries and 26
record queries previously refused solely for `getiterator` now succeed. The 826
method and 254 record failure identity sets match the pre-refusal baseline.
All 13,256 successful method outputs and 122 record outputs independently parse.
An AST-guided comparison against the pre-refusal baseline matches all method and
record outputs after accounting for the explicit iterator helper and the two
previously corrected exception messages. This static comparison does not prove
runtime equivalence or arbitrary input behavior.

`method:0x6ea73` and `record:0x6d77a` each emit source and exactly one warning;
`method:0x6af27` emits no warning. The former retains both throw-path `v12` stores,
and the latter retains both property reads, receiver and await. A nested for-in
query still fails with empty stdout and identifies its actual method. ABC/HAP
results and diagnostics match for these and selected other targets.

Evidence is under ignored `target/verification/ark/standard-iterator-final-*` and
`standard-iterator-compare.cjs`. Scoped Clippy, formatting and whitespace checks
passed. No new tests, analyzed-code execution, performance measurements or
production CLI changes were made.

## Runtime-semantics hardening milestone (historical)

The pinned ordinary interpreter returns a non-callable `Symbol.iterator` property
from `RuntimeGetIterator`, while the pinned ASM interpreter throws at the same
opcode through `StubBuilder::GetIterator`. Both are present in the same runtime
revision; the ABC and ISA do not select an interpreter mode. At that milestone,
`getiterator` failed explicitly rather than selecting either interpretation. In
`record:0x6e94d`, `method:0x6ea73`, the failure was at PC 16. The current
contract instead selects standard JavaScript semantics and emits a warning.

`definemethod` also fails explicitly until its home object and length can be
modelled. In this sample all 117 methods containing it fail earlier on class
construction, so the new guard was not exercised by a successful sample query.
The `throw.undefinedifholewithname` and `throw.ifnotobject` messages now match
the pinned runtime; string concatenation preserves the original UTF-16 name.
Per-method analysis and function lowering errors include the failing method
coordinate, even for a nested dependency. For example, `method:0x757e6` reports its
for-in dependency as `method:0x75802`.

At that milestone, 195 method queries and 26 record queries failed on
`getiterator` in a target or required dependency; no prior failure became a
success. Three previously failing queries reported `getiterator` as their first
error instead of for-in. The 13,061 remaining method outputs and 96 record
outputs parsed. An AST-guided projection of the previous outputs, changing only
the two expected error messages, was byte-identical for every surviving method
and record output; 4,467 method queries and 56 records changed. The 195 failures
were matched to actual `getiterator` PCs in their reported method.
ABC/HAP parity holds for selected success and failure queries, including
`method:0x7c07f`, `method:0x6ea73` and the nested for-in case.

The final audit is byte-identical to the initial hardening audit for all method,
record and audit outputs; its Babel and projection evidence was therefore reused.
The ordinary interpreter, ASM stub source, raw bytecode, scripts and reports are
retained under ignored `target/verification/ark/runtime-semantics-final-*` and
`runtime-semantics-reference-stub_builder.cpp`. The latter SHA-256 is
`b9360961050f7dd939da3cf4e27a6b3704264bb44262ac11397106cf3d172c38`.
Scoped Clippy, workspace formatting and whitespace checks passed. No new tests,
analyzed-code execution, performance measurements or production CLI changes were
made.

## Effect-order hardening milestone (historical)

A concrete order-sensitive case is `method:0x6e6b3` in `record:0x6e613`.
Bytecode executes `ldobjbyvalue` at PC 50 before `stobjbyvalue` at PC 53;
the corresponding keys are both copies of the same argument. Prior output was:

```js
t11[p0] = t13[p0];
```

Current output snapshots the read at its instruction before the store:

```js
const t14 = t13[p0];
t11[p0] = t14;
```

For an object-valued key, coercion may be observable. ECMAScript 2024 and 2025
specify different key-coercion timing for a simple computed assignment; the old
shape is therefore not universally wrong, but the snapshot avoids depending on
that version difference. See `REFERENCE.md` for the specifications and runtime.

Nested ordered IR definitions now materialize at their original events unless
there is a single direct local-write or terminal-value sink with no surviving
intervening ordered operation. The scheduling rules do not treat calls, getters,
coercions, lexical loads or suspension points as pure. This change does not prove
general runtime equivalence; generated intrinsic bridges still rely on JS globals.

All 13,256 successful methods and 122 record outputs independently parse; the
826 method and 254 record failure identities remain unchanged. A root-only syntax
scan finds 32 dynamic-key assignments with a potentially effectful RHS in 29
methods before this change and none afterward. This is a conservative syntactic
pattern count, not 29 proven runtime bugs. Normalized static root string/member/
call inventories match; dynamic keys, local callee aliases and constructor
identities are grouped and cannot prove order or equivalence.

The conservative rule changes 9,744 root sources: root `const tN` declarations
increase from 83,954 to 124,348 and roots with PC dispatch from 443 to 533.
Complete method-query source grows from 35,596,525 to 37,797,317 bytes, including
required nested definitions. This is an explicit correctness-first tradeoff, not a
latency measurement. At that milestone, `handleRequest` retained both reads,
receiver and await; `method:0x6ea73` retained both throw-path stores; descriptor
copy remained explicit. The selected dynamic-key method matched between ABC and
HAP, and an unsupported record failed with empty stdout.

Evidence is under ignored `target/verification/ark/effect-order-final-*`. Scoped
Clippy, formatting and whitespace checks passed. No new tests, analyzed-code
execution or performance measurements were added. Production CLI is unchanged.

## Exception-state hardening milestone (historical)

The fixed sample exposed a concrete dead-store error in `method:0x6ea73` of
`record:0x6e94d`: instruction PCs 89 and 149 store `true` into `v12`, then PCs
93 and 153 throw to handler PC 237. That handler reads `v12` at PC 239 to select
iterator cleanup. The original output omitted both writes; the corrected output
at that milestone preserved:

```js
v12 = true;
throw acc;
```

The analysis now joins handler liveness at explicit throw terminators, excluding
only the accumulator replaced by the exception. Protected-region selection also
requires any earlier-priority handler to be a nested region; otherwise it retains
residual dispatch. The pinned runtime uses the first matching encoded entry.

At that milestone, the 13,256 method successes, 122 record outputs, 826 method
failures and 254 record failure identities were unchanged. Babel parsing passed for
all outputs. The correction changes 93 root sources; normalized static root
string/member/call inventories remain unchanged. These counts do not prove
runtime equivalence or exception-path correctness for arbitrary bytecode.

At that milestone, the selected method had byte-identical ABC/HAP output. It was
later rejected on `getiterator` and is now reconstructed with a warning. The
bytecode PCs and before/after sources are
retained under ignored `target/verification/ark/safe-flow-v1-*`. Scoped Clippy,
formatting and whitespace checks passed; no new tests, execution of analyzed
code or performance measurements were added. Production CLI remains unchanged.

## Value-flow milestone (historical)

A worklist joins must-equal parameter snapshots, primitive constants and boolean
facts across blocks. Exception handler input is joined at each potentially throwing
IR event, with the exception replacing the accumulator. Rewrites use a parameter
binding only while its current fact still matches the original snapshot. Captured
lexical/module cells, heap reads, calls and coercions are not propagated as copies.
This is not general cross-block SSA or arbitrary phi construction.

Structured single-write diamonds become conditional assignments. Boolean joins can
merge into a subsequent branch only when the slot is dead on both outgoing paths.
Raw root-only Babel metrics, excluding nested definitions:

| Observation | Protected regions | Value flow |
| --- | ---: | ---: |
| Empty then with nonempty else | 2,618 | 2,463 |
| `const tN` declarations | 87,550 | 83,954 |
| `Reflect.apply` calls | 29,968 | 29,968 |
| Hole checks | 8,132 | 8,132 |
| Native non-forwarding catch roots | 292 | 292 |
| Roots with PC dispatch | 443 | 443 |

All 13,256 successful methods and 122 source records parse independently; the
826 method and 254 record failure identities are unchanged. A normalized static
root inventory of strings, member operations and calls matches the prior milestone
for all methods. Numeric/dynamic member keys and local callee aliases are grouped;
this check does not prove identity, evaluation order, exception state or runtime
equivalence. 2,768 root sources changed. No input/reconstructed source was executed.

| Target | Protected regions | Value flow |
| --- | ---: | ---: |
| `0x6af27` handleRequest | 652 B / 34 lines | 511 B / 23 lines |
| `0x2ab35` calculateFontSizeLinear | 816 B / 38 lines | 750 B / 30 lines |
| `0x8f0f8` getResourceString | 1,230 B / 58 lines | 998 B / 41 lines |
| `0x6b316` checkFileExists | 548 B / 25 lines | 527 B / 23 lines |

Selected sizes are complete method queries including necessary nested definitions.
The two requestHandler reads, receiver and await remain explicit; HAP/ABC output
matches, and an unsupported record still fails with empty stdout. Evidence and
samples are under ignored `target/verification/ark/value-flow-final-*`. Scoped
Clippy and formatting passed; no new tests or performance measurements were run.
The production CLI remains on the existing engine.

## Protected-region milestone (historical)

The same input was scanned through the updated engine with unchanged method and
record success/failure sets. IR value/elimination counts and owned-block totals
also match the initial rewrite. Root-only Babel metrics exclude dependency
factories and nested helpers:

| Root observation | Initial rewrite | Protected regions |
| --- | ---: | ---: |
| Roots containing PC dispatch | 671 | 443 |
| Exception roots containing PC dispatch | 429 | 201 |
| Exception roots containing entry-PC dispatch | 429 | 105 |
| Roots with native, non-forwarding catch | 0 | 292 |
| Synthetic exception-forwarding catches | 6,608 | 2,306 |
| Total catch clauses | 6,608 | 2,678 |
| Native finally clauses | 0 | 0 |

The 429 exception roots include compiler protocols, not just source-level user
catches. Complex/shared-handler and multi-exit shapes still retain dispatch.
Finally is intentionally not fabricated: existing cleanup paths and call counts
remain explicit pending completion-aware reconstruction.

All 13,256 method outputs and 122 record outputs independently parse. Root
literal/member/call multisets match the baseline for all methods; all 423 changed
roots previously contained catch. Every root without catch is byte-identical.
These static checks do not prove execution order or runtime equivalence.

Selected complete-query sizes (including their necessary nested definitions):

| Target | Before | After |
| --- | ---: | ---: |
| `0x6b316` checkFileExists | 1,034 B / 50 lines | 548 B / 25 lines |
| `0x29dfa` getJsonArrayType | 4,350 B / 197 lines | 1,822 B / 75 lines |
| `0x7cbee` parseYAMLInBackground | 902 B / 40 lines | 618 B / 25 lines |
| `0x7cf43` requestIpInfo | 5,126 B / 212 lines | 2,963 B / 108 lines |
| `0x6af27` handleRequest | 652 B | 652 B, unchanged |

ABC/HAP checkFileExists output is byte-identical. Unsupported BuildProfile record
reconstruction still exits nonzero with empty stdout. The pinned runtime's
encoded-table handler priority was checked against 2,737 protected segments in
the fixed sample; the former length-order heuristic happens to agree there, but
has been removed from lifting rather than assumed for other inputs.

Evidence is under ignored `target/verification/ark/try-regions-v1-*`, with
`exception-handler-order.json` and the pinned runtime `references/ets_runtime/method.cpp`.
The full scan used the command below with fresh `try-regions-v1-methods.jsonl`
and `try-regions-v1-records` paths. Scoped Clippy and formatting passed; no new
tests or release benchmarks were run, as requested. The production CLI remains
on its existing engine.

## Reproduction

Use fresh output paths; the audit refuses to overwrite its JSONL or directory:

```sh
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo run --quiet \
  -p rxd-ark --example audit -- /tmp/hapsample/ets/modules.abc \
  --sources target/verification/ark/rewrite-methods.jsonl \
  --records target/verification/ark/rewrite-records

CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo run --quiet \
  -p rxd-ark --example audit -- /tmp/hapsample/clashbox.hap \
  --at method:0x6af27

node tests/corpus/abc/check-source-syntax.cjs \
  /tmp/xpanda/node_modules/@babel/parser \
  target/verification/ark/rewrite-methods.jsonl \
  target/verification/ark/rewrite-syntax.json
```

Babel comes from the pinned xpanda checkout documented in `REFERENCE.md` and is
used only for independent parsing. Each record was also parsed as an ES module.
`cargo tree -p rxd-ark --depth 1` shows no `rxd-abc`, JS parser or old reconstructor.

Applicable formatting and strict scoped Clippy passed:

```sh
python3 scripts/format-workspace.py --check
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo clippy \
  -p rxd-ark --all-targets -- -D warnings
```

Whitespace diff checking excludes the byte-exact upstream ISA YAML, which
retains three upstream trailing spaces; its pinned SHA-256 is checked instead.

Detailed audit JSON, sources, syntax reports, dependency listing, sample and
source/lock hashes are retained under ignored `target/verification/ark/rewrite-*`.
