# WebAssembly backend

Status: implemented.

`rxd wasm` is the Agent-facing static analysis entry for WebAssembly modules. It reads
binary `.wasm` modules only; it never instantiates, executes, or calls a WASM runtime
or WASI host. Parsing uses a pinned commit of `wasmparser`
(bytecodealliance/wasm-tools) for structural decoding only.

## Inputs and identity

The backend accepts core-specification binary modules and checks the magic and
version (`\0asm` + version 1) by content, not by extension. WAT text, Component Model
wrappers, host embeddings, packed or self-extracting distributions are out of scope;
compile text to WASM externally first. A module may enumerate imports that have no
body, but `get-function` requires a module-defined function with a code body.

The backend reads one bounded binary buffer from a regular file and computes SHA-256;
path and extension never replace content identity. Function indices use the core
combined index space: imports first, then module-defined functions. The member ID is
`rxd1:wasm:<sha256>:func:<decimal-index>`; `get-function` verifies the content hash,
the index in the defined-function range, and the presence of a code body. Imported
function indices may appear in `outline` but are not valid `get-function` targets.

Parsing checks section order, lengths and index-space boundaries, reads one function
body on demand, and does not pre-decode all code or build a whole-module IR or data-flow
graph. `wasmparser` validation checks types and indices; corrupt modules are rejected
within the declared scope rather than silently skipped. This is not a claim of runtime
instantiation, link completeness or WASI capability resolution. The `name` custom
section (function/local/global names) is read when present and used only as Agent
navigation labels; it never changes content identity and never replaces an index.
DWARF, source maps and `producers` provenance are not reconstructed.

## Commands and protocol

```sh
rxd wasm outline module.wasm --func-name run
rxd wasm get-function module.wasm 'rxd1:wasm:HASH:func:42'
rxd wasm strings module.wasm --contains 'example'
rxd wasm xref module.wasm --func 42
```

- `outline` emits header/records/summary JSONL with `detail.analysis: metadata`.
  Records cover `type` (function signatures), `import`, `export`, `function`,
  `table`, `memory`, `global`, and `data`/`element` summaries, ordered by each
  section's index order. `header.query` echoes filters. `--func-name` matches the
  name section exactly and preserves duplicates; `--func N` selects a combined index;
  without filters it emits all structural records.
- `get-function` emits one JSON `function_view` record. It first tries structured
  decompilation: on success it returns `representation: clike`, `decompiled: true`,
  language-neutral C-like code, a `mapping` from output lines to code-body byte
  offsets, and the `func`, `source_id`, `bytecode_hex`, locals, and original file
  range. Unsupported or failed reconstruction returns `representation: wasm`,
  `decompiled: false`, an explicit `reason`, `code: null`, and the original
  instruction view. Each instruction's `offset` is relative to the function code
  body; file offsets are separate and must not be mixed. Instructions retain opcode
  names, original immediates (decoded LEB128 integers/floats, block type, memarg,
  `br_table` targets), explicit function/type/global/table indices, and branch
  structure. `complete: true` means the record is complete, not that decompilation
  succeeded; partial guessed code is never returned.
- `strings` enumerates decodable UTF-8 byte sequences in data segments and name
  strings. Offsets are segment-relative and retain the data-segment index. Valid
  UTF-8 becomes a JSON string; invalid bytes stay as raw bytes with `value: null`,
  without lossy replacement. `--contains` matches a byte subsequence. Data-segment
  linear-memory layout is reported as the declared offset expression; no constant
  folding or runtime evaluation is performed.
- `xref --func N` finds instructions in all defined function bodies that explicitly
  reference the target function index (`call`, `return_call`, `ref.func`) and retains
  the caller index and code-body offset. It does not resolve `call_indirect` runtime
  table targets and never claims a statically determined indirect callee;
  `call_indirect` records its type and table indices and is marked indirect.

`-o` atomically writes exactly the stdout content. Failures emit no partial stdout
and never overwrite an existing output. Diagnostics go to stderr. `INVALID_QUERY`
and argument errors exit 2; input, resource and I/O failures exit 1.

## Envelope (schema_version=2)

All structured output uses the shared [ADR 0003](../adr/0003-output-envelope.md)
constructors, with no legacy flat fields or same-name record subobjects. `outline`,
`strings` and `xref` return header → zero or more data records → summary. The header
contains `record, backend, command, source_id, input_bytes, schema_version, query,
detail`; `query` echoes command arguments. The summary contains `record, records,
complete, detail` with `detail: {}`. Only exit 0 plus `summary.complete: true` means a
complete query.

Data records contain `record, id, source_id, parent_id, context_id, start, end,
detail`; inapplicable values are `null`, never omitted. `source_id` is the actual input
file SHA-256. There are no invented structure IDs, so `parent_id` is always `null`.
Reference records have `id: null`; their `context_id` is the caller's content-bound
FETCH ID. Other records have `context_id: null`. String and reference offsets are
segment/heap/function-relative, not file ranges; their outer `start/end` are `null`.
The three streaming commands set `header.detail` to
`analysis, imported_funcs, total_funcs` with `analysis: metadata`. Function and
`function_view` records use the content-bound function ID; other records have
`id: null`. Function ranges are the module file's half-open code-body range,
including locals; other records have `start/end: null`.

Fixed detail fields:

| Record | Detail fields |
| --- | --- |
| `type` | `type_index, params, results` |
| `import` | `module, name, kind, func_index, type_index` |
| `export` | `name, kind, index` |
| `function` | `func_index, type_index, name, params, results, body_file_offset, body_size` |
| `table` | `index, element_type, initial, maximum` |
| `memory` | `index, initial, maximum, memory64, shared` |
| `global` | `index, value_type, mutable, name` |
| `data` | `index, kind, memory_index, offset_expr, byte_length` |
| `element` | `index, kind, count` |
| `string` | `origin, segment_index, offset, value, bytes` |
| `reference` | `caller_index, caller_name, offset, opcode, target_index` |
| `function_view` | `representation, decompiled, reason, code, func, body_file_offset, body_size, locals, instructions, mapping, bytecode_hex, complete` |

`get-function` (default reconstruction or `--raw`) is a single
`record="function_view"` FETCH record without header/summary; it additionally carries
`backend="wasm"`, `command="get-function"` and `schema_version=2` outside. `func` is a
typed object with `func_index, type_index, name, params, results`. Instructions have
`offset, opcode, operand`; locals have `count, value_type`; `mapping` is
`[line, code-body-offset]`. Operand keeps the kind-specific immediate object (memarg:
`align, offset, memory`). These embedded objects are not independent FETCH records.
Names, import indices, maximum, global names, data memory/offset and string values
may be `null` when absent. `bytecode_hex` preserves the complete original code body.

## Structured C-like decompilation

`get-function` tries to produce structured, language-neutral C-like pseudocode for
ordinary defined functions, comparable in spirit to wasm-decompile. It recovers
control flow and expression structure; it does not recover the original Rust/C/C++
source language and makes no claim of matching a source language. Layers are bounded
and independently testable:

1. **Instruction read**: reuse the on-demand single-function decode; branch targets
   become structured label depths. No whole-module SSA or high-level IR.
2. **Control skeleton**: WASM is already structured (`block`/`loop`/`if`/`else`/`end`;
   `br`/`br_if`/`br_table` use relative label depths). Loops become `while`/`for`,
   `block`+`br` becomes labeled `break`/`continue`, `if`/`else` maps directly, and
   `br_table` becomes `switch`. Multi-level exits and `br_table` reuse `petgraph`
   only for dominance/reachability. Unsafe stack permutations or non-structured
   jumps degrade to the instruction view.
3. **Expression and statement reassembly**: a backend-owned stack evaluator inlines
   `local.set`/`local.get` and `global.get`, materializes cross-block stack values as
   stack-slot variables, and rebuilds numeric operations, comparisons, linear-memory
   loads/stores with memarg, calls, and global/local accesses as C-like expressions.
4. **C-like output**: emit the structure tree and expressions; use the type-section
   signature and name-section labels when present.

Only graph algorithms (`petgraph` dominance/reachability) are reused. Stack
reassembly and WASM-specific structure recovery are backend-owned; no other
bytecode decompiler middle end is reused. `call_indirect` is marked indirect and its
runtime target is never guessed. Degradation is part of the correctness contract:
unsupported instructions or failed reconstruction fall back to the instruction view
and original bytes, not guessed syntax.

## Instruction view

The view conservatively preserves exact opcodes, original immediates, and
`block`/`loop`/`if`/`else`/`end` nesting. It is the fallback for failed or unsupported
decompilation and does not claim to reconstruct C/Rust/high-level source. Stack
polymorphism and types follow the specification; the frontend materializes
instructions in original byte order and retains structured control boundaries. It
does not consume unverified inferred types.

Supported: MVP numeric/conversion, parametric and variable instructions, linear
memory and memarg, structured control flow, direct calls, `call_indirect` (marked
indirect), sign-extension, saturating conversions, bulk memory, multi-value, and
reference types. Reachable unsupported instructions/proposals (SIMD,
threads/atomics, GC, experimental tail-call/exception handling outside the supported
set) return `UNSUPPORTED_OPCODE` with the opcode and preserve the remaining original
bytes; they do not guess semantics or fail the entire outline.

Component Model, module linking, WASI capability graphs, and interpreting linear
memory as host structures are out of scope. Instruction decoding is not complete
program-semantics recovery.

## Budgets and performance

Defaults: input 8 GiB (may be lowered; 32-bit spans keep actual addressable content
below 4 GiB), entities 100,000, work 16,777,216, instructions per function 1,000,000,
records 100,000, output 32 MiB. Each limit has a positive-integer `--max-*` override.
Entities count type/import/export/function/table/memory/global/data/element entries.
Work bounds section decode, function-body decode and reference scanning; `xref`
charges actual decode and match work across all defined bodies. Budgets are checked
before allocation, section enumeration, function decode and output, including JSONL
header/summary and complete `get-function` original bytes. Code bodies are validated
against specification bounds; immediate vector lengths and `br_table` target counts
are checked before allocation. Worker isolation is not an OS security sandbox.

`--perf` follows the analysis/CLI stderr JSONL sidecar with content identity, input and
output sizes, and fresh-worker cost. Release acceptance uses ordinary real modules
and reports per-workload samples, median, P95, >150 ms share, sizes, hardware and
cache conditions; artificial fixtures are boundary regressions only. Ordinary index,
reference scan, strings and instruction-view `get-function` remain under the 150 ms
fresh-process requirement. Whole-function structured decompilation is backend-owned
and may exceed 150 ms only under the "inherently large target" ground in
[AGENTS.md](../../AGENTS.md#performance-requirements): a separately scoped input
hash and function index, release measurements, target code-body bytes and instruction
count, output size, and a small-target control from the same input within 150 ms. No
exception is assumed without approval.

## Development references

- `wasmparser` (bytecodealliance/wasm-tools): pinned commit, unnecessary features
  disabled, used only for read-only decoding and validation; `Cargo.lock` pins
  transitive dependencies. Provenance and modifications are in
  `crates/rxd-wasm/REFERENCE.md`.
- Handwritten WASM fixtures with independent per-instruction expectations,
  provenance and generators are in `tests/fixtures/wasm/`. Generators produce `.wasm`
  deterministically from committed WAT; ordinary E2E stays offline and never
  instantiates modules.
- Pinned production WASM corpus and independent `wasm-objdump`/`wasm-tools`
  expectations are in `tests/corpus/wasm/README.md`, with provenance, license,
  versions and hashes.

WASM deobfuscation and control-flow deflattening are not goals, consistent with the
permanent non-goals for JS obfuscation and string decryption. Structured C-like
decompilation recovers existing structured control flow and expressions; it does not
restore deliberately scrambled control flow. LEB128, float and memarg decoding is
parsing, not deobfuscation.

## MCP projection

`wasm_outline`/`strings`/`xref`/`get_function` reuse the same validation/decode and
reconstruction code. JSONL becomes header/records/summary; FETCH keeps the original
function view and raw option. Combined function indices, module file ranges,
body-relative offsets, indirect calls and complete fallback remain unchanged; inputs
are never instantiated. Target capture cost is reported separately from the query.
MCP adds no WASM proposal or source-coverage promise.

Shared snapshot, quota, cancellation and complete-response rules are in the
[MCP design](mcp-design.md); the decision is [ADR 0007](../adr/0007-typed-mcp-service.md).
CLI output and exit codes are unchanged by the MCP projection.
