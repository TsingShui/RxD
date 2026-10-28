# Ark semantic-IR validation

`rxd ark` and its reflected MCP tools use **`rxd-ark`**. Older milestone evidence
below predates the production cutover. Approximations carry source-local warnings;
unmodelled operations and input-safety failures remain explicit errors.

## Fixed input and observed coverage

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
| Method queries / record outputs with any source warning | 1,407 / 303 |
| Emitted method-source bytes, including required nested definitions and warnings | 65,133,201 |

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

## Best-effort class, generator and iteration expansion

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
