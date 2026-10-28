# Standalone Ark reconstruction

`rxd-ark` owns its parser, metadata reader, decoder and reconstruction. It has
no dependency on `rxd-abc`. The validated format code and ISA table were copied
from the RxD tree at `0091440`; the old reconstructor and JS postprocessor were
not copied or called. Removing that crate does not remove this crate's sources.

The production `rxd ark` CLI and reflected MCP tools use this engine directly.
Unsupported semantics fail explicitly; there is no old-engine fallback.

## Pipeline

- Immutable ABC/HAP input, identity/checksum/version validation and bounded
  metadata admission. Only dynamic ABC **12.0.6.0** is admitted.
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

Async lowering requires the complete promise frame, matching await/resume
sequence, rejection throw dispatch and unchanged exception coverage. Branches
into a protocol's interior fail. Register snapshots and separate property reads
are retained rather than assuming getters are pure.

Exception plans retain encoded handler priority, exception-to-accumulator writes,
loop exits and one emission owner per block. A region is structured only when its
higher-priority handlers are nested; otherwise it retains residual dispatch.
Native protection may cross a bridge
only when its surviving IR cannot throw. Residual blocks omit catch wrappers when
they cannot throw, and leave enclosing native handlers to their lexical scopes.
Finally cleanup remains explicit control flow; native finally recognition and
abrupt-completion cleanup merging are not implemented by this milestone.

Class/super construction, `definemethod`, generators, array spread and for-in
normalization remain unsupported. Those targets return `DECOMPILE_FAILED`,
including when a required nested definition is unsupported; no empty bodies or
raw fallback are emitted. Per-method analysis and function lowering errors include
the actual method coordinate. Class-sensitive own stores require a proven ordinary
receiver.

`getiterator` is reconstructed using standard JavaScript iterator semantics. Its
helper reads the iterator property once, calls it with the original receiver and
requires an object result. Each affected reconstruction returns the typed warning
`ARK_ITERATOR_SEMANTICS` alongside its source and places the warning as a JS
comment immediately before every affected iterator operation. `decompile_target`,
`decompile_method` and `decompile_record` return `Reconstruction { source, stats,
warnings }`; raw instructions are unaffected. This contract
does not promise identical behavior for every Ark runtime's edge cases or modified
JS globals.

Recovered source uses explicit lexical/module contexts and JavaScript intrinsic
bridges. Module aliases share binding identity; record contexts share an emitted
`Symbol.for` hole sentinel. The sentinel is forgeable and mutable globals such as
`Reflect` and `Object` affect bridges, so they are not equivalent to private VM
intrinsics under arbitrary host mutation. Output has not been executed or proved
runtime-equivalent; cyclic module initialization and bridge behavior still require
semantic review.

## Reference pins and licenses

- OpenHarmony runtime-core `b4b18c9800ed567f840572eebe94307a47ec48a6`,
  Apache-2.0: file-format/accessor definitions and `isa/isa.yaml`.
  The retained `resources/isa.yaml` SHA-256 is
  `3743c6782ef254e14f32b27b7f23cbb87ae2aa035e5a543021724bbf41f8b66b`.
  Its Huawei Device notices are retained; `src/opcodes.rs` contains 323 encodings.
- OpenHarmony ets_runtime `930f00041dff23e3ca97e18cabf8d7b9b2c4ca2e`,
  Apache-2.0: interpreter, runtime stubs and method call-frame conventions.
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
- Oxc **0.153.0**, MIT: allocator, AST, span/string, syntax and codegen.
  All six direct dependencies are pinned equally; source maps are disabled.
  Registry checksums are retained in `Cargo.lock`.

See `VALIDATION.md` for the actual rewrite coverage and reproduction command.
