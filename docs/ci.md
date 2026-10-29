# Scoped checks and complete rxd builds

[Build rxd](https://github.com/TsingShui/RxD/actions/workflows/build.yml) runs on main
pushes, version tags (`v*`), pull requests, manual runs and weekly compatibility runs. A
read-only changed-path plan selects checks; every commit does not require a full native
distribution. Rust is pinned to 1.98.0, packaging installs .NET SDK 10.0.401 (any 10.x SDK is accepted), and Python 3.12
drives CI scripts.

## Validation tiers

- **Plan and tooling:** compare explicit Git endpoints (PR merge-base, push before SHA),
  including old paths for renames/deletions. Documentation/project skills need no Rust
  job. Upstream/CI tooling changes run their offline CLI contracts.
- **Quick checks:** affected backend package tests and
  feature-scoped debug/release CLI, or default all-backend checks for shared/unknown
  paths, including real stdio MCP checks without bundles. RadGuard selects both DEX and
  JVM. Shared harness changes still use workspace gates. Quick
  jobs initialize only the Kuna Cargo path dependency, not all .NET reference submodules,
  and do not build NativeAOT or bundled specs.
- **Complete distribution:** the four platforms below run for native engines,
  build/dependency/packaging/workflow changes, optional MCP/Kuna integration, every
  version tag, manual runs and the weekly compatibility run. Backend-only and
  maintenance-only PRs do not build all four distributions. Forced full runs also
  validate the default feature profile.

Unknown changed paths widen to shared checks, never silently skip validation. Missing
comparison history fails or forces full acceptance; a failed test/job is not a successful
skip. `Validation gate` is a stable aggregate check: every selected quick/full job must
succeed. Publication additionally requires complete builds. Branch protection should
require **Validation gate**, not individual conditional platform/matrix names. Hosted
workflow success requires a real Actions run; local workflow lint is not cross-platform
execution evidence.

Local scope inspection and validation reuse:

```sh
python3 scripts/ci-scope.py --base HEAD
```

Detail is in [docs/agents/testing.md](agents/testing.md).

## Complete build artifacts

The four native targets are Linux x86_64, Linux arm64, macOS arm64 and Windows x86_64.
Linux runners use Ubuntu 24.04 with a musl runtime, targeting
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`. Each native job builds the
pinned ILSpy native library and the unmodified Kuna submodule, then compiles rxd with
`bundled-kuna,bundled-ilspy`. Release runtime needs one executable. Complete artifacts do
not initialize ILSpy at startup. No Rust, Python, Kuna or .NET SDK/runtime is needed to
run them.

Every archive includes build manifests, upstream notices and a sibling SHA-256 file.
Artifacts are retained for 14 days. A version-tagged build additionally publishes a
persistent GitHub Release after all four native jobs and their formatting, Clippy, CLI/MCP
and relocated-package checks pass.

## Versioned releases

A release manager creates a tag `v<root Cargo package version>` on the intended source
commit; the workflow never creates/advances source tags or bumps versions. For example,
the current package requires `v0.1.0` and reports `rxd 0.1.0`. Tag mismatch fails before a
platform build. Packaging checks the executable version and records version, source
commit, Rust target, bundled features and per-file hashes in `build-info.json`.

Fixed asset names: `rxd-linux-x86_64.tar.gz`, `rxd-linux-arm64.tar.gz`,
`rxd-macos-arm64.tar.gz`, `rxd-windows-x86_64.zip` and `install.sh`. Every asset has a
sibling `.sha256` containing `<sha256>  <asset-name>` and a newline. An archive has one
`rxd-<platform>/` root. Only `rxd` (`rxd.exe` on Windows) is needed at runtime; manifests,
README and licenses remain for audit.

The publication job downloads all four verified build artifacts, adds the installer from
the same source commit, validates the complete set, stages a draft, uploads all ten
assets, downloads them again to verify bytes/identities, then publishes the stable
Release as latest. Missing platforms, mismatching hashes, wrong source identities or
failed uploads leave it unpublished. A rerun encountering an existing draft or published
Release fails without changing assets. After a failed upload, only an unpublished draft
may be inspected/deleted before retry; published versions are never replaced. Corrections
require a new version. Runs for a tag are serialized and an active tag publication is
never cancelled.

Publication requires a **public** distribution repository and a matching pre-existing
tag; private repositories cannot satisfy anonymous downloads. The destination defaults to
the source repository. Cross-repository publication is refused because the installer
embeds the source repository's Release URL. `RXD_RELEASE_REPO` must not point at a
destination whose assets are private. `RXD_RELEASE_TOKEN` may provide release credentials
for the supported destination; the visibility check does not change repository privacy.

Manual Unix installation (macOS uses `shasum -a 256 -c`):

```sh
release_repo=TsingShui/RxD
release_tag=v0.1.0
asset=rxd-linux-x86_64.tar.gz
curl -fSL "https://github.com/$release_repo/releases/download/$release_tag/$asset" -o "$asset"
curl -fSL "https://github.com/$release_repo/releases/download/$release_tag/$asset.sha256" -o "$asset.sha256"
sha256sum -c "$asset.sha256"
tar -xzf "$asset"
./rxd-linux-x86_64/rxd --version
```

On Windows download the versioned zip and checksum from the same Release, compare
`Get-FileHash -Algorithm SHA256`, unzip and run `rxd-windows-x86_64\rxd.exe --version`.
Windows remains a manual-download platform. Release downloads persist independently of
Actions artifacts.

After publication, anonymous verification is:

```sh
python3 scripts/verify-release.py --tag v0.1.0 --commit <source-commit> --repo TsingShui/RxD --public
```

After packaging, all three Unix runners exercise the actual curl-to-shell entry with the
native RxD executable, including latest resolution and repeat update. The publication job
follows anonymous asset verification with a versioned public curl installation and a
default latest update. See [installation and Agent onboarding](install.md).

The four native packaging jobs establish executable behavior. The aggregate verifier
checks archive/checksum layout, complete features, engine targets, build identities and
per-file hashes without executing foreign binaries. Local boundary fixtures:

```sh
python3 scripts/test-release.py
```

## Build and verification details

Build order: ILSpy production, pinned Kuna specs, linked Kuna CLI, then complete
rxd release. All runners use the common `scripts/build-ilspy.py` producer and
.NET SDK 10.x with the 10.0.12 runtime pack. Linux musl-only engines use Zig 0.17.0,
including cross-builds from macOS; Windows/macOS engines retain their native
linkers. The macOS job also builds `linux-musl-arm64`. Native runtime and package
checks remain on each target OS; cross-compilation does not replace them.
No ILSpy prebuilt is checked in. Android uses the shared producer's
`linux-bionic-arm64` target and NDK 28.2.13676358 / API 24, not the Linux musl
artifact. Android cross-build commands are below; App packaging and device
validation remain outside this repository and are not desktop CI gates.
macOS is ARM64 only. Submodule checkout includes full history for Kuna version
calculation. Rust
dependencies and Kuna compiler/spec outputs are cached per platform/toolchain. Newer
PR runs cancel stale runs of that PR; main commits have separate concurrency groups
so a later docs-only push cannot cancel an earlier required native build. Tags are
never cancelled by newer runs.

All published desktop platforms run release CLI and real stdio MCP E2E. Linux x86_64 also checks formatting,
strict workspace Clippy, debug CLI/MCP tests and a no-MCP build. Packaging tests a
relocated copy with no runtime path overrides: actual MCP initialization/discovery,
immutable Unicode/CRLF snapshot and original FETCH, target closure, lazy engine caches,
Kuna JSON decompilation, ILSpy async method/class recovery, lazy cache
creation and rejection of a modified cached engine. These correctness checks do not
establish cross-platform latency.

Local macOS arm64 equivalent:

```sh
python3 scripts/build-ilspy.py osx-arm64
python3 scripts/build-bundled.py --features bundled-ilspy,mcp
KUNA_VERSION="$(python3 -c 'import json; print(json.load(open("target/kuna/aarch64-apple-darwin/manifest.json"))["version"])')" \
RXD_ILSPY_EMBED_RS="$PWD/target/dotnet-native/osx-arm64/embed.rs" \
RXD_KUNA_EMBED_RS="$PWD/target/kuna/aarch64-apple-darwin/embed.rs" \
  cargo test --release --test cli --features bundled-kuna,bundled-ilspy,mcp
python3 scripts/package-release.py --target aarch64-apple-darwin \
  --rid osx-arm64 --name macos-arm64
```

Desktop packaging still uses its native Rust host and RID. The ILSpy producer can
cross-build Linux RIDs on macOS, e.g. `python3 scripts/build-ilspy.py linux-musl-arm64`.
Linux engines and Rust release targets are musl-only, built with the shared Zig
C/C++/archive toolchain. GNU hosts may compile build-time helpers, never GNU
release binaries. Linux execution checks install the matching system musl loader. On Windows use `python` if `python3` is unavailable. Producer
prerequisites: pinned Zig for Linux targets, cargo-zigbuild 0.23.4 for musl Rust
linking, Xcode CLT for macOS, Visual Studio C++ tools for Windows. See
`benches/ilspy/README.md`.

## Android ARM64 Bionic build

Use .NET SDK 10.x, Rust target `aarch64-linux-android` and Android NDK
28.2.13676358 on macOS or Linux. `--ndk DIR` overrides `ANDROID_NDK_HOME` /
`ANDROID_NDK_ROOT`; otherwise the producer finds the pinned NDK under the SDK.
The native bridge and pinned ILSpy package are shared with desktop builds.

```sh
rustup target add aarch64-linux-android
python3 scripts/build-ilspy.py linux-bionic-arm64
python3 scripts/build-bundled.py --target aarch64-linux-android --jobs 2 \
  --features bundled-ilspy,mcp
python3 scripts/package-release.py --target aarch64-linux-android \
  --rid linux-bionic-arm64 --name android-arm64 --no-exec
```

The release executable is `target/aarch64-linux-android/release/rxd`; the archive
is `target/dist/rxd-android-arm64.tar.gz`, with a sibling SHA-256 file. It includes
default backends, MCP, the linked Kuna CLI and compressed specs, and embedded
Bionic ILSpy/reference metadata. Only host build tools run during cross-builds;
`--no-exec` also prevents packaging from executing the target and records
`execution_verified: false` in `build-info.json`. Normal desktop packaging keeps
its execution checks by default.

Both native outputs use 16 KiB ELF LOAD alignment and the CLI uses Android's
system `/system/bin/linker64` and Bionic. No musl loader or private OpenSSL sidecar
is packaged. ELF inspection/build success is not device or App-sandbox validation.
The consuming App owns APK executable placement, permissions/SELinux policy and
writable `TMPDIR` / `RXD_CACHE_DIR`. Never mix a musl ILSpy `.so` with this target.
