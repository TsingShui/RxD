# ADR 0012: Enable all-feature defaults with embedded ILSpy

Status: accepted. Kuna remains opt-in; only ILSpy is embedded by default.

## Decision

Root Cargo defaults to `all-feature`, defined as **`all-backends + mcp +
bundled-ilspy`**. All implemented backend namespaces, stdio MCP and C# recovery
are enabled. Kuna is the explicit exception: `bundled-kuna` stays opt-in and its
existing linked-Rust/specs producer remains available, but it is not compiled,
embedded or advertised as a native MCP tool by the default build. No portable
Kuna prebuilt, root include-generator build script or global Kuna version config
is required or kept for this default profile.

`all-backends` retains its narrower namespace-only meaning. Users can still use
`--no-default-features` with backend/transport/engine subsets. Planned backends,
upstream helper executables and complete Android CLI support are not implied by
this alias.

After `git submodule update --init third_party/kuna` for Cargo path resolution,
ordinary `cargo build` consumes the verified ILSpy target prebuilt. It does not
need .NET SDK, Python, slacomp, pre-build scripts or runtime-include environment
variables. Git, Rust and system compilation/linking tools are still needed;
fresh Rust dependency downloads may need the network. Cargo never initializes
submodules or downloads native engines without explicit consent.

ILSpy uses the existing per-target verified prebuilts. Missing Windows artifacts
still require an explicit producer/prepared override until registered; no foreign
ABI substitution is allowed. macOS remains ARM64-only; Android App C ABI packages
remain separate from complete rxd CLI builds.

## Validation and consequences

Default debug/release real CLI tests must exercise the default MCP and ILSpy
paths. Quick CI deliberately selects unbundled all-backend or all-backend/MCP
profiles where appropriate; these are not a replacement for default/full
platform and publication gates. Explicit Kuna builds retain their own existing
validation and version-handshake workflow.

The default executable is larger than the former namespace-only build, but each
query still initializes only its requested engine. Source flags are not latency
evidence; preserve the ordinary fresh-process 150ms and exact-workload approval
contracts. The earlier default-Kuna prototype is not the accepted
profile and its test/timing results must not be relabeled as this variant.

## Local evidence for the clarified profile

On macOS ARM64, the default real CLI suites passed **391/391 debug and 391/391
release** without include/library/version environment overrides. Strict workspace
Clippy (excluding the optional generated Kuna adapter), formatting, workflow and
producer/scope contracts passed; minimal no-default Cargo check remained valid.
Actual normal-dependency inspection confirms no `rxd-kuna-cli` or `kuna-decomp`
in the default binary graph. Ordinary offline `cargo build` with a failing
`dotnet` shim succeeds, produces C# using the embedded ILSpy, and `rxd kuna
--version` reports the explicit KUNA_NOT_BUNDLED/127 outcome. Existing optional
Kuna construction is not claimed to be part of these default results. Logs remain
under ignored `target/default-ilspy-only/`.
