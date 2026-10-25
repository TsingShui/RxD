# WebAssembly static queries

Ordinary CLI retained references scale with admitted module size. Work, instruction
count and complete serialized output have no input-derived quotas. Canonical coordinates, body bounds and
validation of original instructions remain unchanged.
MCP service quotas remain independent and unchanged.

The examples use the repository's independently rebuildable handwritten WASM fixture;
modules are never instantiated.

## What the backend covers

| Command | Answer |
| --- | --- |
| `types` / `imports` / `exports` | Signatures, imports and exports with their canonical targets |
| `functions` / `tables` / `memories` / `globals` | Module-defined entities, ranges and signatures |
| `data` / `elements` | Active, passive and declarative segments |
| `strings` | UTF-8 literals from data segments and the name section |
| `xref` | Instructions that explicitly reference a target function |
| `get-function` | One function: structured C-like source, or the instruction view |

Every listing takes `-f/--filter` (folded unless `--case sensitive`); `get-function`
and `xref` take `--at func:N`. `-o` writes the stream atomically and `--perf` reports
stages and resource use. Rows carry coordinates, not hashes, so the module identity is
not repeated on every row. Input is one binary core module, not a directory.

Decompilation is per requested function, never a whole-module pass: listings do
not enter the decompiler. `get-function` emits C-like code with a line-to-byte
mapping; an unsupported construct (SIMD, atomics, GC, tail calls, exceptions,
block parameters) returns `representation: wasm` with an explicit `UNSUPPORTED_*`
reason and the decoded instructions instead of guessed source. `--raw` asks for
that instruction view directly. Modules are parsed statically and never
instantiated or executed.

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
$rxd wasm get-function tests/fixtures/wasm/fixture.wasm --at "$at"
# --raw is the one flag for the original instruction view (opcode + immediates +
# code-body offsets); there is no separate --bytecode/--instructions spelling.
$rxd wasm get-function tests/fixtures/wasm/fixture.wasm --at "$at" --raw
$rxd wasm strings tests/fixtures/wasm/fixture.wasm -f hello
$rxd wasm xref tests/fixtures/wasm/fixture.wasm --at "$at"
$rxd wasm xref tests/fixtures/wasm/fixture.wasm --kind function -m ENV.LOG
$rxd wasm functions tests/fixtures/wasm/fixture.wasm --at "$at"
$rxd wasm get-function tests/fixtures/wasm/fixture.wasm --at "$at" --perf
```

`get-function` returns one `schema_version=2` `function_view` envelope; content is under
`detail`. The default returns `representation: clike`, `decompiled: true` with
structured C-like code and a line→code-body-offset `mapping`. Unsupported
instructions/proposals (SIMD, GC, exception handling, etc.), `call_indirect`, or
unstructured cases degrade completely to `representation: wasm`, `decompiled: false`,
`reason: UNSUPPORTED_*`, an instruction view and original bytes; `--raw` always selects
that view. Both retain code-body offsets and original file ranges.

Listings are single-kind data rows, so no row stream
mixes types with functions. Identity is the row's `at` coordinate (`type:0`, `func:1`,
`data:1`, …); rows never repeat a content hash.
`xref --at` rows carry the caller coordinate in `context_at` and the referenced
function in `detail.target`, and report only direct calls and `ref.func`;
`call_indirect` is marked indirect and its runtime table target is not resolved.
Export rows expose `detail.target_at` for direct navigation to the exported function;
function imports expose the same field for caller queries, but have no fetchable body.
Names are not identities, and export aliases do not become name-section labels.
Coordinates refer to the current input; re-run the listing for changed input bytes,
including same-size changes.
A nonexistent coordinate is `NOT_FOUND`; an unmatched name filter is a complete
empty result. Contradictory kinds are argument errors, never silent empty scans. Budgets come from the shared
`rxd_core::Budget` table, so there is no `--max-*` flag. Whole-function decompilation
may exceed 150 ms only for genuinely large targets; ordinary listings/strings/xref and
instruction view get remain under the 150 ms rule.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend-specific coordinates, offsets and fallback semantics are
preserved by the same query implementation.
