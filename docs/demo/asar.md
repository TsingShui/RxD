# ASAR examples

`rxd asar` lists an ASAR index and extracts original bytes on demand. It never starts
the application or executes/interprets entry contents. Only `entries` and `extract` exist;
extracted JavaScript is passed to `rxd js`. See the [ASAR design](../design/asar-design.md).

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

Header backend is `asar`, `header.detail.analysis` is `metadata`. Structured output
follows [ADR 0003](../adr/0003-output-envelope.md), `schema_version=2`: file records
carry path/bytes/storage under `detail` and are ordered by logical path. Packed entries
expose `range_valid`; invalid ranges expose `range_error` (`out_of_bounds` or
`overflow`). `summary.detail.invalid_ranges` counts them; the summary also has
`records` and `complete`. The example has three entries and one out-of-bounds decoy
(`invalid_ranges: 1`); the decoy is listed without reading its claimed payload.

`list -o` bytes are identical to stdout. Complete enumeration does not mean every entry
is readable; unpacked entries and links have no `range_valid`. Links are listed but not
followed.

## extract

```sh
"$RXD" asar entries "$ASAR" | \
  "$RXD" asar extract "$ASAR" --from - -o "$DEMO/main.cjs" > "$DEMO/receipt.json"
"$RXD" asar extract "$ASAR" --at main.cjs -o "$DEMO/main.cjs" > "$DEMO/receipt.json"
"$RXD" asar extract "$ASAR" --at addon.node -o "$DEMO/addon.node" > "$DEMO/native-receipt.json"
```

`--at` is the archive-relative path an `entries` row prints; both `--at` and `-o` are required. Content is not
restricted by extension or UTF-8. Original bytes go to `-o`; stdout is a JSON receipt,
never the file content.

The receipt is one `file` record (`schema_version=2`): backend/command/schema_version
and the extracted SHA-256 `source_id` outside; `detail` contains output path, file_bytes,
storage, `representation: original`, `origin.container/format/entry/header_id` and
`complete`. `header_id` identifies the index header, not a whole-archive hash or
signature. The caller creates the output parent directory; neither the archive nor the
selected unpacked input may be the output. Read/validation/budget/write failures keep
the old `-o`; a stdout failure after replacement may leave the output, so check both
exit status and a success receipt.

An unpacked entry is read from the adjacent `<archive>.unpacked` directory and its
actual length is checked. Ordinary extraction does not access unrelated fake entries; an
out-of-bounds target fails before allocation/seek. This is not whole-archive unpacking,
and bad targets are never silently skipped.

## Continue with JavaScript

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
# The ASAR runner was removed; see benches/README.md "Removed per-backend runners".
```

Defaults: header 8 MiB, 10,000 container entries, 4,096 UTF-8 path bytes. Input defaults
to 8 GiB (32-bit spans keep actual
addressable content below 4 GiB), charged for the selected file; every result record;
no record or output ceiling covering all list JSONL or extract file plus receipt. Parsing ceilings,
path traversal, duplicate paths, malformed headers, links, out-of-range targets and
overflow never rely on running the sample.

Input must be a regular ASAR file. Electron installers, application directories, ZIP and
binwalk integration are out of scope. IPC/bridge, native analysis and V8 bytecode are not
provided by this entry.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend-specific IDs, offsets, inference and degradation are in the
[ASAR design](../design/asar-design.md#mcp-projection).
