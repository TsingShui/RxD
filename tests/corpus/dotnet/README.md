# .NET release measurement inputs

The ordinary offline regression fixture is independently assembled original data
in `tests/fixtures/dotnet/`. It is deliberately synthetic and does not establish
production performance or semantic coverage.

Exploratory release measurements use existing assemblies from the public
`ATRAPSLLC/dotscope` repository, fixed at commit
`3028b1ab92ec1b0ad15f046b3b70d5ee82336a00`, under
`dotscope/tests/samples/`. Reference source and binary artifacts remain in ignored
`target/`; they are not copied into this repository's offline fixtures.

Pinned inputs:

| Input | Bytes | SHA-256 |
| --- | ---: | --- |
| `WindowsBase.dll` | 2,255,024 | `d6712d9472c4e92cfa847d0ba64671deb41d92357d41320d9d6dd88bec960e6e` |
| `dotnet_10.0/System.Private.CoreLib.dll` | 15,767,040 | `a6acd5d87523255556bc17d964779aae71da1809623710aa4c6d091061b3246a` |

These are pinned upstream test artifacts with runtime method bodies, not an
independently collected production acceptance corpus. Preserve that limitation in
reports; do not replace them with reference-assembly stubs. Their upstream
provenance/license must be retained when separately redistributing binaries.

Preparation and measurement (do not execute the analyzed assemblies):

```sh
git clone https://github.com/ATRAPSLLC/dotscope target/dotnet-research/references/dotscope
git -C target/dotnet-research/references/dotscope checkout 3028b1ab92ec1b0ad15f046b3b70d5ee82336a00
benches/ilspy/build-native.sh osx-arm64
export RXD_REFDIR="$HOME/.dotnet10/shared/Microsoft.NETCore.App/10.0.12"
python3 benches/dotnet.py --cpu-model 'verified host CPU' --memory-gb 36
```

The benchmark builds Rust release with `csharp-decompiler` and measures the
pinned v11.1.0.9782 native engine, recording the dylib hash as well as
input/binary hashes, sizes, commands, C# recovery status,
per-workload fresh-process samples, median/P95/>150ms and cache conditions. It
includes rxd startup, analysis worker, reading/hash, parsing, reconstruction and
actual output. Preparation warms file caches; no cold-file claim is made. A
method that falls back to IL cannot prove decompiler latency. No analyzed code is
loaded or executed, and no relative ILSpy speed claim is made.

Aggregate IL acceptance additionally pins the independently published MIT
`Newtonsoft.Json` 13.0.3 NuGet package, entry `lib/net6.0/Newtonsoft.Json.dll`
(712,464 bytes, SHA-256
`22c649f75fce5be7c7ccda8880473b634ef69ecf33f5d1ab8ad892caf47d5a07`).
Package SHA-256 is
`872fc189e638ab1056555b03aaa38f68bcb54286e221aa646eb1129babf63c77`;
source URL and license are recorded in `class-expectations.json`. Keep the package,
assembly and full independent exploratory oracle under ignored `target/`.
The committed selective oracle pins JsonReader/JValue original scope, field and
method tokens, exact IL hashes, no-body declarations and ordered EH. It comes
from trusted System.Reflection.Metadata PEReader plus OpCodes operand descriptors,
never rxd; the analyzed assembly is not loaded or executed.

```sh
python3 benches/dotnet_class.py --binary target/release/rxd \
  --input target/dotnet-closure-corpus/newtonsoft-json-13.0.3/Newtonsoft.Json.dll \
  --cpu-model 'verified host CPU' --memory-gb 36
```

This measures independent production outline → type ID and complete selected
class IL to stdout/atomic files, at least 20 fresh-process samples per workload.
It validates scope and every selected original IL/EH before timing, preserves
actual output/hashes and samples under ignored target, and fails the 150 ms
requirement without averaging commands together. The selected classes do not
establish whole-assembly, cold-cache, semantic-XREF or C# decompilation performance.

String navigation additionally uses `string-expectations.json`, prepared from the
same published Newtonsoft.Json bytes. Independent ECMA-335 #US decoding is
cross-checked with the trusted PEReader's explicit `ldstr` sites; rxd output is
never an expectation source. The compact manifest fixes all 784 heap entries,
all 1,444 loads and 1,291 `(entry, method)` groups through canonical checksums,
including original heap identity, value, owner and sorted IL offsets. Separate
filtered and direct-value workloads also require complete nonzero expected uses.
The full exploratory PEReader oracle remains under ignored target.

```sh
python3 benches/dotnet_strings_oracle.py \
  --input target/dotnet-closure-corpus/newtonsoft-json-13.0.3/Newtonsoft.Json.dll \
  --metadata-oracle target/dotnet-closure-corpus/newtonsoft-json-13.0.3/independent-all.json \
  --output tests/corpus/dotnet/string-expectations.json
python3 benches/dotnet_strings.py --binary target/release/rxd \
  --input target/dotnet-closure-corpus/newtonsoft-json-13.0.3/Newtonsoft.Json.dll \
  --cpu-model 'verified host CPU' --memory-gb 36
```

Each unfiltered/filtered/direct query measures 20 fresh processes both to stdout
and atomic files. The search → method/class IL workflow measures three fresh
processes plus JSON ID routing and reports that cost separately. Optional
`--probe-input` records whole-input diagnostics from larger assemblies without
discarding failures or claiming they establish latency acceptance. Keep exact
argv, sizes/hashes, actual output, samples, hardware/cache conditions and any
failed probe output under ignored target. Ordinary queries retain the 150 ms
requirement; no lower-level timing or incomplete result can satisfy it.

Method XREF release acceptance uses `method-refs-expectations.json`, a selective
independent PEReader/OpCodes operand oracle. Its targets have no same-named
MemberRefs and no MethodSpecs of the definition, independently proving that raw
and semantic modes must return the same complete nonzero original-site set.
Scope/alias/generic broadening is covered separately by the original semantic
fixture, not claimed from these production targets. The source/identity pins
match the class oracle and runtime hashes above.

```sh
python3 benches/dotnet_refs.py --binary target/release/rxd \
  --production target/dotnet-closure-corpus/newtonsoft-json-13.0.3/Newtonsoft.Json.dll \
  --cpu-model 'verified host CPU' --memory-gb 36
```

Every ordinary raw/semantic stdout/atomic-file workload has at least 20 fresh
CLI/worker processes and exact site-set hash, context IDs, output determinism,
input/binary hashes, actual output, median/P95/>150 ms checks. The full fixed
module heap and all readable methods are scanned. Defaults remain unchanged.
CoreLib/WindowsBase large default diagnostics retain successful, slow or
resource-limited outcomes separately and grant no performance exception; failures
cannot count as latency success. Warm cache and selected-input limitations are
recorded. All results and full exploratory metadata remain under ignored target.

Field/type XREF uses `field-type-refs-expectations.json` with the same pinned
production/runtime inputs and independent PEReader/OpCodes seam. The selected
FieldDefs have no same-named MemberRefs; selected non-generic TypeDefs have no
same-named TypeRefs or named/constructed TypeSpec roots. That independent evidence
makes raw/semantic complete site sets identical for these workload targets.
Generic aliases and structural distinctions are separately proved by the original
byte-assembler E2E fixture, not claimed from these benchmark targets. Full static
metadata, trusted inspector source/hash and original-site oracles stay under
ignored target; target assemblies are never loaded or executed.

```sh
python3 benches/dotnet_refs.py --binary target/release/rxd \
  --manifest tests/corpus/dotnet/field-type-refs-expectations.json \
  --production target/dotnet-closure-corpus/newtonsoft-json-13.0.3/Newtonsoft.Json.dll \
  --cpu-model 'verified host CPU' --memory-gb 36 \
  --output target/benchmarks/field-type-xref
```

Each of six field/type workloads runs raw/semantic modes to stdout and atomic
files with at least 20 fresh CLI/worker samples, nonzero exact sites, complete
schema/identity/context checks, actual writes and all per-workload samples and
median/P95/>150 ms metrics. Budgets and the ordinary 150 ms threshold are unchanged.
## Complete returned-ID workflow expectations

`closure-expectations.json` selects the same pinned Newtonsoft.Json 13.0.3
net6.0 input and preserves its MIT provenance. A trusted static PEReader/OpCodes
inspector records all explicit IL token operands, MethodSpecs and MemberRefs;
the manifest pins inspector/output SHA-256. Its JsonReader SetToken definition
has 53 original sites, no same-named MemberRef and no MethodSpec target, so the
semantic query must reproduce that complete independently specified site set.
ReadAsInt32 is the selected original caller; the existing class oracle verifies
every selected JsonReader method and original IL/EH. The independent original
`#US` decoder additionally establishes the exact error-string entry, owner and
load offset, cross-checked with PEReader. Full exploratory oracle/source/build
outputs stay under ignored target, not in the source corpus.

`benches/dotnet_closure.py` consumes this manifest for both complete workflows;
only navigation IDs returned by CLI records are passed to later queries. It
requires successful method/class C# recovery before claiming component latency
acceptance. The manifest's metadata tokens select independent expectations,
not fabricated FETCH identities. The same runner replays existing per-query
oracles and retains default-budget failures; workflow totals are separate from
single-command performance limits. No analyzed input is loaded or executed.

The scoped large-input performance bound is recorded in
[corelib-performance-scope.json](corelib-performance-scope.json). It applies only to
four designated queries against the pinned upstream CoreLib SHA-256 and byte size;
all are complete fresh CLI process timings with a 300 ms upper bound. The original
>150 ms samples remain recorded; parser-only timings, resource errors and unrelated
smaller inputs do not acquire that scope.
