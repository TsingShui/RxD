# WebAssembly static queries

The examples use the repository's independently rebuildable handwritten WASM fixture;
modules are never instantiated.

```sh
cargo build --release
# Rebuild the fixture deterministically from committed WAT (including names).
wat2wasm --debug-names tests/fixtures/wasm/fixture.wat -o tests/fixtures/wasm/fixture.wasm
rxd=target/release/rxd
# One command, one record kind. -f/--filter is a substring, folded unless --case sensitive.
$rxd wasm types tests/fixtures/wasm/fixture.wasm
$rxd wasm imports tests/fixtures/wasm/fixture.wasm
$rxd wasm exports tests/fixtures/wasm/fixture.wasm
$rxd wasm functions tests/fixtures/wasm/fixture.wasm --name sum
$rxd wasm globals tests/fixtures/wasm/fixture.wasm -f g
$rxd wasm data tests/fixtures/wasm/fixture.wasm
```

Each `functions` row prints a short `at` coordinate (`func:1`); pass that back with
`--at` instead of assembling a hash:

```sh
at=$($rxd wasm functions tests/fixtures/wasm/fixture.wasm --name sum | python3 -c 'import json,sys; print(next(r["at"] for r in map(json.loads,sys.stdin) if r["record"]=="function"))')
# Default get tries structured C-like decompilation, then falls back to instructions.
$rxd wasm functions tests/fixtures/wasm/fixture.wasm | \
  $rxd wasm get-function tests/fixtures/wasm/fixture.wasm --from -
$rxd wasm get-function tests/fixtures/wasm/fixture.wasm --at "$at"
# --raw is the one flag for the original instruction view (opcode + immediates +
# code-body offsets); there is no separate --bytecode/--instructions spelling.
$rxd wasm get-function tests/fixtures/wasm/fixture.wasm --at "$at" --raw
$rxd wasm strings tests/fixtures/wasm/fixture.wasm -f hello
$rxd wasm xref tests/fixtures/wasm/fixture.wasm --func 1
$rxd wasm get-function tests/fixtures/wasm/fixture.wasm --at "$at" --perf
```

`get-function` returns one `schema_version=2` `function_view` envelope; content is under
`detail`. The default returns `representation: clike`, `decompiled: true` with
structured C-like code and a line→code-body-offset `mapping`. Unsupported
instructions/proposals (SIMD, GC, exception handling, etc.), `call_indirect`, or
unstructured cases degrade completely to `representation: wasm`, `decompiled: false`,
`reason: UNSUPPORTED_*`, an instruction view and original bytes; `--raw` always selects
that view. Both retain code-body offsets and original file ranges.

Listings are single-kind: header → records of that one kind → summary, so no row stream
mixes types with functions. Identity is the row's `at` coordinate (`type:0`, `func:1`,
`data:1`, …); the header carries the input SHA-256 and rows never repeat it.
`xref --at` rows carry the caller coordinate in `context_at` and the referenced
function in `detail.target`, and report only direct calls and `ref.func`;
`call_indirect` is marked indirect and its runtime table target is not resolved.
Changing any input byte invalidates old coordinates. Budgets come from the shared
`rxd_core::Budget` table, so there is no `--max-*` flag. Whole-function decompilation
may exceed 150 ms only for genuinely large targets; ordinary listings/strings/xref and
instruction view get remain under the 150 ms rule.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend coordinates, offsets, inference and degradation are in the
[WASM design](../design/wasm-design.md#mcp-projection).
