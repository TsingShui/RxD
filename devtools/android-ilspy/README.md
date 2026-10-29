# Android ILSpy: unified musl worker production

The default direction is an isolated ARM64 musl worker using the same producer as
Linux. It retains the Rust CLI -> analysis worker -> NativeAOT shared-library
architecture. The musl engine is not a Bionic JNI library and must not be passed
to `System.loadLibrary`. Public C declarations remain in `rxd_dotnet.h`.

## Common producer

Install .NET SDK10.0.401 and Zig0.17.0 on macOS or Linux, then run from the root:

```sh
python3 scripts/build-ilspy.py linux-musl-arm64
```

This compiles the unchanged production bridge and pinned ILSpy11.2.0.10050 via
NativeAOT10.0.12. It writes the engine, framework reference metadata, licenses,
source/toolchain identities and the normal `bundle.tar.gz`/`manifest.json`/
`embed.rs` under ignored `target/dotnet-native/linux-musl-arm64/`. The package
SHA-256 is checked before compilation. No runtime fork, private OpenSSL loader,
hash stub or dropped linker dependency is used. Musl ELF LOAD alignment is16KiB;
that is not proof of execution on a16KiB-page device.

For an unbundled Rust CLI, use the matching target and dynamic musl linking:

```sh
RUSTFLAGS='-C target-feature=-crt-static -C link-arg=-dynamic' \
  cargo zigbuild --release --no-default-features --features csharp-decompiler \
  --target aarch64-unknown-linux-musl
```

For embedding the normal verified runtime archive, supply its `embed.rs` and use
`bundled-ilspy` instead of `csharp-decompiler`. Cargo never invokes .NET.

## Android deployment is a separate step

Stage a trusted musl loader, the musl CLI and its engine/reference metadata (or
embedded archive), applicable licenses and immutable input files. The standard
Linux interpreter `/lib/ld-musl-aarch64.so.1` is absent on Android. A deployment
must provide an accessible ELF interpreter path, e.g. with a final Rust link arg
`-C link-arg=-Wl,--dynamic-linker=ABSOLUTE_DEVICE_DIR/ld-musl-aarch64.so.1`.
Provide writable `TMPDIR` and, for embedded bundles, `RXD_CACHE_DIR`.

Invoking the loader explicitly changes `/proc/self/exe` to the loader, which must
be accounted for when starting rxd's normal self-spawning worker. Do not assume
that a shell launcher or a hard-coded ADB deployment path solves App installation.
Modern Android App execution/SELinux restrictions, packaged code locations and
worker cancellation/isolation need consumer-side validation before removing an
existing Bionic integration. No consumer APK/AAR or executable installer is here.

The observed proof is Pixel10 / Android16 / API36 /4096-byte pages, in ADB shell:
15 ABI payloads (13 reconstructions, two invalid tokens) and nine normal CLI
outcomes matched the same artifacts on ARM64 Linux. Default-interpreter dynamic
musl failed; a supplied loader and accessible PT_INTERP worked. This is not an
App-sandbox, full-backend or16KiB-page-runtime claim.

On-device fresh-process samples showed higher C# latency than on the Mac:
Newtonsoft method median221ms/P95 254ms; small class205ms/P95 288ms. These are
observations, not a blanket relaxation of desktop/backend performance contracts
or release acceptance. The user selected unified construction as the Android
priority; continue recording target-specific measurements rather than implying
that successful compilation guarantees either deployment or latency.

## Bionic JNI compatibility, not the default worker flow

Existing App-owned NDK/JNI adapters must keep their Bionic engine until the musl
worker consumer is verified. `Engine.csproj` remains this compatibility producer:

```sh
export DOTNET_ROOT="$HOME/.dotnet10"
export NDK_BIN="$HOME/Library/Android/sdk/ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin"
"$DOTNET_ROOT/dotnet" publish devtools/android-ilspy/Engine.csproj -c Release \
  -r linux-bionic-arm64 -p:DisableUnsupportedError=true \
  -p:PublishAotUsingRuntimePack=true -p:StripSymbols=false \
  -p:CppCompilerAndLinker="$NDK_BIN/aarch64-linux-android24-clang" -m:1
"$NDK_BIN/llvm-strip" --strip-unneeded target/android-ilspy/publish/librxd_dotnet_native.so
```

The previous Bionic proof used SDK10.0.401 / NativeAOT10.0.12, NDK28.2.13676358,
and Pixel10 Android16/API36 with4KiB pages. Thirteen targets and two invalid tokens
matched between shell and a targetSdk36 App sandbox without a private OpenSSL
sidecar; this does not cover arbitrary .NET cryptography. Preserve source/package
hashes and applicable licenses, and test exact produced bytes before consumer
publication. Legacy OpenSSL preload/package entries must stay removed.
