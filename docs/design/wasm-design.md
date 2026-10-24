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
its coordinate `func:<decimal-index>`; the module's identity is the header
`source_id`, so no row repeats a hash. `get-function --at` verifies the coordinate
kind, the index in the defined-function range, and the presence of a code body. Imported
function indices may appear in `imports` but are not valid `get-function` targets.

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
rxd wasm types module.wasm -f i32
rxd wasm functions module.wasm --name run
rxd wasm get-function module.wasm --at func:42
rxd wasm strings module.wasm -f 'example'
rxd wasm xref module.wasm --at func:42
rxd wasm xref module.wasm --kind function -m log
```

One command emits one record kind; there is no mixed structural stream. `types`,
`imports`, `exports`, `functions`, `tables`, `memories`, `globals`, `data`,
`elements` and `strings` each emit header/records/summary JSONL whose records are
all of that command's single kind, in section index order, with
`detail.analysis: metadata`. `header.query` echoes the selectors the command took.

Every listing command takes `-f/--filter`, a substring over the
value the row is named by: the name-section label for `functions` and `globals`,
the rendered signature for `types`, `module.name` for `imports`, the export name
for `exports`, the decoded literal for `strings`, and otherwise the row's own `at`
coordinate. Filters fold case by default; `--case sensitive` makes them exact-case.
`functions --name NAME` matches the name section exactly (duplicates preserved).
`functions --at func:N` selects an existing defined-function row;
there is no separate numeric `--func` selector. Exact name, coordinate and substring
selectors AND together. Coordinate existence is validated before filtering: an
out-of-range index, or an imported function with no defined-function row, is
`NOT_FOUND`, never an empty-success listing. Unmatched content/name searches may
return an empty complete listing. `query` echoes `filter, case` for listings (plus
`at, name` for functions) and `at, kind, match, case` for xref.

Import rows retain their import-entry identity and expose a nullable
`detail.target_at` (`func:N` for function imports). Export rows expose
`detail.target_at` in the exported item's own index space. These are not export or
import positions: copy the target coordinate to query a function, including in a
stripped module with no name section. Export names do not become function names.

- `get-function --at COORD` takes the `at` a `functions` row prints (`func:42`).
  A coordinate names only a position, so a malformed one is `INVALID_QUERY` rather
  than guessed and nothing is expanded against a hash. Indices must use canonical
  unsigned decimal spelling. A well-formed missing index is `NOT_FOUND`. A coordinate
  copied from another module is not checked; re-run the listing for this module.
  Imported function indices have no code body and are `INVALID_QUERY`.
- `get-function` emits one JSON `function_view` record. It first tries structured
  decompilation: on success it returns `representation: clike`, `decompiled: true`,
  language-neutral C-like code, a `mapping` from output lines to code-body byte
  offsets, `func`, locals and the original file range. Original bytecode
  (`instructions`, `bytecode_hex`) follows `--raw`; the instruction fallback
  retains it because instructions are then the only representation. Unsupported or failed reconstruction returns `representation: wasm`,
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
  without lossy replacement, and `-f/--filter` then matches their lossy text.
  Data-segment linear-memory layout is reported as the declared offset expression;
  no constant folding or runtime evaluation is performed.
- `xref --at func:N` finds instructions in all defined function bodies
  that explicitly reference the target function index (`call`, `return_call`,
  `ref.func`), including imported targets. `--kind function` is optional for an
  identity; a contradictory kind is `INVALID_QUERY`, naming both kinds.
  `xref --kind function -m TEXT [--case sensitive]` selects target labels: defined
  function name-section labels (otherwise `func:N`) and imported `module.name`.
  It never searches callers, signatures or export aliases. An empty match selects
  all target functions; an unmatched match returns an empty complete scan.
  Multiple selected targets use one bounded body scan, with references in caller
  index/instruction order and no duplicate rows from overlapping target selection.
  Only function targets are supported. The caller coordinate and code-body offset
  are retained. `call_indirect` has no guessed callee and is not a reference hit;
  its type/table indices remain available in the raw instruction view.

`-o` atomically writes exactly the stdout content. Failures emit no partial stdout
and never overwrite an existing output. Diagnostics go to stderr. `INVALID_QUERY`,
`NOT_FOUND` and argument errors exit 2; input, resource and I/O failures exit 1
(ADR 0015).

## Envelope (schema_version=2)

All structured output uses the shared [ADR 0003](../adr/0003-output-envelope.md)
constructors, with no legacy flat fields or same-name record subobjects. Every
listing command and `xref` return header → zero or more data records → summary. The
header contains `record, backend, command, source_id, input_bytes, schema_version,
query, detail`; `query` echoes the command's own selectors. The summary contains
`record, records, complete, detail` with `detail: {}`. Only exit 0 plus
`summary.complete: true` means a complete query.

Data records contain `record, at, source_id, parent_at, context_at, start, end,
detail`; inapplicable values are `null`, never omitted. Identity is the `at`
coordinate: a row that has an index-space identity prints it, and the header already
carries `source_id`. `source_id` is therefore `null` on every row, and there are no invented
structure IDs, so `parent_at` is always `null`. Reference records have `at: null`;
their `context_at` is the caller's coordinate and `detail.target` is the referenced
function's coordinate. Other records have `context_at: null`. String and reference
offsets are segment/function-relative, not file ranges; their outer `start/end` are
`null`. Every listing command sets `header.detail` to `analysis, imported_funcs,
total_funcs` with `analysis: metadata`. Function and `function_view` records use the
function coordinate as `at`; function ranges are the module file's half-open
code-body range, including locals, and other records have `start/end: null`.

`at` coordinates are self-describing and passed back to `--at` verbatim; there is no
hash to expand them against.

| Record | `at` |
| --- | --- |
| `type` | `type:<type index>` |
| `import` | `import:<import index>` |
| `export` | `export:<export index>` |
| `function` / `function_view` | `func:<combined function index>` |
| `table` | `table:<table index>` |
| `memory` | `memory:<memory index>` |
| `global` | `global:<global index>` |
| `data` | `data:<data-segment index>` |
| `element` | `element:<element-segment index>` |
| `string` | `null` (a string has no index space of its own) |
| `reference` | `null`; caller in `context_at` |

Fixed detail fields:

| Record | Detail fields |
| --- | --- |
| `type` | `type_index, params, results` |
| `import` | `module, name, kind, func_index, type_index, target_at` |
| `export` | `name, kind, index, target_at` |
| `function` | `func_index, type_index, name, params, results, body_file_offset, body_size` |
| `table` | `index, element_type, initial, maximum` |
| `memory` | `index, initial, maximum, memory64, shared` |
| `global` | `index, value_type, mutable, name` |
| `data` | `index, kind, memory_index, offset_expr, byte_length` |
| `element` | `index, kind, count` |
| `string` | `index, origin, segment_index, offset, value, bytes` (`index` = position in the decoded string inventory) |
| `reference` | `target, caller_index, caller_name, offset, opcode, target_index` |
| `function_view` | `representation, decompiled, reason, code, func, body_file_offset, body_size, locals, mapping, complete`; `instructions` and `bytecode_hex` are present with `--raw` and in the instruction fallback |

`get-function` (default reconstruction or `--raw`) is a single
`record="function_view"` FETCH record without header/summary; it additionally carries
`backend="wasm"`, `command="get-function"` and `schema_version=2` outside. `--raw` is
the only spelling for the original instruction view; there is no separate
`--bytecode` or `--instructions` flag. `func` is a
typed object with `func_index, type_index, name, params, results`. Instructions have
`offset, opcode, operand`; locals have `count, value_type`; `mapping` is
`[line, code-body-offset]`. Operand keeps the kind-specific immediate object (memarg:
`align, offset, memory`). These embedded objects are not independent FETCH records.
Names, import indices, maximum, global names, data memory/offset and string values
may be `null` when absent. `bytecode_hex` preserves the complete original code
body in the raw and fallback views.

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
bytes; they do not guess semantics or fail the whole listing.

Component Model, module linking, WASI capability graphs, and interpreting linear
memory as host structures are out of scope. Instruction decoding is not complete
program-semantics recovery.

## Budgets and performance

Ordinary CLI admission, work, instructions, reference metadata and complete output
use the input-scaled envelopes in [ADR 0016](../adr/0016-input-scaled-cli-resources.md); they are not fixed
8 GiB / one-million-work / 128 MiB quotas. Explicit library defaults and MCP keep
their fixed policy. The CLI exposes no budget flags. Existing 32-bit span and format
bounds remain. Section work counts
type/import/export/function/table/memory/global/data/element entries.
Work bounds section decode, function-body decode and reference scanning; `xref`
charges actual decode and match work across all defined bodies once, independent
of the number of selected targets. Target count is work-bounded, and reference
metadata is charged before cloning names into retained rows. Budgets are checked
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

The typed tools mirror the listing commands (`wasm_types`, `wasm_functions`, …) and
`wasm_strings`/`wasm_xref`/`wasm_get_function`, reusing the same validation/decode and
reconstruction code. JSONL becomes header/records/summary; FETCH keeps the original
function view and raw option. Combined function indices, module file ranges,
body-relative offsets, indirect calls and complete fallback remain unchanged; inputs
are never instantiated. Target capture cost is reported separately from the query.
MCP adds no WASM proposal or source-coverage promise.

Shared snapshot, quota, cancellation and complete-response rules are in the
[MCP design](mcp-design.md); the decision is [ADR 0007](../adr/0007-typed-mcp-service.md).
CLI output and exit codes are unchanged by the MCP projection.
