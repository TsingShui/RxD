# ADR 0019: Build ILSpy natively and ship no checked-in runtime

Status: accepted. Supersedes [ADR 0010](0010-prebuilt-ilspy-runtimes.md) and the
default ILSpy embedding in [ADR 0012](0012-default-all-feature.md).

## Problem

Checked-in NativeAOT bundles tied ordinary builds and Android App embedding to
binaries produced on other operating systems. Every shared native-source change
invalidated the bundles, and each refresh added another roughly 11 MB archive to
Git history. Desktop bundles also duplicated the native build that release CI
already performs on each runner. Keeping even the Android bundle alone still made
the repository carry a stale compiled engine and its import/verification tooling.

## Decision

- Remove the checked-in `prebuilt/` runtime collection and its import/verification
  tool. No ILSpy binary archive is distributed from this repository.
- Desktop C# reconstruction is produced per target by
  `scripts/build-ilspy.py <rid>` under `target/dotnet-native/<rid>/`.
  `bundled-ilspy` requires `RXD_ILSPY_EMBED_RS` pointing at that generated
  `embed.rs`; Cargo never downloads, invokes .NET or compiles NativeAOT.
- `build-bundled.py` auto-selects the host's prepared desktop include when present
  and fails with the producer command when it is absent.
- Root `all-feature` is `all-backends + mcp`. C# reconstruction stays opt-in through
  `csharp-decompiler` (external library) or `bundled-ilspy` (embedded producer
  output). Kuna remains opt-in through `bundled-kuna`.
- Android App embedding is source-built only. The producer instructions and
  private OpenSSL requirements remain in `devtools/android-ilspy/`; consumers build
  the ARM64 Bionic engine and package their own App adapter. No Android AAR, APK,
  signing material or ready-to-load ILSpy bundle is checked in.
- Release CI installs the pinned .NET 10 SDK on every native runner, builds the
  engine once, and then builds `bundled-kuna,bundled-ilspy,mcp`. The package still
  verifies and embeds the engine, reference metadata and licenses into one
  executable.

## Consequences

The repository no longer stores a compiled Android runtime or its manifests, and a
source change cannot leave a checked-in binary with a stale source identity.
Ordinary source builds and repository clones are smaller. Android consumers must
provide .NET 10, the NDK and the private OpenSSL build when producing an engine;
that cost moves to the producer instead of every repository consumer.

Removing the files updates the current tree only. Git history still contains the
old archives until a deliberate history rewrite is performed; that rewrite is a
separate destructive operation and is not part of this decision.
