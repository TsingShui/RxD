# Standalone rewrite validation

This evidence exercises **`rxd-ark`**, not `rxd-abc` or the production CLI.
The rewrite is not yet a complete replacement.

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
| Successful method reconstructions | 13,256 |
| Explicit method failures | 826 |
| Successful module records | 122 / 376 |
| Independent Babel method parses / failures | 13,256 / 0 |
| Independent Babel record parses / failures | 122 / 0 |
| Emitted method-source bytes, including required nested definitions | 35,596,525 |

Nine auxiliary records have no module initializer. The remaining unsupported
methods group as follows; failures do not produce source or invoke the old engine:

| First unsupported operation | Methods |
| --- | ---: |
| `defineclasswithbuffer` | 233 |
| `throw.ifsupernotcorrectcall` | 229 |
| `supercallthisrange` | 190 |
| `starrayspread` | 114 |
| `creategeneratorobj` | 35 |
| for-in normalization | 11 |
| `supercallspread` | 9 |
| `ldsuperbyname` | 5 |

`method:0x6af27` (`SandboxFinder.handleRequest`) now emits 511 bytes. Its source
retains two distinct `requestHandler` reads, an explicit call receiver and await;
ABC and HAP requests produce byte-identical source.

No input or reconstructed code was executed. Coverage, decoding, static
inspection and parseability are not a runtime-equivalence proof. There is no new
all-method string-content audit or release-latency claim. Performance measurement
and new tests were intentionally not added; the operator requested real-sample
implementation work instead.

## Exception-state hardening milestone

The fixed sample exposed a concrete dead-store error in `method:0x6ea73` of
`record:0x6e94d`: instruction PCs 89 and 149 store `true` into `v12`, then PCs
93 and 153 throw to handler PC 237. That handler reads `v12` at PC 239 to select
iterator cleanup. Previous output omitted both writes. Current output preserves:

```js
v12 = true;
throw acc;
```

The analysis now joins handler liveness at explicit throw terminators, excluding
only the accumulator replaced by the exception. Protected-region selection also
requires any earlier-priority handler to be a nested region; otherwise it retains
residual dispatch. The pinned runtime uses the first matching encoded entry.

The 13,256 method successes, 122 record outputs, 826 method failures and 254
record failure identities are unchanged. Independent Babel parsing passed for
all outputs. The correction changes 93 root sources; normalized static root
string/member/call inventories remain unchanged. These counts do not prove
runtime equivalence or exception-path correctness for arbitrary bytecode.

The selected method has byte-identical ABC/HAP output, and an unsupported record
still fails with empty stdout. The bytecode PCs and before/after sources are
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
