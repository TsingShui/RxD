# Standalone Ark reconstruction

`rxd-ark` owns its parser, metadata reader, decoder and reconstruction. It has
no dependency on `rxd-abc`. The format code and initial ISA table were copied
from the RxD tree at `0091440`; the old reconstructor and JS postprocessor were
not copied or called. Removing that crate does not remove this crate's sources.

The production `rxd ark` CLI and reflected MCP tools use this engine directly.
Best-effort semantic bridges and residual control flow carry source-local warnings;
all 332 pinned opcode encodings have lowering paths, including legacy forms.
There is no old-engine fallback. Listings use the shared `# ` TAB-column header,
including empty successes; text cells are JSON-quoted. Source/raw FETCH remains
plain, and CLI/MCP carry the same bytes.

## Pipeline

- Immutable ABC/HAP input, identity/checksum/format validation and bounded
  metadata admission. Dynamic versions are not gated by an exact version number.
- Semantic IR with source PCs, block-local value IDs, explicit register and
  captured-cell reads/writes, receiver-bearing calls, effects and exception edges.
- A worklist propagates must-equal parameter snapshots, primitive constants and
  boolean facts over normal and per-throw exception edges. Joins retain facts
  only when all paths agree; captured cells remain mutable. Exception-aware
  liveness includes explicit throw terminators as well as throwing IR events.
  Ordered definitions are materialized unless their single use is a direct local
  write or terminal value without intervening observable events; this keeps
  nested assignment targets, keys and callees from preceding earlier effects.
- CHK dominators, reverse-CFG postdominators, natural loops and single-owner
  structured plans. Shared predicate tails become short-circuit conditions;
  single-write diamonds become select expressions, and dead boolean bridges
  can merge with their following branch. Difficult regions use state dispatch.
  Validated protected regions emit native try/catch, including nested handlers;
  ambiguous/shared-entry regions retain scoped dispatch and last-resort whole-body fallback.
- Direct Oxc AST construction and codegen, including imports/exports and closure
  contexts. There is no generated-JS parser, old emitter, generic minifier or
  production Node/xpanda invocation. Cross-block phi construction is still pending.

Canonical async lowering requires a complete promise frame, matching await/resume
sequence, rejection throw dispatch and unchanged exception coverage. Other valid
frames retain explicit promise/resume operations and carry `ARK_ASYNC_FRAME`.
Async generators use a queued JS async-generator driver with separate internal
suspension and externally yielded iterator results. Multiple generator identities
share one continuation with an explicit `ARK_COROUTINE_ALIASING` warning; independent
VM frame snapshots are not recovered. Unknown function kinds retain their encoded
call-type frame and carry `ARK_FRAME_CONVENTIONS`.
Register snapshots and separate property reads are retained, not assumed pure.
Writable implicit function, new-target and receiver registers have independent
mutable shadows initialized from their call frame. `ldthis` still reads the VM
frame receiver; `ldnewtarget` reads its actual register when one is present.

Exception plans retain encoded handler priority, exception-to-accumulator writes,
loop exits and one emission owner per block. A region is structured only when its
higher-priority handlers are nested; otherwise it retains residual dispatch.
Native protection may cross a bridge
only when its surviving IR cannot throw. Residual blocks omit catch wrappers when
they cannot throw, and leave enclosing native handlers to their lexical scopes.
Finally cleanup remains explicit control flow; native finally recognition and
abrupt-completion cleanup merging are not implemented by this milestone.

Class literals retain constructor/member dependencies, instance/static order and
lengths. Native JS class shells validate heritage and delegate to recovered bodies
with explicit function, receiver, new-target and home-object contexts. Super calls
use `Reflect.construct`; super property access uses the retained home object.
`definemethod`, accessors and class-sensitive stores retain descriptors; emitted
class markers are tracked in WeakSets. External class markers, method constructibility,
reflection, VM error details and modified globals remain best-effort, not equivalent.
`ARK_CLASS_RECONSTRUCTION` marks the affected statements and constructor factories.

Synchronous generators retain the bytecode's explicit next/throw/return branches.
A native generator carries the reconstructed frame; a driver primes the initial
suspension at call time and supplies Ark resume modes 2/1/0. Yielded iterator-result
objects pass through, while completion is wrapped once. `ARK_GENERATOR_RECONSTRUCTION`
marks frame creation, suspension and resume reads: private VM identity, prototypes,
reflection and cross-generator method borrowing are not reproduced exactly.

For-in uses a suspended native enumeration rather than an eager own-key array.
`ARK_PROPERTY_ENUMERATION` warns about enumeration timing, proxies and mutations.
Array spread preserves UTF-16 code-unit strings, target writes, result descriptors
and the returned insertion index; non-string input uses JS iterator selection,
not unobservable Ark fast-path tags (`ARK_ARRAY_SPREAD`). Apply and super spread
use JS iteration (`ARK_SPREAD_CALL`). Difficult control flow retains state dispatch
and handler priority with `ARK_CONTROL_FLOW_RECONSTRUCTION`; warning never means
that an empty body or disassembly was substituted for source.

BigInt and regex literals, object rest, template objects, iterator closing and
async iteration use directly constructed JS ASTs and focused intrinsic bridges.
Regex flags follow the pinned VM bit assignments. Template arrays retain raw-array
identity, descriptors and freezing. Object rest preserves key conversion, exclusions,
getters and descriptors. Legacy forms retain their explicit register operands,
function ranges, module bindings and definition environments rather than changing
frame state to create a closure. Nested dynamic literal arrays are bounded and
cycle-checked; class buffers accept method and scalar/accessor properties.

Private fields use per-object hidden storage and explicit lexical keys; private
getter/setter functions are called with their receiver. Sendable field metadata,
local environments and concurrent results remain explicit, but VM sharing, field
type enforcement, lexical class lookup and scheduling are approximate. Lazy module
loads use ordinary eager JS imports. Global/patch operations retain explicit binding
records; name collisions and non-scalar globals use a warned runtime-binding bridge.
`ARK_PRIVATE_PROPERTIES`, `ARK_SENDABLE_SEMANTICS`, `ARK_LAZY_MODULE_TIMING`,
`ARK_RUNTIME_BINDINGS`, `ARK_ITERATOR_OPERATIONS` and `ARK_JS_INTRINSICS` identify
these limits at affected operations. The unimplemented comparison branches in the
pinned runtime are reconstructed according to their opcode names with
`ARK_BRANCH_SEMANTICS`. Typed catches follow that runtime's first encoded handler,
not an invented type test (`ARK_EXCEPTION_TYPES`); overlapping ranges use dispatch.

Dynamic ABC versions use the structure and opcode definitions, not a fixed-version
admission list. The older header literal index and the newer reserved sentinel are
both recognized. Version labels outside the pinned runtime's declared compatibility
range carry `ARK_VERSION_SEMANTICS` on each source factory. Unknown instructions
and unmodelled layouts fail explicitly; static ABC and typed-array formats require
a different decoder. Named-call error formatting and sendable-local module timing
use warned JavaScript bridges. Corrupt input, invalid references, cyclic literal
expansion and resource failures remain explicit errors. Line-number index bounds
are checked at admission; a method's debug entry and target offset are validated
when its instructions are fetched or reconstructed. Unrelated methods and
unreferenced stale debug entries are not traversed for that query. Per-encoding coverage is not proof of correct behavior for every
instruction combination, runtime path or unusual program.

`getiterator` is reconstructed using standard JavaScript iterator semantics. Its
helper reads the iterator property once, calls it with the original receiver and
requires an object result. Each affected reconstruction returns the typed warning
`ARK_ITERATOR_SEMANTICS` alongside its source and places the warning as a JS
comment immediately before every affected iterator operation. `decompile_target`,
`decompile_method` and `decompile_record` return `Reconstruction { source, stats,
warnings }`; raw instructions are unaffected. This contract
does not promise identical behavior for every Ark runtime's edge cases or modified
JS globals. Warning kinds are also returned as a deterministic per-query vector;
CLI and MCP preserve the comments in source, without diagnostic lines on stderr.

Recovered source uses explicit lexical/module contexts and JavaScript intrinsic
bridges. Module aliases share binding identity; record contexts share an emitted
`Symbol.for` hole sentinel. The sentinel is forgeable and mutable globals such as
`Reflect` and `Object` affect bridges, so they are not equivalent to private VM
intrinsics under arbitrary host mutation. Output has not been executed or proved
runtime-equivalent; cyclic module initialization and bridge behavior still require
semantic review.

## Reference pins and licenses

- OpenHarmony runtime-core `6b3a0de5e971b667a1e46e17283595df67a99f0b`,
  Apache-2.0: current format/accessor definitions and `isa/isa.yaml`.
  The retained `resources/isa.yaml` SHA-256 is
  `690d960793e07d18c94d81a644e916592bb29962c48c7e2f5f48376989f530f9`;
  its Huawei Device notices are retained, and `src/opcodes.rs` has 332 encodings.
  Earlier format and interpreter evidence also uses runtime-core
  `b4b18c9800ed567f840572eebe94307a47ec48a6`.
- OpenHarmony ets_runtime `930f00041dff23e3ca97e18cabf8d7b9b2c4ca2e` and
  `edc5231fdbc81d0d7477d03347c0ea44408f6726`, Apache-2.0: interpreter, runtime
  stubs, named calls, sendable-local bindings and method call-frame conventions.
  `ecmascript/method.cpp::Method::FindCatchBlock` selects the first matching
  encoded try/catch entry; region-length sorting is not its selection rule.
  `RuntimeLdObjByValue` and `RuntimeStObjByValue` perform property-key coercion
  at their respective instructions, in bytecode order. The ordinary interpreter's
  `RuntimeGetIterator` returns a non-callable iterator property; the generated ASM
  stub throws instead. Later OpenHarmony ASM stubs also check certain non-object
  results. The ISA does not choose a runtime path, so the selected JavaScript
  semantics are accompanied by an explicit warning rather than claimed equivalent
  to every runtime implementation.
- ECMAScript [2024](https://tc39.es/ecma262/2024/multipage/ecmascript-language-expressions.html#sec-evaluate-property-access-with-expression-key)
  converts a computed assignment key before the RHS; [2025](https://tc39.es/ecma262/2025/multipage/ecmascript-language-expressions.html#sec-evaluate-property-access-with-expression-key)
  specifies conversion after it. Emitting an earlier read as a separate statement
  avoids relying on that version-dependent assignment detail.
  Descriptor copy follows `RuntimeCopyDataProperties`, not `Object.assign` or
  object spread. `jeqz`/`jnez` use boolean/numeric tests, not generic truthiness.
- xpanda `8f27d3ce8b63581c7eb5203bbfdfbb92e28cfccf`, Apache-2.0:
  architectural reference for semantic/value graphs and structure analysis,
  not an implementation dependency or a correctness oracle.
- Pinned `jspandafile/class_info_extractor.cpp` and `literal_data_extractor.cpp`
  define class property partitioning and method-affiliate lengths;
  `method_literal.cpp::ParseFunctionName` strips compiler name prefixes/suffixes.
  `js_generator_object.{h,cpp}` and `generator_helper.cpp` define resume modes,
  suspended-start abrupt completion and iterator-result wrapping.
  `template_string.cpp`, `regexp/regexp_parser.h` and `global_env_constants.h`
  supply template descriptors, regex flag bits and error-message constants.
- Oxc **0.153.0**, MIT: allocator, AST, span/string, syntax and codegen.
  All six direct dependencies are pinned equally; source maps are disabled.
  Registry checksums are retained in `Cargo.lock`.

See `VALIDATION.md` for the actual rewrite coverage and reproduction command.
