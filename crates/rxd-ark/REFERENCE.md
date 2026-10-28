# Standalone Ark reconstruction

`rxd-ark` owns its parser, metadata reader, decoder and reconstruction. It has
no dependency on `rxd-abc`. The validated format code and ISA table were copied
from the RxD tree at `0091440`; the old reconstructor and JS postprocessor were
not copied or called. Removing that crate does not remove this crate's sources.

This is an independent rewrite milestone, not the production CLI cutover.
`rxd ark` still uses the existing adapter while missing semantics are ported.

## Pipeline

- Immutable ABC/HAP input, identity/checksum/version validation and bounded
  metadata admission. Only dynamic ABC **12.0.6.0** is admitted.
- Semantic IR with source PCs, block-local value IDs, explicit register and
  captured-cell reads/writes, receiver-bearing calls, effects and exception edges.
- Constant propagation, exception-aware slot liveness, dead stores, use counts,
  evaluation-order materialization and a must-analysis for ordinary receivers.
- CHK dominators, reverse-CFG postdominators, natural loops and single-owner
  structured plans. Shared predicate tails become short-circuit conditions;
  difficult local regions use state dispatch without duplicating shared blocks.
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
loop exits and one emission owner per block. Native protection may cross a bridge
only when its surviving IR cannot throw. Residual blocks omit catch wrappers when
they cannot throw, and leave enclosing native handlers to their lexical scopes.
Finally cleanup remains explicit control flow; native finally recognition and
abrupt-completion cleanup merging are not implemented by this milestone.

Class/super construction, generators, array spread and for-in normalization
remain unsupported. Those targets return `DECOMPILE_FAILED`, including when a
required nested definition is unsupported; no empty bodies or raw fallback are
emitted. Class-sensitive own stores require a proven ordinary receiver.

Recovered source uses explicit lexical/module contexts and JavaScript intrinsic
bridges. Module aliases share binding identity; record contexts share an emitted
hole sentinel. Output has not been executed or proved runtime-equivalent; cyclic
module initialization and bridge behavior still require semantic review before
CLI migration.

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
  Descriptor copy follows `RuntimeCopyDataProperties`, not `Object.assign` or
  object spread. `jeqz`/`jnez` use boolean/numeric tests, not generic truthiness.
- xpanda `8f27d3ce8b63581c7eb5203bbfdfbb92e28cfccf`, Apache-2.0:
  architectural reference for semantic/value graphs and structure analysis,
  not an implementation dependency or a correctness oracle.
- Oxc **0.153.0**, MIT: allocator, AST, span/string, syntax and codegen.
  All six direct dependencies are pinned equally; source maps are disabled.
  Registry checksums are retained in `Cargo.lock`.

See `VALIDATION.md` for the actual rewrite coverage and reproduction command.
