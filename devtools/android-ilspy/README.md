# Android ARM64 App embedding producer

This is a separate `linux-bionic-arm64` NativeAOT build of the unchanged production
C# bridge, not a GNU/Linux library or complete rxd Android CLI port. It produces a
C ABI engine for an App-owned NDK/JNI adapter. No PoC instrumentation JNI class,
APK, test key or consumer AAR is shipped. Public declarations: `rxd_dotnet.h`.

## Producer toolchains

Measured producer: .NET SDK10.0.401 / NativeAOT10.0.12, NDK28.2.13676358,
`aarch64-linux-android24-clang`; fixed ILSpy11.1.0.9782 and its existing NuGet
SHA-256. Output ELF LOAD alignment is 16KiB; actual execution proof is on a 4KiB
Android16/API36 device only. Do not claim a real 16KiB-runtime result. A refreshed
bundle that has not been rerun on that device must say so in its validation
metadata and documentation.

From the repository root (macOS host shown):

```sh
export DOTNET_ROOT="$HOME/.dotnet10"
export NDK_BIN="$HOME/Library/Android/sdk/ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin"
mkdir -p target/android-ilspy
"$NDK_BIN/aarch64-linux-android24-clang" -c -fPIC -O2 -fvisibility=hidden \
  devtools/android-ilspy/private_loader.c -o target/android-ilspy/private_loader.o
"$DOTNET_ROOT/dotnet" publish devtools/android-ilspy/Engine.csproj -c Release \
  -r linux-bionic-arm64 -p:DisableUnsupportedError=true \
  -p:PublishAotUsingRuntimePack=true -p:StripSymbols=false \
  -p:CppCompilerAndLinker="$NDK_BIN/aarch64-linux-android24-clang" -m:1
"$NDK_BIN/llvm-strip" --strip-unneeded target/android-ilspy/publish/librxd_dotnet_native.so
```

Verify downloaded/restored NuGet bytes against the official fixed package hash
in `crates/rxd-dotnet/native/NOTICE`. Producer downloads/intermediates stay under
`target/`; no build happens during consumption of the prebuilt package.

## Private OpenSSL

The engine requires its private `librxdopenssl.so`, not Android's BoringSSL or
Termux. This artifact uses official OpenSSL3.6.5 source archive SHA-256
`a2157c2830efdec3788939b00c9b0638306d3f0bbb76dc4832ee503bb397df98`.
Build with NDK android-arm64/API24, `no-shared no-tests no-module -fPIC`, then
combine `libssl.a` and `libcrypto.a` with `--whole-archive`, unique SONAME
`librxdopenssl.so`, `-Bsymbolic`, 16KiB max/common-page alignment and libdl/libc.
Retain the Apache-2.0 OpenSSL license and actual library hash in the runtime
manifest. Updating OpenSSL requires rebuilding/revalidating this private library.

`private_loader.c` is linked only into the NativeAOT engine with `--wrap=dlopen`.
It redirects libssl basename probes to the private SONAME; other calls pass
through. It does not mutate environment or hook the whole App. There is no
.NET runtime/ILSpy source fork. NativeAOT static single-library OpenSSL linking
is not asserted to work; two libraries are intentionally shipped.

Stage engine, private OpenSSL, the same 167 .NET10.0.12 reference DLLs, applicable
licenses and this C ABI header as a bounded runtime bundle with source/package
identities. The archive's native build records `source_inputs` for the production
Exports/NOTICE and this Engine project/loader. Import the Android collection with:

```sh
python3 scripts/prebuilt-ilspy.py --android-app add android-arm64 --source target/android-ilspy/bundle
python3 scripts/prebuilt-ilspy.py --android-app check
```

The producer inputs and archive metadata are reviewed provenance, not proof that
an arbitrary third-party binary was built from the declared source. Test the exact
library hashes in an actual App sandbox before updating the checked-in bundle.
