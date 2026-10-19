# ADR 0005: Build complete single-file distributions in CI

Status: accepted. Amended by [ADR 0006](0006-linked-kuna-cli.md) and
[ADR 0010](0010-prebuilt-ilspy-runtimes.md); the single-file runtime contract
remains unchanged.

CI builds the entire rxd on Linux x86_64/arm64, Windows x86_64 and macOS arm64.
Each native runner builds the pinned ILSpy NativeAOT library and the unmodified
Kuna submodule, then compiles rxd with `bundled-kuna,bundled-ilspy`. Releases
require only one executable at runtime. Cargo itself never builds .NET or Kuna.

Both runtimes are deterministic compressed archives embedded in rxd. ILSpy's
archive includes its .NET 10 reference metadata and notices, allowing async,
LINQ and class reconstruction without an installed SDK or configured reference
directory. `scripts/runtime_bundle.py` creates manifests, per-file checksums and
Rust include files; `rxd-core::bundle` verifies and atomically extracts them.
The runtime namespaces are separate (`rxd-kuna` and `rxd-ilspy`), content keyed,
and initialized only when their backend needs the engine.

Kuna remains an executable process with its existing forwarding semantics.
ILSpy remains a shared library loaded in the analysis worker, with an optional
C ABI initializer for its trusted reference directory. Existing explicit engine
and reference-directory overrides remain available. No analyzed code executes.
The existing `csharp-decompiler` external-library development mode remains usable.

Every platform runs release CLI E2E tests. Packaging checks a relocated copy of
only rxd with runtime overrides removed, native and C# decompilation, lazy cache
creation, and refusal to load a corrupted cached engine. Linux x86_64 additionally
runs formatting, Clippy and debug CLI tests. Artifacts include the executable,
build manifests, licenses and archive SHA-256; no engine sidecars are required.
Pushes to main, pull requests and manual dispatch trigger the workflow.

Compilation and first cache installation have separate costs. Neither CI's
correctness checks nor this distribution decision relax the existing per-query
latency contract. Cross-platform performance requires platform-specific evidence.
