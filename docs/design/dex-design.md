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
| `get-class INPUT CLASS` | Java decompilation text by dotted name or Dalvik descriptor; `--members` prints a field/method index table. |
| `entries INPUT` | Archive entries in central-directory order with compression and sizes; duplicate names are preserved. |
| `strings INPUT` | String-table order with entry, index and value; keeps `--filter`, `--offset`, `--limit`. |
| `findrefs INPUT string\|type VALUE` | Literal string or type references, with referring member and matched target. |
| `findrefs INPUT method\|field [NAME] --class CLASS` | Member name and declaring class; keeps `--fuzzy-class`; at least one of name/class is required. |
| `manifest INPUT` | Decode AndroidManifest.xml to XML; ordinary XML keeps its original bytes. |
| `classes INPUT` | Class descriptors, dotted names, package and simple names; `-f/--filter` is case-insensitive. |
| `fields-plan --apk INPUT... --descriptor CLASS` | One class's `field_plan` JSON envelope, without header/summary. |
| `member-by-index --apk INPUT... --descriptor CLASS --method-index N\|--field-index N` | Resolve a runtime member index and retain all ambiguous candidates. |

APK/DEX commands keep their original argument positions, `--threads` / `--thread`
alias and supported range. The default thread count comes from
`available_parallelism`; zero threads is an error. `findrefs -o` sits in the
string/type/method/field subcommand arguments. All diagnostics go to stderr. `--debug`
is preserved; DEX adds no `--perf` flag.

`-o` writes the same bytes as stdout and still emits stdout. This differs from the
JS/ASAR file-output contract: DEX keeps its original direct write and does not promise
atomic rollback on failure. Input and output must be different files.

Normal queries exit 0 (including zero references or results); input/analysis/write
errors exit 1; argument errors exit 2. `fields-plan`, `member-by-index` and
`get-class --members` preserve ambiguity semantics: 3 means no result, 4 means
ambiguous. `member-by-index` still prints all candidates at 4, while `fields-plan` and
`get-class --members` refuse to select a definition. A missing class for `get-class`
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

`classes`, `strings`, `findrefs`, `entries` and `member-by-index` use the shared
constructors and return header → zero or more records → summary; empty results still
include header/summary. `member-by-index` emits zero records at exit 3 and all
candidates at exit 4; `summary.complete=true` means candidate enumeration is complete,
not that resolution is unique. General success requires exit 0 and `complete=true`.
`fields-plan` emits one `field_plan` object only for a unique class definition; exits
3/4 produce no stdout.

The header contains `record, backend, command, source_id, input_bytes, schema_version,
query, detail`, with `backend=dex`. `header.detail` and `summary.detail` are `{}`.
`input_bytes` is the sum of supplied input file sizes (repeated inputs count again).
`query` fields: `classes`=`filter`; `strings`=`filter, offset, limit`; `entries`=`{}`;
`findrefs` string/type=`kind, value`, method/field=`kind, name, class, fuzzy_class`;
`member-by-index`=`descriptor, method_index, field_index`, with descriptor normalized
to Dalvik form. Inapplicable query values are null. Summary contains `record, records,
complete, detail`; `records` counts data records only.

Data records contain `record, id, source_id, parent_id, context_id, start, end,
detail`. DEX deliberately does not rehash large archives, avoiding extra reads/hashing
against the 150 ms budget; header/record `source_id` is null and all other navigation
fields are null. Class identity and routing come from detail `descriptor`/`name` (and
`entry`). `findrefs.class` is the referring member's Dalvik descriptor, directly usable
with `get-class`, not the matched target's class.

Fixed detail fields:

| Record | Command | Detail |
| --- | --- | --- |
| class | classes | `entry, descriptor, name, package, simple_name` |
| string | strings | `entry, index, value, class` |
| reference | findrefs | `entry, class, member, matched` |
| entry | entries | `name, compression, compressed_size, uncompressed_size` |
| field | member-by-index --field-index | `entry, class, name, type, access_flags, static, field_index, slot` |
| method | member-by-index --method-index | `entry, class, name, access_flags, method_index` |
| field_plan | fields-plan | `descriptor, dex_class_def_idx, dex_type_idx, class_access_flags, instance_fields, static_fields, counts, instance_ref_mask` |

`class.name` is dotted; `package` keeps DEX slash separators. `string.index` is the
DEX `string_ids` index and `value` is decoded text. `strings` enumerates the global
string table, including names, descriptors and unreferenced strings, without owner
classes; `string.class` is always null. `reference.member` keeps `CLASS->MEMBER`;
`matched` is the target text without the old `matched=(...)` wrapper. Repeated
references within one member are listed once; ordering is preserved.
`entry.compression` is stored/deflated/other; sizes are bytes. `field.slot` is the
instance-first runtime slot, `field_index` is the DEX `field_ids` index, and
`method_index` is the DEX `method_ids` index. `access_flags` is an integer.

`field_plan` is one JSON object without header/summary, with
`backend="dex"`, `command="fields-plan"`, `schema_version=2`. Instance/static field
entries contain `field_index, name, type, access_flags`; `counts` contains `instance,
static`; `instance_ref_mask` is a hex string where bit j corresponds to instance slot
j, up to 64 bits.

`get-class` Java and `get-class --members` text (including the member comment table)
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

`dex_get_class/entries/strings/findrefs/manifest/classes/fields_plan/member_by_index`
reuse existing parsing and result assembly in isolated workers. JSONL projects to
header/records/summary; Java/XML is wrapped with representation/source/complete/
source_id. Original null id/source_id semantics remain; a target only pins input and
does not claim entity closure. `fields_plan`/`member_by_index` accept a mutually
exclusive input or ordered inputs, preserve duplicates and complete ambiguous
candidates, and report NOT_FOUND/AMBIGUOUS separately. Only these two native
multi-input operations can use an open DEX bundle.

Shared snapshot, quota, cancellation and complete-response rules are in the
[MCP design](mcp-design.md); the decision is [ADR 0007](../adr/0007-typed-mcp-service.md).
CLI output and exit codes are unchanged by the MCP projection.
