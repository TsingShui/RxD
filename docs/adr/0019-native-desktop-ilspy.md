# ADR 0019: Build desktop ILSpy natively and ship only the Android prebuilt

Status: accepted. Supersedes the desktop-prebuilt decision in
[ADR 0010](0010-prebuilt-ilspy-runtimes.md) and the default ILSpy embedding in
[ADR 0012](0012-default-all-feature.md); the Android App collection remains.

## Problem

Checked-in NativeAOT bundles under `prebuilt/ilspy/` tied every ordinary default
build to four binaries produced on four operating systems. Any change to a shared
native source invalidated every bundle while only the producer's platform could
rebuild it. The checked-in Linux/Android bundles were already behind the current
bridge, and a stale desktop bundle blocked the default build before any Rust
backend could compile. The desktop prebuilt also duplicated a build that release
CI already performs on each native runner.

## Decision

- Remove checked-in desktop ILSpy bundles and their index. `prebuilt/` retains only
  the separately verified Android ARM64 App C ABI collection.
- Desktop C# reconstruction is built per target by `scripts/build-ilspy.py <rid>`
  in `target/dotnet-native/<rid>/`. The resulting `embed.rs`, archive and manifest
  remain reproducible build products rather than source-controlled artifacts.
- `bundled-ilspy` requires `RXD_ILSPY_EMBED_RS` pointing at that generated
  `embed.rs`; it has no checked-in fallback. `build-bundled.py` auto-selects the
  host's prepared include when present and fails with the producer command when it
  is absent. Cargo still never downloads, invokes .NET or compiles NativeAOT.
- Root `all-feature` is `all-backends + mcp`; C# reconstruction stays opt-in through
  `csharp-decompiler` (external library) or `bundled-ilspy` (embedded producer
  output). Kuna remains opt-in through `bundled-kuna`.
- Release CI installs the pinned .NET 10 SDK on every native runner, builds the
  engine once, and then builds `bundled-kuna,bundled-ilspy,mcp`. No desktop
  prebuilt preparation step remains. The package still verifies and embeds the
  engine, reference metadata and licenses into one executable.

## Consequences

Ordinary `cargo build` is offline, SDK-free and no longer fails because a foreign
platform bundle is stale. It also no longer includes C# reconstruction; users who
need it run the explicit producer and pass `RXD_ILSPY_EMBED_RS`. Default debug/
release CLI suites keep C# coverage conditional on the bundled feature rather than
claiming the engine is always present.

Source identity is stricter for desktop builds: the engine is produced from the
same checked-out native sources in the same CI run. The Android App bundle remains
a separate consumer artifact with its own producer, index, source-input hashes,
private OpenSSL and App validation; it is not a desktop CLI include. Changes to
shared native sources require re-producing that bundle separately, as they always
did.
