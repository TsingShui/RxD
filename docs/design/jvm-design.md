# JVM frontend

Status: implemented.

`rxd jar` reads ordinary `.class` files and JAR/ZIP archives containing JVM class
files using pure Rust. The CLI namespace is `jar`; the backend and content IDs use
`jvm` to denote JVM bytecode semantics. Inputs are never executed or loaded, and no
Java, JADX, D8 or dx process is invoked. JADX is a development reference only; version
and license are in `crates/rxd-jvm/REFERENCE.md`.

The capability set follows [ADR 0002](../adr/0002-backend-capability-closure.md):
ENUMERATE=`classes`/`methods`/`fields`/`entries`, SEARCH=`strings`, XREF=`xref`, FETCH=`get-method` (unit) and
`get-class` (class aggregate).

## Backend boundary

`crates/rxd-jvm` owns fixed input, JAR selection, class metadata, JVM instructions and
stack data flow. Method bodies are lowered on demand to the radguard backend IR
(stack slots are registers; arguments occupy the end of the register file), reusing its
data-flow, structuring, declaration sinking and Java output. radguard is a general
backend (`radguard::backend::{ClassSpec, FieldSpec, MethodSpec, render_class,
render_method}`). JVM itself lowers JVM instructions to Statement/Value, splits basic
blocks at branch targets, folds constructor calls (`new` + `<init>`), and applies
LocalVariableTable/MethodParameters names and types. Undecodable methods degrade to an
`UnsupportedOperationException` stub during whole-class rendering; single-method
queries still return `UNSUPPORTED_BYTECODE`. There is no shared cross-JS/.NET IR.

## Commands and identity

| Command | Output |
| --- | --- |
| `jar classes [--class CLASS]`, `jar methods|fields [--class CLASS] [--name NAME] [-o FILE]` | header/match/summary JSONL, one object kind per command, `detail.analysis: metadata` |
| `jar get-method INPUT --at METHOD_ID [-o FILE]` | Decompiled Java declaration for one method |
| `jar get-method INPUT --at METHOD_ID --raw [-o FILE]` | Raw JVM instruction JSON with opcode, original bytes, byte_offset, operands, explicit constant references, branch targets and exception table |
| `jar get-class INPUT --at CLASS [-o FILE]` | Whole-class Java source (fields, constructors, all method bodies) by internal/dotted/descriptor name |
| `jar strings INPUT [--class CLASS] [--filter TEXT] [-o FILE]` | header/string/summary JSONL, `detail.analysis: metadata`; string literals in the selected class constant pool with class and pool index |
| `jar get-method INPUT --at COORD \| --from JSONL` | One method as Java, or its original instructions with `--raw`; `--from` reads the coordinate from a previous stream (header included) and refuses a stream from another input |
| `jar get-class INPUT --at COORD \| --from JSONL` | One class as Java, same selector |
| `jar xref INPUT --at COORD` | header/reference/summary JSONL; the coordinate is an exact identity (`Lx/y;`, `Lx/y;->name(proto)`, `Lx/y;->name:type`); `context_at` is the caller, `detail.target` the matched coordinate, `detail.byte_offset`/`detail.mnemonic` the reference site |
| `jar xref INPUT --from JSONL` | The same anchor, read from a previous stream's first `method`/`field`/`type` record, with the stream's content identity checked against this archive |
| `jar xref INPUT -f PATTERN --kind KIND` | the same rows for a substring search over the target's value, name or type, with `--class` narrowing to one exact owner class; `--kind` is required |

`classes`/`methods`/`fields` filter exactly by internal name, dotted name or descriptor,
and methods exactly by name while retaining all overloads; a method filter omits field
records. `header.query` records conditions; summary completeness applies only to the
selected query.

Only real files are accepted; directories, nested JARs, WAR and JMOD are not scanned.
Class and ZIP are identified by content, not extension. ZIP processing selects class
entries only and never modifies or extracts to a host path. Stored/deflate are
supported; multi-disk and ZIP64 inputs are rejected explicitly.

For a JAR declaring `Multi-Release: true`, `--release N` (default 8, minimum 8)
selects the highest version not above N for each logical class path, otherwise the base
entry. Without multi-release, `META-INF/versions/` is ignored. Logical paths are emitted
lexicographically; fields and methods keep class-table order. Duplicate class entries
at the same version or duplicate declared class names fail instead of selecting one.
Release selection does not verify runtime compatibility.

`header.source_id` is the input file SHA-256; class/method `source_id` is the actual
class-byte SHA-256. JAR records retain the physical entry. A method ID is
`rxd1:jvm:<class-sha256>:method:<method-table-index>`; the index is not a DEX
`method_ids` index. The same class in different containers has the same ID. `get`
validates the complete ID against the fixed input; wrong, stale or foreign IDs return
`INVALID_QUERY`.

`--raw` outer `start/end` point at Code bytes inside the class file, not the
compressed JAR. `byte_offset` is the original JVM byte offset within the method; bytes
concatenated in order reproduce the original body. One JVM instruction may expand to
several IR instructions whose addresses are `byte_offset * 32 + subinstruction index`,
used only for internal ordering, jumps and exception ranges; public output keeps
original JVM offsets.

Success plus final `summary.complete: true` means the selected metadata enumeration is
complete. It does not mean every method decompiles, that unknown attributes were
semantically analyzed, or that the JVM verified the class. `get` Java is a decompiled
representation, not original source. Abstract/native methods without Code can be
enumerated but have no method body to fetch. `--raw` does not decompile and can
inspect modern dynamic instructions. `get-class` builds one
`radguard::backend::ClassSpec` (class header, fields, each body); undecodable methods
become `UnsupportedOperationException` stubs (the same honest degradation as DEX
`get-class`) and never guessed Java. `get-method` renders one body, so an undecodable
method is returned as an explicit diagnostic rather than a stub. Whole-class
decompilation follows the 150 ms rule; only genuinely large single classes may use the
"inherently large target" exception in
[AGENTS.md](../../AGENTS.md#performance-requirements), with scope, measurements and
approval. There is no blanket JVM exception.

Known radguard backend limitations (type-family precision, declaration scope inside
monitor/try regions, supplementary-plane character escaping) are tracked in
`third_party/radguard/RAGUARD-FORK-ISSUE.md`.

Java text style is radguard's writer: opening brace on a new line, parenthesized
compound expressions, `java.lang` prefix omitted, fully qualified types without import
management. Declaration placement follows DAD rules: variables defined inside a
conditional branch are hoisted outside, but variables defined inside
`synchronized`/`try` and used outside remain declared inside (so the text is not
guaranteed to compile); this is frozen DAD behavior and is not fixed here.

`strings` lists only `CONSTANT_String` literals in the constant pool, not class names,
member names, descriptors or other Utf8 entries. `index` is the local pool index (JVM
has no global string table). `value` is decoded text; unpaired surrogates also retain
the raw `utf16` code units to avoid lossy conversion.

`xref` scans method instructions in every class and distinguishes four kinds:
string/type use case-sensitive substring matching on literals/type names; method/field
match exact member names, with an optional exact owner class via `--class`. Each hit
includes owner class, method, byte_offset, mnemonic and target and can be routed to
`get-method`/`get-class`. As an index query, `xref` follows the ordinary 150 ms
requirement; it is not aggregate decompilation.

Success writes stdout by default; `-o` atomically replaces the file and does not copy
to stdout. Any read, validation, analysis, budget or write failure preserves the old
output and emits no partial stdout. Exit 0 is success; `INVALID_QUERY` and argument
errors exit 2; other failures exit 1. Diagnostics go to stderr.

## Envelope (schema_version=2)

All structured output uses the shared [ADR 0003](../adr/0003-output-envelope.md)
constructors. The listings, `strings` and `xref` return header → zero or more
records → summary, with no legacy flat-field shim.

The header contains `record, backend, command, source_id, input_bytes, schema_version,
query, detail`. Streaming commands set `header.detail` to `analysis, release` with
`analysis: metadata`. `query` echoes command conditions. Summary contains `record,
records, complete, detail`; `classes` sets `summary.detail.classes`, while
strings/xref use `{}`.

Data records contain `record, id, source_id, parent_id, context_id, start, end,
detail`; inapplicable values are null. `source_id` is the actual owning class-byte
SHA-256. Class/field/string/reference have no FETCH ID (`id: null`); method/bytecode use
the content-bound method coordinate. There are no invented class coordinates, so `parent_at` is always
null. `reference.context_id` is the owning method ID and can be passed to `get-method`;
other records have `context_id: null`. Streaming `start/end` are null; method-relative
`byte_offset` is never treated as a class/JAR file range.

Fixed detail fields:

| Record | Detail fields |
| --- | --- |
| class | `access_flags, super, interfaces, major, minor, signature, source_file, fields, methods` (`at` is `Lclass;`) |
| field | `signature, constant_index` (`at` is `Lclass;->name:type`; modifiers come from `get-class`) |
| method | `proto, signature, exceptions, has_code` (`at` is `Lclass;->name(proto)`; modifiers come from `get-class`/`get-method`) |
| string | `class, index, value, utf16` (`index` = constant-pool index) |
| reference | `byte_offset, mnemonic, target` (`context_at` is the caller coordinate) |
| bytecode | `representation, entry, class, name, descriptor, max_stack, max_locals, handlers, instructions, complete` |

Field objects are flattened into detail; `constant_index` keeps the original constant
pool index. Member rows do not repeat `access_flags`: `get-class --at` renders the Java
modifiers (`public abstract class`, `private static final` field, `public static final`
method) and `get-method --at` renders one member, so a per-row copy would be the same fact
twice. Class rows keep it because the listing is the only place that states whether a class
is an interface, annotation or enum without fetching each class. `entry` is null for standalone class input. Signature/super/source_file/
constant_index/utf16 may be null. `fields`/`methods` are class-table totals. `utf16` is
null when decoding is lossless, otherwise an array of raw UTF-16 code units.

`get-method --raw` is one `record="bytecode"` FETCH record without header/summary;
it adds `backend="jvm"`, `command="get-method"`, `schema_version=2` outside. `detail.
representation` is always `bytecode`; `complete` is true. Outer `start/end` replaces
the old `file_range` and points at the actual class-file Code range (half-open), not
compressed container bytes.

Each bytecode instruction has `byte_offset, opcode, mnemonic, bytes, operands, targets,
constant_index, reference`; each exception-table entry has `start, end, target,
catch_type`. These are typed detail content, not independent FETCH records; offsets are
method-relative original JVM offsets. Inapplicable values are null.

Default `get-method` Java and `get-class` Java stay byte-stable outside the envelope;
they are decompiled source text, not structured JSON.

## Supported scope

Metadata reads the constant pool, superclass/interfaces, field/method descriptors,
generic Signature, ConstantValue, Exceptions, MethodParameters, SourceFile, Code,
exception table, line numbers and local-variable table. Unknown attributes are skipped
by bounded length; annotations, inner/enclosing metadata, StackMapTable and
BootstrapMethods do not yet participate in reconstruction. The frontend checks reachable
instruction stack shapes and does not use StackMapTable as a data-flow replacement.

Supported instructions include constants, locals and wide, numeric operations and
conversions, comparisons/jumps, both switches, array loads/stores and one-dimensional
allocation, fields, ordinary calls, object initialization, checkcast, instanceof,
throw and monitor. Narrow/wide stack values, branch merges and common dup patterns are
covered by real CLI fixtures; these are not proof of every instruction combination.

Java reconstruction returns `UNSUPPORTED_BYTECODE` for reachable invokedynamic,
dynamic/method-handle constants, multianewarray, qualified interface super calls and
jsr/ret, without guessing or emitting a throwing stub. Ordinary complex methods may
also be rejected by the semantic analyzer as `DECOMPILE_ERROR`. Full JVM instruction
semantics, every compiler pattern, cross-dependency complete type resolution and
whole-class Java output are not yet promised.

## Resources and validation

Defaults: input 8 GiB (lowerable; 32-bit spans keep actual addressable content below
4 GiB); class raw bytes and decoded metadata each 16 MiB; selected-entry expanded total
and archive metadata each 128 MiB; 10,000 archive physical entries; data-flow work
1,000,000; output 32 MiB. These are internal floors (`Limits::default`), not flags: the
CLI exposes no per-backend budget knobs, matching DEX. Work
covers frontend state copies, type representation and queue transfer, not every backend
pipeline operation. Code is limited to 1–65,535 bytes by the specification; descriptors
to 4,096 bytes, array dimensions to 255, parameters to 255 words (including `this` for
instance methods). Metadata strings are limited to 4,096 UTF-16 units; attribute
catalogs and per-method instruction materialization each to 16 MiB; string constants
retain full UTF-16.

One bounded buffer is read from an open regular file; hashing and analysis use those
actual bytes. ZIP central-directory budgets are checked before generic ZIP decoding;
entries are read bounded and size/CRC validated. JVM uses the existing isolated
analysis worker with a 32 MiB stack; the worker is not an OS sandbox.

E2E uses the real CLI with handwritten classes, stored/deflated JARs, Unicode, wide
values, branches/loops, constructors, switches, exceptions/synchronization, content
identity, failure atomicity, corrupt input, budgets and dynamic-instruction diagnostics.
Precompiled fixtures and fixed production JARs keep ordinary tests offline and JDK-free.
Trusted fixture Java is recompiled with `javac -proc:none` as an additional syntax check;
samples are not executed. This does not prove semantic equivalence. Production corpus
and independent javap expectations are in `tests/corpus/jvm/README.md`.

`benches/jvm.py` measures complete fresh CLI/worker wall time including real output,
per-workload samples, median, P95, >150 ms share, input sizes/hashes, hardware and cache
conditions. The 150 ms requirement applies; JVM adds no performance exception (rules in
[AGENTS.md](../../AGENTS.md#performance-requirements)).

## MCP projection

`jar_classes/methods/fields/strings/xref/get_method/get_class` keep the CLI namespace `jar` and
record `backend: jvm`. JSONL becomes header/records/summary; Java text gains a
representation/source/complete/source_id wrapper, while bytecode and Java fallback keep
their single-record shape. A container `target_id` does not replace class SHA-256 or
method content IDs; release selection, physical entry, method byte_offset and original
bytes are unchanged.

Shared snapshot, quota, cancellation and complete-response rules are in the
[MCP design](mcp-design.md); the decision is [ADR 0007](../adr/0007-typed-mcp-service.md).
CLI output and exit codes are unchanged by the MCP projection.
