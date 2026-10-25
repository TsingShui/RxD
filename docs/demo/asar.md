# ASAR examples

`rxd asar` lists an ASAR index and extracts original bytes on demand. It never starts
the application or executes/interprets entry contents. Only `entries` and `extract` exist
in that namespace; JS, HTML and WASM queries accept `--entry` to analyze one selected file directly.

## Setup

Run from the repository root; all outputs go under ignored `target/`. Setup never
executes JavaScript or native modules.

```sh
cargo build
RXD=./target/debug/rxd
DEMO=target/demo/asar
mkdir -p "$DEMO"
python3 -B - <<'PY'
import json, struct
from pathlib import Path
files = {
    'main.cjs': Path('tests/fixtures/example.js').read_bytes(),
    'addon.node': b'\x7fELF\0\xff\x80native',
    'pages/index.html': b'<!doctype html><script>const version=1;</script>',
    'assets/core.wasm': Path('tests/fixtures/wasm/fixture.wasm').read_bytes(),
}
tree, data = {"files": {}}, bytearray()
for entry, source in sorted(files.items()):
    node = tree
    parts = entry.split('/')
    for part in parts[:-1]:
        node = node["files"].setdefault(part, {"files": {}})
    node["files"][parts[-1]] = {"size": len(source), "offset": str(len(data))}
    data.extend(source)
# One artificial 1 GiB entry the archive does not hold: the index claims it, no payload.
tree["files"]["decoy"] = {"size": 1 << 30, "offset": str((1 << 53) - 1 - (1 << 30))}
header = json.dumps(tree, separators=(",", ":")).encode()
payload = (len(header) + 7) // 4 * 4
Path('target/demo/asar/app.asar').write_bytes(
    struct.pack('<IIII', 4, payload + 4, payload, len(header))
    + header + bytes(payload - 4 - len(header)) + data)
PY
ASAR="$DEMO/app.asar"
```

This is an artificial container with artificial binary bytes. It cannot prove analysis
of a real native module or run the application.

## list

```sh
"$RXD" asar entries "$ASAR" -o "$DEMO/list.jsonl"
```

Each row has backend `asar`; its detail reports metadata. Structured output
uses `schema_version=2`: file records
carry path/bytes/storage under `detail` and are ordered by logical path. Packed entries
expose `range_valid`; invalid ranges expose `range_error` (`out_of_bounds` or
`overflow`). Invalid ranges remain visible as `range_error` on the affected row. The
example has five entries and one out-of-bounds decoy
(`invalid_ranges: 1`); the decoy is listed without reading its claimed payload.

`list -o` bytes are identical to stdout. Complete enumeration does not mean every entry
is readable; unpacked entries and links have no `range_valid`. Links are listed but not
followed.

## extract

```sh
"$RXD" asar extract "$ASAR" --at main.cjs -o "$DEMO/main.cjs" > "$DEMO/receipt.json"
"$RXD" asar extract "$ASAR" --at addon.node -o "$DEMO/addon.node" > "$DEMO/native-receipt.json"
```

`--at` is the archive-relative path an `entries` row prints; both `--at` and `-o` are required. Content is not
restricted by extension or UTF-8. Original bytes go to `-o`; stdout is a JSON receipt,
never the file content.

The receipt is one `file` record (`schema_version=2`): backend/command/schema_version
and the extracted file's identity digest `source_id` outside; `detail` contains output path, file_bytes,
storage, `representation: original`, `origin.container/format/entry` and `complete`.
The receipt carries no archive hash: `source_id` identifies the extracted bytes alone.
The caller creates the output parent directory; neither the archive nor the
selected unpacked input may be the output. Read/validation/budget/write failures keep
the old `-o`; a stdout failure after replacement may leave the output, so check both
exit status and a success receipt.

An unpacked entry is read from the adjacent `<archive>.unpacked` directory and its
actual length is checked. Ordinary extraction does not access unrelated fake entries; an
out-of-bounds target fails before allocation/seek. This is not whole-archive unpacking,
and bad targets are never silently skipped.

## Analyze JavaScript without extracting

Build with both `backend-js` and `backend-asar` (the default includes both):

```sh
"$RXD" js entities "$ASAR" --entry main.cjs -o "$DEMO/entry-entities.jsonl"
"$RXD" js find "$ASAR" --entry main.cjs -f '/api/login'
AT=$(jq -r 'select(.detail.kind == "program") | .at' "$DEMO/entry-entities.jsonl")
"$RXD" js slice "$ASAR" --entry main.cjs --at "$AT" -o "$DEMO/entry-source.cjs"
"$RXD" js slice "$ASAR" --entry main.cjs --at "$AT" --json > "$DEMO/entry-source.json"
```

`--entry` copies the exact `asar entries.at`; `--at` still selects a JS structure,
fragment or symbol inside that entry. Every JS query uses the same fixed entry
buffer without writing an extraction artifact. Auto syntax uses the entry extension,
including `.tsx` and `.d.ts`; `--syntax` and `--source-type` retain their normal meaning.
Coordinates, UTF-8 byte offsets and source identity match the same extracted bytes
under the same parse options. JSON `slice`/`locate` carry ASAR `detail.origin`; raw
`slice` stays byte-exact. Other entries' invalid payload ranges do not block a valid
selection; links, unsafe paths and missing/size-mismatched unpacked files fail.
The output may not replace the archive or selected unpacked source. HTML and WASM
use the same selector below; other backends still use explicit extraction. MCP
retains its `asar_extract` child-target workflow.

## HTML and WASM entries

The default build includes these backends and ASAR; reduced builds need `backend-asar`
plus the requested backend. Every HTML/WASM command accepts `--entry`:

```sh
"$RXD" html entries "$ASAR" --entry pages/index.html -o "$DEMO/html-entries.jsonl"
"$RXD" html resources "$ASAR" --entry pages/index.html --include-content
HTML_AT=$(jq -r 'select(.detail.tag == "script") | .at' "$DEMO/html-entries.jsonl")
"$RXD" html extract "$ASAR" --entry pages/index.html --at "$HTML_AT" --content \
  -o "$DEMO/inline.js" > "$DEMO/inline-receipt.json"
"$RXD" wasm functions "$ASAR" --entry assets/core.wasm -o "$DEMO/wasm-functions.jsonl"
WASM_AT=$(jq -r 'select(.record == "function") | .at' "$DEMO/wasm-functions.jsonl" | head -n 1)
"$RXD" wasm get-function "$ASAR" --entry assets/core.wasm --at "$WASM_AT" --raw
"$RXD" wasm xref "$ASAR" --entry assets/core.wasm --at "$WASM_AT"
```

HTML node ranges and WASM body/instruction offsets are relative to the selected entry,
not the archive. Results match ordinary file queries; no temporary extraction file is
created. HTML resource discovery keeps its normal semantics: it neither reads relative
URLs as archive entries nor fetches them, and `--base-url` remains an explicit HTTP(S)
document URL. HTML extraction receipts retain their immediate HTML `detail.origin`
and add the enclosing ASAR as `detail.container_origin`. WASM function FETCH adds ASAR
`detail.origin`; listings keep their existing data-row schemas.

## Continue with extracted JavaScript

```sh
"$RXD" js entities "$DEMO/main.cjs" -o "$DEMO/entities.jsonl"
"$RXD" js find "$DEMO/main.cjs" -f '/api/login' -o "$DEMO/find.jsonl"
AT=$(jq -r 'select(.record == "match" and (.parent_at | contains("arrow_function:"))) | .parent_at' "$DEMO/find.jsonl" | head -n 1)
"$RXD" js slice "$DEMO/main.cjs" --at "$AT" -o "$DEMO/function.js"
"$RXD" js symbols "$DEMO/main.cjs" -o "$DEMO/symbols.jsonl"
"$RXD" js xref "$DEMO/main.cjs" --byte 43 -o "$DEMO/xref.jsonl"
"$RXD" js locate "$DEMO/main.cjs" --byte 100 -o "$DEMO/locate.json"
```

Keep the original extension and parse options. Identical bytes under identical parse
options keep the same JS coordinates; offsets refer to the extracted file. The
receipt is the link to the original archive; ordinary JS queries do not add ASAR origin.

## Budgets and performance

```sh
"$RXD" asar entries "$ASAR" | head
"$RXD" asar extract "$ASAR" --at main.cjs -o "$DEMO/profile-main.cjs" --perf
```

Header/index allowances grow with the archive, without fixed CLI input/header/
entry-count quotas. Paths retain the 4096-byte guard and selected content retains
32-bit spans. There is no record/output quota for complete listings or artifacts. Parsing bounds,
path traversal, duplicate paths, malformed headers, links, out-of-range targets and
overflow never rely on running the sample.

Input must be a regular ASAR file. Electron installers, application directories and ZIP
are out of scope. IPC/bridge, native analysis and V8 bytecode are not provided by this entry.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend-specific coordinates, offsets and fallback semantics are
preserved by the same query implementation.
