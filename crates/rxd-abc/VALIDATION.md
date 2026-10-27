# Dynamic Ark backend validation

This file records the reproducible acceptance evidence for `rxd ark`. The
analyzed sample is not committed; hashes and counts identify it.

## Pinned inputs and references

- ClashBox v1.7.4 unsigned HAP: 107,951,762 bytes,
  SHA-256 `9977a4b9e40c96e2fcb7831340f43829a25f95c5aa5ffe7addb811992cfdfd0e`.
- `ets/modules.abc`: 7,203,484 bytes,
  SHA-256 `a2046b75ebd368e0599a50b2b5e5c6dab8070007364af5a09d67d267582c197c`,
  dynamic ABC `12.0.6.0`.
- xpanda reference: commit `8f27d3ce8b63581c7eb5203bbfdfbb92e28cfccf`
  (Apache-2.0, author jmp0).
- Official references: `openharmony/arkcompiler_runtime_core`
  `b4b18c9800ed567f840572eebe94307a47ec48a6` and
  `openharmony/arkcompiler_ets_runtime`
  `930f00041dff23e3ca97e18cabf8d7b9b2c4ca2e`.

## Entity mapping

The sample has 385 records, 376, module-data records with `func_main_0`, and
14,082 methods with code. The other 9 records are auxiliary: seven
`L@ohos.*`/`L@system.*` namespace records and
`L_ESConcurrentModuleRequestsAnnotation;`/`L_ESSlotNumberAnnotation;`; they
have no module initializer, so neither xpanda (376 JS files) nor RxD emits a
record source for them.

## Reconstruction coverage

- Methods: **14,082 / 14,082** reconstructed; 0 failures.
- Source records: **376 / 376** reconstructed; 0 failures.
- Every rendered method/record parses as a JavaScript module.
- Content audit: 0 of 14,082 methods miss a string constant or property name
  referenced by a decoded instruction. Module hole-guard names are excluded
  because imports/exports carry the binding.
- Unsupported constructs raise `DECOMPILE_FAILED`; there is no disassembly
  fallback, skipped method, empty body or placeholder.

Reproduce with the offline audit example against the pinned ABC:

```sh
cargo run --release -p rxd-abc --example audit -- modules.abc \
  --source-out source.jsonl --record-source records --content-check
```

## Fast verification

`scripts/verify-ark.sh` runs the scoped completion gates in a few minutes and
does not build release E2E suites: formatting, the 17 backend-scoped CLI tests,
strict Clippy for the Ark crates, and the generated opcode-table check. Set
`ARK_ABC=/path/modules.abc` to add the full-sample reconstruction/content
audit. Release E2E and cross-platform packaging are pre-publication gates, not
completion evidence.

## xpanda differential

- Decode: all 14,082 methods, 796,370 instructions and 1,078,001 operands
  compared method-by-method/instruction-by-instruction with xpanda; zero
  unexplained differences. The comparison script is
  `scripts/check-abc-decode.py`.
- Source: 376 reference record sources compared with
  `tests/corpus/abc/source-diff.cjs`. No missing records; the only reference
  parse failure is xpanda's invalid `let =ens0` output for a non-identifier
  local export (RxD binds `__abc_export_N` and exports the original string
  name). Structural/source deltas are classified as representation classes:
  call fusion (`obj.m(...)` vs loading the callee and `Reflect.apply`), object
  key quoting, explicit lexical environments vs `local_N` names, class static
  fields vs `Object.defineProperty`, and synthetic checks. Differences with
  static semantic evidence are recorded in `REFERENCE.md`: class static
  stores are non-enumerable in Ark (`RuntimeStOwnByName`), derived
  constructors must not read `this` before `super()`
  (`RuntimeThrowIfSuperNotCorrectCall`), and dynamic calls pass callee in the
  accumulator plus an explicit receiver register (no inferred call edges).
- The optional oracle and diff tools run offline against the pinned sample
  only; production execution never invokes Node/xpanda.

## Known semantic boundaries

These are general code-shape boundaries, not observed reconstruction failures:

- `copydataproperties` is emitted as `Object.assign(target, source)`. Ark's
  CopyDataProperties creates own data properties; if the target has a setter
  or a non-writable inherited property, `Object.assign` calls the setter or
  throws in strict mode instead of defining a new own property. A fresh
  object-literal target (the common spread shape) is equivalent.
- Class prototype/constructor stores use `enumerable:
  !(__abc_class_prototypes.has(obj) || __abc_class_constructors.has(obj))`.
  Both WeakSets are module-local and only register classes created by the
  emitted factory in that record. An imported, native or global class is not
  registered, so its store is emitted enumerable where `RuntimeStOwnByName`
  would make it non-enumerable. Access is unchanged; enumeration and JSON
  output can differ.
- `ldsuperbyname` takes its receiver from the incoming accumulator
  (`RuntimeLdSuperByValue` passes it to `GetProperty`). The emitter renders
  `super[key]`, which always uses the current `this`. This matches when the
  compiler stored `this` in the accumulator; all five sample uses do
  (`acc = __abc_this` before `super["toJSON"]`, `super["toString"]`,
  `super["getTunFd"]`, `super["stopVpn"]`). A site passing another object as
  receiver would observe a different getter receiver.
- Generator/async output uses native `yield`/`await` in place of the bytecode
  state machine; `getresumemode` is rendered as `acc = 0` (normal resume).
  Recognized compiler patterns get native throw/return behavior at
  `yield`/`await`; a residual site that branches on the resume mode would
  take the normal path.

The source differential is a structural classification (records, classes,
members, function/property keys) plus runtime-derived semantic review, not a
per-method formal equivalence proof. `Reflect.apply(callee, receiver, args)`
and `__abc_env.slots[...]` are explicit representations of the accumulator/
receiver call convention and lexical environment, not gaps themselves.

## Safety and CLI contracts

- ABC checksum, size, region/index bounds, MUTF8, try/handler boundaries and
  instruction boundaries are validated from one immutable buffer.
- HAP selection reuses ZIP central-directory validation (CRC, lengths,
  duplicate paths, traversal); multiple ABCs require `--entry`. Bare ABC and
  the corresponding HAP entry produce byte-identical navigation, raw and
  source output.
- `rxd unpack` keeps whole-archive extraction unchanged (152 files for the
  sample matched Python `zipfile` paths, lengths and SHA-256 values).
- Listings are shared TSV with JSON-quoted text and `\N` for absence.

## Performance (release, user-approved)

macOS 26.5.1 arm64, 12 CPUs, release profile, fresh process per sample, 10
samples, no warm-up, pinned ABC/HAP:

| workload | median | P95 | >150 ms |
|---|---:|---:|---:|
| list record | 31.9 ms | 34.5 ms | 0 |
| list method | 32.2 ms | 33.3 ms | 0 |
| small method | 32.3 ms | 34.5 ms | 0 |
| largest method (11,373 code bytes) | 33.5 ms | 35.2 ms | 0 |
| small record | 33.0 ms | 34.2 ms | 0 |
| 690 KB record | 39.6 ms | 40.7 ms | 0 |
| 554-method record | 42.4 ms | 42.8 ms | 0 |
| HAP entry | 46.0 ms | 48.4 ms | 0 |

ABC parse/checksum is ~16 ms fixed; target work is 0.1-5.5 ms and grows with
the selected target, not the application. HAP adds ~1.1 ms inflate and ~14 ms
read for the 108 MB archive (peak RSS 124 MB). Full-sample 376-record source
generation takes 0.54-0.65 s in one process; xpanda's 109.2 s run includes AST
construction, JSON serialization and simplify passes, so this is not a
like-for-like comparison. Reproduce with `benches/ark.py` after an approved
`cargo build --release`.

## Latest checks

- `cargo test --test cli` (default features): 110 passed.
- `cargo test -p rxd --no-default-features --features backend-abc --test cli abc::`: 17 passed.
- MCP `mcp::abc`: passed with `--features mcp`.
- `cargo clippy -p rxd-abc -p rxd --all-targets -- -D warnings` and workspace
  formatting: clean.
