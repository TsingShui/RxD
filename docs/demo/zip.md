# ZIP-family containers

`rxd zip` enumerates ZIP central-directory records and decodes one selected file.
The same namespace handles ZIP, AAR, VSIX, XPI, wheel and NuGet packages; filenames
do not determine archive parsing. Stored/Deflate, ZIP64 and data descriptors are
supported. Encryption, split/self-extracting archives, links, bulk extraction and
automatic nested recursion are not supported.

`entries.at` is `zip:<central-directory-index>`, including duplicate names.
`extract --at` and code-backend `--entry` also accept a unique exact filename;
duplicate names fail with `AMBIGUOUS`, never choose the first or last silently.
Coordinates are archive-scoped: re-list when bytes change. Enumeration retains
directories, links, unsafe paths and unsupported compression with explicit
`detail.kind`, `path_valid`, `range_valid`, `encrypted` and `readable` metadata.
It does not decompress entry payloads.

## Setup and AAR handoff

Run from the repository root. These pinned production inputs are distributed
archives, not executed code; provenance is in [the corpus](../../tests/corpus/zip/README.md).
All generated outputs stay under ignored `target/`.

```sh
cargo build
RXD=./target/debug/rxd
DEMO=target/demo/zip
mkdir -p "$DEMO"
AAR=tests/corpus/zip/core-1.15.0.aar
VSIX=tests/corpus/zip/vscode-yaml-2.0.0-400.vsix

"$RXD" zip entries "$AAR" -f jar -o "$DEMO/aar-entries.jsonl"
"$RXD" zip extract "$AAR" --at classes.jar -o "$DEMO/classes.jar" > "$DEMO/aar-receipt.json"
"$RXD" jar classes "$DEMO/classes.jar" -f ContextCompat -o "$DEMO/classes.jsonl"
```

AAR Java/Kotlin bytecode normally lives in `classes.jar` or `libs/*.jar`, not
DEX. The explicit JAR handoff does not recurse into nested archives automatically.
Extraction verifies decoded length and CRC before publishing bytes; failure leaves
an existing output untouched. stdout is one original-file receipt, not file bytes.

## JavaScript without extraction

Every JS query uses the shared container input loader when `backend-zip` and
`backend-js` are enabled (both are in the default build):

```sh
"$RXD" zip entries "$VSIX" -f extension.js -o "$DEMO/vsix-entries.jsonl"
"$RXD" js entities "$VSIX" --entry extension/dist/extension.js -f activate -o "$DEMO/activation.jsonl"
"$RXD" js find "$VSIX" --entry extension/dist/extension.js -f yaml -o "$DEMO/yaml-literals.jsonl"
```

No intermediate extraction file is created. `--entry` selects the container file;
`--at` selects an object inside its decoded bytes. JS auto syntax uses the entry
filename even when `--entry` is a `zip:N` coordinate. Original-source FETCH keeps
container provenance; coordinates and content identity match those decoded bytes
analyzed independently.

## HTML and WASM

These code backends use the same loader, not independent ZIP implementations.
This small carrier is synthetic; its payloads are maintained repository inputs.

```sh
python3 - <<'PY'
from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED
with ZipFile('target/demo/zip/web.zip', 'w', compression=ZIP_DEFLATED) as archive:
    archive.write('site/index.html', 'pages/index.html')
    archive.write('tests/fixtures/wasm/fixture.wasm', 'assets/core.wasm')
PY
WEB="$DEMO/web.zip"
"$RXD" html entries "$WEB" --entry pages/index.html --tag script -o "$DEMO/scripts.jsonl"
"$RXD" html resources "$WEB" --entry pages/index.html -o "$DEMO/resources.jsonl"
"$RXD" wasm functions "$WEB" --entry assets/core.wasm -o "$DEMO/functions.jsonl"
FUNCTION_AT=$(python3 - <<'PY'
import json
from pathlib import Path
rows = [json.loads(line) for line in Path('target/demo/zip/functions.jsonl').read_text().splitlines()]
print(rows[0]['at'])
PY
)
"$RXD" wasm get-function "$WEB" --entry assets/core.wasm --at "$FUNCTION_AT" --raw -o "$DEMO/function.json"
```

HTML node/resource offsets remain HTML-relative. HTML extraction retains the
immediate HTML `detail.origin` and adds the enclosing ZIP `detail.container_origin`.
WASM function FETCH adds ZIP `detail.origin`; listings stay byte-for-byte equivalent
to ordinary decoded-module queries. Resource discovery never automatically opens
other entries or fetches external resources.

## Release measurements

Run `python3 benches/zip.py` for the complete reproducible workload set. On an
Apple M3 Pro, macOS 26.5.1, Rust 1.98.0 release build, with warm OS file caches
and no overlapping owned build/test job, each row below used 20 fresh CLI
processes including complete atomic output writes:

| Workload | Carrier bytes | Selected bytes | Median ms | P95 ms | >150 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| AndroidX AAR entries | 1336135 | — | 13.32 | 17.64 | 0/20 |
| AAR classes.jar extraction | 1336135 | 1357224 | 14.68 | 15.81 | 0/20 |
| YAML VSIX entries | 1104200 | — | 11.47 | 12.01 | 0/20 |
| VSIX JS functions (4020 rows, 1368330 output bytes) | 1104200 | 745879 | 28.73 | 29.65 | 0/20 |
| Same decoded JS, ordinary file | 745879 | 745879 | 28.05 | 30.58 | 0/20 |

All 17 measured workloads totaled 340 samples, none over 150 ms. The synthetic
5000-extra-entry ZIP selected a 44-byte HTML file with median/P95 49.42/51.70 ms;
a sparse ZIP with 256 MiB of unrelated bytes selected the same file in
11.30/12.19 ms. HTML/WASM carriers and handwritten WASM fixtures are not
production-wide WASM evidence. Full per-workload samples and scope are retained
in `target/benchmarks/zip/results.json`.

## MCP

CLI `--entry` is omitted from MCP schemas. Use `zip_entries` → `zip_extract` →
`child_target` → backend query. Child targets retain original entry filename
extensions and live independently of the parent; see [MCP examples](mcp.md).
