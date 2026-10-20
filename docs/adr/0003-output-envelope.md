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

Enumeration/search/reference commands (`entities`, `find`, `strings`, `xref`, `xref`,
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
| `at` | The row's coordinate, usable by FETCH | When the row names one; else `null` or absent |
| `source_id` | SHA-256 of the owning content unit | Header only (null or absent on records) |
| `parent_at` | Parent coordinate for tree structures | When there is one; else `null` or absent |
| `context_at` | Nearest FETCH context for a hit record | When there is one; else `null` or absent |
| `start` / `end` | Original byte range for location/slicing | When there is one; else `null` or absent |
| `detail` | Format-specific content | Always |

`record` and `detail` are always present. The nullable navigation keys are `null` or
absent, and **the two are the same thing**: a consumer must read them as
`row.at ?? null`. The section "Absent and null are equivalent" below records why the
contract allows omission, and what a consumer may rely on.

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
| `source_id` | Input file SHA-256 | Header only (null on records) |
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
const out = await tools.bash({ command: "rxd jar classes app.jar" });
const rows = out.output.trim().split("\n").map(JSON.parse);

const header = rows[0];
const summary = rows.at(-1);
const methods = rows.filter(r => r.record === "method");

for (const m of methods) {
  const src = await tools.bash({ command: `rxd jar get-method app.jar '${m.id}'` });
  // m.detail.descriptor / m.detail.access
}
```

The same glue works for every structure listing; only `detail` differs.

## Amendment: the FETCH handle is a coordinate, not a hash

The `id` key is being replaced by `at`, and `parent_id`/`context_id` by
`parent_at`/`context_at`. Values are the backend's own coordinate
(`0x06000003`, `func:42`, `1234..5678`) with the schema prefix
`rxd1:<backend>:<sha256>:` stripped; the hash lives once in `header.source_id`.
The value is the backend's own coordinate in the shape it prints, read back verbatim: a
`.NET` coordinate is its ECMA-335 metadata token (`0x0a00001f`), which already encodes
its table, so it carries no noun prefix; backends whose coordinates are not
self-describing keep a prefix (`func:42`, `code:9`). A prefix that a backend does not
use is refused with the value to use instead.

Rationale: an Agent had to copy 80-103 character handles in which about 70% of the
bytes were a hash repeated 1-4 times per record. Measured on a 168 KB production
bundle, `id`/`source_id`/`parent_id`/`context_id` were 49% of an entity stream.

Content binding moves to the paths where it can still fail closed: the header carries
`source_id`, and FETCH commands accept `--from <jsonl|->`, which re-reads the coordinate
from the producing stream and rejects a mismatched `source_id`. The removed guarantee is
that a handle alone identifies its input; a coordinate is deliberately file-scoped so the
same coordinate can address the same metadata row in two versions of an input. A
content-bound `rxd1:<backend>:<sha256>:...` spelling is still accepted and still fails
with `SOURCE_MISMATCH` when the hash differs, so a caller who keeps the full form keeps
the old guarantee.

Every backend is migrated: dex, jar, dotnet, js, wasm, pyc, evm, html and asar print
`at`/`parent_at`/`context_at` as coordinates and never repeat `source_id` on a record.
An id nested inside `detail` (a caller id, a symbol or owner id, a reference target) uses
the same copy-only coordinate, so any coordinate a caller can copy is one `--at` accepts.

## Amendment: absent and null are equivalent

The original text required every outer key to be present with `null` when it had no
value. Two things made that a bad contract:

- **Cost on the widest streams.** A nullable key that is always null costs its name plus
  `null` on every row. DEX builds its rows by hand precisely because `xref` on a 523 MiB
  APK emits ~118k rows and `methods` emits 5.19M; the six keys would add ~10 MB and 39%
  of a class row for no information.
- **It was already untrue.** DEX omits these keys on rows that have no coordinate, so the
  contract described something no backend satisfied.

The rule is now: `record` and `detail` are present on every data record; a nullable
navigation key is present when the row has that value, and is either `null` or absent when
it does not. Consumers must treat absence and `null` identically — `row.at ?? row.detail`,
`row.context_at ?? null` — and may not require presence. MCP `outputSchema` marks these
fields optional rather than required-and-nullable, so schema validation accepts both
spellings.

What a consumer *can* rely on is unaffected: a coordinate printed in `at`, `parent_at`,
`context_at` or `detail.*` is one the backend's `--at` accepts, and a row whose `at` is
absent or null is an occurrence (a reference site, a literal, an archive entry with no
FETCH) rather than an addressable entity.

## Consequences

- All streaming JSONL and single JSON commands change shape; tests, demos and benchmarks
  follow.
- `schema_version` moves from 1 to 2. The old constant is removed; no compatibility shim.
- Each record gains one `detail` layer, slightly increasing tokens; fixed outer keys buy
  predictable codemode/MCP handling.
- Nullable outer keys are present only when they have a value (see the amendment), so a
  consumer reads them defensively; the keys that are always there are `record` and
  `detail`.
- The future MCP `outputSchema` uses this envelope directly.
- Performance contracts are unchanged (ADR 0002 and the 150 ms requirement).
