# Dynamic Ark ABC references

The Rust implementation is owned by RxD. It currently admits dynamic ABC
12.0.6.0, not static ArkTS bytecode or an unverified range of ABC versions.

## Official format and ISA

- Repository: <https://github.com/openharmony/arkcompiler_runtime_core>
- Revision: `b4b18c9800ed567f840572eebe94307a47ec48a6`
  (`OpenHarmony-v5.0.0-Release`). Its `isa/isa.yaml` declares `12.0.6.0`.
- References: `docs/file_format.md`, `libpandafile/file{,-inl}.h`,
  `class_data_accessor*`, `method_data_accessor*`, `field_data_accessor*`,
  `code_data_accessor*`, `literal_data_accessor*`, `index_accessor.h`,
  `file_items.h`, and `helpers.h`.
- `resources/isa.yaml` is an unchanged copy of the pinned official ISA;
  SHA-256: `3743c6782ef254e14f32b27b7f23cbb87ae2aa035e5a543021724bbf41f8b66b`.
  `python3 scripts/gen-abc-opcodes.py --check` verifies the generated table of
  323 encodings. No external YAML/Node tool is needed for production builds.
- License: Apache-2.0; Huawei Device Co., Ltd. copyright notices apply to
  upstream material. Retain those notices when deriving or generating code.
- The dynamic method-region index is shared by method, string and literal
  operands. The method flags select its index header in bits 16–31; this can
  differ from the region containing the method. Bits 8–15 encode function kind.
  Class/method code pointers are tagged values, not flag-gated fields.
- MUTF8 lengths count UTF-16 units, not bytes. Surrogate pairs use six bytes;
  lone surrogates must not silently become replacement characters in output.

## Dynamic runtime semantics

- Repository: <https://github.com/openharmony/arkcompiler_ets_runtime>, tag
  `OpenHarmony-v5.0.0-Release` at commit `930f00041dff23e3ca97e18cabf8d7b9b2c4ca2e`;
  `ecmascript/interpreter/interpreter-inl.cpp`, `ecmascript/stubs/runtime_stubs-inl.h`,
  `ecmascript/jspandafile/{method_literal.h,method_literal.cpp,js_pandafile.cpp}`
  and `ecmascript/js_function.cpp`.
- `jeqz` branches for boolean false or numeric zero; `jnez` branches for true
  or numeric nonzero. Neither instruction is generic JS truthiness.
- Runtime property stores use strict assignment semantics; generated method
  bodies carry `"use strict"` so a failed assignment does not silently succeed.
- `L_ESCallTypeAnnotation;` selects the implicit function, new-target and this
  arguments; an absent annotation defaults to all three. Function kinds 2 and 6
  are nonconstructible lexical-this functions, not ordinary JS functions.
- A fully checked async resolve/reject wrapper without suspension is emitted as
  an async function; other async state machines still fail explicitly.
- A method using lexical state is printed as a factory. Its explicit context has
  `env` (a parent-linked object with mutable `slots`) and `hole` (the shared
  uninitialized sentinel). New environments initialize all slots to that sentinel;
  this represents captures without guessing variable names or decompiling callers.

## Source reconstruction scope

- Methods and records are reconstructed on demand from the selected coordinate;
  a record composes its module initializer, imports, exports, nested functions
  and classes. Selected methods also collect only their own nested definitions.
- Accumulator/register flow, properties, calls, literals, scalar literal arrays,
  module bindings, lexical environments, branches, loops, catch-all and nested
  try ranges, class definitions with `extends`/`super`, and async resolve/reject
  wrappers with await are covered. Own-property stores use the runtime
  enumerability rule through emitted class-prototype/constructor WeakSets.
- Unsupported constructs raise `DECOMPILE_FAILED`; there is no disassembly
  fallback. Known gaps still under work: irreducible conditional/loop layouts,
  generator state machines, some typed handlers, and a few object/array literal
  shapes. These are explicit failures, not silently approximated output.

## Independent implementation reference

- xpanda: <https://github.com/asmjmp0/xpanda>
- Revision: `8f27d3ce8b63581c7eb5203bbfdfbb92e28cfccf` (core 2.28.0).
- Author attribution: jmp0; license: Apache-2.0.
- References: `packages/xpanda-core/src/file/`, `src/disasm/`,
  `scripts/gen-opcodes.ts` and `resources/isa.yaml`.
- Its opcode resource declares 24.0.0.0; it is a comparison source, not the
  authority for accepting other input versions. Its 28-bit integer masking and
  inline-string byte-size assumptions are not adopted by the Rust parser.
- No xpanda/Node dependency is introduced into production execution. The
  optional corpus oracle runs only on the fixed validation sample and writes
  into ignored `target/`; it must never execute the analyzed program.
