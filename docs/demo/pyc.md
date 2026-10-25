# PYC navigation and decompilation

`rxd pyc decompile` and `codes/strings/get-code/xref` for final CPython 3.8–3.13 are
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
target/debug/rxd pyc codes tests/corpus/pyc/3.12.14/graph.timestamp.pyc
target/debug/rxd pyc codes tests/corpus/pyc/3.12.14/graph.timestamp.pyc -f same
target/debug/rxd pyc codes tests/corpus/pyc/3.12.14/graph.timestamp.pyc --name same
target/debug/rxd pyc codes tests/corpus/pyc/3.12.14/graph.timestamp.pyc --at code:4 --perf
target/debug/rxd pyc codes tests/corpus/pyc/3.12.14/graph.marshal --version 3.12 -o target/codes.jsonl
```

`codes` is schema-v2 code-row JSONL: root index 0, deterministic DFS preorder,
and both `-f/--filter` (substring, `--case sensitive` for exact case) and `--name` (exact) preserve the
matching results with their original indices. Zero matches is empty output and exit 0; a missing `--at code:N` exits 2 with `NOT_FOUND`.
`--at`, `--name` and `-f` intersect. Each row prints the copy-only
`at: code:<index>`; coordinates do not
change on rename. Parent is first containment; kind is `unknown`, not a runtime type
judgment. Original marshal definition/field/payload and alias sites use real half-open
file spans; when there are no new contiguous occurrence bytes they are null.

```sh
target/debug/rxd pyc codes tests/corpus/pyc/3.12.14/graph.timestamp.pyc --name same | \
  python3 -c 'import json,sys; rows=[json.loads(line) for line in sys.stdin]; print([r["at"] for r in rows if r["record"]=="code"])'
```

`codes` does not scan bytecode or recover source; `instruction_validation: not_scanned`.
Body status is present/empty/unavailable. Static entry accepts only the six final magic
formats listed below; other versions/implementations are rejected explicitly.
Ordinary CLI input/metadata allowances grow with admitted input; work, instruction
count and output have no input-derived quotas and no command exposes a `--max-*` flag.
Perf stderr `resource_limits` echoes active quotas; `counters.output_bytes` reports
actual writes. Errors do not replace existing `-o`.

```sh
target/debug/rxd pyc decompile tests/fixtures/pyc/hello.3.12.pyc --json --perf
# stderr resource_limits: input_bytes/ast_depth; counters.work/output_bytes: actual usage
```

Work covers actual recovery, repeated nested calls, scans, AST checks/copies and source
generation; it is not time cancellation or an OS memory limit. Work and marshal
object/copy envelopes scale with input; recursion, cycle rejection and numeric
conversion guards remain independent. Resource failure
behaves the same in default/JSON modes and never replaces `-o`; because the CLI has no
ceiling knob, boundary accounting is exercised through the `rxd_pyc` library in tests.

## Constant search

```sh
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc -f shared-container-marker
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc --filter-hex e99baa
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc --filter-hex eda080
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc --filter-hex ff00 --perf
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.marshal --version 3.12 -o target/strings.jsonl
```

A shared marker has five owner/path records; the UTF-8 hex query `e99baa` hits three;
surrogatepass `eda080` hits three strs with `value: null` and raw bytes `eda080`; `ff00`
hits non-UTF-8 bytes with `value: null`. Each `at` is a copy-only `const:<owner>:<path>`
and `context_at` is the owning `code:<index>`. The first constant-path component is the
top-level slot; later components are tuple/frozenset serialized child ordinals. Immutable-container alias leaves have no new contiguous occurrence/payload
bytes, so outer start/end and the occurrence pair are null; aliases separately retain the
real reference token. Definition/payload pairs always locate original bytes.

```sh
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc -f shared-container-marker | \
  python3 -c 'import json,sys; rows=[json.loads(line) for line in sys.stdin]; strings=[r for r in rows if r["record"]=="string"]; assert len(strings)==5; print([(r["at"],r["context_at"],r["detail"]["constant_path"]) for r in strings])'
```

Text filtering is a substring (folded unless `--case sensitive`); bytes uses the query's UTF-8 bytes
byte-exactly; undecodable strs do not participate in text matching. Hex is a byte-exact raw-payload
subsequence, case-insensitive, without `0x` or spaces. The two filters are mutually
exclusive; zero hits return a complete 0-record result. Neither strings nor codes
validates unscanned bytecode.

## Single-code FETCH

```sh
target/debug/rxd pyc get-code tests/corpus/pyc/3.12.14/graph.timestamp.pyc --at code:11
```

`get-code` accepts any enumerated module/class/function/lambda/comprehension code and
returns one schema-v2 `code_view` object with original body,
instruction rows, tables, values, line mapping and exceptions. `--at` takes the short
`code:<index>` a codes row prints. Code coordinates do not embed a digest; re-run the
listing for a different input. `complete: true` means original body bytes were accounted
for and supported evidence validated, not full opcode semantics or source equivalence.
Use the row coordinate, not filenames/names.

## Name and constant references

```sh
target/debug/rxd pyc xref tests/corpus/pyc/3.12.14/graph.timestamp.pyc --kind name --at shared
target/debug/rxd pyc xref tests/corpus/pyc/3.12.14/graph.timestamp.pyc --kind name -m SHARED --within code:4
target/debug/rxd pyc strings tests/corpus/pyc/3.12.14/graph.timestamp.pyc -f shared-container-marker
target/debug/rxd pyc xref tests/corpus/pyc/3.12.14/graph.timestamp.pyc \
  --at 'const:<code-index>:<path>'
```

Use exactly one of `--at` or `-m/--match`. `--kind name --at NAME` is exact
original-payload equality. `--kind name -m TEXT` is substring matching, folded unless
`--case sensitive`; each hit preserves its actual `detail.target_name`. It searches
referenced names, not enclosing code names or opcodes. No normalization or lossy
replacement is used. Constant coordinates infer their kind from the printed `const:` path.
Constant IDs come from actual `strings` output; their path may traverse only tuple/
frozenset and end at str/bytes. Without `--within`, only the constant's owner is scanned;
with `--within`, the selected code is scanned. Both selectors accept only the coordinate
a row prints; it carries no hash, so a coordinate absent from this input is
`INVALID_QUERY`. Reference
`context_at` routes to `get-code --at`.
Name references describe explicit name-bearing opcodes, not runtime dispatch; constant
references preserve the containing top-level slot and never claim a nested leaf was
directly loaded.

## Formats and budgets

Static commands support final CPython 3.8.20, 3.9.6, 3.10.16, 3.11.11, 3.12.14 and
3.13.2. `.pyc` headers allow PEP 552 flags 0/1/3; raw marshal requires `--version X.Y`.
The code index uses deterministic DFS preorder and first containment, with IDs stable
across renames and processes. codes/strings/get-code/xref share the selected
metadata/index policy in CLI and MCP. Work and instruction count have no
input-derived quota. All
output is complete before atomic stdout/`-o` publication. `rxd pyz` and PyInstaller
containers are separate and still planned.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend-specific coordinates, offsets and fallback semantics are
preserved by the same query implementation.
