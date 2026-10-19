# WebAssembly static queries

The examples use the repository's independently rebuildable handwritten WASM fixture;
modules are never instantiated.

```sh
cargo build --release
# Rebuild the fixture deterministically from committed WAT (including names).
wat2wasm --debug-names tests/fixtures/wasm/fixture.wat -o tests/fixtures/wasm/fixture.wasm
rxd=target/release/rxd
$rxd wasm outline tests/fixtures/wasm/fixture.wasm --func-name sum
```

Take the content-bound ID from a `function` record and query:

```sh
id=$($rxd wasm outline tests/fixtures/wasm/fixture.wasm --func-name sum | python3 -c 'import json,sys; print(next(r["id"] for r in map(json.loads,sys.stdin) if r["record"]=="function"))')
# Default get tries structured C-like decompilation, then falls back to instructions.
$rxd wasm get-function tests/fixtures/wasm/fixture.wasm "$id"
# --raw forces the instruction view (opcode + immediates + code-body offsets).
$rxd wasm get-function tests/fixtures/wasm/fixture.wasm "$id" --raw
$rxd wasm strings tests/fixtures/wasm/fixture.wasm --contains hello
$rxd wasm xref tests/fixtures/wasm/fixture.wasm --func 1
$rxd wasm get-function tests/fixtures/wasm/fixture.wasm "$id" --perf
```

`get` returns one `schema_version=2` `function_view` envelope; content is under `detail`.
The default returns `representation: clike`, `decompiled: true` with structured C-like
code and a line→code-body-offset `mapping`. Unsupported instructions/proposals (SIMD, GC,
exception handling, etc.), `call_indirect`, or unstructured cases degrade completely to
`representation: wasm`, `decompiled: false`, `reason: UNSUPPORTED_*`, an instruction view
and original bytes; `--raw` always selects that view. Both retain code-body offsets and
original file ranges. `xref --func` reports only direct calls and `ref.func`;
`call_indirect` is marked indirect and its runtime table target is not resolved. Changing
any input byte invalidates old IDs. Outline/strings/xref follow the
header/records/summary completeness contract. Whole-function decompilation may exceed
150 ms only for genuinely large targets; ordinary outline/strings/xref and instruction
view get remain under the 150 ms rule.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend IDs, offsets, inference and degradation are in the
[WASM design](../design/wasm-design.md#mcp-projection).
