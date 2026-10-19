# PYC navigation and decompilation

`rxd pyc decompile` and `outline/strings/get-code/xref` for final CPython 3.8–3.13 are
implemented. Repository PYC fixtures are offline; analysis never runs their code and
does not need a Python interpreter.

```sh
cargo build
target/debug/rxd pyc decompile tests/fixtures/pyc/hello.3.12.pyc
target/debug/rxd pyc decompile tests/fixtures/pyc/hello.3.12.pyc --json
target/debug/rxd pyc decompile tests/fixtures/pyc/hello.3.12.pyc -o target/recovered.py
```

`-o` must have a different input identity: an absolute/relative or linked alias back to the
analyzed file makes all five PYC commands exit 2 with `INVALID_QUERY` and preserve the
input; `target/recovered.py` above is a normal independent target.

Default output is Python text. `--json` returns a schema-v2 `source` envelope; outer
`source_id` identifies the input, while `detail.python_version`, `detail.complete` and
`detail.source` describe reconstruction. Generated code is a decompiled representation,
not the original `.py`. If pycdc detects incomplete structure recovery, default mode fails
without partial source; `--json` can inspect the result with `detail.complete: false`.

```sh
target/debug/rxd pyc decompile tests/fixtures/pyc/hello.3.12.pyc --json | \
  python3 -c 'import json,sys; r=json.load(sys.stdin); assert r["detail"]["complete"]; print(r["detail"]["source"], end="")'
```

Raw marshal without a `.pyc` header requires an explicit version; the flag does not
override an existing header:

```sh
target/debug/rxd pyc decompile /path/to/raw.marshal --version 3.12 --json
```

PYC payloads and raw marshal must contain exactly one complete code root. Truncated
instructions, unterminated/overflowing `EXTENDED_ARG`, insufficient known inline cache
spans and trailing bytes (including zero padding) fail with exit 1, stderr
`INVALID_INPUT` and empty stdout; directory input follows the same contract on Windows
and Unix. Existing `-o` files and input bytes are preserved. Malformed input also fails
under `--json`; `detail.complete: false` is only for incomplete source structure.

Build a trailing-byte regression without running the bytecode:

```sh
python3 -c 'from pathlib import Path; p=Path("tests/fixtures/pyc/hello.3.12.pyc"); Path("target/trailing.pyc").write_bytes(p.read_bytes()+bytes([0]))'
target/debug/rxd pyc decompile target/trailing.pyc --json
# exit 1; stderr INVALID_INPUT; stdout empty
```

Static code navigation uses the pinned CPython 3.12.14 fixtures:

```sh
target/debug/rxd pyc outline tests/corpus/pyc/3.12.14/graph.timestamp.pyc
target/debug/rxd pyc outline tests/corpus/pyc/3.12.14/graph.timestamp.pyc --name same
target/debug/rxd pyc outline tests/corpus/pyc/3.12.14/graph.timestamp.pyc --index 4 --perf
target/debug/rxd pyc outline tests/corpus/pyc/3.12.14/graph.marshal --version 3.12 -o target/outline.jsonl
```

`outline` is schema-v2 header/code/summary JSONL: root index 0, deterministic DFS
preorder, and name filtering preserves both `same` results with their original indices
and IDs. Zero name matches still returns complete header/summary; a missing index exits 2
with `INVALID_QUERY`. IDs combine the full input SHA-256 and code index and do not change
on rename. Parent is first containment; kind is `unknown`, not a runtime type judgment.
Original marshal definition/field/payload and alias sites use real half-open file spans;
when there are no new contiguous occurrence bytes they are null.

```sh
target/debug/rxd pyc outline tests/corpus/pyc/3.12.14/graph.timestamp.pyc --name same | \
  python3 -c 'import json,sys; rows=[json.loads(line) for line in sys.stdin]; assert rows[-1]["complete"]; print([r["id"] for r in rows if r["record"]=="code"])'
```

Outline does not scan bytecode or recover source; `instruction_validation: not_scanned`.
Body status is present/empty/unavailable. Static entry accepts only the six final magic
formats in the design matrix; other versions/implementations are rejected explicitly.
Five positive static budgets cover input/entities/work/records/output; perf stderr
`resource_limits` echoes effective values and all JSONL bytes count toward output.
Errors do not replace existing `-o`.

```sh
target/debug/rxd pyc decompile tests/fixtures/pyc/hello.3.12.pyc --json --perf \
  --max-work 16777216 --max-ast-depth 128
target/debug/rxd pyc decompile tests/fixtures/pyc/hello.3.12.pyc --json --max-work 1
# exit 1; stderr RESOURCE_LIMIT; stdout empty
```

`--max-work` is a positive integer; `--max-ast-depth` is 1..=128. Work covers actual
recovery, repeated nested calls, scans, AST checks/copies and source generation; it is not
a time cancellation or OS memory limit. Existing marshal hard limits remain. Resource
failure behaves the same in default/JSON modes and never replaces `-o`.

## Constant search

```sh
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc --contains shared-container-marker
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc --contains-hex e99baa
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc --contains-hex eda080
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc --contains-hex ff00 --perf
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.marshal --version 3.12 -o target/strings.jsonl
```

A shared marker has five owner/path records; the UTF-8 hex query `e99baa` hits three;
surrogatepass `eda080` hits three strs with `value: null` and raw bytes `eda080`; `ff00`
hits non-UTF-8 bytes with `value: null`. Each context_id is the owning code ID. The first constant-path
component is the top-level slot; later components are tuple/frozenset serialized child
ordinals. Immutable-container alias leaves have no new contiguous occurrence/payload
bytes, so outer start/end and the occurrence pair are null; aliases separately retain the
real reference token. Definition/payload pairs always locate original bytes.

```sh
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc --contains shared-container-marker | \
  python3 -c 'import json,sys; rows=[json.loads(line) for line in sys.stdin]; assert rows[-1]["complete"] and rows[-1]["records"]==5; print([(r["id"],r["context_id"],r["detail"]["constant_path"]) for r in rows if r["record"]=="string"])'
```

Text filtering is strict substring; bytes uses the query's UTF-8 bytes; undecodable strs
do not participate in text matching. Hex is a raw-payload subsequence, case-insensitive,
without `0x` or spaces. The two filters are mutually exclusive; zero hits return a
complete 0-record result. Neither strings nor outline validates unscanned bytecode.

## Single-code FETCH

```sh
target/debug/rxd pyc get-code tests/corpus/pyc/3.12.14/graph.timestamp.pyc \
  'rxd1:pyc:<input-sha256>:code:11'
```

`get-code` accepts any enumerated module/class/function/lambda/comprehension code and
returns one schema-v2 `code_view` envelope (no header/summary) with original body,
instruction rows, tables, values, line mapping and exceptions. `complete: true` means
original body bytes were accounted for and supported evidence validated, not full opcode
semantics or source equivalence. Use real outline IDs, not filenames/names.

## Name and constant references

```sh
target/debug/rxd pyc xref tests/corpus/pyc/3.12.14/graph.timestamp.pyc --name shared
target/debug/rxd pyc xref tests/corpus/pyc/3.12.14/graph.timestamp.pyc --name shared --within '<code-id>'
target/debug/rxd pyc xref tests/corpus/pyc/3.12.14/graph.timestamp.pyc \
  --const-id 'rxd1:pyc:<input-sha256>:const:<code-index>:<path>'
```

`--name` and `--const-id` are mutually exclusive and exactly one is required. Name
matching is exact original-payload equality, not substring/case-fold/normalization.
Constant IDs come from actual `strings` output; their path may traverse only tuple/
frozenset and end at str/bytes. Without `--within`, only the constant's owner is scanned;
with `--within`, the selected code is scanned. Reference context_id routes to `get-code`.
Name references describe explicit name-bearing opcodes, not runtime dispatch; constant
references preserve the containing top-level slot and never claim a nested leaf was
directly loaded. See the [PYC design](../design/pyc-design.md#name-references).

## Formats and budgets

Static commands support final CPython 3.8.20, 3.9.6, 3.10.16, 3.11.11, 3.12.14 and
3.13.2. `.pyc` headers allow PEP 552 flags 0/1/3; raw marshal requires `--version X.Y`.
The code index uses deterministic DFS preorder and first containment, with IDs stable
across renames and processes. Outline/strings/get-code/xref share input, entity, work,
record and output budgets; get-code adds `--max-instructions` (default 1,000,000). All
output is complete before atomic stdout/`-o` publication. `rxd pyz` and PyInstaller
containers are separate and still planned. Control-flow deflattening, deobfuscation and
decryption are permanent non-goals.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend IDs, offsets, inference and degradation are in the
[PYC design](../design/pyc-design.md#capability-status-and-mcp).
