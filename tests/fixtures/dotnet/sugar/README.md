# Original C# sugar fixture (ticket #10)

`Sugar.dll` is original repository test data compiled from the complete source in
`Sugar.cs`. It is a managed library with no process entry point. It covers
async/await (`Task.Yield`), iterator `yield return`, a captured lambda calling a
local function, LINQ query syntax, array/object/collection initializers, `using`,
`lock`, null propagation/coalescing and string interpolation. Compiler-generated
state machines and closure helpers are intentionally retained. No framework
assembly or third-party source is copied into the fixture.

Ordinary CLI tests use the committed DLL and JSON offline. They require neither
.NET nor the native decompiler, and never execute the analyzed assembly. Only
explicit regeneration requires the fixed SDK; the inspector executes its own
metadata reader, **not** fixture methods (no `Assembly.Load` or reflection invoke).
The handcrafted `../Fixture.dll`, its generator and expectations are unchanged.

## Pinned provenance

Any .NET 10 SDK regenerates these fixtures (`global.json` rolls forward within the
10.0 feature bands); the values below are what produced the committed oracle.

- SDK: .NET **10.0.401**, SDK/MSBuild commit `e34a38d2ae`, MSBuild `18.9.11`.
- Roslyn `csc -version`: **5.9.0-1.26423.113
  (e34a38d2ae1fc26406a317517196e55c68ff83ab)**.
- `csc.dll` SHA-256:
  `99148255a90b5a6a0a37b7be3a51e8753bcdbb52f3a7785eb8bb227e99e34414`.
- Target: `net10.0`, C# `14.0`, `Microsoft.NETCore.App.Ref` **10.0.12**
  (SDK targeting pack); inspector runtime `Microsoft.NETCore.App` **10.0.12**.
- NuGet PackageReferences: **none**. `NuGet.Config` clears all package sources;
  builds use installed SDK packs only. The generator rejects package dependencies.
- `Sugar.cs` SHA-256:
  `46dbbaea665125ccbbcc5164a76cf44bf08672006ca391e65b27f65988f54fa6`.
- `Sugar.dll`: **8,704 bytes**, SHA-256
  `84de7739c8717a9860e419feeeec2e8c1cfe69faae2c72e91c778cbfdcb618bc`.
- MVID: `599f0c31-21c9-4ad6-ad95-2f4e0432fe8a`.
- PE timestamp: `0xa9ee0cfc` (deterministic content-derived value, not wall time).
- Initial generation: macOS 26.5 / Darwin 25.5, osx-arm64, Apple M3 Pro.

`Sugar.csproj` fixes Release optimization, deterministic and CI builds, nullable
analysis, assembly/file version `1.0.0.0`, package version `1.0.0`, no debug
symbols/PDB, and no git revision in informational version. The generator maps
source paths to `/_/sugar`; all intermediates stay in ignored
`target/dotnet-sugar/`. `global.json` disables SDK roll-forward. MVID and timestamp
are compiler-derived but stable for the pinned inputs; neither is used as content
identity. A clean rebuild and a build from a relocated source directory both
produced byte-identical DLLs; a clean regeneration also reproduced the JSON.

## Regenerate explicitly

From the repository root:

```sh
DOTNET_ROOT=$HOME/.dotnet10 PATH=$HOME/.dotnet10:$PATH \
  DOTNET_CLI_TELEMETRY_OPTOUT=1 python3 tests/fixtures/dotnet/sugar/generate.py
shasum -a 256 tests/fixtures/dotnet/sugar/Sugar.dll
```

The Python-stdlib driver invokes only `~/.dotnet10/dotnet`, verifies SDK selection,
compiles the library and independent inspector, and commits no intermediate
outputs. Do not install into or use a corrupted `~/.dotnet`: on macOS overwriting
signed SDK pages can cause SIGKILL. SDK installation is not part of regeneration
or ordinary tests.

## Independent original IL/EH oracle

`Inspect.cs` uses the SDK's `System.Reflection.Metadata` and
`System.Reflection.PortableExecutable.PEReader` to read the compiled file as
bytes. It obtains MethodDef tokens, declaring types (including nested types),
RVAs, **complete original IL bytes**, max stack, init-locals flag, local signature
tokens and ordered exception regions. `expected.json` is its recorded output
plus toolchain/source provenance, never rxd/dotscope/ILSpy output. Empty EH arrays
are recorded explicitly; catch tokens and all try/handler ranges are preserved.

Public `Rxd.Sugar.Probe` method tokens:

| Method | Token |
| --- | --- |
| AsyncAwait | `0x06000001` |
| Iterator | `0x06000002` |
| ClosureAndLocalFunction | `0x06000003` |
| Linq | `0x06000004` |
| Initializers | `0x06000005` |
| Using | `0x06000006` |
| Lock | `0x06000007` |
| NullPropagation | `0x06000008` |
| Interpolation | `0x06000009` |

The JSON records all **30** methods keyed by token (not name, since `.ctor` and
`MoveNext` repeat). Item accessors/constructor occupy `0x0600000a..0x0600000c`;
LINQ cached lambdas `0x0600000d..0x06000011`; captured local/lambda helpers
`0x06000012..0x06000014`; async state machine `0x06000015..0x06000016`;
iterator state machine `0x06000017..0x0600001e`.

Nonempty EH expectations:

- Using (`0x06000006`): finally, try `[7,16)`, handler `[16,26)`.
- Lock (`0x06000007`): finally, try `[4,32)`, handler `[32,42)`.
- Async `MoveNext` (`0x06000015`): catch `0x0100001e`, try `[7,111)`,
  handler `[111,136)`.

`tests/dotnet/sugar.rs` launches the real CLI through the existing `run` harness:
all 30 methods must be indexed with exact SHA-256/token-bound IDs and yield
complete byte-exact `get --il` records, ordered EH and contiguous full instruction
coverage. Renaming unchanged bytes preserves identity/output; mutating padding
changes identity and rejects a stale ID without partial output. With
`--features csharp-decompiler`, the same CLI seam also asserts C# recovery and
syntax evidence for all nine public sugar methods, without formatting snapshots.
Those recovery tests require the separately built native engine and are
cfg-excluded from the default offline build.
