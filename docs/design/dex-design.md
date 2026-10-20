# DEX backend

Status: implemented.

`rxd dex` analyzes Android APK/ZIP archives and standalone DEX files. Analysis lives
in `crates/rxd-dex/src/`; CLI parsing, dispatch and output live in `src/cli/dex/`.
The shared Agent skill is installed with `rxd skill install`; there is no DEX-only
skill entry. `rxd apk` is a visible alias with the same arguments, dispatch, output and
exit codes. The backend never installs or runs the application and never calls an
external d8/dx tool.

## CLI contract

Structured output follows [ADR 0003](../adr/0003-output-envelope.md),
`schema_version=2`. rasc's pipe-delimited text protocol is intentionally not
preserved; argument positions and exit-code semantics are.

| Command | Query and output |
| --- | --- |
| `entries INPUT` | Archive entries in central-directory order with compression and sizes; duplicate names are preserved. |
| `strings INPUT` | String-table order with entry, index and value; keeps `--filter`, `--offset`, `--limit`. |
| `xref INPUT --at COORD` | References to one identity, matched exactly: `Lx/y;` a type, `Lx/y;->name(proto)` a method, `Lx/y;->name:type` a field, and, with `--kind string`, a literal. `--kind` is inferred from the coordinate and must agree with it. `context_at` is the referring member, `detail.target` the matched coordinate, `detail.entry` the DEX it lives in. One row is one (caller, target) pair, so `detail.target` is always a single coordinate. |
| `xref INPUT --from JSONL` | The same anchor, read from a previous stream's first `method`/`field`/`type`/`string` record. The stream header's `input_bytes` is checked against this archive: that catches a stream from another file, but it is a mismatch guard, not a content identity. |
| `xref INPUT -f PATTERN --kind KIND` | Content search over the target's value, name or type, and `--class` narrows to one exact owner class (descriptor, dotted or slashed form; never a pattern). `--kind` is required because a content search cannot say what kind it looks for. Rows keep the one-target-per-row shape, so a caller that matches several targets is reported once per target. |
| `manifest INPUT` | Decode AndroidManifest.xml to XML; ordinary XML keeps its original bytes. |
| `classes INPUT` | Class descriptors; `--class` selects one exact class, `-f/--filter` is a substring over the dotted name, folded unless `--case sensitive`. |
| `fields INPUT` | Field declarations as `Lclass;->name:type`; `--class`, `--name` (exact), `-f/--filter`. |

`get-class` and `get-method` select by `--at` (the coordinate a `classes`/`methods` row
prints) or `--from JSONL`, which reads that coordinate from a previous stream. The stream
is checked against the archive size, because a DEX header carries no content hash; that is
a mismatch guard, not an identity. The positional Dalvik-name form was removed so every
FETCH has one selector spelling. APK/DEX commands
keep `--threads` / `--thread`
alias and supported range. The default thread count comes from
`available_parallelism`; zero threads is an error. All diagnostics go to stderr. `--perf`
emits the shared perf record as JSONL; the per-entry trace is behind `RXD_TRACE`.

`--perf` carries the library's own stage profile in the shared record: `stages_ns` holds
the same map/zip/select/inflate/header/members/scan/render/engine/emit/manifest stages,
`counters` holds bytes/classes/code_items/hits, and `extra` holds the DEX entry counts
(`entries`, `dex`, `classes`, `code_items`, `hits`, `strings`). Stages are declared once
(`crates/rxd-dex/src/profile.rs`) so two records are comparable key by key. Stage times
are summed across the parallel workers (a 97-DEX archive reports inflate as the total of
every worker's inflate work); counters are aggregated work units. With profiling off every
call site costs one relaxed load and no clock read.

The per-entry trace (`[APK] … skipped`, per-entry inflate/process, per-command record
counts) is developer output, not part of the contract, and is enabled with `RXD_TRACE=1`.
It stays off by default so stderr carries only the perf record.

`-o` writes the same bytes as stdout and still emits stdout. This differs from the
JS/ASAR file-output contract: DEX keeps its original direct write and does not promise
atomic rollback on failure. Input and output must be different files.

Normal queries exit 0 (including zero references or results); input/analysis/write
is still exit 1. Consumers must not assume that a non-zero exit implies empty stdout.

DEX uses the CLI streaming path directly and preserves SIGPIPE/broken-pipe behavior;
JS/ASAR keep their isolated worker and forward stdout after complete success. Neither
executes input code; DEX has no OS sandbox or extra worker.

## Inputs and lookup

Inputs are APK/ZIP or standalone DEX. Root `classes*.dex` is preferred; if absent,
root `*.dex` is used. Nested DEX files are not scanned. For duplicate DEX names the
first central-directory entry is used; the last Manifest entry is used. `entries`
retains every physical entry. ZIP64, encrypted ZIP and unsupported compression methods
are out of scope.

DEX 041 class/reference/member resolution keeps the logical member name
`physical.dex!classesN.dex`; single members keep the physical name. `strings` keeps
physical entry names; string indices may repeat across logical members, so a row alone
does not identify a unique logical member. `--method-index` is a `method_ids` index;
`--field-index` is an instance-first runtime slot, while output `field_index` is the
DEX table index. They are not interchangeable.

Java output is a decompiled representation, not original source. Unrecoverable methods
may become `UnsupportedOperationException` stubs from radguard and should be cross
checked with references/member data. Known radguard output limitations are tracked in
`third_party/radguard/RAGUARD-FORK-ISSUE.md`.

## Envelope (schema_version=2)

constructors and return header → zero or more records → summary; empty results still
candidates at exit 4; `summary.complete=true` means candidate enumeration is complete,
not that resolution is unique. General success requires exit 0 and `complete=true`.
3/4 produce no stdout.

The header contains `record, backend, command, source_id, input_bytes, schema_version,
query, detail`, with `backend=dex`. `header.detail` and `summary.detail` are `{}`.
`input_bytes` is the sum of supplied input file sizes (repeated inputs count again).
`query` fields: `classes`=`filter`; `strings`=`filter, offset, limit`; `entries`=`{}`;
`xref`=`kind, at, filter, class`;
to Dalvik form. Inapplicable query values are null. Summary contains `record, records,
complete, detail`; `records` counts data records only.

Data records contain exactly `record` and `detail`, plus the one navigation key that
applies (`at` for a class/method/field row, `context_at` for a reference row). The other
nullable keys of ADR 0003 are omitted, not repeated as `null`; the ADR's
"absent and null are equivalent" rule is what makes that legal, and it saves 39% of a
class row and ~10 MB on a wide `xref`. The header still carries
backend/command/source_id once, and DEX deliberately does not rehash large archives,
avoiding extra reads against the 150 ms budget. Class identity and routing come from
detail `descriptor`/`name` (and `entry`); `xref.detail.member` carries the referring
member's Dalvik descriptor, directly usable with `get-class`, and its declaring class is
the part before `->` rather than a repeated field.

Fields derivable from another field are not repeated: `package`/`simple_name` follow
from `descriptor`/`name`, and `xref.class` follows from `member`. On a 928,795-class
APK this halves the payload (`classes` 296.5 MB → 151.8 MB) and removes ~95 ms of JSON
assembly; measured medians on that input: `classes` 500.7 → 406.1 ms.

Fixed detail fields:

| Record | Command | Detail |
| --- | --- | --- |
| class | classes | `at` = the class descriptor |
| field | fields | `at` = `Lclass;->name:type`; `detail` is empty (modifiers come from `get-class`) |
| method | methods | `at` = `Lclass;->name(proto)`; `detail` is empty (modifiers come from `get-class`/`get-method`) |
| string | strings | `entry, index, value` (`index` = DEX `string_ids` index) |
Ordering: records follow the archive central-directory order, and inside one entry the
`class_defs` (classes) or code-item (xref) order. No backend sorts by identity, so a class
list is comparable with `js`/`jar` output and never depends on a global sort. A descriptor
defined by more than one entry appears once per entry; duplicate detection is the caller's
`uniq`, not a hidden merge.

| reference | xref | `target, entry` |
| entry | entries | `name, compression, compressed_size, uncompressed_size` |

`class.name` is dotted; `package` keeps DEX slash separators. `string.index` is the
DEX `string_ids` index and `value` is decoded text. `strings` enumerates the global
string table, including names, descriptors and unreferenced strings, without owner
classes; `string.class` is always null. `reference.member` keeps `CLASS->MEMBER`;
`matched` is the target text without the old `matched=(...)` wrapper. Repeated
references within one member are listed once; ordering is preserved.
`entry.compression` is stored/deflated/other; sizes are bytes. `field.slot` is the
instance-first runtime slot, `field_index` is the DEX `field_ids` index, and
`method_index` is the DEX `method_ids` index.

Access flags are not repeated on `methods`/`fields` rows: `get-class --at` renders the
whole class as Java (`public abstract class`, `private static final` field, `public static
final` method) and `get-method --at` renders one member's modifiers, so a per-row integer
would be the same fact spelled twice. On the 5.19M-method corpus that omission is also
what keeps `methods` at 679 MB instead of ~731 MB.

and Manifest XML stay outside the envelope and are byte-stable; JSON and Java are never
mixed. `-o` copies stdout exactly. Input/analysis errors emit no success envelope.

## Resources and dependencies

The original CLI's mmap reads, on-demand inflation, DEX prefix optimization and
deterministic parallel order are preserved. The input should not change during a CLI
invocation. MCP uses ADR 0007's private immutable snapshot and explicit `target_id`
without caching parse/decompile state.

The default inflate limit per selected entry is 256 MiB. The compatibility variable
`RASC_MAX_INFLATED_ENTRY` accepts a non-zero `usize`; 0, empty and unparseable values
use the default. Range and size are checked while reading; exceeding the limit fails
without silently skipping. This is not a whole-input, result-byte or peak-RSS limit.

DEX and JVM share `radguard::backend` (method/class rendering) as a normal path
dependency; `third_party/axmldecoder` is a separately configured patched dependency
excluded from the root workspace and mapped through `[patch.crates-io]`. Licenses,
fork notes and patch notes are retained. Parser, decompiler IR and backend indices stay
in the DEX backend.

radguard source generation is Java-only: no language selection or generic backend
dispatch, and batch calls accept class descriptors. Kotlin compiler output heuristics
remain Java-side; not every input is promised to recover successfully or semantically
equivalently. Existing ABI preparation, cache revision, termination analysis and nested
class collection order are preserved.

## Validation

`tests/cli.rs` includes `tests/dex/` scenarios that run the real rxd and cover all
non-MCP commands, stored/compressed APKs and standalone DEX, consistent output,
member indices, ambiguity exit codes, DEX 041, Manifest, inflate budgets, bad input and
rejection of the retired skill entry. Shared skill installation/update/printing and
single-file offline installation are covered by root CLI E2E. Small binary fixtures
come from authorized handwritten generators. Existing backend/dependency regression
tests remain; real APK tests stay explicit opt-in and a skip is not a pass.

`benches/dex.py` measures fresh-process end-to-end wall time on explicitly supplied
authorized APK/DEX inputs and records per-query samples, median, P95, >150 ms share,
input hashes/sizes, hardware and cache conditions. The 150 ms requirement applies to
ordinary index/reference/manifest/member queries. Whole-class decompilation may exceed
150 ms only under the "inherently large target" ground in
[AGENTS.md](../../AGENTS.md#performance-requirements), with a scoped input hash,
release measurements and approval; ordinary index/reference queries are not exempt.
rasc schema-v1 byte comparisons are not used for the schema-v2 JSON protocol.

## MCP projection

`dex_get_class/entries/strings/xref/manifest/classes` reuse existing parsing and
result assembly in isolated workers. JSONL projects to header/records/summary;
Java/XML is wrapped with representation/source/complete/source_id. Original null
id/source_id semantics remain; a target only pins input and does not claim entity
closure.

Shared snapshot, quota, cancellation and complete-response rules are in the
[MCP design](mcp-design.md); the decision is [ADR 0007](../adr/0007-typed-mcp-service.md).
CLI output and exit codes are unchanged by the MCP projection.

## Performance baseline

Measured on a 523 MiB production APK (97 root DEX entries, 928,795 classes,
4,939,532 code items, 725 MiB of DEX once inflated), release build, three runs,
median:

| command | records | output | wall |
| --- | ---: | ---: | ---: |
| `classes` | 928,795 | 78 MB | 0.165 s |
| `methods` | 5,194,250 | 679 MB | 0.36 s |
| `methods --class <one class>` | 1 | tiny | 0.11 s |
| `xref -f java/lang/String --kind type` | 108,126 | 15 MB | 0.20 s |

`xref` matches the member name as a substring and `--class` as one exact owner class
(ADR 0014). These numbers were taken while `--class` still folded into the same substring
search; an exact owner is strictly narrower than the wide row below, so the shapes it
covers can only have gotten cheaper. Same APK, release, five runs, median (minimum in
parentheses):

| command | wall |
| --- | ---: |
| `xref -f LIZ --class 'Lkotlinx/coroutines/internal/StackTraceRecoveryKt;' --kind method` | 0.156 s (0.148 s) |
| `xref -f LIZ --class a/a --kind method` (wide substring; no longer reachable) | 0.219 s (0.196 s) |
| `xref -f Toast --kind method` (name only) | 0.206 s (0.185 s) |
| `xref -f String --kind type` | 0.250 s (0.245 s) |
| `xref -f Authorization --kind string` | 0.196 s (0.165 s) |
| `classes` (same run, for reference) | 0.180 s (0.172 s) |

The class-constrained query stays the cheap shape (0.156 s against 0.206 s for a name-only
query) because `bounded_by_class` still enables the per-entry prefix pre-check; that
pre-check resolves the same query against the entry tables, so it stays correct for an
exact `--class`. Wide `xref` shapes were already 0.20 s before this change.

The fixed cost is inflating the entries: ~115 ms of wall time across 10-12
threads. Everything above that scales with the bytes a command prints, so
`sorted`/`deduped` output would only have added cost - a class index is reported
in archive order for exactly this reason.

Two optimizations were measured and rejected, and must not be retried without
new evidence:

- **Prefix inflation for the class index.** The last class descriptor sits at 81%
  of an entry both on this 97-entry APK and on a 13-entry one, so the saving is
  ~19% of the inflate work; a streaming inflater is slower per byte than
  libdeflate, which makes the net negative.
- **Batched rendering for `methods`.** Rendering entries in bounded batches
  instead of collecting every payload measured 0.53 s (16-entry batches) and
  0.44 s (64-entry batches) against 0.36 s collected, while peak RSS fell only
  from 797 MB to 692 MB: the peak is dominated by inflated DEX buffers, not by
  the rendered payload.
