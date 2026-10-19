# ADR 0003: Unified structured output envelope

Status: accepted as the contract for structured output from every backend.

This ADR defines the shared envelope for JSONL streams: a header, zero or more data
records and a summary; a fixed outer meta layer and a backend-specific `detail` object;
and consistent field presence. The goal is one backend-independent codemode consumer and
one MCP `outputSchema` skeleton. MCP and codemode share this contract.

Backend design documents reference this ADR and enumerate each record type's detail
fields.

## Context

JSONL output had three inconsistent styles: JS/JVM records were flat, WASM/.NET used a
same-name record subobject (`{"record":"type","type":{...}}`), and header field names
differed (`file_bytes`/`path` in JS vs `input_bytes` elsewhere; `analysis` was sometimes
absent).

Both consumers need a stable, predictable outer envelope with backend-specific detail
isolated in a deterministic schema:

- **codemode**: `JSON.parse` then filter/map; fields must be isomorphic across backends
  and support `record → id → get → start/end` glue.
- **headless MCP**: one tool has one `outputSchema`; output must be stable and typed,
  not opaque free-form JSON.

## Decision

### 1. Three-part JSONL stream

Enumeration/search/reference commands (`outline`, `find`, `strings`, `findrefs`, `refs`,
`symbols`, etc.) emit exactly one `header`, zero or more data records and exactly one
`summary`. Success means exit 0 plus a summary with `complete: true`.

### 2. Meta/detail split

Each data record has:

- **Outer meta**: cross-backend navigation/location fields with fixed names.
- **`detail`**: fields specific to the `(backend, record)` pair.

Fixed outer keys:

| Key | Meaning | Presence |
| --- | --- | --- |
| `record` | Record discriminator | Always |
| `id` | Content-bound ID usable by FETCH | Always; `null` when none |
| `source_id` | SHA-256 of the owning content unit | Always |
| `parent_id` | Parent record ID for tree structures | Always; `null` when none |
| `context_id` | Nearest FETCH context for a hit record | Always; `null` when not applicable |
| `start` / `end` | Original byte range for location/slicing | Always; `null` when none |
| `detail` | Format-specific content | Always |

Outer keys are always present and use `null` rather than omission. Nullable MCP
`outputSchema` fields and codemode `r.start ?? x` both handle this cleanly.

### 3. Detail is a deterministic per-record schema

The `detail` field set is uniquely determined by `(backend, record)`; it is never
dynamic or input-shaped. The outer `record` is the discriminator. Therefore MCP can
express a `oneOf` by record and codemode can read `r.detail.<field>` without backend
branches. Adding fields is backward-compatible; removing/changing them is a contract
change.

### 4. Fixed header keys

| Key | Meaning | Presence |
| --- | --- | --- |
| `record` | Always `"header"` | Always |
| `backend` | Backend ID | Always |
| `command` | Subcommand | Always |
| `source_id` | Input file SHA-256 | Always |
| `input_bytes` | Input byte count (replaces JS `file_bytes`) | Always |
| `schema_version` | Envelope version | Always |
| `query` | Query argument echo | Always |
| `detail` | Backend header details (`path`, `source_type`, `root_id`, imported/total function counts, `analysis`, etc.) | Always |

Backend-specific header fields live under `detail`, keeping outer header keys isomorphic.

### 5. Fixed summary keys

| Key | Meaning | Presence |
| --- | --- | --- |
| `record` | Always `"summary"` | Always |
| `records` | Number of data records | Always |
| `complete` | Whether the query completed | Always |
| `detail` | Backend-specific counts (`classes`, recursive `files`/`scan_summary`, etc.) | Always |

### 6. Single JSON commands

Commands returning one result object (for example JS `locate`) do not wrap it in
header/summary; codemode uses the parsed object directly. The result still follows the
same meta/detail split, and embedded entity/ancestor subrecords follow the record-body
convention.

### 7. Out of scope

- Raw byte output (JS/JVM `get` source, ASAR `extract` `-o` bytes, DEX Java text) is not
  structured output and stays byte-exact. ASAR's stdout receipt is still one envelope
  record.
- Single-record FETCH commands return one JSON record following the record-body
  convention; header/summary are not mandatory.

## Codemode shape (target)

```js
const out = await tools.bash({ command: "rxd jar outline app.jar" });
const rows = out.output.trim().split("\n").map(JSON.parse);

const header = rows[0];
const summary = rows.at(-1);
const methods = rows.filter(r => r.record === "method");

for (const m of methods) {
  const src = await tools.bash({ command: `rxd jar get-method app.jar '${m.id}'` });
  // m.detail.descriptor / m.detail.access
}
```

The same glue works for JS/JVM/.NET/WASM outline; only `detail` differs.

## Consequences

- All streaming JSONL and single JSON commands change shape; tests, demos and benchmarks
  follow.
- `schema_version` moves from 1 to 2. The old constant is removed; no compatibility shim.
- Each record gains one `detail` layer, slightly increasing tokens; fixed outer keys buy
  predictable codemode/MCP handling.
- The future MCP `outputSchema` uses this envelope directly.
- Performance contracts are unchanged (ADR 0002 and the 150 ms requirement).
