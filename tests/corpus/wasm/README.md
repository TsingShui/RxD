# WABT wasm-decompile differential corpus

Upstream reference tests used as a differential oracle for `rxd wasm get`'s
structured decompilation. rxd's decompiler is modeled on WABT's `wasm-decompile`;
these are its own official test cases, so they are an independent, authoritative
check on rxd output.

## Provenance

- Source: WebAssembly Binary Toolkit (WABT) `test/decompile/*.txt`.
- Upstream version: 1.0.36. License: Apache-2.0
  (`../licenses/wabt-1.0.36-Apache-2.0.txt`).
- `*.txt` are committed verbatim: each contains a WAT module and the expected
  `wasm-decompile` STDOUT. SHA-256 of each `.txt` is recorded in
  `../wasm-expectations.json`.
- `*.wasm` are generated once from the embedded WAT via
  `wat2wasm --debug-names` (deterministic for a fixed WABT version) by
  `../generate-wasm.py`. Intermediate `.wat` is written to a tempdir and never
  committed. `names.txt` is kept for provenance but not compiled (it uses a
  `@magic` annotation the pinned wat2wasm rejects).

## How the tests use it

`tests/wasm_wabt/mod.rs` runs the actual `rxd wasm` CLI against the committed
`.wasm` offline and compares with the committed `wasm-decompile` expectations.
WABT is never invoked at test time.

Comparison is structural, not byte-for-byte, because rxd deliberately scopes out
some of wasm-decompile's readability passes. What is asserted:

- **Control-flow recovery equivalence**: `loop`/`if`/`else` counts must match the
  reference wherever rxd produces decompiled output. This is the
  correctness-critical skeleton.
- **Operator precedence parity**: the `precedence` case body must match
  wasm-decompile byte-for-byte.
- **Index scaling parity**: the `loadstore` dynamic index fold
  `(base + (i<<2))[0]` → `base[i]` and the offset-after-index fold
  `(base + (i<<2))[1]` → `base[i + 1]` match the reference, and a mismatched
  shift is left unfolded as the reference does.
- **Safe degradation**: `basic`'s multi-value `block (param …)` / block-as-
  expression is a documented divergence; rxd must degrade cleanly (explicit
  `UNSUPPORTED` reason, `code: null`, never a crash or guessed code), not match.

## Known divergences from wasm-decompile (intentional, scoped out)

- Struct detection (`a:{a:float,b:float}` + `a.a`): rxd emits explicit
  `a[0]:float` indexed accesses instead.
- Full multi-value stack flushing (`let t0 = …` prefix materialization) and
  block parameter/result signatures: rxd degrades to the instruction view.
- `export` marker on function signatures: rxd records export status in `outline`
  rather than in the decompiled signature.

## Regenerate

```sh
python3 tests/corpus/generate-wasm.py "$(command -v wat2wasm)"
```
