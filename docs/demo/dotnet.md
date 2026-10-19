# .NET static queries

The examples use the repository's independently rebuildable handwritten PE/CLI fixture;
no method is executed.

```sh
cargo build --release
python3 tests/fixtures/dotnet/generate.py
rxd=target/release/rxd
$rxd dotnet outline tests/fixtures/dotnet/Fixture.dll --method Add
```

Take the content-bound ID from a `method` record and query:

```sh
id=$($rxd dotnet outline tests/fixtures/dotnet/Fixture.dll --method Add | python3 -c 'import json,sys; print(next(r["id"] for r in map(json.loads,sys.stdin) if r["record"]=="method"))')
$rxd dotnet get-method tests/fixtures/dotnet/Fixture.dll "$id"
$rxd dotnet get-method tests/fixtures/dotnet/Fixture.dll "$id" --il
$rxd dotnet strings tests/fixtures/dotnet/Fixture.dll --contains hello
$rxd dotnet refs tests/fixtures/dotnet/Fixture.dll --token 0x06000006
$rxd dotnet get-method tests/fixtures/dotnet/Fixture.dll "$id" --perf
```

JSONL `schema_version` is 2 and content is under `detail`. Method records navigate by
outer `id`; refs hits route to the caller through outer `context_id`. `get-method`
returns one `method_body` envelope with `backend/command/schema_version`; read C# from
`detail.code`, original IL from `detail.il_hex`, completeness from `detail.complete`, and
embedded metadata from `detail.method.detail`.

```sh
caller=$($rxd dotnet refs tests/fixtures/dotnet/Fixture.dll --token 0x70000001 | python3 -c 'import json,sys; print(next(r["context_id"] for r in map(json.loads,sys.stdin) if r["record"]=="reference"))')
$rxd dotnet get-method tests/fixtures/dotnet/Fixture.dll "$caller" --il
```

String hits already carry method/class FETCH IDs; no heap token needs to be constructed.
Check the complete summary, then route the hit contexts:

```sh
fixture=tests/fixtures/dotnet/Fixture.dll
$rxd dotnet strings "$fixture" --contains hello -o target/string-hits.jsonl
caller=$(python3 -c 'import json; rows=[json.loads(line) for line in open("target/string-hits.jsonl")]; assert rows[-1]["complete"]; print(next(r["context_id"] for r in rows if r["record"]=="string" and r["context_id"]))')
owner=$(python3 -c 'import json; rows=[json.loads(line) for line in open("target/string-hits.jsonl")]; assert rows[-1]["complete"]; print(next(r["detail"]["declaring_type_id"] for r in rows if r["record"]=="string" and r["context_id"]))')
$rxd dotnet get-method "$fixture" "$caller" --il
$rxd dotnet get-class "$fixture" "$owner" --il
$rxd dotnet refs "$fixture" --string $'hello\nlambda'
```

`string.detail.il_offsets` lists every original `ldstr` offset of that heap entry in the
same method; shared strings appear per method, and distinct equal-valued heap entries stay
separate. Unreferenced inventory has explicit status and empty navigation:

```sh
python3 tests/fixtures/dotnet/strings/generate.py
$rxd dotnet strings tests/fixtures/dotnet/strings/Strings.dll --contains unused
```

The unreferenced entry has `status: unreferenced`; context_id, parent_id, method_token
and declaring_type_id are null and il_offsets is `[]`. NUL or isolated surrogates cannot
be expressed in UTF-8 argv; use the returned original token with `refs`. Entries with
`value: null` still retain complete `utf16_units`.

Exception methods are fetched by the same member ID; with the engine enabled the C#
reconstruction and the preserved EH table are both visible:

```sh
id=$($rxd dotnet outline tests/fixtures/dotnet/Fixture.dll --method NestedFinally | python3 -c 'import json,sys; print(next(r["id"] for r in map(json.loads,sys.stdin) if r["record"]=="method"))')
$rxd dotnet get-method tests/fixtures/dotnet/Fixture.dll "$id"
```

Original IL `leave` crosses two finally handlers; `get --il` preserves original
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
  id=$($rxd dotnet outline "$sugar" --type Rxd.Sugar.Probe --method "$method" | python3 -c 'import json,sys; print(next(r["id"] for r in map(json.loads,sys.stdin) if r["record"]=="method"))')
  $rxd dotnet get-method "$sugar" "$id"
  $rxd dotnet get-method "$sugar" "$id" --il
done
```

Successful output uses `csharp`/true/non-empty code and demonstrates async/await, yield,
captured lambdas/local functions, LINQ and common syntax sugar. Recovered state machines
do not disguise MoveNext offsets as the entry method; mapping may be empty. Recovery is
not semantic equivalence or a recompilation proof.

`get` always includes original IL and instruction offsets. With `csharp-decompiler` and a
prebuilt engine, success returns `csharp`, `decompiled: true`, code and an IL→line
mapping; a default build or missing runtime library returns complete `il`,
`decompiled: false`, `code: null` and `engine_unavailable`. Engine failures keep complete
IL/EH and an explicit reason. See [the engine guide](../../benches/ilspy/README.md).
Changing any input byte invalidates old IDs. Outline/refs follow the
header/records/summary completeness contract.

## Type to complete original IL

```sh
closure=tests/fixtures/dotnet/closure/Closure.dll
type_id=$($rxd dotnet outline "$closure" --type 'Rxd.Closure.Root`1' | python3 -c 'import json,sys; print(next(r["id"] for r in map(json.loads,sys.stdin) if r["record"]=="type"))')
$rxd dotnet get-class "$closure" "$type_id" --il
$rxd dotnet get-class "$closure" "$type_id" --il -o target/closure-il.json --perf
$rxd dotnet get-class "$closure" "$type_id"
$rxd dotnet get-class "$closure" "$type_id" -o target/closure-csharp.json --perf
```

The type and its members bind the same assembly bytes; a member's `parent_id` can be
passed directly to `get-class`. `--il` bypasses the C# engine and preserves complete
original bodies/EH.

## Method, field and type XREF

```sh
$rxd dotnet refs tests/fixtures/dotnet/Fixture.dll --token 0x0a000001
$rxd dotnet refs tests/fixtures/dotnet/Fixture.dll --token 0x0a000001 --semantic
$rxd dotnet refs tests/fixtures/dotnet/Fixture.dll --id '<method-or-field-or-type-id>'
$rxd dotnet refs tests/fixtures/dotnet/Fixture.dll --string 'hello'
```

String refs match UTF-16 exactly across every equal heap entry and original `ldstr` site.
`--token` preserves raw operand identity; `--id` and `--token --semantic` prove member/
type identity and keep unproven targets `external`/`unresolved`/`ambiguous` with null
canonical ID. Method/field/type XREF reports only explicit instruction operands; it does
not infer reflection, cross-assembly loading or virtual dispatch. See the
[.NET design](../design/dotnet-design.md#commands-and-protocol).

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
