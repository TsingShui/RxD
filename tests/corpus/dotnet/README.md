# Pinned .NET validation inputs

Small offline boundary fixtures are independently assembled original data in
`tests/fixtures/dotnet/`; they do not establish production performance or semantic
coverage. Analyzed assemblies are never loaded or executed.

## Published Newtonsoft.Json assembly

`newtonsoft-json-13.0.3/` retains the published MIT `Newtonsoft.Json` 13.0.3
assembly and its original license. The NuGet entry is
`lib/net6.0/Newtonsoft.Json.dll` (712,464 bytes):

- Assembly SHA-256: `22c649f75fce5be7c7ccda8880473b634ef69ecf33f5d1ab8ad892caf47d5a07`.
- Package SHA-256: `872fc189e638ab1056555b03aaa38f68bcb54286e221aa646eb1129babf63c77`.
- Source URL and license: `class-expectations.json`.

The independent PEReader/OpCodes inspector reads metadata, original IL and
exception handlers without loading the target assembly. Its full exploratory
source/output stays under ignored `target/`; committed manifests retain the
inspector and output hashes. Metadata tokens in the manifests identify expected
rows, not coordinates to reuse with another input.

| Manifest | Independent expectation |
| --- | --- |
| `class-expectations.json` | JsonReader/JValue scope, field/method tokens, original IL hashes, no-body declarations and ordered EH |
| `newtonsoft-json-13.0.3/metadata.json` | PE/CLI table counts and mutation offsets from `metadata-oracle.py` |
| `string-expectations.json` | All 784 `#US` entries, 1,444 `ldstr` loads and 1,291 `(entry, method)` groups; values, owners and sorted offsets |
| `method-refs-expectations.json` | Selected MethodDef original-site sets, with no same-named MemberRef or MethodSpec aliases |
| `field-type-refs-expectations.json` | Selected field/type site sets with independently checked alias exclusions |
| `closure-expectations.json` | SetToken's 53 original sites, ReadAsInt32 caller, selected class IL/EH and an exact error-string load |

Generic aliases, scope broadening and unsupported cases are covered by separate
handcrafted fixtures, not inferred from these selected production targets.
`tests/dotnet/` consumes the committed expectations through the real CLI. Ordinary
CLI work grows with admitted input; MCP retains separate fixed service quotas.

Reproduce the small metadata manifest:

```sh
python3 tests/corpus/dotnet/newtonsoft-json-13.0.3/metadata-oracle.py > target/newtonsoft-metadata.json
cmp target/newtonsoft-metadata.json tests/corpus/dotnet/newtonsoft-json-13.0.3/metadata.json
```

Regenerate the compact string oracle from an independently prepared full
PEReader manifest:

```sh
python3 benches/dotnet_strings_oracle.py \
  --input tests/corpus/dotnet/newtonsoft-json-13.0.3/Newtonsoft.Json.dll \
  --metadata-oracle target/dotnet-closure-corpus/newtonsoft-json-13.0.3/independent-all.json \
  --output target/newtonsoft-string-expectations.json
cmp target/newtonsoft-string-expectations.json tests/corpus/dotnet/string-expectations.json
```

## Exploratory upstream runtime inputs

The public `ATRAPSLLC/dotscope` repository at commit
`3028b1ab92ec1b0ad15f046b3b70d5ee82336a00` retains these artifacts under
`tests/samples/`:

| Input | Bytes | SHA-256 |
| --- | ---: | --- |
| `WindowsBase.dll` | 2,255,024 | `d6712d9472c4e92cfa847d0ba64671deb41d92357d41320d9d6dd88bec960e6e` |
| `dotnet_10.0/System.Private.CoreLib.dll` | 15,767,040 | `a6acd5d87523255556bc17d964779aae71da1809623710aa4c6d091061b3246a` |

These are upstream test artifacts, not an independently collected production
acceptance corpus. Their runtime bodies must not be replaced with reference
assembly stubs. Retain upstream provenance/licenses when redistributing them.
Downloaded history, assemblies and exploratory reports belong under `target/`:

```sh
git clone https://github.com/ATRAPSLLC/dotscope target/dotnet-research/references/dotscope
git -C target/dotnet-research/references/dotscope checkout 3028b1ab92ec1b0ad15f046b3b70d5ee82336a00
```

Build the native C# engine with [the ILSpy guide](../../../benches/ilspy/README.md).
Current queries are in [the .NET examples](../../../docs/demo/dotnet.md);
benchmark commands and fresh-process measurement rules are in
[benches/README.md](../../../benches/README.md). Metadata regression success is
not current C# reconstruction or latency evidence.

The existing scoped large-input bound is recorded in
[corelib-performance-scope.json](corelib-performance-scope.json). It covers only
four designated queries against the pinned upstream CoreLib bytes, with a 300 ms
upper bound. Its original >150 ms samples remain recorded; unrelated inputs,
parser-only timings and resource failures do not acquire that scope.
