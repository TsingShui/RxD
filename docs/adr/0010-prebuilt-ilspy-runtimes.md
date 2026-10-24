# ADR 0010: Consume verified NativeAOT runtimes from prebuilt/

Status: accepted; its desktop ILSpy prebuilt decision is superseded by
[ADR 0019](0019-native-desktop-ilspy.md). The desktop import/prepare instructions
below are retained as history. The Android App collection and runtime semantics
remain current.

## Problem

NativeAOT production requires the target OS/toolchain. Rebuilding it on every
ordinary rxd build is unnecessary and prevents simple reuse of already validated
artifacts.

## Decision

- Curate compressed ILSpy runtime bundles under `prebuilt/ilspy/<Rust target>/`,
  with the full runtime manifest and an index pinning archive/manifest SHA-256
  plus exact native source/project/NOTICE identity. Include reference metadata
  and license notices, not SDKs or intermediate build products. This is a narrow
  exception to the usual ignored-`target/` rule.
- `cargo build --features bundled-ilspy` chooses a checked-in artifact by
  `TARGET` when no explicit `RXD_ILSPY_EMBED_RS` is supplied. Validate schema,
  target, manifest/archive size/hash and current C# source inventory/content
  before generating data-only Rust includes in `OUT_DIR`. Stale, corrupted or
  absent artifacts fail the build. No network, subprocess SDK invocation, native
  compilation or foreign-ABI fallback occurs in Cargo.
- Preserve explicit prepared-include overrides for trusted producer/developer
  builds, and preserve default feature-off/external-library behavior. macOS C#
  native builds support ARM64 only; `osx-x64` producer entries are removed.
- `scripts/build-ilspy.py` remains the explicit producer. It records source
  identities and refuses packaging if their bytes change during compilation.
  `scripts/prebuilt-ilspy.py add` validates bounded regular archive members,
  exact paths/hashes, source identity, reference metadata and notices before
  importing. Replacing a different artifact requires `--replace`.
- The same tool's `check` verifies registered artifacts; `prepare` stages
  bundle, manifest, Rust includes, build metadata and license copies for
  existing release packaging without .NET. It never executes imported code. CI
  prefers prepared prebuilts where present; an absent platform retains an
  explicit native producer job rather than pretending that target is available.
  Invalid existing artifacts do not silently trigger native rebuild/download.
- Keep rxd's runtime verified lazy extraction, worker isolation, IDs, original
  bytes, budgets and atomic failures unchanged. No new runtime network access or
  loader format. Kuna remains directly linked Rust; this does not ship a
  platform-independent rxd executable or replace its specs policy.

## Scope

Only artifacts actually produced are registered: macOS ARM64 and Linux
ARM64/x64. Windows x64 requires a Windows producer. Further registered targets
must retain their actual build/test scope; macOS x64 is not supported.

Normal builds gain an offline, SDK-free interface; source/package updates
require a deliberate per-target bundle refresh. Source checks prevent silently
embedding an old bridge after edits, but hashes do not independently authenticate
a binary's producer or prove general semantic correctness. Run platform-specific
correctness/relocation/performance gates before release.

## Separate Android App collection

`prebuilt/android-ilspy/` holds an ARM64 Bionic App C ABI bundle with the
production bridge, private OpenSSL, trusted reference metadata, headers and
licenses. `scripts/prebuilt-ilspy.py --android-app` selects this separate index;
without that explicit collection flag, Android imports are rejected. It cannot
be prepared as a CLI include. The complete rxd Android CLI remains unsupported;
no generic JNI library, AAR, probe APK or signing material is distributed.

Usage and artifact policy: [`prebuilt/README.md`](../../prebuilt/README.md).
