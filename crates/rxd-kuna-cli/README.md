# Kuna CLI library adapter

This crate links the command dispatcher from rxd's pinned `third_party/kuna`
submodule. It never edits upstream files. `build.rs` validates the gitlink and
clean checkout, and emits a library entry into Cargo's ignored `OUT_DIR`.

Generated adaptations are limited to:

- accepting argv as `run(Vec<String>)` and returning the native exit status;
- resolving upstream module paths from their original files;
- adding `kuna` when persistent workers re-execute the current rxd binary;
- optionally capturing the existing stdout boundary for rxd's JSON-to-code adapter.

Command parsers, discovery, analysis, decompilation, scheduling and project
writers remain upstream code. Catalog metadata comes from the linked engine.
Imported modules are exempt from rxd's Clippy policy, including upstream
expect attributes tied to upstream clippy.toml. The generator and rxd dispatcher
remain checked by the workspace's normal Clippy gate. Upstream unit tests stay
in their original workspace; rxd runs the actual CLI integration suite. This is a one-shot
CLI entry, with upstream's process-wide environment, exit and worker behavior;
it is not an API for concurrent in-process requests.

Each textual adaptation asserts exactly one source match. Upstream layout or
entry changes fail the build instead of silently applying a partial adapter.
Do not use `cargo fmt --all`: it also formats external path dependencies. Use
`python3 scripts/format-workspace.py` (or `--check`) to format only rxd members.

`scripts/build-bundled.py` sets the upstream `KUNA_VERSION` for the CLI and its
console-library build handshake. When invoking Cargo manually after that build,
use the version from `target/kuna/<host>/manifest.json` as `KUNA_VERSION` to retain
the same external-engine identity. CI exports it before Clippy and tests.
