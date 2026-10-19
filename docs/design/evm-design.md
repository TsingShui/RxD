# EVM bytecode analysis

Status: implemented.

`src/cli/evm/` is the command adapter; `crates/rxd-evm/` fixes input bytes, validates
encoding and calls unchanged EVMole 0.9.4. The engine is pinned by crates.io version
and `Cargo.lock`, with only serde enabled. It is linked into default rxd; there is no
Node, WASM runtime, RPC, browser, signature database or external EVM program. Source
and MIT license are in `crates/rxd-evm/REFERENCE.md` and distributed with releases.

EVM is capability-first and has no 150 ms or other numeric latency acceptance gate.
Input, output and record budgets remain, and performance sidecars are observational.
The backend never executes contracts, transactions or chain interactions, and never
performs deobfuscation.

## Commands

All commands analyze one explicit local file; `--encoding raw|hex` is required. Native
commands emit one schema-v2 JSON object (`--json` selects the same format explicitly).
Navigation commands use the JSONL/single-record contracts below.

| Command | EVMole request |
| --- | --- |
| `evm analyze` | All native analysis capabilities |
| `evm interface`, alias `functions` | Selector, parameter types, state mutability and terminal metadata |
| `evm storage` | Persistent/transient storage plus function candidates and metadata |
| `evm disassemble` | Decoded-byte offset/opcode pairs |
| `evm basic-blocks` | Native basic-block ranges |
| `evm cfg` | Basic blocks and control-flow graph |
| `evm metadata` | Terminal CBOR map |

Native commands preserve EVMole semantics. Language-neutral pseudocode reconstruction
for selected functions does not claim original Solidity/Yul. See the
[EVM examples](../demo/evm.md).

## Inputs and identity

- `raw` reads bytes directly; `hex` accepts ASCII hex with optional `0x`/`0X`, mixed
  case and outer ASCII whitespace.
- Odd hex, internal whitespace/separators, BOM, non-ASCII characters and invalid UTF-8
  return `INVALID_INPUT` (exit 1).
- Empty bytecode is valid; an empty result does not imply no interface. `EF00` EOF
  containers return `UNSUPPORTED_INPUT`.
- Directories return `INVALID_INPUT`; missing files and real read errors return
  `IO_ERROR`. The backend uses one already-open handle and a bounded fixed buffer.
- `source_id` is the original file SHA-256, preserving prefix/case/whitespace. The
  decoded bytecode SHA-256 is in `detail.bytecode.sha256`. Neither is an Ethereum
  Keccak code hash.
- `--include-bytecode` returns exact decoded bytes as lowercase hex without prefix;
  otherwise the text is omitted while bytes/hash remain.
- Output may not overwrite the input carrier. `-o` publishes atomically on success;
  any failure emits no partial stdout and preserves an existing output.

## Native JSON

The outer object is `record=evm_analysis` with `backend=evm`, command,
`schema_version=2`, id, source_id, parent_id, context_id, start, end, detail.
id/parent/context are null; start/end cover the original carrier `[0, bytes)`.

`detail` contains engine, carrier, bytecode, offset_unit, cfg_destination_unit,
basic_block_end, disassembly_complete, undecoded_tail, abi_verified,
discovery_complete, input_role, hardfork, result and complete. `result` uses EVMole's
native Rust serde shape, including camelCase producer fields, null/unrequested fields
and upstream metadata omission. Bytes and undecoded CBOR are native byte arrays.

Nested function/opcode/metadata offsets are decoded-byte offsets, never hex text
offsets. CFG `to`/`path` values are upstream block IDs and must not be assumed equal to
block start addresses. A basic-block end is the start byte of the last opcode, not an
exclusive end. The upstream disassembler omits a trailing PUSH with insufficient
operands; `disassembly_complete` says whether returned instructions cover all bytes and
`undecoded_tail` records the exact remainder. That is not proof that every opcode or
runtime execution path was covered.

`complete: true` only means the requested upstream result was delivered completely.
`discovery_complete: null`, `abi_verified: false`, `input_role: unknown` and
`hardfork: null` preserve unknown properties. Selector provenance is upstream
`dispatch`; function names are not recovered. Parameters, mutability, storage layout
and control flow are inference, not a verified source ABI or runtime trace. Metadata
presents upstream text-key entries; other encodings and non-portable integers remain
undecoded bytes. The original carrier is always independent evidence.

## Resource and execution bounds

Defaults: raw file 8 GiB (`--max-input-bytes` lowerable; 32-bit spans keep actual
addressable content below 4 GiB), 100,000 records, 16 MiB complete output including
JSON and trailing newline. `--max-records` and `--max-output-bytes` adjust the rest.
The record budget counts functions, persistent/transient storage, opcodes, basic
blocks, CFG blocks and metadata entries; a structure may count in two producer views.
The record budget is checked after upstream analysis; it is not an upstream compute or
memory limit. The native engine keeps its own analysis budgets; rxd does not modify the
engine or claim hard CPU/RSS limits.

Results are serialized through a bounded writer; overflow exits 1 with
`RESOURCE_LIMIT` and no partial success. The standard isolated analysis worker/thread
is used. `--perf` emits the analysis/cli stderr sidecar with decode/analyze phases and
resource parameters; stdout is unchanged.

## Navigation stream schema

`outline`, `strings` and every xref branch (block/function/PUSH/data, including empty
results) share the same fixed `header.detail` and `summary.detail` field sets; absent
values are explicit null. Objects are built before bounded serialization, so new
nullable fields and the trailing newline are covered by `max-output-bytes`; failure
leaves no partial stdout and preserves an existing `-o` file.

Header detail has 23 fixed fields: carrier, bytecode, offset_unit, basic_block_end,
boundary_status, target_kind, target_decoded_offset, target_semantics, coverage,
literal_scope, excluded_unindexed_pushes, discovery_complete, disassembly_complete,
undecoded_tail, source_call_graph_complete, abi_verified, data_scope, metadata_range,
copy_witnesses, unknown_copy_witnesses, unvisited_copy_sites, data_reference_coverage
and unknown_copy_evidence.

Summary detail has 20 fixed fields: blocks, functions, target_kind,
target_decoded_offset, references, literals, data_literals, reference_status,
excluded_unindexed_pushes, resolved_edges, unknown_jumps, invalid_jumps,
analysis_problems, scanned_contexts, scanned_blocks, external_call_sites,
resolution_complete, discovery_complete, disassembly_complete and
source_call_graph_complete.

Query echoes are fixed: outline `{encoding}`; strings `{encoding, kind, query,
match_mode}`; xref `{encoding, id, reference_scope}`. Null does not mean an analysis
ran or a count is zero. `outline.basic_block_end=exclusive` and
`boundary_status=producer_candidate`; xref keeps source_call_graph/ABI false. Function
inventory and `function_code` FETCH use different discriminators while identity kind
stays `function`, so detail fields do not change between ENUMERATE and FETCH.

## Basic blocks

`evm outline INPUT --encoding raw|hex` emits a schema-v2 JSONL header, zero or more
`basic_block` records and a summary. `evm get-block INPUT ID --encoding raw|hex`
returns one `block` JSON record with only the selected block's raw bytes/instructions.
The native `basic-blocks` command remains the original EVMole range array.

Block ID:
`rxd1:evm:<carrier-sha256>:<raw|hex>:block:<decoded-start>:<decoded-end-exclusive>`.
The carrier hash is 64 lowercase hex; encoding is raw/hex; offsets are canonical
decimal without leading zeros (except 0). The ID binds the fixed carrier and explicit
encoding interpretation; decoded SHA-256 stays in detail. Copy/rename does not change
the ID. Wrong grammar/boundaries return `INVALID_ID`; a different input identity returns
`SOURCE_MISMATCH`; user IDs never drive arbitrary slicing.

Navigation decodes the fixed input boundedly and requests native disassemble/
basic_blocks. Each producer range start/last opcode must exist in the complete decoded
instructions; the end is validated by last-opcode/PUSH width and converted to an
exclusive end. Native block ranges have no independent CFG block ID, so
`upstream_block_id` is null and a bytecode address is never called a CFG ID. Incomplete
PUSH does not produce a decoded instruction/boundary; the complete prefix remains
navigable and the exact tail is recorded. Blocks are producer candidates, not proof of
function recovery, reachability or boundary completeness. Empty input is valid; EOF and
invalid encoding follow native rules.

`basic_block` has the ADR 0003 record meta; parent/context are null; start/end are the
original carrier's byte-exclusive range (hex includes only digits, no prefix/outer
whitespace). Detail: `unit_kind=block`, decoded_start, decoded_end, last_opcode_start,
instruction_count, `upstream_block_id=null`, `boundary_status=producer_candidate`,
owner_ids, shared. Standalone outline/get-block set owner_ids/shared null; embedded
function records fill them with the same field set. Summary uses the fixed nullable
schema and `complete` only means delivery is complete.

`block` adds backend/command/schema_version and detail includes all basic_block fields
plus carrier, bytecode, raw_hex, instructions, disassembly_complete, undecoded_tail and
complete. `bytecode.hex` is always null for FETCH; raw_hex covers only the block's
decoded bytes; instructions are `{decoded_offset, decoded_end, opcode, operand_hex,
text}`. `disassembly_complete=true` and `undecoded_tail=null` mean the selected range
decoded completely, not that the input or program semantics are complete.

Navigation adds `--max-work` (default 16,777,216), charged in decoded bytes, complete
opcodes in bounded preflight, and validated producer blocks. It is an input/analysis
size budget, not CPU instruction count or all engine loops. A bounded opcode/PUSH scan
checks byte/instruction work first; then only disassemble/basic_blocks are requested,
with per-block work checks; the block stage does not call selector/storage/CFG/
decompilation. `get-block` uses only this stage; outline then performs selector/
ownership analysis. `--max-records` bounds delivered data records (blocks for outline,
1 for FETCH). Failures emit no partial stdout and preserve existing outputs; perf
resource_limits echoes work, counters.work echoes visited work, and a `navigate` phase
is separate from decode/analyze.

## Function candidates and FETCH

`outline` emits zero or more `function` candidates before block records. Summary adds
`functions`; records count functions plus blocks. `get-function INPUT ID --encoding
raw|hex [--raw]` returns one `function_code` record (identity kind remains function), or
selects a unique candidate with `--selector HEX8` (optional 0x/0X, mixed case). ID and
selector are mutually exclusive; malformed selector is `INVALID_QUERY`, zero candidates
is `NOT_FOUND`, multiple is `AMBIGUOUS_SELECTOR`; no silent choice. Default FETCH
reconstructs language-neutral EVM pseudocode for declared-supported targets
(status=success); unsupported/incomplete targets use `raw_fallback` with a reason and
exact raw evidence. `--raw` sets `not_requested` and skips reconstruction.

Function ID:
`rxd1:evm:<carrier-sha256>:<raw|hex>:function:<lowercase-selector-8-hex>:<abi|fallback>:<canonical-decimal-entry>`.
Carrier/encoding rules match block IDs. FETCH validates selector/dispatch/entry against
current discovery; user-reported ranges are rejected. Wrong grammar/kind/undiscovered
units return `INVALID_ID`; wrong carrier/encoding returns `SOURCE_MISMATCH`. Copy/rename
is stable.

Candidates come from unchanged EVMole selector/dispatch; the native result map collapses
duplicate selectors, while rxd's bounded native dispatcher provenance adds valid entries
observed from PC0 selector EQ/JUMPI, preserving multiple entries for selector ambiguity.
An added candidate must be proven reachable by the selected selector's dispatcher path;
arbitrary PUSH4/EQ in other function bodies is not a dispatcher. Selector discovery
completeness is unknown and ABI is unverified.

Function ownership is built from independently validated instruction/JUMPDEST indices
and bounded native stack traversal; it never uses contiguous slices between adjacent
selector entries or unions all helper return targets from the upstream CFG. JUMPDEST
must be a complete instruction start in a validated block; `5b` inside PUSH payload is
invalid. Dynamic/invalid JUMP and the true edge of JUMPI keep reasons; the false edge is
validated separately instead of inheriting a producer simplification that may drop
exceptional edges.

The dispatcher derives the selected entry's stack shape and observed MSTORE memory facts
from an empty initial stack. The selected selector proves only the first four calldata
bytes; the rest of CALLDATALOAD(0) stays unknown, and only SHR224/equivalent DIV selector
projection yields a four-byte constant. Memory records only justified exact word writes;
overlapping/unknown writes, copies or calls clear facts and never allocate infinite
address space. Unsupported entry modes stay unknown rather than guessing empty stack,
zero arguments or unobserved memory initialization.

The context graph keys blocks by each stack slot's valid constant jump address. Ordinary
changing data widens monotonically to unknown at the same-key join/loop entry; return
address slots are preserved, supporting two callers, deep/nested helpers and data loops
before helpers. Edges retain caller-specific stack transfers; a helper's global multi-
return relation never extends A's ownership to B's return block. Block owner_ids are the
deterministic set of independently supported traversals; shared code is emitted once.
Control flow crossing another known selector entry keeps `cross_entries` with
`ownership_status=cross_entry_unknown`; exact source function boundaries are not
claimed. Without an entry-stack witness, the entry block keeps its raw FETCH route and
no farther ownership is invented.

Function meta follows ADR 0003; start/end are carrier ranges only when the whole selected
region is one contiguous interval, otherwise null. Outline detail: `unit_kind=function`,
selector, dispatch, decoded_entry, `entry_evidence=selector_dispatch_candidate`,
entry_state_status (`dispatcher_derived`/`unknown`), entry_states, included_block_ids,
segments, blocks, instruction_count, context_count, boundary_status
(`static_reachable`/`incomplete`), boundary_complete, ownership_status
(`candidate`/`cross_entry_unknown`), cross_entries, unknown_successors,
`discovery_complete=null`.

`entry_states` items are `{stack, memory_words}`; stack is bottom-to-top with
`{kind, hex}` where kind is constant/unknown/calldata_word_0 and only constant has a
64-hex word. Memory words are `{offset, value_hex, write_pc}`. Dispatcher selector_word/
selector_equals are internal provenance and never appear in selected function entry
states. Segments are `{decoded_start, decoded_end, carrier_start, carrier_end, raw_hex}`;
outline raw_hex is null, FETCH has exact lowercase bytes. Adjacent selected blocks merge;
discontiguous regions are not padded. `blocks` uses the fixed basic_block meta/detail with
owner_ids/shared filled; `included_block_ids` matches them and can be passed to
get-block. `unknown_successors` items are `{decoded_offset, kind, destination_hex,
reason}`; unknown destinations are null, known invalid values keep 64-hex. Reasons:
dynamic_jump_destination, invalid_jump_destination, undecoded_or_unindexed_successor,
stack_underflow/overflow, unsupported_opcode, entry_state_unresolved,
entry_not_validated.

`function_code` adds backend/command/schema_version and detail adds carrier, bytecode,
instructions, control_flow, unresolved_evidence, reconstruction, complete. Instructions
are the unique raw instructions of selected blocks (same fields as get-block).
`control_flow` items are `{context, block_id, entry_state, exit_state, edges, problems}`;
context is a deterministic graph index. Edges are `{instruction_offset, kind, target,
target_context, resolution}`; target is a decoded PC, target_context the context index,
never an upstream CFG ID; kind is jump/branch_true/branch_false/fallthrough; resolution
is resolved/unknown/invalid. `complete=true` means known evidence was delivered fully;
`boundary_complete=true` means every validated successor/termination was established
from a supported dispatcher witness — not source-function boundaries, complete selector
discovery, ABI or arbitrary execution equivalence. Dynamic/invalid targets, truncation,
unknown entries and cross-entry ownership set boundary_complete=false.

New budgets: `--max-states` default 100,000 and `--max-stack-cells` default 1,048,576.
They bound all function dispatcher/selected graph allocations; stack_cells counts
entry/exit/control-key stack-slot arrays and memory-word maps. Each stack has at most
1,024 slots; exceeding valid EVM stack shape records stack_overflow. `--max-work` also
charges candidates, state/stack/memory visits, opcode transfer, ownership and output
evidence. Context-key widening prevents unbounded ordinary loop states. Budget failure
is `RESOURCE_LIMIT` with empty stdout and preserved files. Output lower bounds are
checked before building large nested function JSON, then a bounded writer enforces exact
bytes. Native selector engine budgets are unchanged. Perf instructions are whole-input
decode size; get-function matches are selected raw instruction count, semantic_nodes are
selected contexts, entities are function candidates. analyze/navigate and serialize are
timed separately.

## `xref`

`evm xref INPUT --id ID --encoding raw|hex` accepts outline block/function IDs and emits
a schema-v2 JSONL header, zero or more `code_reference` records and a summary. ID
grammar/canonical validation/carrier/encoding/unit validation follow the corresponding
FETCH; stale/forged/wrong-kind IDs are errors, not zero hits. Copy/rename remains valid.

Block queries match that block's exact decoded start; function queries match only a
validated candidate's decoded entry, not every owned block. A producer-candidate entry
not yet validated as a block/JUMPDEST boundary remains queryable, but
`target_semantics=unvalidated_candidate_entry`, zero resolved hits and
`resolution_complete=false`; the candidate ID does not prove validation. Returned
relations are supported static control-flow references: jump, branch_true, branch_false,
fallthrough and selector_dispatch are kept distinct; an internal JUMP is not a verified
source-level function call. JUMP/JUMPI true targets must be validated JUMPDESTs on
complete instruction boundaries; fallthrough/false successors must be exact decoded
block starts.

Discovered functions use FunctionNavigation's concrete contextual edge witness: each
reference routes only to the function/context that produced the edge. Shared helper
returns are not globally unioned; A→return_A does not create a B route. Shared source
blocks stay marked shared, but owner_ids contains only concretely supported owners.
Contexts/functions supported independently are emitted in deterministic function,
context, edge order, then fallback block order.

Blocks without a concrete function context witness use block-local native stack evidence
with context_id routing to the exact get-block and `ownership_status=unresolved`. PC0
starts from a real empty stack; other blocks preserve unknown input-stack preconditions,
and `incoming_stack_words` reports the minimum unknown words needed for the local
transfer, without guessing saved helper return addresses. Necessary preconditions above
1,024 words or transfer stack overflow retain an analysis problem; no resolved reference
is fabricated. This evidence does not prove block reachability or that input-stack
conditions hold on a real path; evidence_kind and ownership_status preserve scope.

Only edges with `resolution=resolved` and a target exactly matching the queried ID produce
hits. Unknown dynamic targets and known invalid targets count separately in
summary.unknown_jumps/invalid_jumps; they cannot produce a forged target-ID hit. Zero
hits with resolution_complete=true differ from false; ID/budget/read failure exits
non-zero with no success summary. resolution_complete only says this contextual/
block-local scan had no unknown/invalid edges, analysis problems or undecoded tail; it
is not an arbitrary runtime call graph, complete selector discovery, source-function
boundary or execution trace.

CALL/CALLCODE/DELEGATECALL/STATICCALL count as physical decoded opcode sites only in
`external_call_sites`; the count proves no reachability, execution, address, ABI or
resolved external call. Addresses/selectors inside PUSH are not code-unit references;
source_call_verified/abi_call_verified are always false and source_call_graph_complete/
abi_verified false. Data, metadata and unknown ownership are never promoted to executed
function calls.

`xref` header query is `{encoding, id, reference_scope}`; code scope is
`static_control_flow`. Header detail uses the common nullable schema with target_kind
block/function, target_decoded_offset (target PC), target_semantics (exact_block_start/
validated_candidate_entry/unvalidated_candidate_entry), coverage
(contextual_and_block_local), source_call_graph_complete=false, abi_verified=false;
literal/data fields are null. `--include-bytecode` fills bytecode.hex, otherwise null.

`code_reference` meta: id/parent null; context_id is the referencing function ID
(usable by get-function --raw) or block ID (usable by get-block); start/end are the exact
referencing instruction's original carrier range. Detail fixed fields include target_id/
target_kind/target_decoded_offset, reference_kind, resolution, instruction_offset,
opcode, operand_hex, context_kind (function/block), referencing_block_id,
referencing_context_index, target_context_index, evidence_kind
(contextual_stack/block_local_stack), incoming_stack_words, owner_ids, shared,
ownership_status (candidate/unresolved), source_call_verified=false,
abi_call_verified=false. Contextual route indices come from that function FETCH's
control_flow and incoming_stack_words=null; block-local route indices are null,
incoming_stack_words is the minimum unknown input words, owner_ids=[].

Summary uses the fixed 20-field schema. Code xref fill target fields/references/
disassembly_complete/resolved_edges/unknown_jumps/invalid_jumps/analysis_problems/
scanned_contexts/scanned_blocks/external_call_sites/resolution_complete and
source_call_graph_complete=false; other fields null. Resolved/unknown/invalid counts
cover all scanned evidence, independent of query matches; the same raw site in different
functions/contexts may count separately. analysis_problems counts entry/stack/opcode/
successor problems not covered by jump counts. scanned_contexts is the number of
concrete function witnesses; scanned_blocks is fallback blocks without one; records only
counts returned hits and complete only means delivery.

Budgets: work/states/stack_cells build the function graph and scan contexts/edges/
problems/raw blocks/instructions/external opcode sites; local stack shape/transfer also
charges work and stack_cells. Hits are bounded by max_records and a per-record writer.
Failure is atomic with empty stdout and preserved output; a normal empty result still has
header+summary. Perf separates query/serialize; counters.records/matches are hits,
semantic_references is scanned edges; resource_limits echoes work/states/stack_cells.

## `strings` and data candidates

`evm strings INPUT --encoding raw|hex --kind text|bytes|number [--query QUERY]` emits
three-part JSONL. kind is required; without query it inventories that kind's immediates
and explicit data-candidate evidence.

- text: substring match within contiguous valid UTF-8, non-control-character runs of one
  complete PUSH operand, preserving overlaps. Empty query is `INVALID_QUERY`; invalid
  UTF-8, control characters and different PUSH operands never match across boundaries.
  These are raw readable bytes; `runtime_string_proven=false`; no runtime-string claim.
- bytes: exact full-width PUSH operand bytes equality; query is even-length ASCII hex
  with optional 0x/0X, mixed case, normalized to lowercase without prefix. Empty query
  matches PUSH0's empty operand; `00` does not match PUSH2 `0000`.
- number: unsigned big-endian U256 equality; query is decimal or 0x/0X hex normalized to
  a 64-hex word. Leading zeros/PUSH width do not affect equality, but width_bytes/
  bytes_hex retain original encoding. PUSH0 is zero; negatives, no digits, invalid
  digits or >U256 are `INVALID_QUERY`. Selectors/addresses may be searched as bytes/
  number but add no ABI/external-call inference.

Scope is `validated_block_push_immediates`: only complete PUSH0..PUSH32 inside validated
producer blocks. Opcode-like bytes inside a PUSH payload are not separate instructions;
short PUSH remains in undecoded_tail and produces no literal. Linear-disassembly PUSH-
like interpretations without a validated block (e.g. compiler metadata) create no FETCH
owner and are counted in `excluded_unindexed_pushes`. Scope completeness is not whole-
file literal discovery; discovery_complete=null. Data/metadata candidates and CODECOPY
evidence use a separate data discriminator.

Literal ID:
`rxd1:evm:<carrier-sha256>:<raw|hex>:literal:push:<canonical-decimal-opcode-PC>:<canonical-decimal-PUSH-width>`,
width 0..32. It binds one real validated PUSH occurrence; equal immediates are not
merged. Copy/rename is stable; carrier/encoding mismatch is `SOURCE_MISMATCH`; wrong
grammar/kind/width/position or no established literal is `INVALID_ID`. IDs never drive
slicing.

`evm xref --id LITERAL_ID` queries only that exact occurrence's decoded PUSH uses, not
all equal immediates or arbitrary runtime uses. Each supported function owner returns a
route; shared blocks keep deterministic function order; one literal ID may have multiple
context_ids. Without a function owner, the actual block ID is returned for get-block.
Strings hits use the same routes, so SEARCH → literal ID → xref → context_id → get-
function --raw/get-block works. Function context_boundary_complete is separate from
query complete; literal existence never upgrades boundary proof.

Strings query is `{encoding, kind, query, match_mode}` with normalized query or null;
match_mode is inventory/contains/equal_bytes/equal_u256. Literal xref query is
`{encoding, id, reference_scope}`, scope `exact_literal_occurrence`. Strings sets
literal_scope, excluded_unindexed_pushes and data coverage fields; target/coverage/
source_call_graph_complete/abi_verified are null. Immediate xref set target_kind=literal,
target_decoded_offset (opcode PC), target_semantics=exact_literal_occurrence,
coverage=validated_block_push_immediates, literal_scope, excluded_unindexed_pushes,
source_call_graph_complete=false, abi_verified=false, data fields null.
`--include-bytecode` returns full decoded hex; otherwise null.

Literal detail fixed fields: literal_id, literal_kind, instruction_offset,
instruction_end, opcode, width_bytes, decoded_start, decoded_end, bytes_hex, number_hex,
block_id, owner_count, context_kind, context_boundary_status,
context_boundary_complete, runtime_string_proven, query_kind, evidence_kind,
match_status, text_candidates, matches. literal_kind=push_immediate; number_hex is
always a 64-hex U256 word. instruction_offset/end cover opcode+operand; decoded_start/
end cover only operand. context_kind is function/block; function boundary status is
static_reachable/incomplete, block is producer_candidate with complete=null.
evidence_kind is push_immediate_text_candidate or push_immediate; match_status is
matched/inventoried. text_candidates are `{decoded_start, decoded_end, text}`; matches are
`{decoded_start, decoded_end, carrier_start, carrier_end}` for real substring hits only,
empty arrays otherwise.

`literal_reference` meta has id=null; detail.literal_id is the queried occurrence,
context_id is the FETCH route, start/end the operand range. Detail contains the first 16
literal fields through runtime_string_proven plus evidence_kind, reference_kind,
instruction_carrier_start/end; evidence_kind=decoded_push_use,
reference_kind=push_immediate. The field set never varies by query kind; it is a distinct
discriminator from code_reference.

Strings summary uses the fixed nullable schema: literals is the number of distinct
matching PUSH/data occurrences, data_literals the data count, references/reference_status
null. Refs summary uses the same 20-field schema with target_kind=literal,
target_decoded_offset=opcode PC, literals=1, references=routes, data_literals=0,
reference_status=decoded_push_use, excluded_unindexed_pushes/disassembly_complete/
discovery_complete inherited from the immediate scope; graph fields null,
source_call_graph_complete=false. Null means the graph scan did not run, not proof of
zero unknown jumps. Records count actual route records; a zero-hit SEARCH still emits
header + complete summary.

Data candidates: `strings` also returns `data_literal`, and `xref --id` accepts its ID as
`data_reference`. Origins are static evidence, not proof of bytecode role:

- `unindexed`: complement of validated producer blocks, including internal gaps and the
  tail. Metadata ranges are excluded separately; the complement is not claimed to be
  necessarily non-executed data.
- `metadata`: unchanged EVMole validated terminal CBOR map plus its two-byte length
  suffix; input role remains unknown. The designation does not prove compiler origin.
- `copied`: fixed-input intersection of contextual CODECOPY, added when not fully inside
  one of the above regions. It may cover opcodes or PUSH operands; such copied bytecode is
  marked and split by metadata boundaries. Copies fully contained in an existing region
  add no duplicate origin.

Each region keeps exact full-width byte candidates and additionally keeps maximal
contiguous valid UTF-8, non-control-character text runs of at least 4 decoded bytes;
duplicate origin/range entries merge. Different regions, invalid UTF-8 and Unicode
control characters split candidates and never match across boundaries. Text queries only
select readable runs, with overlapping substrings; KMP query preprocessing is done once
and scan/compare/hit work is budgeted. Bytes compares exact complete candidate bytes;
number only selects candidates ≤32 bytes and compares unsigned big-endian U256;
number_hex is null for longer candidates. Boundaries are this policy's evidence, not
Solidity/Yul string object boundaries; runtime_string_proven is always false.

Data ID:
`rxd1:evm:<carrier-sha256>:<raw|hex>:literal:data:<unindexed|metadata|copied>:<decoded-start>:<decoded-end-exclusive>`.
The fixed input must re-establish the exact origin/range; IDs never slice directly.
Wrong grammar/candidate is `INVALID_ID`; carrier/encoding mismatch is
`SOURCE_MISMATCH`. The same candidate keeps its ID across query kinds; copy/rename is
stable; raw/hex bind their carrier and outer start/end map exact digits.

CODECOPY evidence reuses functions.rs native abstract transfer, replaying each function
and actual Context.entry and capturing the instruction's destination/source/length pre-pop
values. `context_index` matches the original FETCH control_flow.context; references and
unknown-operand evidence retain that actual entry stack/memory state. Ownerless block
`entry_evidence_block_ids` keeps one concrete validated PC0→block path (including the
current block) that can be checked with get-block for earlier argument PUSHes; function
witnesses have an empty array because the function FETCH already retains the graph and
instructions. Shared helpers use each caller's own entry/return stack, not a block's
global owner union. Ownerless code builds bounded path evidence only from the real empty
PC0 entry and only gives get-block routes to blocks reached on that path; other blocks
keep unvisited coverage without guessing input stacks. Replay does not execute bytecode,
simulate EVM memory or allocate source-length-driven memory.

Only witnesses with known source and length and a real input intersection produce a
data_reference; destination may be unknown and copy success/runtime strings remain
unproven. length/source keep full 64-hex U256; the input intersection is first clamped by
the decoded buffer length so U256 never drives usize addition/slicing/allocation.
CODECOPY reads zero fill beyond source end (unlike RETURNDATACOPY OOB failure);
zero_padding_hex is requested length minus available source bytes; fully outside source
or length=0 references no raw candidate. Partial intersections keep exact copied bytes/
range; uncopied candidate bytes never fill reference evidence.

A SEARCH candidate without a witness keeps context_id/context_kind/context_index null.
reference_status=statically_copied means a known source-range witness; unreferenced means
none in the supported scope; unknown operands or unvisited CODECOPY sites mean
unreferenced_or_unknown. A zero-reference xref result still emits header + complete summary
and zero data records. discovery_complete=null; delivery completeness does not prove no
dynamic use, source string use, executable path or function boundary.

Header detail includes data_scope, metadata_range, copy_witnesses,
unknown_copy_witnesses, unvisited_copy_sites, data_reference_coverage,
unknown_copy_evidence. Strings/data xref set them; PUSH/code-unit xref set them null and
do not claim a data scan. Data xref keep the common three-key query, 23-field header and
20-field summary; target_kind=literal, target_decoded_offset=candidate start,
target_semantics=bytecode_data_candidate, coverage=contextual_static_evidence,
literal_scope=null, data_scope=unindexed_and_contextually_copied_bytecode,
data_reference_coverage=contextual_static_evidence, metadata_range null or
`{decoded_start, decoded_end}`. copy_witnesses counts all visited contextual operations,
unknown_copy_witnesses counts operations with incomplete source/length, unvisited_copy_sites
counts validated CODECOPY PCs without witnessed transfer. unknown_copy_evidence items:
`{instruction_offset, block_id, context_id, context_index, context_entry_state,
entry_evidence_block_ids, source_offset_hex, length_hex, destination_offset_hex, reason}`;
reason is source_offset_unknown/length_unknown/source_offset_and_length_unknown; known
words are 64-hex, others null. These are coverage evidence, never disguised as references
to the queried data.

`data_literal` detail: literal_id, literal_kind, region_kind, decoded_start, decoded_end,
bytes_hex, number_hex, query_kind, evidence_kind, match_status, text_candidates, matches,
reference_status, reference_count, context_kind, context_index,
runtime_string_proven. literal_kind=data_candidate; evidence_kind depends on origin and
query kind (unindexed_data_text_candidate/compiler_metadata_text_candidate/
copied_bytecode_text_candidate or the non-text variants). text_candidates/matches use the
same item schema as immediates; reference_count is contextual witnesses, each shared use
gets one actual route, and an unreferenced candidate gets one null-context candidate.
context_kind is function/block/null; outer id is the candidate ID and start/end its
carrier range.

`data_reference` detail: literal_id, reference_kind, evidence_kind, instruction_offset,
instruction_end, instruction_carrier_start, instruction_carrier_end, source_offset_hex,
length_hex, destination_offset_hex, available_source_start, available_source_end,
zero_padding_hex, copied_decoded_start, copied_decoded_end, copied_bytes_hex, block_id,
context_kind, context_index, context_entry_state, entry_evidence_block_ids, resolution,
runtime_string_proven. reference_kind=codecopy,
evidence_kind=contextual_constant_codecopy, resolution=resolved means only static
source-range evidence. available_* is the copy∩input; copied_* is the further
intersection with the candidate. Outer id=null; context_id is the witness function/block
FETCH; start/end is the copied intersection's carrier range; instruction_carrier_* is the
actual CODECOPY opcode. Data xref summary literals/data_literals=1, references/records =
contextual witness routes, reference_status as above.

Budgets preserve fixed input, atomic output and old-file protection. max-work charges
metadata/input scans, region/copy/context visits, native transfer, text/bytes scans, KMP
comparisons/hits and routes. max-states also bounds data candidates, temporary UTF-8 runs,
copy witness storage, ownerless contextual graphs, stored entry stacks/memory and
predecessor block-index paths (stack_cells). A deterministic output lower bound is checked
before large match/copy evidence; a bounded writer then enforces exact output; overflow is
RESOURCE_LIMIT with no partial stdout. Perf separates search, analyze, references and
serialize; matches count distinct PUSH+data occurrences, records count delivered routes.

## Declared reconstruction scope

Supported: native EVM stack/arithmetic expressions, memory/calldata, persistent/transient
state and termination, acyclic branches/joins, simple natural loops and shared contextual
regions. Successful contextual_gotos means a complete IR whose structured shape was not
recovered; it is distinct from raw_fallback. Unknown/invalid jumps, incomplete boundaries/
entries and unsupported opcodes keep reasons and original segments/instructions/
control_flow/unresolved_evidence. Function granularity does not promise reliable source
boundaries, ABI, complete selector discovery or a runtime call graph. There is no source-
level class aggregation; get-function keeps the selected necessary shared regions and
does not add artificial whole-input FETCH. Delivery, boundary/ownership, discovery,
reconstruction and structuring are reported separately. Input is always analyzed offline
without execution, chain contact or deobfuscation.

## Verification and MCP

Real CLI tests cover raw/hex identities and matching producer output, function parameters/
mutability, persistent/transient storage, explicit bytecode disassembly/CFG, CBOR native
bytes and large-integer encoding, empty/EOF/invalid carriers, budgets, atomic output,
cross-platform directory diagnostics and perf sidecars. Navigation tests cover outline →
ID → get-block/get-function, PUSH-payload fake JUMPDESTs, trailing PUSH lengths, complete
prefix/incomplete PUSH, empty input, copy identity, wrong source/forged ranges, work/
record/output budgets and fixed field sets. Pinned upstream samples and provenance are in
`tests/fixtures/evm/README.md`; instruction positions and Solidity parameter examples are
independently checkable facts, not semantic-equivalence proofs.

`evm_analyze/interface/storage/disassemble/basic_blocks/cfg/metadata` keep their single
`evm_analysis` object. Encoding must be explicit; carrier/source_id and decoded-bytecode
SHA-256 stay separate; producer offset/CFG units, undecoded_tail, abi_verified/
discovery_complete and inference properties are preserved. MCP adds no unit ID/SEARCH/
XREF/FETCH closure beyond the accepted EVM scope.

Shared snapshot, quota, cancellation and complete-response rules are in the
[MCP design](mcp-design.md); the decision is [ADR 0007](../adr/0007-typed-mcp-service.md).
CLI output and exit codes are unchanged by the MCP projection.
