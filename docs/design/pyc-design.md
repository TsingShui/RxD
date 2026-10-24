# Python bytecode backend

Status: implemented; `rxd pyz` is planned.

`rxd pyc` is the Agent-facing entry for compiled CPython bytecode. `decompile` uses
the imported pycdc Rust library to turn a `.pyc` or explicitly versioned raw marshal
code object into Python text without importing/executing input or loading a Python
runtime. `codes`, `strings`, `get-code` and `xref` provide static navigation,
lossless constant search, original FETCH and explicit name/constant references for
final CPython 3.8–3.13 formats. `rxd pyz` container handling is planned separately.

## Decompilation

```sh
rxd pyc decompile module.pyc
rxd pyc decompile module.pyc --json
rxd pyc decompile raw.marshal --version 3.12 -o recovered.py
```

Default stdout is UTF-8 Python text; `-o` atomically writes the file and does not
repeat it on stdout. `--json` returns one ADR 0003 `record: source` envelope with
`schema_version: 2`, `backend: pyc`, `command: decompile`, input identity digest `source_id`,
record/at/parent_at/context_at/start/end and detail. There is no code-unit navigation or
original source range, so at/parent/context/start/end are null; generated text offsets
never become input offsets. Detail fields are `input_bytes`, `python_version`,
`representation: decompiled_python`, `complete`, `source`. Incomplete output sets
`complete: false`; default text mode then fails with `INCOMPLETE_DECOMPILATION` instead
of emitting partial text. `complete: true` means pycdc detected no unrecoverable
structure, not byte-identical source or behavioral equivalence.

Without `--version`, input must be a `.pyc` with recognizable magic. With `--version
X.Y`, the payload is raw marshal code for that exact version and an existing `.pyc`
header is not overridden. Both modes require exactly one complete root code object;
non-code roots, truncated marshal objects and any trailing bytes (including zero padding
or a second object) return `INVALID_INPUT`. Unknown magic is never guessed from a
nearby version.

Every code object actually used for source recovery gets strict instruction decoding:
3.6+ wordcode must have even byte length; older variable instructions need a complete
two-byte argument; `EXTENDED_ARG` must terminate in an argument-bearing instruction and
the expanded value must fit 32 bits (at most three prefixes for wordcode, one for older
encoding); known opcodes must have their version-required inline cache units. Cache
payload is never interpreted as independent instructions. Corruption propagates through
functions, lambdas, class bodies, comprehensions, type parameters and annotations and
returns `INVALID_INPUT`, even with `--json`. Known-but-unsupported structures and unknown
opcodes use `complete: false` / `INCOMPLETE_DECOMPILATION`; that flag describes source
recovery only, not whole-file bytecode semantics or behavior equivalence.

Input must be a regular file; directories return `INVALID_INPUT` (exit 1) with empty
stdout and preserved `-o` target. Ordinary CLI work, metadata/copies, marshal
objects/reference expansion, instruction counts and output use the input-scaled
policy in [ADR 0016](../adr/0016-input-scaled-cli-resources.md); there are no `--max-*` flags or fixed total quotas.
Explicit library defaults and MCP keep fixed quotas. The 32-bit input span guard,
AST depth, marshal recursion (128), code allocation-hint guards and long-to-decimal
squared-digit work (16,777,216) remain independent safety constraints. Cycles are rejected
immediately; shared objects cannot bypass object budgets by exponential expansion.
Source generation checks output before each append; all JSON/JSONL commands use the
shared fallibly growing output writer, counting metadata and each trailing newline.
Allocation or output-envelope exhaustion reports `RESOURCE_LIMIT`. Failure emits no partial stdout and does not replace an
existing `-o` file.

All PYC commands share `-o` input identity protection: reject absolute/relative and `..`
paths, symlinked parent directories and inputs that resolve to the same file; Unix also
compares device/inode for hard links and case aliases, returning `INVALID_QUERY`.
Different targets use atomic replacement. `--perf` writes only to stderr.
Isolated workers are not OS sandboxes.

`decompile` takes work from the selected policy and retains the core AST-depth guard;
there is no CLI knob for either. Work starts at instruction recovery: decoder/recovery/
generator helper entries, explicit loop iterations and scan callbacks each count one; AST safety
traversal counts visited nodes and enqueued edges along actual repeated paths;
collection reservations/copies/expansions count elements; string copies/source appends
count bytes. Format-spec copies, temporary formatter/join/append work and lossy UTF-8
conversion are charged before allocation; UTF-8 conversion also counts scan and exact
replacement bytes; cache-span validation counts code units. Nested code recovery, repeat
scans, AST clones and intermediate generation share one invocation budget. This is
deterministic engine work, not instruction count, wall-clock timeout, OS memory or CPU
quota. Marshal object/reference/copy allowances follow the selected input-scaled or fixed
policy; the separate numeric-conversion and recursion guards remain unchanged.

AST depth is the structural path-node count built/folded (root expression/statement
depth 1) covering expressions, statement bodies, patterns, parameters/annotations, type
parameters and f-string format structures. Each new expression is checked with a
non-recursive bounded traversal before `Rc` allocation; statement folding and recursive
clone check before expansion; the complete body is checked again before postprocessing
and generation. All recovery/traversal/generator helper entries share a recursion-frame
limit `4 * max_ast_depth + 32`. Shallow marshal with instruction-derived depth is still
rejected.

Any work/depth exhaustion exits 1 with stderr `RESOURCE_LIMIT`, empty stdout and no
`clean=true`/partial source; existing `-o` and input bytes remain. Result-less third-party
helpers use a private typed unwind cancellation; only the invocation boundary converts
that payload to a resource diagnostic, without replacing/calling the panic hook; other
panics propagate. Resource failure retains consumed work/depth and phase timings.

`--perf` follows the analysis/CLI stderr JSONL sidecar with content identity, input/
output sizes and fresh-worker cost. Stages: read/hash/parse/decompile/generate/serialize/
write; utf8/visit/query are zero and limits null; `resource_limits` echoes
input_bytes/output_bytes/work/ast_depth plus marshal_depth/marshal_objects/
marshal_expanded_objects/marshal_copied_bytes/marshal_long_work.
`counters.work` is accepted cumulative units, `counters.ast_depth` accepted peak; failed
requests do not count rejected reservations/depth, and unrelated semantic counters are
null.

The embedded pycdc is an rxd-maintained source fork and does not qualify as a
third-party engine. Whole-module reconstruction follows the 150 ms rule; a genuinely
large single module may use the "inherently large target" ground in
[AGENTS.md](../../AGENTS.md#performance-requirements) with input hash, selected target,
code-object count, native bytecode/instruction volume, output size, release measurements
and a small-target control within 150 ms. Eligibility is not approval. There is no
blanket PYC exception. The PYC runner was removed with the other per-backend runners
(`benches/README.md`); its pins and expectations live under `tests/corpus/pyc/`.

## Static formats and code identity

Static commands accept only the six independently verified final CPython 3.8–3.13
formats. A 16-byte PEP 552 header's flags must be 0/1/3. Magic cannot identify the
producer patch version or prove producer identity; other implementation/development
magic returns `INVALID_INPUT` rather than guessing. `--version X.Y` only selects the exact
format for raw marshal; it does not override an existing header. Raw marshal has no
producer/version header, so the caller must provide the format. Decompilation keeps its
larger version set.

The code index uses the same bounded fixed input and one marshal reader. Root index 0;
`co_consts` serialization order, depth-first preorder; code directly inside tuple/list/
set/frozenset/slice participates; dict uses alternating key/value positions. A distinct
code object is indexed once; its coordinate is `code:<decimal-index>`, and the input
identity lives in the stream header's `source_id` rather than inside the id. `get-code
--at` and `xref --within` take that coordinate as printed. A coordinate is file-scoped:
re-run the listing for a different input. Rename/new process/filtering does not change
the ID or renumber. The first discovery containment determines
`parent_at`; this does not prove unique runtime ownership. `kind` is always `unknown`;
name/compile mode/flags are not inferred as class/function identity.

Each owner's shared container definition is traversed once to avoid exponential alias
expansion. `incoming` retains code containment edges found in this traversal with owner
ID and constant path; owner info is bounded but does not expand every path through a
shared container, and `incoming_complete: false` says it is not a full occurrence list.
A direct code alias has real TYPE_REF start/end; a leaf reached through an alias container
or alias const table has no new contiguous occurrence bytes, so its range is null and
`alias_start/alias_end` preserves the alias. All serialized occurrence/definition and
field positions come from parser evidence and are reused by SEARCH/FETCH.

`codes` does not build a decompiler AST, recover source, scan `co_code` instructions or
decode line/exception tables. Complete summary means header/marshal/index/filtering were
fully processed; `instruction_validation: not_scanned` explicitly excludes unscanned
instruction validity. Empty `co_code` is `body_status: empty`; non-bytes fields keep
metadata and original field evidence with `unavailable` and `bytecode_bytes: null`. A
later single-code FETCH rejects an unreadable body.

`-f/--filter` is a substring over the code name (folded unless `--case sensitive`) and `--name` matches
the original UTF-8 name exactly, both preserving global indexes; zero matches is a
complete empty result. `--at code:N` selects an existing code row;
there is no numeric `--index` flag. Coordinate, exact name and substring selectors
AND together. A missing coordinate is `NOT_FOUND` (exit 2), validated before name
filtering, not an empty result. Stream identity is checked against the same fixed
analysis buffer used by the query. Marshal surrogatepass payloads keep raw bytes: valid
UTF-8 metadata uses a string, otherwise text is null and corresponding `*_bytes` give
lowercase payload hex; the lossy decompiler compatibility view is never used. Non-ASCII
payloads for ASCII tags are rejected. Cycles/forward xref are rejected; FLAG_REF
containers reserve reference slots before children, and definition and reference site
stay separate.

### Codes schema v2

ADR 0003 three-part JSONL: exactly one header, zero or more `record: code` sorted by
global index, exactly one summary. Header query is `filter/case/name/at/version` (null
when absent; case is `insensitive` or `sensitive`; `at` is the echoed anchor); detail is
`analysis/python_version/code_count/instruction_validation`.
Summary records/complete describe data records; detail is `code_count/analysis/
instruction_validation`; analysis is always `metadata`.

Code meta: record/detail always present, the other ADR 0003 keys present when they have a
value (absent or `null` otherwise);
`at` is `code:<index>`, source_id is null (the header carries it), context_at null, root
parent null. Outer start/end are the real half-open file range of
the code marshal **definition** in the fixed input, never instruction offsets or generated
text. All fields/payload ranges use full-file coordinates; raw marshal starts at 0 and PYC
is rebased +16.

Detail fields: index/name/qualname/filename/name_bytes/qualname_bytes/filename_bytes/
kind; arg_count/posonly_arg_count/kwonly_arg_count/n_locals/stack_size/flags/first_line;
body_status/bytecode_bytes/consts_count/names_count/varnames_count/freevars_count/
cellvars_count/localsplus_count/line_table_bytes/exception_table_bytes; fields/incoming/
incoming_complete/ownership (always `first_containment`).

3.11–3.13 `fields` keys: arg_count/posonly_arg_count/kwonly_arg_count/stack_size/flags/
code/consts/names/localsplus_names/localsplus_kinds/filename/name/qualname/first_line/
line_table/exception_table. 3.8–3.10: arg_count/posonly_arg_count/kwonly_arg_count/
n_locals/stack_size/flags/code/consts/names/varnames/freevars/cellvars/filename/name/
first_line/line_table. Older formats do not store qualname/localsplus/exception table;
the outer fixed keys remain, qualname/qualname_bytes/localsplus_count are null,
exception_table_bytes is 0, and no field spans are synthesized. Each field has start/end/
definition_start/definition_end/payload_start/payload_end/reference/tag; start/end is
the actual field/reference position; definition/payload point at the referenced object's
real file bytes; integer fields without object occurrence have the last three ranges/tag
null and reference false. Each incoming item has owner_id/constant_path/start/end/
alias_start/alias_end/reference.

Static commands use ADR 0016's input-scaled CLI work, metadata, marshal-object/copy
and serialized-output envelopes; MCP and explicit default library calls retain
fixed quotas. Format spans remain 32-bit. Evidence occurrence admission follows
the selected work allowance, not a separate CLI knob. Entities count every parsed
marshal occurrence (code, constants/names/localsplus entries, field objects; each alias
occurrence counts 1). Work accumulates from the same parser
into index/filtering: each occurrence, index/traversal visit, copied child handle,
incoming path handle, query candidate, substring candidate and filter bytes count 1;
exact-name comparison charges UTF-8 bytes. Container reservations, index paths/edges and
record allocation are checked before use; serialization checks actual JSONL bytes
including header/summary/newlines. Marshal depth 128, cycle rejection, code-header
allocation hints and long squared work 16,777,216 remain; object/reference/copy
allowances grow with admitted input for ordinary CLI calls.
`codes` has no instruction/AST budget because it builds neither; the shared
`instructions` ceiling is only relevant to FETCH/xref.

`-o` atomically writes the same complete response and does not repeat stdout. Any input/
budget/query error has no partial result and leaves an existing output. Diagnostics/
`--perf` goes to stderr. `resource_limits` echoes the effective input_bytes,
entities, work, metadata_bytes, output_bytes and marshal object/expansion/copy
allowances; counters.entities/work/records are actual; ast_depth null; stages read/hash/parse/visit/
query/serialize/write. Decompile keeps decompile/generate; new nullable perf fields are
null. Worker isolation is not an OS sandbox; no analyzed code executes.

Independent offline oracle/provenance/generator hashes/licenses are in
`tests/corpus/pyc/README.md`. E2E covers graphs, duplicate names, filters, shared/cyclic
data, raw input, Unicode, original spans, shared-budget accounting (driven through the
library because the CLI has no ceiling flag), schema, atomic output and header ranges.
`pyc codes` is measured on release with fresh processes.

## Constant search

```sh
rxd pyc strings module.pyc
rxd pyc strings module.pyc -f URL
rxd pyc strings module.pyc --filter-hex ff00
rxd pyc strings graph.marshal --version 3.12 -o target/strings.jsonl
```

`strings` reuses codes' fixed input/marshal evidence/code index, supports the same
final matrix and raw-version mode, and does not start the decompiler/AST or scan
`co_code`. It emits every str/bytes occurrence by global code index, top-level
`co_consts` slot and immutable-container serialized child order. Immutable containers are
tuple/frozenset, nested; path starts with the top-level slot and then child ordinals.
frozenset uses the current input's marshal order, not runtime or recompilation-stable
order. Mutable list/dict/set/slice do not expand, and dynamic expressions, runtime
concatenation or f-strings are not evaluated.

Constant ID:
`const:<code-index>:<dot-separated-index-path>`; each row
prints the copy-only coordinate `const:<code-index>:<path>`, which `xref --at`
expands against the header identity. Different owner/slot/path is a different ID; equal
text or shared objects do not deduplicate. Tuple/frozenset and whole-`co_consts` shared
xref still expand every owner/path; raw payload shares a definition handle; path/alias
copies and actual traversal charge work first. Owner is the owning code definition; a
shared code's literal is enumerated once under its distinct index. context_at/parent_at
are the owner `code:<index>`; context can be passed to get-code --at.

`value` is used only for str payload strictly representable as UTF-8; valid marshal
surrogatepass or other non-lossless Unicode is null, never replacement characters. bytes
value is always null. `raw_bytes` gives lowercase hex for every str/bytes literal,
including empty/non-UTF-8 bytes, read only from the real fixed-input payload range, never
from `PyObject::Str`'s decompiler compatibility view.

Mutually exclusive `-f/--filter` is a substring over decodable str (folded unless `--case sensitive`)
literals (Unicode lowercase folding) and a byte-exact UTF-8 query-byte subsequence over
bytes literals; undecodable str does not match text. `--filter-hex` does raw
payload-byte subsequence, accepts mixed-case even-length hex without prefix/separators;
invalid values are clap argument errors (exit 2). The hex form is byte-exact and never
case-folds, while text matching folds case only. Empty hex matches
every literal including empty; empty text matches decodable str and all bytes. NUL queries
use hex because OS argv cannot carry NUL. Zero matches exits 0 with complete
header/summary and records=0.

### Strings schema v2

ADR 0003 header/string/summary JSONL. Header query is filter/filter_hex/version
(absent null); detail analysis/python_version/code_count/instruction_validation/
containers; analysis=`constants`; containers=`["tuple","frozenset"]`. Summary records are
actual string records; complete only means the supported marshal/index literal enumeration
and filter completed, not unscanned instruction validity; detail code_count/analysis/
instruction_validation, with `not_scanned`.

String meta: record/at/source_id/parent_at/context_at/start/end/detail; `at` is the
short const coordinate, source_id null (the header carries it), and context_at/parent_at
are the owner `code:<index>`. Detail keys:
code_index/index/constant_slot/constant_path/kind/value/raw_bytes (`index` is the
constant's slot in its code object, the same value as constant_slot);
definition_start/
definition_end/payload_start/payload_end; occurrence_start/occurrence_end/reference/
aliases. All spans are real half-open file bytes (PYC includes its 16-byte header; raw
marshal starts at 0). definition_* is the original literal object definition including
tag/length; payload_* is the raw literal bytes without tag/length. Direct definitions use
the real payload for outer start/end; a literal TYPE_REF or any occurrence inherited from
a container alias has outer start/end null, never a short reference token or code offset
as a payload range. Direct literal definition/reference occurrence_start/end preserve the
actual serialized object range; an inherited alias leaf has no new contiguous serialized
bytes, so that pair is null. reference marks whether the original container definition's
leaf is a TYPE_REF, even when the leaf inherits an alias. Aliases are start/end/
constant_path, outer-to-inner actual container reference tokens and the referenced
container's owner path; an empty path means the owner's entire co_consts field alias.
Shared literal TYPE_REF uses its own occurrence pair and is not added to container
aliases.

Budgets inherit the selected static and marshal allowances plus the independent
recursion/cycle/numeric-conversion guards. Records count matching
output only, not filtered literals. Work continues from parser/index and additionally
charges every owner-slot/container visit, path push/clone, alias path retention/
projection, query bytes, literal raw-byte validation, actual substring byte comparisons
and matched payload hex generation before processing/allocation. The same complete
payload UTF-8 validation reservation covers unfiltered literals' strict value view.
Remaining output budget is checked before allocating hex/ID/detail; a bounded writer then
enforces all header/records/summary/newline bytes. Query/serialize failure keeps actual
perf work and produces no partial stdout or replacement.

Independent CPython 3.12.14 compile-only/marshal oracle covers Unicode/surrogatepass,
bytes, shared immutable values and multiple owners. Real CLI E2E covers empty/control
literals, duplicate definitions, container/co_consts multi-layer aliases with real spans,
raw input offsets, filters/zero results, schema, rename/new process/content changes,
shared-budget accounting through the library and atomic output. Release measurements use
real pinned stdlib
modules; `pyc strings` takes repeatable `-f`/`--filter-hex`.

## Code FETCH

```sh
rxd pyc codes module.pyc
rxd pyc get-code module.pyc --at code:11
rxd pyc get-code raw.marshal --at code:0 --version 3.12
```

`get-code` is the final name (no `get` alias or selected-code source rebuild). It accepts
any enumerated module root, class body, function, lambda or comprehension code; a class
body is code evidence, not a complete runtime class definition. `--at` takes the
`code:<index>` a codes row prints; nothing is expanded, and an index past the table is
`NOT_FOUND` (exit 2). A malformed coordinate is `INVALID_QUERY`. Missing readable body
is `INVALID_INPUT`; readable empty bytecode differs from unavailable.

Default output is one schema-2 `code_view` JSON envelope without header/summary. Outer
keys: schema_version/backend/command/record/at/source_id/parent_at/context_at/start/end/
detail; backend=pyc, command=get-code, record=code_view, `at` is the short code
coordinate, source_id null, context_at null, start/end is the selected code definition's
half-open marshal range. Detail keys: input_bytes/
python_version/representation/complete/query/metadata/bytecode_hex/instructions/tables/
values/line_table_hex/line_mapping/exception_table_hex/exceptions. representation=
original_bytecode; complete=true means all original body bytes were accounted for and
supported structural evidence validated, not all opcode semantics/source recovery/
behavior equivalence. Query keys code_id/version; code_id echoes the `--at` value the
caller supplied, and metadata is identical to the complete fixed codes detail.

Each instruction row has offset/size/start/end/opcode/name/raw_operand/operand/raw_hex/
role/owner_offset/logical_start/logical_size/prefixes/jump_target/targets/status/line.
All body code units are returned in original order: role=instruction/extended_arg/cache;
raw opcode/name/raw_operand/raw_hex are never dropped; meaningless operand/targets/
logical ranges are null. Cache includes arbitrary raw bytes; name is only the version
table lookup of its first raw byte, and cache payload is never interpreted as an
instruction/reference; status=RAW_CACHE, targets empty. Normal logical instructions use
DECODED; safely separable unknown/RESERVED/instrumented opcodes use
UNSUPPORTED_OPCODE without guessed targets. Prefixes are offset/opcode/raw_operand/
raw_hex; normal operand is the fully expanded value. logical_start/size include prefixes
and inline caches; size is the raw row length. offset/logical_start/owner_offset/
jump_target are code-relative byte offsets; start/end are real file offsets of the code
bytecode **definition payload**, never a short TYPE_REF field. All matrix formats are
2-byte wordcode with one raw operand byte; 3.8/3.9 jump units are bytes, 3.10+ words.
Relative base includes the opcode's cache span; 3.11 relative jumps have no cache;
3.12/3.13 cached jumps use the base after cache. Static evidence preserves the original
EXTENDED_ARG jump target rather than the source-recovery legacy remap. Older variable
encoding outside the matrix is not statically supported.

Targets are table/index/id/value_id. Const operands (including 3.11/3.12 KW_NAMES) index
consts exactly; from 3.11 LOAD_GLOBAL uses arg>>1, from 3.12 LOAD_ATTR uses arg>>1,
LOAD_SUPER_ATTR uses arg>>2; other names use direct name indices. 3.8–3.10 locals use
varnames; free uses the cellvars+freevars operand space projected to actual cellvars/
freevars tables and indices. 3.11+ uses raw localsplus; free must have CELL/FREE kind,
and 3.11/3.12 locals must have LOCAL. 3.13 FAST uses the complete GETLOCAL slot space,
allows cell-only/free-only, and LOAD_CLOSURE has been lowered to LOAD_FAST; no
closure_capture is inferred. The three 3.13 packed FAST operations decode arg>>4 and
arg&15, retaining both targets. All supported name targets must be raw str. COMPARE_OP
indexes directly through 3.11, uses arg>>4 in 3.12, arg>>5 in 3.13; original flags/operand
remain. Out-of-range targets, cache-interior/out-of-body jumps, corrupt prefix/cache/
line/exception evidence fail immediately. KW_NAMES references a tuple entry and does not
claim each leaf was literal-loaded.

Tables are consts/names/localsplus/varnames/freevars/cellvars; each has present/start/end/
definition_start/definition_end/entries. 3.8–3.10 var/free/cell are independent raw
tables with index/span/alias, kind=null; localsplus present=false, ranges null, entries
empty. 3.11+ derived tables filter raw localsplus kind 0x20/0x80/0x40, keep original
indices, allow local|cell overlap and do not renumber. Entries have index/id/value_id/
start/end/alias_start/alias_end/reference/kind. Const IDs use
`const:<owner>:<slot>`; other table entries use
`table:<owner>:<table>:<index>`; a value uses `value:<parser-definition-ordinal>`. The latter two are navigation keys,
not FETCH arguments. Aliased nested occurrences without contiguous bytes use start/end
null; alias_start/end preserves the container reference; reference says whether the
serialized leaf itself is a ref.

Values are the flat deduplicated definitions reachable from tables; code values are
navigated only by code_id and do not expand fields/body. Items: id/tag/kind/value/
raw_hex/definition_start/definition_end/payload_start/payload_end/children/code_id. tag
preserves the raw marshal tag including FLAG_REF; kind is none/bool/ellipsis/
stop_iteration/int/float/complex/str/bytes/code/tuple/list/dict/set/frozenset/slice.
value is lossless UTF-8 str, a determinate numeric/bool/complex text form, or null for
other/unrepresentable Unicode. str/bytes raw_hex is raw payload; other scalars use the
complete raw marshal definition (so float/complex binary remains checkable); non-empty
containers/code have raw_hex null to avoid recursive duplication. Children are index/
value_id/start/end/reference preserving serialized child definitions/references; dict is
key/value order. Definition and payload spans are strictly separate; shared payload is
not copied into many huge values. Surrogatepass bytes are retained; no replacement
characters impersonate the original. Line mapping items are start/end/line/end_line/
column/end_column/table_start/table_end; start/end are code-relative half-open ranges;
table_start/end are original line-table entry ranges. Null no-location and every location
entry are retained, not just line starts. 3.8/3.9 co_lnotab uses byte address + signed
line deltas; the trailing first-line continuation has no table entry, so table_start/end
equal the table length; an empty lnotab covers the body from first_line. 3.10 co_linetable
uses interval-end address/line deltas; -128 means no line and does not update the
cumulative line; a zero-length pair only updates the line. 3.8–3.10 have no end-line/
column and use null. 3.11+ returns raw location entries even when CPython co_lines merges
adjacent same-line records. Exceptions are start/end/target/depth/lasti/table_start/
table_end; first three are code-relative byte offsets; table fields are original
exception-entry ranges. Varints, markers, line/column/range/coverage and exception range/
target/depth are strictly checked; truncation/overflow/unestablishable instruction
boundaries are rejected. 3.10+ empty line table gives empty mapping; 3.8–3.10 have no
exception table (hex empty, exceptions []), and stored field spans are not synthesized.
Original line/exception payload bytes are retained in both hex and metadata.fields.

Budgets: the static limits plus the selected `instructions` envelope, counted in
actual raw units (prefix/cache included) before decode. Ordinary CLI follows
ADR 0016; explicit fixed callers retain the default 100,000 instruction allowance.
No ceiling flag is exposed. Work continues from
parser/index/query and charges every scanned/raw projected unit, table/definition/child
visit and scalar byte evidence. Metadata scalars/incoming, raw rows, table/child rows and
hex payload have output lower-bound prechecks before expansion; the complete JSON plus
newline is then limited by actual writes. All output is complete and bounded before
atomic stdout/`-o` publication. Failure has no partial stdout, never replaces output or
modifies input. Perf resource_limits echoes the static allowances plus instructions;
instructions are raw units retained, work cumulative, ast_depth null; instructions outside the selected code
are not scanned and no AST is built.

## Name references

```sh
rxd pyc xref module.pyc --kind name --at marker
rxd pyc xref module.pyc --kind name -f MARK --within code:9
rxd pyc xref graph.marshal --version 3.12 --kind name --at cell -o target/xref.jsonl
```

Exactly one of `--at` or `-f/--filter` is required. `--kind name --at NAME`
interprets the anchor as a literal name (not a coordinate) and compares the original
payload against query UTF-8 bytes exactly. Empty names are allowed; `--case` does not
alter exact anchors. `--kind name -f TEXT` searches name payloads with the shared
substring rule, folding case unless `--case sensitive`. It never searches code-owner
names or opcode names. Unrepresentable surrogatepass names are not matched via a
lossy replacement, and names are never Unicode-normalized. Hits carry the actual
matched UTF-8 `detail.target_name`, including for exact anchors; constant hits have
that field null. A filter selecting multiple names still scans each body once.
Empty filters match all representable names; ordinary zero hits return a complete
empty stream. Constant content search remains `strings -f` followed by `xref --at`;
`xref -f` supports only `--kind name`.

`--within` accepts the `code:<index>` coordinate a row prints. Malformed scope is
`INVALID_QUERY`, missing scope is `NOT_FOUND` (both exit 2).
With it, only that code body is scanned; without it, all
distinct codes are scanned by global index and shared aliases are not rescanned. Same-name
owners and multiple real instructions in one owner are retained, ordered by code index then
actual instruction offset. The complete input marshal/index is still strictly parsed; a
narrow range only narrows instruction semantics, never input safety.

The same strict pycdc evidence decoder and get-code operand target resolver are reused; no
AST, source or unused code-view JSON is built. Prefix/cache produce no references; operand
flags/table rules are identical to get-code. Supported 3.12 opcodes/operations:

| Opcode | Operation |
| --- | --- |
| LOAD_NAME / STORE_NAME / DELETE_NAME | name_read / name_write / name_delete |
| LOAD_GLOBAL / STORE_GLOBAL / DELETE_GLOBAL | global_read / global_write / global_delete |
| LOAD_FAST, LOAD_FAST_CHECK / STORE_FAST / DELETE_FAST | local_read / local_write / local_delete |
| LOAD_FAST_AND_CLEAR | local_read_clear |
| MAKE_CELL / LOAD_CLOSURE | cell_create / closure_capture |
| LOAD_DEREF / STORE_DEREF / DELETE_DEREF | closure_read / closure_write / closure_delete |
| LOAD_ATTR, LOAD_SUPER_ATTR / STORE_ATTR / DELETE_ATTR | attribute_read / attribute_write / attribute_delete |
| IMPORT_NAME / IMPORT_FROM | import_name / import_from |
| LOAD_FROM_DICT_OR_GLOBALS / LOAD_FROM_DICT_OR_DEREF | dict_or_global_read / dict_or_closure_read |

3.8–3.11 LOAD_METHOD is attribute_read; LOAD_CLASSDEREF is class_closure_read, preserving
an explicit class namespace-or-closure read without inferring the runtime binding. 3.13
LOAD_FAST_LOAD_FAST is two local_read; STORE_FAST_STORE_FAST two local_write;
STORE_FAST_LOAD_FAST local_write then local_read; same-offset hits keep operand order.
3.13 has no LOAD_CLOSURE/KW_NAMES bytecode and uses actual FAST/CALL_KW/constant evidence.

Operations describe explicit opcode evidence; MAKE_CELL may read an existing local and
create a cell; LOAD_CLOSURE takes a cell for a closure; read_clear reads and clears a
local. Imports only name the instruction's name, not module execution/import hooks/
dispatch; CALL does not infer that a preceding load is the actual Python callee; an
attribute is not necessarily a method call. LOAD_CONST/RETURN_CONST/KW_NAMES only validate
table operands and produce no name hit.

### Refs schema v2

Header/reference/summary JSONL per ADR 0003. Header query: at/kind/filter/case/within/version
(optional null; kind=`name`/`constant`, case=`insensitive`/`sensitive`); detail: analysis/python_version/code_count/scope/instruction_validation;
analysis=`explicit_name_references`; scope=`all_codes`/`selected_code`; instruction_
validation=`scanned_scope`.

Reference meta: record/at/source_id/parent_at/context_at/start/end/detail; at/parent
null and source_id null (the header binds the input); context_at is the enclosing short
`code:<index>` usable by get-code --at. Name selector constant fields are null. start/end
is the real half-open file range of the actual 2-byte instruction inside the original
bytecode **definition payload** (PYC includes
the 16-byte header; raw starts at 0), excluding prefix/cache; shared bodies still use
definition bytes; short TYPE_REF tokens/code-relative offsets are never file spans. Detail:
code_index/offset/opcode/name/raw_operand/operand/target/target_name/operation/constant_id/
constant_slot/constant_path/literal_kind/relation. name is the raw opcode's version-table
name; raw_operand is the unexpanded byte; operand is the EXTENDED_ARG-folded value;
offset is code-relative. Target is table/index/id/value_id with the same identities as
get-code; `target_name` is the matched raw name and `query.at` is the literal name
only for exact name queries.

Summary records are real hit count; complete only means the selected selector's explicit
reference scan covered the selected scope, not source recovery or the get-code raw view.
Detail: analysis/code_count/scanned_code_count/instructions/unsupported_count/unsupported/
instruction_validation. instructions is the actual scanned raw-unit count including
prefix/cache; unsupported is always an array of code_index/context_at/offset/start/end/
opcode/name/status, where context_at is the short enclosing code coordinate. Safely
separable unknown/RESERVED/instrumented opcodes do not guess semantics;
status=UNSUPPORTED_OPCODE and summary.complete=false even with records=0.
Unsupported bodies outside the selection do not change narrow coverage. Any unreadable
body/bad instruction/prefix/cache/operand index/local kind/jump boundary in the scanned
range is `INVALID_INPUT` (exit 1), with atomic stdout/`-o`. Refs does not scan line/
exception semantics; get-code validates those.

Budgets inherit the selected static/marshal allowances and per-code `instructions`
envelope; entities count parser definitions/occurrences/index edges, with no new
AST entities. The per-code instruction ceiling is checked before decode. Work accumulates from
parser/index and charges actual decoder scans/raw projection, boundary construction/
checks, each supported operand resolution, name candidate/actual byte comparisons, hit/
diagnostic retention and serialization projection before expansion; it is not reset per
code. The shared metadata ceiling bounds retained hits and unsupported evidence; summary.records/
perf.records count only real hits. Output lower-bound guards run before appending
evidence; a bounded serializer then limits all header/reference/summary/newlines, and an
exact complete boundary may succeed. Budget/structure failures have no partial stdout and
do not replace output or modify input; failed perf preserves actual work/instructions/hits.

## Constant references

```sh
rxd pyc xref module.pyc --at 'const:9:2.0'
rxd pyc strings module.pyc -f marker
rxd pyc xref module.pyc --at 'const:9:2.0'
rxd pyc xref graph.marshal --version 3.12 --at "$CONST_AT" --within "$CODE_AT"
```

CONST_ID comes from a real strings row `at`, not a literal value. It accepts only the
`const:<owner>:<path>` coordinate; nothing expands it against a hash. Validation covers
kind=const, an existing owner code, canonical decimal top-level slot and every path
component. The path may traverse only tuple/frozenset and end at str/bytes;
containers, code, other literal kinds and mutable containers are outside the selected
str/bytes profile (`INVALID_QUERY`). Malformed IDs are `INVALID_QUERY`; a canonical
missing owner or path slot is `NOT_FOUND`. Path syntax is validated before looking up an
owner. The selected path is located from the
same parser evidence definition/children; equal literals are not traversed and all shared
aliases are not expanded; work scales with actual path depth. Shared payload keeps distinct
owner/slot/path identity. `.pyc` and raw marshal compute full digest and file spans
separately.

Without within, only the selected constant's owner is scanned and
header.detail.scope=`constant_owner`; with within, the existing single-code scope applies
(`selected_code`). If it does not contain the owner, complete=true, records=0,
scanned_code_count=0, instructions=0. An unused literal also yields a complete zero-hit
result after the owner scan. Unselected code is not semantically scanned; full input
marshal/index is always strictly parsed. Every real LOAD_CONST/RETURN_CONST/KW_NAMES
explicit constant operand in the same slot is retained separately, with operations
constant_load/constant_return/keyword_names; KW_NAMES takes the keyword-name container and
does not claim a Python operand-stack push. Raw prefix/cache or dynamically constructed
values are not references.

Reference detail.constant_id/path/slot is the selected strings occurrence; literal_kind is
its str/bytes kind; target is the instruction's **top-level consts slot** and raw value
identity, exactly matching get-code. A path of length 1 uses relation=direct_constant,
meaning direct reference to the literal slot; deeper paths use containing_constant, which
only says the instruction references the containing top-level constant and never claims
it directly loads the nested leaf. The caller's context_at routes to get-code --at. Equal values
in different slots/owners are not linked; multiple nested leaves in one containing slot may
share instruction evidence, but each query echoes only the selected occurrence. Ordering is
code index then instruction offset.

The common xref query retains at/kind/filter/case/within/version with absent selectors null;
constant header/summary detail.analysis=`explicit_constant_references`; the rest of the
fixed schema is shared with name selectors. Summary/perf records count only real hits;
unsupported evidence is counted separately and remains subject to the selected
work/metadata/output allowances, not a separate CLI record-count quota. MCP's
service record admission remains independent. Unknown/RESERVED/instrumented
opcodes make that owner's coverage incomplete. The supported matrix,
input safety, atomic output and non-goals are the same as name xref.

## Capability status and MCP

Per [ADR 0002](../adr/0002-backend-capability-closure.md), PYC ENUMERATE, SEARCH, XREF and
single-code FETCH apply with no format exemption. For the supported final 3.8–3.13 matrix,
the backend delivers code listing, constant search, single-code FETCH, explicit name
references and explicit constant-occurrence references, while retaining whole-module
source reconstruction. PYZ container ownership remains `rxd pyz`, still planned.
Control-flow deflattening, deobfuscation and decryption are permanent non-goals.

`pyc_codes/strings/xref/get_code/decompile` reuse the fixed input/evidence/index and
preserve native source/fallback/completeness. JSONL projects to header/records/summary;
single FETCH keeps its original record. Unsolvable raw text, code IDs and original spans
remain unchanged. The MCP `xref` tool mirrors at/kind/filter selectors and within scope;
resource limits map to input/entities/work/records/output/instructions where supported.

Shared snapshot, quota, cancellation and complete-response rules are in the
[MCP design](mcp-design.md); the decision is [ADR 0007](../adr/0007-typed-mcp-service.md).
CLI output and exit codes are unchanged by the MCP projection.
