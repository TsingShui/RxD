# .NET static queries

The examples use the repository's independently rebuildable handwritten PE/CLI fixture;
no method is executed.

```sh
cargo build --release
python3 tests/fixtures/dotnet/generate.py
rxd=target/release/rxd
$rxd dotnet methods tests/fixtures/dotnet/Fixture.dll --method Add
```

Copy the `at` coordinate from a `method` record and query; the stream can also be
piped straight into the FETCH command, which validates that it came from this input:

```sh
at=$($rxd dotnet methods tests/fixtures/dotnet/Fixture.dll --method Add | python3 -c 'import json,sys; print(next(r["at"] for r in map(json.loads,sys.stdin) if r["record"]=="method"))')
$rxd dotnet get-method tests/fixtures/dotnet/Fixture.dll --at "$at"
$rxd dotnet get-method tests/fixtures/dotnet/Fixture.dll --at "$at" --raw
$rxd dotnet methods tests/fixtures/dotnet/Fixture.dll --method Add | $rxd dotnet get-method tests/fixtures/dotnet/Fixture.dll --from -
$rxd dotnet strings tests/fixtures/dotnet/Fixture.dll -f hello
$rxd dotnet xref tests/fixtures/dotnet/Fixture.dll --at 0x06000006
$rxd dotnet xref tests/fixtures/dotnet/Fixture.dll -f hello --kind string
$rxd dotnet methods tests/fixtures/dotnet/Fixture.dll > target/methods.jsonl
$rxd dotnet xref tests/fixtures/dotnet/Fixture.dll --from target/methods.jsonl
$rxd dotnet get-method tests/fixtures/dotnet/Fixture.dll --at "$at" --perf
```

JSONL `schema_version` is 2 and content is under `detail`. Method records navigate by
outer `at`; xref hits route to the caller through outer `context_at`. `get-method`
returns one `method_body` envelope with `backend/command/schema_version`; read C# from
`detail.code`, original IL from `detail.il_hex`, completeness from `detail.complete`, and
embedded metadata from `detail.method.detail`.

```sh
caller=$($rxd dotnet xref tests/fixtures/dotnet/Fixture.dll --at 0x70000001 | python3 -c 'import json,sys; print(next(r["context_at"] for r in map(json.loads,sys.stdin) if r["record"]=="reference"))')
$rxd dotnet get-method tests/fixtures/dotnet/Fixture.dll --at "$caller" --raw
```

String hits already carry method/class coordinates; no heap token needs to be constructed.
Check the complete summary, then route the hit contexts:

```sh
fixture=tests/fixtures/dotnet/Fixture.dll
$rxd dotnet strings "$fixture" -f hello -o target/string-hits.jsonl
caller=$(python3 -c 'import json; rows=[json.loads(line) for line in open("target/string-hits.jsonl")]; assert rows[-1]["complete"]; print(next(r["context_at"] for r in rows if r["record"]=="string" and r["context_at"]))')
owner=$(python3 -c 'import json; rows=[json.loads(line) for line in open("target/string-hits.jsonl")]; assert rows[-1]["complete"]; print(next(r["parent_at"] for r in rows if r["record"]=="string" and r["context_at"]))')
$rxd dotnet get-method "$fixture" --at "$caller" --raw
$rxd dotnet get-class "$fixture" --at "$owner" --raw
$rxd dotnet xref "$fixture" -f $'hello\nlambda' --kind string
```

`string.detail.il_offsets` lists every original `ldstr` offset of that heap entry in the
same method; shared strings appear per method, and distinct equal-valued heap entries stay
separate. Unreferenced inventory has explicit status and empty navigation:

```sh
python3 tests/fixtures/dotnet/strings/generate.py
$rxd dotnet strings tests/fixtures/dotnet/strings/Strings.dll -f unused
```

The unreferenced entry has `status: unreferenced`; context_at, parent_at, method_token
and parent_at are null and il_offsets is `[]`. NUL or isolated surrogates cannot
be expressed in UTF-8 argv; use the returned original token with `xref`. Entries with
`value: null` still retain complete `utf16_units`.

Exception methods are fetched by the same member ID; with the engine enabled the C#
reconstruction and the preserved EH table are both visible:

```sh
at=$($rxd dotnet methods tests/fixtures/dotnet/Fixture.dll --method NestedFinally | python3 -c 'import json,sys; print(next(r["at"] for r in map(json.loads,sys.stdin) if r["record"]=="method"))')
$rxd dotnet get-method tests/fixtures/dotnet/Fixture.dll --at "$at"
```

Original IL `leave` crosses two finally handlers; `get --raw` preserves original
instructions and the ordered EH table.

## Enable C# and syntax sugar

A registered prebuilt bundle needs no .NET SDK (macOS ARM64 only):

```sh
cargo build --release --no-default-features --features backend-dotnet,bundled-ilspy
```

Cargo validates the matching `prebuilt/ilspy/` target and source identity offline;
missing/stale bundles fail instead of compiling or downloading automatically. The
NativeAOT producer is only for bundle maintenance:

```sh
python3 scripts/build-ilspy.py osx-arm64  # linux-x64, linux-arm64, win-x64
```

The pinned engine is ILSpy `11.1.0.9782`; see [ADR 0009](../adr/0009-dotnet-ilspy-v11.md)
and [`prebuilt/README.md`](../../prebuilt/README.md). Native builds need .NET 10 SDK and
the platform toolchain; Cargo never calls dotnet. Analysis only reads assemblies.

```sh
sugar=tests/fixtures/dotnet/sugar/Sugar.dll
for method in AsyncAwait Iterator ClosureAndLocalFunction Linq Initializers Using Lock NullPropagation Interpolation; do
  at=$($rxd dotnet methods "$sugar" --type Rxd.Sugar.Probe --method "$method" | python3 -c 'import json,sys; print(next(r["at"] for r in map(json.loads,sys.stdin) if r["record"]=="method"))')
  $rxd dotnet get-method "$sugar" --at "$at"
  $rxd dotnet get-method "$sugar" --at "$at" --raw
done
```

Successful output uses `csharp`/true/non-empty code and demonstrates async/await, yield,
captured lambdas/local functions, LINQ and common syntax sugar. Recovered state machines
do not disguise MoveNext offsets as the entry method; mapping may be empty. Recovery is
not semantic equivalence or a recompilation proof.

`get` always includes original IL and instruction offsets. With `csharp-decompiler` and a
prebuilt engine, success returns `csharp`, `decompiled: true`, code and an IL→line
mapping; a default build or missing runtime library returns complete `il`,
`decompiled: false`, `code: null`, `engine_unavailable` and a `reason_detail` explaining
the state. `representation: il` with `reason: null` means IL was requested with `--raw`,
not that reconstruction failed. See [the engine guide](../../benches/ilspy/README.md).

Coordinates are file-scoped: `0x0600000b` names metadata row 11 in *this* input, so
the same coordinate addresses the same row in another version, and a coordinate copied
from a different stream is caught by `--from` (which compares `source_id`) rather than by
the handle itself. Outline/xref follow the header/records/summary completeness contract.

## Type to complete original IL

```sh
closure=tests/fixtures/dotnet/closure/Closure.dll
type_at=$($rxd dotnet methods "$closure" --type 'Rxd.Closure.Root`1' | python3 -c 'import json,sys; print(next(r["at"] for r in map(json.loads,sys.stdin) if r["record"]=="type"))')
$rxd dotnet get-class "$closure" --at "$type_at" --raw
$rxd dotnet get-class "$closure" --at "$type_at" --raw -o target/closure-il.json --perf
$rxd dotnet get-class "$closure" --at "$type_at"
$rxd dotnet get-class "$closure" --at "$type_at" -o target/closure-csharp.json --perf
```

A body the engine reconstructs with caveats is returned together with those caveats;
`warnings` names the member and IL offset each one applies to:

```sh
$rxd dotnet get-method app.exe --at 0x06000003 | jq '.detail | {decompiled, warnings}'
{"decompiled":true,
 "warnings":["button1_Click: IL_0033: Unknown result type (might be due to invalid IL or missing references)", …]}
```

The type and its members bind the same assembly bytes; a member's `parent_at` can be
passed directly to `get-class`. `--raw` bypasses the C# engine and preserves complete
original bodies/EH.

## Method, field and type XREF

```sh
$rxd dotnet xref tests/fixtures/dotnet/Fixture.dll --at 0x0a000001
$rxd dotnet xref tests/fixtures/dotnet/Fixture.dll --at 0x06000006
$rxd dotnet xref tests/fixtures/dotnet/Fixture.dll -f hello --kind string
```

`-f ... --kind string` matches every heap entry that *contains* the pattern, and every
original `ldstr` site of those entries.
The header names the literal it resolved to rather than `null`:

```sh
$rxd dotnet xref tests/fixtures/dotnet/strings/Strings.dll --at 0x70000001 | head -1
{"record":"header",...,"detail":{"target":{"target_kind":"user_string","token":"0x70000001","value":"shared 世界\nλ"}}}
```

Reading IL needs no string lookup either: a `ldstr` operand carries `target_at` (its own
`0x70xxxxxx` coordinate) and the decoded literal as `string`:

```json
{"mnemonic":"ldstr","offset":3,"operand":{"kind":"token","value":1879048193,
 "target_at":"0x70000001","string":"shared 世界\nλ"}}
```

Any metadata token (`--at 0x0a000001` for an external member, `--at 0x06000006` for a
local definition) resolves member/type identity, and unproven targets stay
`external`/`unresolved`/`ambiguous` with null canonical ID. Method/field/type XREF reports
only explicit instruction operands; it does not infer reflection, cross-assembly loading
or virtual dispatch. See the [.NET design](../design/dotnet-design.md#commands-and-protocol).

## Android App embedding

The ARM64 Bionic engine, private OpenSSL, reference metadata, headers and licenses are in
[`prebuilt/android-ilspy/`](../../prebuilt/android-ilspy/README.md):

```sh
python3 scripts/prebuilt-ilspy.py --android-app check
```

Place both `.so` files in the App according to that directory and call the C ABI from
your own JNI/NDK adapter. It is not a generic AAR or the complete rxd Android CLI; do not
substitute GNU/Linux ARM64 libraries for Bionic.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend IDs, offsets, inference and degradation are in the
[.NET design](../design/dotnet-design.md#mcp-projection).
