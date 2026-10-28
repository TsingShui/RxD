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
| Emitted method-source bytes, including required nested definitions | 36,105,587 |

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

`method:0x6af27` (`SandboxFinder.handleRequest`) emits 652 bytes. Its source has
two distinct `requestHandler` reads, an explicit call receiver and native await;
its empty-value guard is structured rather than state-dispatched. ABC and HAP
requests produce byte-identical source. Source SHA-256:
`d22aab7bb11f53665d582bb60f55185d693adf9082c696ee645a7004777f5ed7`.

No input or reconstructed code was executed. Coverage, decoding, static
inspection and parseability are not a runtime-equivalence proof. There is no new
all-method string-content audit or release-latency claim. Performance measurement
and new tests were intentionally not added; the operator requested real-sample
implementation work instead.

## Protected-region milestone

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
