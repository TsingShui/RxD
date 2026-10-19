# Vendored radguard

rxd's maintained fork of [radguard](https://github.com/MG1937/radguard), the DEX
decompiler whose rendering backend the APK/DEX and JVM backends share.

## Provenance

* Upstream: `git@github.com:MG1937/radguard.git`
* Baseline: `290de20c925c4a2c7642b1a63c9997185a06e130` (2026-10-08, "remove dad interface")

## What this fork changes

* **Library facade.** The public surface is five re-exports plus `backend`,
  `decompiler`, `dex`, `error`; the IR, CFG, dataflow passes and writer are
  `pub(crate)`. `Decompiler::from_arc` shares the DEX bytes instead of copying.
* **`backend` module.** Renders Java-like source for IR built by another front
  end (`rxd-jvm`): `ClassSpec`/`FieldSpec`/`MethodSpec`, `render_class`,
  `render_method` and the IR contract. Both front ends render through one
  implementation (`src/render.rs`).
* **`SourceStyle`.** The DEX path keeps byte-exact Androguard DAD parity (the
  frozen gate `tests/quality_gate.py` upstream); a foreign front end may ask for
  `boolean` literals rendered from the declared type, which the JVM stack needs.
* **Named variables.** `VariableTable::set_name`/`is_named`, so a front end that
  knows the original identifiers (a JVM `LocalVariableTable`) can render them.
* **No four-byte `code_item` alignment rejection.** A DEX 041 container view
  legitimately places members at container-relative offsets; every field is read
  byte-wise, so parsing is correct either way. `rxd-dex` hands over exactly that
  view.
* **Not carried:** the command-line shell, the APK/ZIP pipeline, the `libdeflate`
  and `mimalloc` dependencies, the white-box pipeline tests and the frozen DAD
  quality gate. rxd has its own APK layer and drives the library directly. The
  crate therefore has no dependencies at all.
* **Tests are reference only.** `tests/` keeps the library-level tests, but the
  package sets `autotests = false`: rxd's own suites (`cargo test --test cli`,
  `-p rxd-jvm`, `-p rxd-dex`) are what gate this crate, and the fork's tests are
  not part of a workspace or CI run.

## Licence

Apache License 2.0 (see `LICENSE` and `NOTICE`): upstream is licensed that way,
and RadGuard is an independent Rust implementation of Androguard's DAD design and
behavior, so the DAD attribution travels with it.

## Rebasing on upstream

Copy `src/` and `Cargo.toml` from the new upstream revision, then re-apply the
changes above: they are the facade (`src/lib.rs`), `src/backend.rs`,
`src/render.rs`, the `SourceStyle` plumbing in `src/writer.rs` and
`src/dataflow.rs`, the naming API in `src/ir/mod.rs`, and the alignment change in
`src/dex/mod.rs`. `rustfmt` and Clippy must pass for the workspace.
