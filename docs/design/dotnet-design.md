# .NET IL backend

Status: implemented.

`rxd dotnet` is the Agent-facing static entry for ordinary managed PE assemblies. It
reads assemblies without loading or executing target code and does not require a
user-installed .NET runtime. Metadata parsing uses a pinned dotscope commit; C# method
and class reconstruction uses a pinned ILSpy `ICSharpCode.Decompiler` NativeAOT shared
library through a stable C ABI. The native library is an optional distribution
dependency enabled by `csharp-decompiler`; the default Cargo build does not involve
.NET. It only decompiles and never loads/executes the analyzed assembly.

See [ADR 0009](../adr/0009-dotnet-ilspy-v11.md) for the current engine pin and
[ADR 0010](../adr/0010-prebuilt-ilspy-runtimes.md) for prebuilt runtime handling.

## Distribution and engine loading

`cargo build --features bundled-ilspy` selects `prebuilt/ilspy/` by Rust `TARGET`,
validates the manifest/archive SHA-256 and current native C#/project/NOTICE identity,
then generates embedded data without a .NET SDK, network access or NativeAOT build.
Missing targets, corrupt bundles or stale source artifacts fail the build explicitly.
macOS supports ARM64 only, not x64. See `prebuilt/README.md`.

`scripts/build-ilspy.py <rid>` is the explicit producer/update path: validate the
pinned NuGet package, build the NativeAOT shared library, and create a deterministic
archive with the engine, .NET 10 reference assemblies and licenses. Then
`scripts/prebuilt-ilspy.py add <rid>` validates and imports it.
`scripts/build-bundled.py --features bundled-ilspy` builds the default backends with
embedded C# and Kuna as a single binary. No CI assumption is part of the runtime
contract.

`bundled-ilspy` implies `csharp-decompiler`. The first C# FETCH validates and atomically
extracts `<cache-base>/rxd-ilspy/<archive-sha256>/` through `rxd-core::bundle`, then
loads and validates the library on demand with `libloading`. Metadata queries and
`--il` do not extract it. Kuna shares the cache implementation under a separate
namespace. Corrupt caches fail; a mismatched library is never executed. The optional
`rxd_set_reference_directory` ABI supplies the cached reference metadata path without
changing process environment or probing input directories. Explicit
`RXD_DOTNET_NATIVE_LIBRARY` and `RXD_REFDIR` take precedence; the external-library
development mode remains available. Reference assemblies are for static metadata
resolution only; runtime does not depend on a .NET SDK/runtime.

`prebuilt/android-ilspy/` separately stores an ARM64 Bionic engine, private OpenSSL,
reference metadata, C ABI headers and licenses for App embedding. It is validated with
`scripts/prebuilt-ilspy.py --android-app check`, is not registered in the desktop CLI
runtime index, and does not enable a full rxd Android build. Consumers provide their
own App JNI/NDK adapter; probe APKs and signing material are not distributed.

## Inputs and identity

The backend accepts DLL/EXE with readable metadata and IL, identified by content.
NativeAOT machine code, mixed-module native methods, single-file bundle unpacking and
cross-assembly resolution are out of scope. Metadata may enumerate abstract/PInvoke
members without IL, but `get-method` requires IL.

One bounded binary buffer is read from a regular file and hashed with SHA-256;
extension, path and MVID never replace content identity. dotscope's automatic PE
repair result is rejected; rewritten bytes are never labeled original input. Entity ID
format is `rxd1:dotnet:<sha256>:<kind>:<8-lowercase-hex-token>` where kind is `type`,
`field` or `method`, bound to TypeDef, FieldDef or MethodDef. Method IDs keep the
original contract; members carry `declaring_type_id`. ID lookup validates backend,
content hash, kind, token table and row range.

Parsing first validates metadata table/heap layout, references and ranges, then reads
a method body on demand; it does not build a whole-assembly `CilObject`, SSA or decode
all custom attributes. dotscope's minimal raw validation plus this backend's structure/
reference/budget checks form the operation's validation. Token validation retains all
upstream logic and caches only successful checks of the same token on the immutable
reference graph; provenance and changes are in
`crates/rxd-dotnet/vendor/dotscope-validation/NOTICE`. This is not a claim of CLR
executability, complete custom-attribute decoding, signature verification or all
cross-assembly semantics. Queries are complete within their declared scope and never
silently ignore corrupt data.

## Commands and protocol

```sh
rxd dotnet outline app.dll --type Example.Worker --method Run
rxd dotnet get-method app.dll 'rxd1:dotnet:HASH:method:06000001'
rxd dotnet get-method app.dll 'rxd1:dotnet:HASH:method:06000001' --il
rxd dotnet get-class app.dll 'rxd1:dotnet:HASH:type:02000002' --il
rxd dotnet strings app.dll --contains 'example'
rxd dotnet refs app.dll --token 0x0a000001
rxd dotnet refs app.dll --string 'example'
rxd dotnet refs app.dll --id 'rxd1:dotnet:HASH:method:06000001'
rxd dotnet refs app.dll --token 0x2b000001 --semantic
```

- `outline` emits header, type/field/method and summary JSONL. Header echoes filters;
  order follows metadata tokens. `--type` matches the full type name exactly;
  `--method` matches a method name exactly and preserves overloads.
- `get-method` emits one JSON record with method, source_id, raw `il_hex`, instructions,
  exception handlers, locals and original file range. Instruction `offset` is the IL
  byte offset within the method code; file offsets are separate and never mixed.
- `get-method --il` returns `representation: il`. Default `get-method` attempts
  reconstruction. Success returns `representation: csharp`, `decompiled: true`,
  structured C# code and a mapping from output lines to real IL offsets. After async/
  iterator recovery, MoveNext's independent offset domain is not used as the entry
  method mapping; mapping may be empty.
- An unsupported method returns `representation: il`, `decompiled: false`, an explicit
  reason, `code: null`, and all original IL/EH. `complete: true` means the record is
  complete, not that reconstruction succeeded. Partial guessed code is never returned.
- `get-class INPUT TYPE_ID --il` returns the selected TypeDef and recursively nested
  types under explicit NestedClass ownership, including generated types; sibling and
  inherited definitions are excluded, and `<Module>` is rejected. Types retain
  class/interface/struct/enum kind, base token, interfaces, generic parameters and
  constraints; member inventory retains fields, constructors, methods, properties and
  event accessor relationships. Abstract, PInvoke and other declarations without
  ordinary IL keep `body_present: false` and null body fields. Every readable method
  includes complete original IL, instructions, locals, file range and ordered EH.
  Default whole-class invocation selects the TypeDef directly in the pinned ILSpy
  NativeAOT engine and returns whole-class `representation: csharp`, `decompiled:
  true` and complete C#; properties/events/generated members may use C# syntax sugar,
  while original inventory and all IL/EH remain complete. A directly selected
  generated type is also handled by TypeDef. Engine disabled/unavailable returns
  `class_engine_unavailable`; an older library without the class ABI returns
  `class_abi_unavailable`; reconstruction failure or invalid response/mapping returns
  an explicit reason, `decompiled: false`, `code: null`, empty mapping and complete
  original inventory. `--il` sets reason null and bypasses the native engine.
- `strings` scans the `#US` heap and all readable IL methods, emitting one record per
  `(heap offset, method token)` with `il_offsets` preserving all ordered, unique
  `ldstr` offsets. `context_id` routes to get-method; `detail.declaring_type_id` routes
  to get-class; duplicate heap entries keep independent identity. An unreferenced
  entry emits exactly one `status: unreferenced` inventory record with null context/
  owner/method and `offsets: []`; no FETCH route is invented. Valid UTF-16 becomes a
  JSON string; isolated surrogates retain original `utf16_units` with `value: null`
  and no lossy replacement. Empty strings, NUL and supplementary Unicode are
  preserved. `--contains` matches UTF-16 subsequences (empty needle matches all) with a
  bounded linear algorithm.
- `refs --string VALUE` matches UTF-16 values exactly and case-sensitively across all
  equal heap entries and every original `ldstr` site, ordered by method token then IL
  offset. It is mutually exclusive with `--token`; exactly one target is required.
  When CLI UTF-8 argv cannot express NUL or isolated surrogates, use the original
  `token` returned by strings with `refs --token`. String SEARCH/XREF still validates
  the whole heap and readable methods even with zero hits; corrupt/resource failures
  never become a complete zero result.
- `refs --token TOKEN` matches the original operand token exactly. `refs --id
  METHOD_ID/FIELD_ID/TYPE_ID` and `refs --token TOKEN --semantic` query provable
  member/type identity. Exactly one selector is required; `--semantic` only modifies
  token mode. Missing/conflicting selectors, bad entity IDs and non-member/type
  semantic tokens are `INVALID_QUERY`.
- Method definition queries include provable MethodSpec instances and local MemberRefs;
  a MethodSpec/constructed-owner MemberRef keeps owner and method arguments distinct.
  A MemberRef merges into a local MethodDef only when the fixed module scope, type
  ownership, member name, calling convention flags, generic arity, sentinel, return
  type and all parameter structural signatures match. Constructed-owner generic
  parameters are substituted once, never twice; TypeRef/TypeSpec inside signatures
  retains scope, structure and actual arguments, and equal display names are never a
  proof.
- Module-scope TypeRefs and explicit nested TypeRefs may merge into local TypeDefs; an
  AssemblyRef is this assembly only with exact full name/version/culture/flags/key
  identity. Full public keys and public-key tokens are not equated without proof; a
  ModuleRef with the current module name is not guessed local. Legal but unprovable
  targets keep `external`/`unresolved`/`ambiguous` status and descriptor, match by
  original token or complete provable descriptor, and have canonical ID null; multiple
  candidates never select the first. MethodDef-parent vararg call-site optional
  parameters keep their original signature and unresolved state.
- Method XREF covers every explicit method-bearing operand: call/callvirt/jmp/newobj/
  ldftn/ldvirtftn and method ldtoken. It retains original token, method-relative IL
  offset, opcode, caller method/type IDs, and provable canonical identity, scope and
  complete structural signature. It reports only explicit instruction relationships in
  the fixed input; reflection, cross-assembly loading and virtual dispatch are not
  inferred.
- Field XREF covers explicit field-bearing operands (ldfld/ldflda/stfld/ldsfld/ldsflda/
  stsfld and field ldtoken). A MemberRef merges into a local FieldDef only with matching
  owner/scope, member kind, exact name and complete substituted field signature; a
  field-definition query includes provable owner instances, while a constructed-owner
  query keeps arguments. Same-name different-signature/owner/scope fields stay
  distinct; multiple candidates remain ambiguous.
- Type XREF covers only explicit TypeDef/TypeRef/TypeSpec instruction operands,
  including `constrained.` and type ldtoken; it does not expand to declarations,
  signatures, inheritance, attributes or EH catch types, and never infers a type hit
  from every member-call owner. TypeDef queries include provable named aliases/
  constructed instances; TypeSpec queries retain the full AST (class/valuetype, array
  shape, pointer, modifiers and arguments). Explicit class/valuetype in named/
  constructed signatures must match the proven local TypeDef kind; owner and internal
  references use the same rule, and contradictions are `INVALID_INPUT`. External
  unproven kinds are not inferred by name; array/pointer/generic-variable roots keep
  unresolved structural descriptions and never invent canonical TypeDefs.

`-o` atomically writes the same bytes as stdout. Failure emits no partial stdout and
does not overwrite an existing output. Diagnostics go to stderr; `INVALID_QUERY` exits
2; input/resource/I/O failures exit 1.

## Envelope (schema version 2)

Structured output follows [ADR 0003](../adr/0003-output-envelope.md). JSONL queries
emit exactly one header, all data records and a `complete: true` summary; header and
summary `detail` are always present. Refs `header.detail.target` retains the queried
member/type descriptor (null for raw token mode); other header/summary extras are `{}`.
The header retains `backend`, `command`, `source_id`, `input_bytes`,
`schema_version: 2`, `query`; the summary retains `records` and `complete`.

Every data record has `record`, `id`, `source_id`, `parent_id`, `context_id`, `start`,
`end`, `detail`; absent values are null. `source_id` is the fixed PE buffer SHA-256.
Fixed detail fields:

| Record | Detail fields |
| --- | --- |
| `type` | `token, name, namespace, full_name, flags, kind, declaring_type_token, declaring_type_id, extends_token, interfaces, generic_parameters` |
| `field` | `token, declaring_type_token, declaring_type_id, declaring_type, name, signature, flags` |
| `method` | `token, declaring_type, declaring_type_token, declaring_type_is_value_type, name, signature, parameters, signature_flags, return_type, has_this, has_body, rva, flags, impl_flags, declaring_type_id, generic_parameters` |
| `string` | `offset, token, value, utf16_units, status, method_token, declaring_type_id, il_offsets` |
| `reference` | `method_token, declaring_type, declaring_type_id, name, il_offset, opcode, target_token, target_kind, canonical_target_id, canonical_target_token, resolution_status, scope, owner_token, original_owner_token, owner_name, target_name, signature, type_signature, owner_arguments, method_arguments` |
| `method_body` | `method, body_present, representation, decompiled, reason, code, mapping, file_offset, code_offset, code_size, work_used, max_stack, init_locals, locals, local_signature_token, instructions, handlers, il_hex, complete` |

Type/field/method content-bound IDs live only in outer `id`; string id is null. A
top-level type has parent null; nested types use the direct owning type ID. Field/
method/method_body parents are the declaring type ID. `reference.id` is null;
`context_id` is the caller method ID, directly usable by get-method, while its original
token and IL offset live in detail. `reference.parent_id` and
`reference.detail.declaring_type_id` are the caller's declaring type, usable for
whole-class FETCH; other outline records have `context_id: null`.

`string.parent_id` is the using declaring type and `context_id` the using method; an
unreferenced inventory record has both and detail method/type IDs null. Its start/end
are always null; one group may contain multiple non-contiguous use sites. Heap offset,
user-string token and method IL offsets are independent domains and never PE file
ranges. Groups sort by heap offset then method token; offsets are in original IL order.
All statuses have the same fixed detail keys.

`reference.detail.target_kind` is semantic `method`/`field`/`type`, string route
`string`, otherwise `token`. `signature` is non-null only for method semantic targets
(flags, generic_arity, sentinel, return_type, parameters); `type_signature` only for
field/type semantic targets, using structural AST kind/element/token/value/arguments.
Raw/string set both null but keep the same fixed key set. Named tokens in the AST merge
only within the fixed input by full scope and never prove cross-assembly loading or
external definitions. `header.detail.target` has target_kind/original_token/
canonical_token/canonical_id/resolution_status/scope/owner_token/original_owner_token/
owner_name/name/signature/type_signature/owner_arguments/method_arguments; a zero-hit
semantic query still keeps the descriptor.

`get-method` returns one `method_body` record without header/summary; the top-level
FETCH envelope carries `backend="dotnet"`, `command="get-method"`,
`schema_version=2`. `detail.complete` states completeness; `detail.method` is the same
full method envelope as outline. Outer start/end is the original PE code range
`[code_offset, code_offset + code_size)`, excluding method header and EH table;
`reference.start/end` is the original PE byte range of that IL instruction.
Outline/string use null when no such range exists. `detail.file_offset` is the method
body header; `detail.code_offset` is the code start. Heap offsets, tokens and method-
relative IL offsets are never file positions.

`get-class` is one `class_body` envelope with `backend="dotnet"`,
`command="get-class"`, `schema_version=2`; nested records use standard bodies and do
not repeat command identity. Fixed detail: `type, types, fields, properties, events,
methods, work_used, representation, decompiled, reason, code, mapping, complete`.
Class mapping entries are `method_id, method_token, method IL offset, 1-based line`,
sorted/deduplicated by token/offset/line. Each must fall on real instruction boundaries
and C# output lines; bodyless methods cannot have mappings. Points that cannot be
proven after async/iterator recovery are omitted; empty mapping is not reconstruction
failure. `type` is the root type envelope; `types` contains all selected types ordered
by token; `fields`/`types` use outline envelopes; `methods` are method_body envelopes
identical to single FETCH, with `body_present` distinguishing declarations from
readable bodies and null body fields when absent. Nested method representation is
always `il/false/null code`; `class_body` owns whole-class reconstruction state.
`class_body` has no single contiguous range, so start/end are null.

Property detail: `token, declaring_type_token, declaring_type_id, name, signature,
signature_hex, flags, accessors`; event detail: `token, declaring_type_token,
declaring_type_id, name, event_type_token, flags, accessors`. Both have null id/start/
end and parent/context set to the declaring type. Accessors retain `method_token,
method_id` and original MethodSemantics `semantics`, ordered by method token and
semantics. Generic parameter entries: `token, number, flags, name, constraints`
(original TypeDefOrRef token list). Properties, events, interfaces and generic
relationships are verified from metadata tables, never guessed by name.

This is an explicit schema migration; no flat or same-name subobject output is kept.
Consumers read content from `detail` and route references to methods through
`context_id`.

## Structured C# reconstruction

The production path produces structured, source-like C# by calling the pinned ILSpy
`ICSharpCode.Decompiler` NativeAOT shared library:

1. **Input and identity (Rust)**: fix the input buffer, validate content identity and
   tokens, parse metadata/ranges (`metadata.rs`, `il.rs`). `get-method`/`get-class`
   pass the same fixed buffer and validated MethodDef/TypeDef token to the marshal
   layer.
2. **C ABI marshal layer**: export `rxd_decompile_method(buf_ptr, buf_len, token,
   out_len)`, optional `rxd_decompile_class(buf_ptr, buf_len, type_token, out_len)`,
   `rxd_decompile_assembly(buf_ptr, buf_len, out_len)` and `rxd_free`. Input is a fixed
   memory buffer and token; output is a UTF-8 byte buffer. Rich objects never cross the
   boundary. `UniversalAssemblyResolver` uses explicit search paths; cross-assembly
   resolution is out of scope.
3. **ILSpy engine**: `CSharpDecompiler` performs IL→ILAst→C#, recovering loops,
   conditions, switches, expressions and syntax sugar. The marshal layer returns text
   to Rust, which adapts it to this backend's JSON contract.

The shared library is an in-process self-contained native library (dlopen/link), not a
subprocess tool, and does not add a CoreCLR runtime dependency. Rust enables the C ABI
via `csharp-decompiler`; `build.rs` only locates the prebuilt library and never calls
dotnet. To keep the CLI starting without the library, the engine is not hard-linked;
render dlopens/dlsyms on demand and returns `engine_unavailable` with complete IL/EH
when missing or disabled. See `benches/ilspy/README.md`.

### Scope and degradation

The engine recovers complete structured C# for ordinary methods, including async/await
and iterator `yield` state machines; lambdas, closure capture classes, local functions
and delegate construction sugar; LINQ query expressions (fixture-verified from/where/
select restoration); and collection/object/array initializers, `using`/`lock`, null
propagation and string interpolation. Expression-tree support depends on the engine.

Default `get-method` attempts reconstruction. Success returns `representation:
csharp`, `decompiled: true`, structured C# and mapping. When the engine errors or
produces no result, the whole record degrades to `representation: il`, `decompiled:
false`, explicit reason, `code: null`, and complete original IL/EH; no partial guessed
code. Degradation is a correctness guarantee: `complete: true` means the record is
complete, not that C# reconstruction succeeded. Failed compiler-generated members
remain indexable, queryable and extractable.

## Exception regions and continuation

Provably nested/disjoint try/catch/filter/finally/fault regions are supported with
original clause order. In `get-method --il`, the exception table is the ordered method
dispatch table with original half-open IL ranges; kind, catch token, filter entry and
handler ranges determine dispatch.

C# reconstruction restores provable try/catch/finally to structured statements and
`leave` to normal control flow. Regions the engine cannot safely restore fall back to
original IL and the exception table. Async/lambda/iterator/LINQ recovery is engine-owned;
generated members remain indexable, queryable and extractable.

## Budgets and performance

Defaults: input 8 GiB (lowerable; 32-bit spans keep actual addressable content below
4 GiB), entities 100,000, work 16,777,216, instructions per method 100,000, records
100,000, output 32 MiB. Each has a positive `--max-*` override. Entities count types,
fields and methods; work bounds parsing stages; refs charge actual decode, signature,
string and EH work. Full refs/strings scans share one fixed `#US` heap validation and
reuse entry boundaries; FETCH shares instruction/token-kind/locals/parameter/branch/
prefix/EH validation and skips IL hex and non-token mnemonic allocation when not
needed. Every readable method is validated; a corrupt method is never skipped to
produce a complete zero result. Loop/depth and cumulative work failures are explicit.
Budgets are checked before allocation, entity traversal, method decode and output,
including JSONL header/summary and complete FETCH IL. Whole-class work accumulates
metadata parsing, ownership/member scanning, each method decode and pre-reconstruction
signature validation; instruction limits stay per method. Whole-class records count
the outer class_body, root type, nested types/fields/properties/events and each
method_body plus its method, checked before decoding. Serialized body volume is
computed before keeping it; the complete envelope has a separate output limit, and
errors never become successful degradation. The native ABI has fixed 64 MiB input and
32 MiB encoded-JSON caps; native input/output limits and query budget exhaustion are
atomic `RESOURCE_LIMIT`, never successful degradation. Native allocations are released
exactly once with matching `rxd_free` on legal, illegal and over-budget responses.
Recursive nesting is at most 64; worker behavior is unchanged, with no new wall-clock
timeout or adjustable time budget. Termination is reported as failure.

Generic substitution charges the complete inserted AST storage before copying; deep
copies of owner, MethodSpec and return descriptors charge work first. Signature
descriptors are size-checked with a bounded writer before building JSON Value; budget
failure keeps empty stdout and the existing file. Public envelopes and whole-class
inventory move generated JSON maps/arrays instead of copying complete bodies again;
no original IL, identity or validation is reduced. SHA-256 uses locked sha2 0.11.0 with
hardware acceleration when available, otherwise software; identity remains SHA-256.

`--perf` uses the analysis/CLI stderr JSONL sidecar with content identity, input/output
sizes and fresh-worker cost including dlopen/NativeAOT initialization. Release
acceptance uses ordinary real assemblies and reports per-workload samples, median, P95,
>150 ms share, sizes, hardware and cache conditions; artificial fixtures are boundary
regressions only. Failed correctness/reconstruction results never prove decompiler
performance. Benchmarks and pins are in `benches/dotnet.py` and
[ADR 0009](../adr/0009-dotnet-ilspy-v11.md). The engine is pinned to v11.1.0.9782.
Whole-function/class reconstruction may exceed 150 ms only under the third-party
engine or inherently-large-target grounds in
[AGENTS.md](../../AGENTS.md#performance-requirements); a small target must remain fast
and any exception needs its own input hash, selected target, measurements and bound.

`benches/dotnet_class.py` validates selected whole-class IL and C# using an independent
Newtonsoft.Json 13.0.3 net6.0 DLL and PEReader expectations; `benches/dotnet_refs.py`
and the field/type XREF runner use independent metadata expectations. `--perf` reports
stage timings separately from real serialization and stdout/atomic writes. C#
reconstruction success rates from coverage runs do not prove correctness and are never
inflated by filtering successful methods.

## Development references

- dotscope: `3028b1ab92ec1b0ad15f046b3b70d5ee82336a00` with default features off;
  `Cargo.lock` pins transitive dependencies. Used for metadata/IL parsing, content
  identity and token validation.
- C# engine: official `ICSharpCode.Decompiler` NuGet `11.1.0.9782` (ILSpy repository
  commit `a6909b2e1062d2404a52711926fa74030bd96119`, package SHA-256
  `1f76df4d35193ba4eeb50ffcd84eae381a2c48c67f95c1104dc406759448a811`), built as a
  self-contained NativeAOT shared library with `net10.0`, `NativeLib=Shared` and
  `InvariantGlobalization=true`. ILSpy is MIT-licensed; its C# source is not copied.
  v11's `IAssemblyResolver.BeginSnapshot` delegates to the underlying resolver; the
  existing reference allow-list, fixed input, C# 10 output settings, budgets, mapping
  and failure contracts are unchanged.
- NativeAOT cross-OS compilation is unsupported; each `OS×arch` builds on its target
  OS. The project maintains macOS ARM64 only, not macOS x64. Release CI builds on Linux
  x86_64/arm64, Windows x86_64 and macOS arm64.
- The marshal layer exports the C ABI above; Rust uses feature-gated `libloading` with
  each platform's native loader. See [ADR 0009](../adr/0009-dotnet-ilspy-v11.md).
- `benches/dotnet.py` and `benches/dotnet_ilspy.py` measure the shipped native engine
  through the real CLI. Each workload has at least 20 fresh processes; dylib identity,
  C# evidence and original IL/EH preservation are recorded. Unexpected degradation or
  a bound violation is reported, never counted as performance success.
- Exception continuation rules cross-check Microsoft `leave`/`endfinally`; legal but
  unsupported cross-region jumps degrade completely.
- Handwritten PE/CLI fixtures, independent IL expectations, generators and fixed-SDK
  sugar fixtures (async/iterators/captured lambdas/local functions/LINQ) are in
  `tests/fixtures/dotnet/`. `tests/dotnet/sugar.rs` validates complete original IL/EH at
  the real CLI seam; C# restoration tests compile only with `--features
  csharp-decompiler`, assert syntax evidence and do not snapshot formatting. Ordinary
  E2E is offline, SDK-free and never executes assemblies.

JS deobfuscation and string decryption remain permanent non-goals.

## MCP projection

`dotnet_outline/strings/refs/get_method/get_class` reuse metadata, semantic reference
and FETCH implementations. JSONL projects to header/records/summary; single FETCH keeps
its original record. Actual MethodDef/TypeDef IDs, original UTF-16 code units, original
IL/EH and C# representation/decompiled/reason/mapping are preserved; `complete` does
not mean reconstruction succeeded. Protocol startup/tool listing/metadata/`il: true`
does not load ILSpy; default C# initializes the engine on demand in the worker and
degrades explicitly when unavailable.

Shared snapshot, quota, cancellation and complete-response rules are in the
[MCP design](mcp-design.md); the decision is [ADR 0007](../adr/0007-typed-mcp-service.md).
CLI output and exit codes are unchanged by the MCP projection.
