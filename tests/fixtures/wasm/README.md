# WASM structural fixtures

Hand-written, deterministic fixtures for `rxd wasm` CLI contract tests.
Offline; the module is never instantiated or executed.

## Files

- `fixture.wat` — authoritative source, committed and human-reviewed.
- `fixture.wasm` — generated from `fixture.wat`, committed for offline tests.
- `simd.wat` — a function using `v128.load`, outside the covered opcode set.
- `simd.wasm` — generated from `simd.wat`; drives `get` instruction-view
  degradation (`UNSUPPORTED_OPCODE`).
- `scale.wat` — aligned static and dynamic memory accesses.
- `scale.wasm` — generated from `scale.wat`; drives decompilation index
  scaling (`a[12]:int` -> `a[3]:int`, `(base + (i << 2))[0]` -> `base[i]`).

## Regenerate

Deterministically from the committed WAT, including the name custom section:

```sh
wat2wasm --debug-names tests/fixtures/wasm/fixture.wat \
  -o tests/fixtures/wasm/fixture.wasm
wat2wasm tests/fixtures/wasm/simd.wat \
  -o tests/fixtures/wasm/simd.wasm
wat2wasm tests/fixtures/wasm/scale.wat \
  -o tests/fixtures/wasm/scale.wasm
```

Tool: WABT `wat2wasm` (WebAssembly Binary Toolkit). `--debug-names` emits the
`name` custom section so the name-label and name-origin string paths are
covered. Encoding is deterministic for a fixed WABT version; record the version
used when regenerating.

## What it covers

- Merged function index space: imported `env.log` is func index 0; defined
  `sum`/`run` are indices 1/2.
- `refs`: `run` references `sum` via both `ref.func` and `call`.
- Structural records: type, import, export, function, table, memory, global,
  data, element.
- `strings`: a data segment containing valid UTF-8 (`hello`) and an invalid
  byte run (`\ff\fe...`), plus name-section labels.
