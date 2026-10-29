# ILSpy decompiler engine (ICSharpCode.Decompiler, NativeAOT shared library)

The `.NET` C# reconstruction backend calls a pinned ILSpy
`ICSharpCode.Decompiler`, built as a NativeAOT shared library and invoked
in-process over a stable C ABI. It statically decompiles managed assemblies;
it never loads or executes the analyzed assembly, and the shared library is
self-contained (no CoreCLR dependency on the target machine).

The reference is the official `ICSharpCode.Decompiler` NuGet package
`11.2.0.10050`, repository commit `c86190ef4e24f3137c13a9275316042842d2fada`.
Package SHA-256:
`e29000428db534f834bb074b795a178ef21dece652505f1d05d4310ca5f48b3b`.
ILSpy is MIT licensed; the library is downloaded and built, not vendored.

The full CLI validates/hashes/decodes the input and writes its record; ABI-only
numbers cannot replace fresh-process CLI acceptance. The only required v11 API
adaptation delegates `IAssemblyResolver.BeginSnapshot` to the underlying
resolver; reference restrictions, C# 10 settings and output/limit policies stay
unchanged.

## Build and embed the native runtime

Desktop ILSpy is produced per target; no desktop bundle is checked in.

```sh
python3 scripts/build-ilspy.py osx-arm64
RXD_ILSPY_EMBED_RS="$PWD/target/dotnet-native/osx-arm64/embed.rs" \
  cargo build --release --features bundled-ilspy
```

Cargo embeds only the explicit producer include and never downloads or invokes
.NET. `bundled-ilspy` without `RXD_ILSPY_EMBED_RS` fails with the producer command.
macOS is ARM64 only. No ILSpy runtime is checked in; Android consumers use the
same `linux-musl-arm64` producer. No Android SDK/NDK build path is maintained here.

## Build the NativeAOT shared library

Requires .NET SDK 10.0.401 / NativeAOT 10.0.12 and Python 3. Linux targets use
Zig 0.17.0 and musl, including cross-builds from macOS. GNU engine targets are
not produced. Linux Rust CLI and engine bundle identities must both be musl.
macOS uses Xcode CLT and Windows uses Visual Studio C++ tools on their native OS.
The CI validates each desktop target and also cross-builds ARM64 musl on macOS.

```sh
python3 scripts/build-ilspy.py osx-arm64
python3 scripts/build-ilspy.py linux-musl-arm64
python3 scripts/build-ilspy.py linux-musl-x64
python scripts/build-ilspy.py win-x64
# Existing macOS entry point remains available (ARM64 only):
benches/ilspy/build-native.sh osx-arm64
```

Linux musl commands run on either macOS or Linux without a Linux build VM.
The script selects an explicit `--dotnet` path, `DOTNET_ROOT`, the isolated
`~/.dotnet10` SDK, or `dotnet` on PATH, in that order; `--zig` overrides Zig on PATH.
It never installs an SDK or silently falls back to another linker. The downloaded
and restored ILSpy package SHA-256 is verified before the common first-party
`crates/rxd-dotnet/native/rxd-dotnet-native.csproj` is published. Linux uses target
NativeAOT runtime packs, forwards all linker arguments, and strips only ELF debug
sections with Zig. Intermediates are isolated per RID. Downloads, SDK CLI home,
NuGet cache, compiler wrappers and products stay under `target/dotnet-native/`.

Products under `<rid>/`: `librxd_dotnet_native.dylib` (macOS),
`librxd_dotnet_native.so` (Linux), or `rxd_dotnet_native.dll` (Windows), plus
`build-info.json` recording SDK/runtime, Zig/linker target, package identity,
source hashes and library size/hash. Linux builds use matching musl Rust targets;
`build-bundled.py` selects musl by default on Linux and uses pinned
cargo-zigbuild 0.23.4 for Rust linker compatibility. The shared engine requires
dynamic musl linking, so Linux execution needs its architecture-matched musl loader.
The library uses `net10.0`, `PublishAot=true`, `NativeLib=Shared` and invariant
globalization. Expected upstream IL2026/IL3002 warnings remain visible.

Android deployment supplies its musl loader, an accessible ELF interpreter path,
and writable `TMPDIR`/`RXD_CACHE_DIR`. Explicit loader invocation changes
`/proc/self/exe`, which matters to the self-spawning CLI worker. App packaging
and device validation are consumer responsibilities, not repository build gates.

## Enable the Rust renderer

For the complete single-file distribution:

```sh
python3 scripts/build-bundled.py --features bundled-ilspy
export RXD_KUNA_EMBED_RS="$PWD/target/kuna/aarch64-apple-darwin/embed.rs"
RXD_ILSPY_EMBED_RS="$PWD/target/dotnet-native/osx-arm64/embed.rs" \
  cargo test --test cli --features bundled-kuna,bundled-ilspy
RXD_ILSPY_EMBED_RS="$PWD/target/dotnet-native/osx-arm64/embed.rs" \
  cargo test --release --test cli --features bundled-kuna,bundled-ilspy
```

`build-ilspy.py` also emits `bundle.tar.gz`, `manifest.json` and `embed.rs` in
its RID output directory. The deterministic archive includes the native library,
SDK .NET 10 reference-pack DLLs (about 5.77 MiB before compression) and licenses.
It excludes XML documentation, debug symbols and the SDK/runtime distribution.

`bundled-ilspy` implies `csharp-decompiler`. Cargo only embeds the prepared
archive, never invokes .NET. First C# FETCH verifies and extracts the runtime
atomically into `<cache-base>/rxd-ilspy/<bundle-sha256>/`; subsequent calls check a
recorded size/mtime/inode stamp and re-hash the native library only when it changed.
Cache policy is shared with Kuna in
`rxd-core::bundle`: `RXD_CACHE_DIR`, XDG cache, or platform user-cache defaults.
Queries without reconstruction and explicit `--raw` do not create an ILSpy cache.
A corrupt cache fails with a diagnostic; remove that version directory to rebuild it.

`libloading` retains the library for process lifetime. The optional
`rxd_set_reference_directory` initialization ABI passes the trusted installed
reference directory to the resolver without changing process environment.
`RXD_DOTNET_NATIVE_LIBRARY` and `RXD_REFDIR` remain explicit runtime overrides;
older supplied libraries without the initializer preserve their existing behavior.

External-library development is still supported:

```sh
cargo build --release --features csharp-decompiler
```

This mode locates the produced library under `target/dotnet-native/<rid>/`,
with a build-time `RXD_DOTNET_NATIVE_LIBRARY` override. At runtime, an explicit
override wins, followed by an adjacent library. Only a binary still in its
original Cargo profile directory falls back to the build-time path. The prepared
library's adjacent `dotnet-refs/` is supplied through the initialization ABI.

The CI archive requires only `rxd` / `rxd.exe` at runtime. The package check
copies the executable with no engine or metadata beside it and removes runtime
overrides before testing Kuna, C# async/class recovery, lazy extraction and
cached-library corruption rejection. Default `all-feature` enables all parsers and
MCP but not desktop ILSpy; explicit unbundled builds
(`--no-default-features --features all-backends`) return original IL/EH when C# is
unavailable. Explicit invalid external libraries retain that fallback; an
embedded-runtime integrity failure is an error.

## Marshalling and resolution

The C ABI uses the caller's immutable module bytes, **not** the source file
path. This preserves rxd's fixed-input identity invariant: `PEFile` reads a `MemoryStream` with synthetic name `rxd-input.dll`,
never a reopened mutable original path. No target assembly is loaded/executed.

```c
uint8_t *rxd_decompile_method(const uint8_t *module, int module_len,
                             int metadata_token, int *out_len);
uint8_t *rxd_decompile_class(const uint8_t *module, int module_len,
                            int type_definition_token, int *out_len);
void rxd_free(uint8_t *buffer);
```

C calling convention; `module_len` and `out_len` are byte counts (32-bit signed).
The caller keeps the readable input alive through the call, supplies a writable
`out_len`, and frees the returned allocation only with `rxd_free`. No NUL
terminator is included. Null input/invalid lengths yield a failure payload;
null `out_len` or failure to allocate the error payload yields null (length zero
when writable). `rxd_free(NULL)` is valid. Invalid pointers cannot be validated
by this ABI and must never be passed.

Returned UTF-8 JSON has exactly this schema:

```json
{"ok":true,"reason":null,"code":"C# text","mapping":[{"il_offset":0,"line":3}]}
{"ok":false,"reason":"invalid_method_token","code":null,"mapping":[]}
```

The optional class endpoint selects the validated TypeDef directly with
`CSharpDecompiler.Decompile(handle)`, including an explicitly selected generated
type. It never decompiles the whole module and trims text or concatenates method
snippets. Class scope is the selected type and recursively owned nested types,
excluding sibling and inherited definitions. Its successful payload uses
`mapping:[{"method_token":100663303,"il_offset":0,"line":3}]`; the method payload
and existing method ABI are unchanged. A method-capable older library without
this symbol still renders methods; class FETCH returns `class_abi_unavailable`
and complete original data. Rust qualifies every class point with its actual
file-scoped method coordinate and validates that method's original instruction boundary.
C# may express generated original members as syntax sugar, while the complete
raw metadata/IL inventory remains available in the same record. No-body members
remain declarations. Failed reconstruction produces no partial C# or fabricated
throwing stubs.

The native layer does not decide `representation`/`decompiled`; Rust `render`
owns those policies. Engine errors, prohibited non-framework references,
IL warnings or invalid IL produce no partial code. Missing optional framework
metadata may instead leave unresolved-type diagnostics in successful C#. Mapping contains real ILSpy sequence points only for
the selected MethodDef, sorted by offset/line, with 1-based lines in `code`.
Generated methods' independent IL offsets are excluded. In particular ILSpy
labels recovered async/iterator functions with the kickoff token but retains
MoveNext sequence points; those foreign-domain entries are excluded entirely.
State-machine recovery can therefore return `mapping:[]` with valid C#.
Unavailable sequence points can also yield empty mapping.

Native backstops are 64 MiB input and 32 MiB encoded JSON output (including
escaping/mapping). Code text and JSON growth are bounded; these are not an
adjustable work/time budget or an OS sandbox. Native hard-cap errors propagate
as atomic `RESOURCE_LIMIT` errors, with no successful reconstruction fallback.
Nested scope traversal is bounded to depth 64. The existing isolated worker
introduces no wall deadline; process termination reports failure. Every returned
allocation is freed exactly once, including malformed and oversized responses.
Rust applies its cumulative class query budgets before entry and retains worker
isolation. The bridge exposes method and class reconstruction only.

Framework metadata is optional build-independent data, not a runtime:
`RXD_REFDIR` overrides the installed metadata with a path-separated list of **trusted core-framework directories**
(e.g. a copied `Microsoft.NETCore.App/10.0.12` directory). A restricted wrapper
around `UniversalAssemblyResolver` accepts only core-framework names/key tokens
and paths in those directories; it rejects arbitrary sibling dependencies,
GAC/system fallback paths and other modules. Missing optional framework probes return null to ILSpy; unresolved-type
diagnostics remain in successful C# output. Genuine engine failures still
return a conservative fallback. Do not point
`RXD_REFDIR` at untrusted analyzed-input directories. CI embeds the .NET 10 reference metadata, extracted into the runtime cache; loading the dylib itself requires no .NET installation.

For a temporary dlopen harness, read `System.Linq.dll` to a fixed buffer, call
`rxd_decompile_method(buffer, length, 0x06000001, &out_len)`, parse the JSON,
assert `ok`/nonempty `code` and valid mapping offsets/lines, then `rxd_free`.
Use `RXD_REFDIR=$HOME/.dotnet10/shared/Microsoft.NETCore.App/10.0.12` for this
SDK's framework references. Also check invalid-token/corrupt-input failures
have `code:null` and empty mapping.

## Recovery and release acceptance

C# rendering quality is the engine's own behavior. rxd does not assert which
constructs a given assembly renders to, so there is no C# snapshot or syntax
oracle in the test suite. What rxd owns is the seam: a fetch answers with source
or with the original IL rather than with nothing, and `--raw` stays the original
instruction listing. The fixtures keep independently read byte-exact IL/EH
records to diff that output against.

For benchmark commands and measurement conditions, see
[CLI benchmarks](../README.md). Keep release query measurements separate from
functional E2E; ordinary queries and raw IL retain the 150 ms target.
