# ILSpy decompiler engine (ICSharpCode.Decompiler, NativeAOT shared library)

The `.NET` C# reconstruction backend calls a pinned ILSpy
`ICSharpCode.Decompiler`, built as a NativeAOT shared library and invoked
in-process over a stable C ABI. It statically decompiles managed assemblies;
it never loads or executes the analyzed assembly, and the shared library is
self-contained (no CoreCLR dependency on the target machine).

The reference is the official `ICSharpCode.Decompiler` NuGet package
`11.1.0.9782`, repository commit `a6909b2e1062d2404a52711926fa74030bd96119`.
Package SHA-256:
`1f76df4d35193ba4eeb50ffcd84eae381a2c48c67f95c1104dc406759448a811`.
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
macOS is ARM64 only. No ILSpy runtime is checked in; Android App engines are
produced separately from source.

## Build the NativeAOT shared library

Requires the .NET 10 SDK (CI pins 10.0.401), Python 3, and the native toolchain:
Xcode Command Line Tools on macOS, clang + zlib1g-dev on Ubuntu, or Visual Studio
C++ Build Tools on Windows. NativeAOT cross-OS compilation is unsupported.
The CI matrix builds natively for Linux x64/arm64, Windows x64 and macOS arm64.

```sh
python3 scripts/build-ilspy.py osx-arm64
python3 scripts/build-ilspy.py linux-x64
python3 scripts/build-ilspy.py linux-arm64
python scripts/build-ilspy.py win-x64
# Existing macOS entry point remains available (ARM64 only):
benches/ilspy/build-native.sh osx-arm64
```

Use the command matching the current host OS. The script selects an explicit
`--dotnet` path, `DOTNET_ROOT`, the existing isolated `~/.dotnet10` SDK, or `dotnet`
on PATH, in that order. It never installs or overwrites an SDK. It verifies both
the downloaded and restored ILSpy package SHA-256, then publishes the first-party
`crates/rxd-dotnet/native/rxd-dotnet-native.csproj`. Downloads, the SDK CLI home,
NuGet cache, intermediates and native products stay under `target/dotnet-native/`.

Products under `<rid>/`: `librxd_dotnet_native.dylib` (macOS),
`librxd_dotnet_native.so` (Linux), or `rxd_dotnet_native.dll` (Windows), plus
`build-info.json` recording SDK, package identity and library size/hash.
The library uses `net10.0`, `PublishAot=true`, `NativeLib=Shared` and invariant
globalization. Expected upstream IL2026/IL3002 warnings remain visible.

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
atomically into `<cache-base>/rxd-ilspy/<bundle-sha256>/`; subsequent calls verify
the native library before loading it. Cache policy is shared with Kuna in
`rxd-core::bundle`: `RXD_CACHE_DIR`, XDG cache, or platform user-cache defaults.
Queries without reconstruction and explicit `--il` do not create an ILSpy cache.
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
Latency acceptance below is macOS-specific; CI correctness tests do
not claim Linux/Windows latency acceptance.

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
content-bound method ID and validates that method's original instruction boundary.
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
Unavailable sequence points and whole-module calls can also yield empty mapping.

Native backstops are 64 MiB input and 32 MiB encoded JSON output (including
escaping/mapping). Code text and JSON growth are bounded; these are not an
adjustable work/time budget or an OS sandbox. Native hard-cap errors propagate
as atomic `RESOURCE_LIMIT` errors, with no successful reconstruction fallback.
Nested scope traversal is bounded to depth 64. The existing isolated worker
introduces no wall deadline; process termination reports failure. Every returned
allocation is freed exactly once, including malformed and oversized responses.
Rust must apply its cumulative class query budgets
before entry and retain worker isolation. The assembly endpoint is available,
not the ordinary method query or a latency acceptance workload.

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

`tests/dotnet/sugar.rs` is the real CLI recovery seam. Feature-enabled tests
assert C# success and lexical evidence of async/await, yield, captured lambda
and local function, LINQ query syntax, initializers, using, lock, null propagation
and interpolation. They are cfg-excluded from explicitly unbundled tests, not from the full default profile. The
independent byte-exact raw IL/EH checks and genuine failed-method fallback tests
remain in both configurations. No C# formatting snapshot is an oracle.

After the separate native build and feature-enabled release build above:

```sh
python3 benches/dotnet.py --samples 20 --cpu-model 'Apple M3 Pro' --memory-gb 36
# Alternate entry point to the same shipped-engine acceptance workloads:
python3 benches/dotnet_ilspy.py --samples 20 --cpu-model 'Apple M3 Pro' --memory-gb 36
python3 benches/dotnet_coverage.py --methods 48 --cpu-model 'Apple M3 Pro' --memory-gb 36
```

Prepare the pinned runtime assemblies as described in
[`tests/corpus/dotnet/README.md`](../../tests/corpus/dotnet/README.md).
Reports and atomic output workloads stay under ignored `target/benchmarks/`.
The latency runner builds Rust with `csharp-decompiler` unless `--binary` is
supplied; it never builds .NET or launches an external managed runner. Override
`--native-library` to select a trusted produced dylib; its path, size and SHA-256
are recorded and applied to every CLI invocation. `RXD_REFDIR` is also recorded.
It preserves all per-workload samples and stops on unexpected recovery failure
or a >150 ms latency sample instead of accepting a fallback as latency evidence.
WindowsBase's non-framework Accessibility dependency is an explicit expected
single-module conservative-degrade correctness workload, excluded from C#
latency acceptance; full IL/EH equality is still checked. System.Linq and
System.Text.Json use hash-pinned .NET 10.0.12 osx-arm64 runtime artifacts from
`--framework` (default `~/.dotnet10/shared/Microsoft.NETCore.App/10.0.12`). The coverage survey
retains every failure and source mapping; it is not a latency run.

Explicit unaffected JS, ASAR and JVM latency workloads use authorized pinned
inputs (ASAR is a synthetic wrapper). DEX functional non-regression is covered
by CLI E2E; DEX latency is not measured here because there is no authorized
representative input. These are separate per-workload numbers, never pooled
with .NET to hide slow queries.

Report original IL/EH equality checks, independent fixtures, every selected
method's C# output, reference diagnostics/errors, input sizes/hashes,
library hash, and per-workload fresh-process latency samples, median, P95 and
>150 ms proportion. Latency is end-to-end wall time of a fresh CLI process
including dlopen and NativeAOT runtime initialization. File cache is warm,
without CPU isolation. Rendering succeeds independently of whether C# is
complete, recompilable or semantically equivalent; unresolved-type diagnostics
remain visible. Both selected successes and failures are retained. Synthetic
fixtures and pinned upstream test artifacts are not broad independent production
acceptance.

A selected whole-class measurement keeps each workload's raw >150 ms proportion and
records its applicable bound. The runner that used to do this was removed with the rest
of the per-backend runners (`benches/README.md`). Whole-
class/whole-function decompilation exceptions follow the two grounds in
[AGENTS.md](../../AGENTS.md#performance-requirements); each exception must record
its input hash, selected target, release measurements and numeric bound. Ordinary
queries and raw IL retain 150 ms.
