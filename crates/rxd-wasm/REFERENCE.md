# rxd-wasm dependency reference

## wasmparser

- Crate: `wasmparser` (bytecodealliance/wasm-tools).
- Pinned version: `=0.221.3` (see `Cargo.toml`); transitive deps pinned via the
  workspace `Cargo.lock`.
- Use: read-only binary decode and validation only. The backend never
  instantiates or executes a module, and does not use any runtime/WASI feature.
- Features: default. Non-essential features are not enabled by this crate.

### Why pinned

Decode and validation behavior, and the structural record shapes derived from
it (index spaces, section ordering, immediate decoding), must stay
deterministic across builds. Bump the pin deliberately and re-run the CLI
contract tests and benches when upgrading.

### Scope boundaries

- WAT text, Component Model, module linking, and WASI are out of scope.
- `call_indirect` is recorded as indirect (type + table index); it is never
  resolved to a concrete callee.
- The name custom section is read only as navigation labels and never changes
  content identity.
