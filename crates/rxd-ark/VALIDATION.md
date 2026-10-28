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
| Emitted method-source bytes, including required nested definitions | 37,294,887 |

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
