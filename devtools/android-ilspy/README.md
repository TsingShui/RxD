# Android ARM64 App embedding producer

This is a separate `linux-bionic-arm64` NativeAOT build of the unchanged production
C# bridge, not a GNU/Linux library or complete rxd Android CLI port. It produces a
C ABI engine for an App-owned NDK/JNI adapter. No PoC instrumentation JNI class,
APK, test key or consumer AAR is shipped. Public declarations: `rxd_dotnet.h`.

## Producer toolchains

Measured producer: .NET SDK10.0.401 / NativeAOT10.0.12, NDK28.2.13676358,
`aarch64-linux-android24-clang`; fixed ILSpy11.2.0.10050 and the NuGet SHA-256 in
`crates/rxd-dotnet/native/NOTICE`. Output ELF LOAD alignment is 16KiB; actual
execution proof is on a Pixel10 Android16/API36 device with 4KiB pages, not a
real 16KiB-runtime result. Record exact library hashes and device results before
consumer publication.

From the repository root (macOS host shown):

```sh
export DOTNET_ROOT="$HOME/.dotnet10"
export NDK_BIN="$HOME/Library/Android/sdk/ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin"
"$DOTNET_ROOT/dotnet" publish devtools/android-ilspy/Engine.csproj -c Release \
  -r linux-bionic-arm64 -p:DisableUnsupportedError=true \
  -p:PublishAotUsingRuntimePack=true -p:StripSymbols=false \
  -p:CppCompilerAndLinker="$NDK_BIN/aarch64-linux-android24-clang" -m:1
"$NDK_BIN/llvm-strip" --strip-unneeded target/android-ilspy/publish/librxd_dotnet_native.so
```

Verify downloaded/restored NuGet bytes against the official fixed package hash
in `crates/rxd-dotnet/native/NOTICE`. NuGet restores use the normal global package
cache (`NUGET_PACKAGES` or `~/.nuget/packages`); native build intermediates stay under
`target/`. No Android ILSpy runtime is checked in; each consumer produces its own
engine and packages it with the App.

## Runtime contents

Stage only the engine, the same 167 .NET10.0.12 reference DLLs, applicable licenses
and this C ABI header as a bounded runtime bundle with source/package identities.
Do not package or preload `librxdopenssl.so`; the producer has no private OpenSSL
loader or link wrapper. No .NET runtime/ILSpy source fork or hash stub is used.

ILSpy11.2's metadata public-key hashes use managed SHA-1. With no OpenSSL sidecar,
13 method/class targets and two invalid-token cases were checked in both ADB
shell and a targetSdk36 App sandbox: async/iterator, LINQ, closures/generics and
published Newtonsoft.Json. Source, warnings and mappings agreed; an audited
engine made no OpenSSL `dlopen` attempt. This covers the renderer's tested paths,
not arbitrary .NET cryptography. Revalidate after engine, runtime or scope changes.

The archive's native build should record `source_inputs` for the production
Exports/NOTICE and this Engine project. No prebuilt archive is checked in; pass
the produced `.so` and reference metadata directly to the App build. Consumers
must also remove legacy OpenSSL preloads and package entries.

The producer inputs and archive metadata are reviewed provenance, not proof that
an arbitrary third-party binary was built from the declared source. Test the exact
library hashes in an actual App sandbox before updating the checked-in bundle.
