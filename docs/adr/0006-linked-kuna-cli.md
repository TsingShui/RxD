# ADR 0006: Link Kuna CLI through an rxd-owned generated adapter

Status: accepted. Supersedes the executable-packaging design and the Kuna
execution portion of [ADR 0005](0005-single-file-distribution.md). ILSpy remains
compressed and embedded.

`rxd kuna ...` calls the original CLI dispatcher as a Rust library. The pinned
`third_party/kuna` submodule is never edited. rxd pins the newest formal Kuna
version tag available at each integration point; ordinary builds remain bound to
that exact gitlink for reproducibility, while `scripts/check-kuna-latest.py`
reports a stale formal tag and requires an explicit reviewed gitlink update.
`crates/rxd-kuna-cli/build.rs`
validates the checkout and generates a library entry in `OUT_DIR`: argv becomes
an explicit argument, native exit status is returned, module paths resolve to
upstream files, and persistent workers restart `rxd kuna decompile-all ...`.
Only generated copies of the few affected files are changed. Exact-match guards
fail on upstream entry/layout drift. Existing command parsers, analyzers,
serializers, scheduling and project writers remain upstream implementations.

Kuna executable code is linked with rxd under the same release profile.
The `bundled-kuna` feature name stays compatible, but its archive contains only
compiled runtime specs and notices. No Kuna executable or catalog JSON is
extracted. `slacomp` is a build-time tool; `decomp_dbg` is still excluded.
Catalog metadata comes directly from the linked engine. Starting with pinned
Kuna v1.876, the ordinary catalog path is in-process and does not invoke
`decomp_dbg`; only an explicit upstream external-console override uses it.
Help/version/docs and default catalog queries require no specs cache.

The previous slim command contract is preserved: default single-function text
extracts `functions[].code` from JSON, while explicit external engines retain
upstream handling. A temporary, thread-local capture of upstream's stdout
boundary avoids launching a second executable for text conversion. Native
JSON, batch workers, streaming project exports, stderr and exit codes retain
their behavior. The entry is one-shot CLI execution, not a concurrent service API.

Specs retain checksum-verified first extraction and private atomic cache
publication. Resource-only manifests have no executable field. Existing ILSpy
manifests retain executable integrity checks. The ordinary commands continue
to bypass rxd's generic worker/envelope protocol.

Formatting must use `scripts/format-workspace.py`: `cargo fmt --all` traverses
local path dependencies outside the workspace and would reformat Kuna.
The comparison baseline and linked release use the same compiler, optimization
settings, Kuna gitlink and embedded ILSpy bytes. Compare executable, compressed
archive and expanded resource sizes separately; direct linking is not assumed
to reduce the uncompressed executable. Actual results belong in the Kuna demo.
