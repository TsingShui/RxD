# ADR 0003: Structured output rows

Status: accepted. Amended: streaming commands emit data rows only; the earlier
header/summary envelope is removed.

This ADR defines the shared shape of structured output from every backend: JSONL streams
are zero or more data records, one JSON object per line, with a fixed outer meta layer and
a backend-specific `detail` object. Single-object results keep their own record shape.
There is no stream header and no stream summary.

Backend design documents reference this ADR and enumerate each record type's detail
fields.

## Context

Earlier versions wrapped every JSONL stream in a `header` line and a `summary` line. The
envelope carried stream-level metadata (`backend`, `command`, `source_id`, `input_bytes`,
`query`, `analysis`) and a `complete`/`records` summary. In practice:

- The caller already knows the backend, command, input and query it invoked.
- `source_id` lost its only functional consumer when `--from` was removed (ADR 0018).
- `complete` was `true` on every successful stream; failures already emit no stdout and a
  non-zero exit, and partial coverage is backend-specific data.
- The two wrapper lines added parsing boilerplate and offset every data row.

The remaining consumers need one predictable per-row shape: codemode parses a line,
filters/maps by `record`, reads `detail`, and passes `at`/`parent_at`/`context_at` back to
`--at`. MCP exposes the same rows, unchanged.

## Decision

### 1. Data rows only

Enumeration/search/reference commands (`entities`, `find`, `strings`, `xref`, `symbols`,
`codes`, `classes`, `methods`, etc.) emit zero or more data records and nothing else.
A successful empty result is an empty stdout with exit 0.

Progress, timings, warnings and diagnostics go to stderr. `--perf` remains a separate
stderr JSONL stream.

### 2. Meta/detail split

Each data record has:

- **Outer meta**: cross-backend navigation/location fields with fixed names.
- **`detail`**: fields specific to the `(backend, record)` pair.

Fixed outer keys:

| Key | Meaning | Presence |
| --- | --- | --- |
| `record` | Record discriminator | Always |
| `at` | The row's coordinate, usable by FETCH | When the row names one; else `null` or absent |
| `source_id` | Identity digest of the owning content unit | Single-object receipts only; never a stream row |
| `parent_at` | Parent coordinate for tree structures | When there is one; else `null` or absent |
| `context_at` | Nearest FETCH context for a hit record | When there is one; else `null` or absent |
| `start` / `end` | Original byte range for location/slicing | When there is one; else `null` or absent |
| `detail` | Format-specific content | Always |

`record` and `detail` are always present. The nullable navigation keys are `null` or
absent, and **the two are the same thing**: a consumer must read them as
`row.at ?? null`. The section "Absent and null are equivalent" below records why the
contract allows omission.

### 3. Detail is a deterministic per-record schema

The `detail` field set is uniquely determined by `(backend, record)`; it is never
dynamic or input-shaped. The outer `record` is the discriminator. MCP can express a
`oneOf` by record and codemode can read `r.detail.<field>` without backend branches.
Adding fields is backward-compatible; removing/changing them is a contract change.

### 4. No stream-level query echo

A stream carries no `backend`, `command`, `schema_version`, `input_bytes`, `query` or
`source_id` line. The caller knows what it invoked. A saved stream is data, not a
self-describing transaction; save the command alongside it if that context is needed.

### 5. Single JSON commands

Commands returning one result object (for example a FETCH body, an extract receipt, or a
native analysis object) do not add lines around it. The result is one JSON object that
may carry `backend`, `command`, `schema_version`, `source_id` and `detail` at the top
level. It follows the same meta/detail split.

### 6. Completion and counts

Success is exit 0; failure is a non-zero exit with no stdout and a diagnostic on stderr.
There is no `complete` boolean and no `records` count. A consumer counts rows by parsing
lines. Backend-specific coverage evidence, where it exists, stays in that record's
`detail` or in `--perf`; a zero-row result is complete within the command's declared
scope because partial failures are failures.

### 7. MCP projection

MCP analysis tools that capture a JSONL stream return `{"records": [...]}` with the data
rows unchanged. Single-object commands return their object unchanged. Source/XML text
still arrives as `{"text": "..."}`.

### 8. Out of scope

- Raw byte output (JS/JVM `get` source, ASAR `extract` `-o` bytes, DEX Java text) is not
  structured output and stays byte-exact. ASAR's stdout receipt is one JSON object.
- Single-record FETCH commands are one JSON object; they are not streams.

## Amendment: the FETCH handle is a coordinate, not a hash

> Superseded in part by [ADR 0018](0018-remove-from-selectors.md): `--from` and its
> stream-identity check were removed. A coordinate is still copy-only and file-scoped;
> no FETCH path re-reads a previous stream.

Values are the backend's own coordinate (`0x06000003`, `func:42`, `1234..5678`), read
back verbatim: a `.NET` coordinate is its ECMA-335 metadata token (`0x0a00001f`), which
already encodes its table, so it carries no noun prefix; backends whose coordinates are
not self-describing keep a prefix (`func:42`, `code:9`). A prefix that a backend does not
use is refused with the value to use instead.

Rationale: an Agent had to copy 80-103 character handles in which about 70% of the
bytes were a hash repeated 1-4 times per record. Measured on a 168 KB production
bundle, `id`/`source_id`/`parent_id`/`context_id` were 49% of an entity stream.

A coordinate is deliberately file-scoped so the same coordinate can address the same
metadata row in two versions of an input. No content-bound spelling remains: a handle
that still carries an input digest is rejected as `INVALID_QUERY` rather than rewritten.
A coordinate copied from another input is not checked; re-run the listing for the input
being fetched.

Every backend prints `at`/`parent_at`/`context_at` as coordinates and never repeats a
content hash on a stream row.

## Amendment: `source_id` is a sampled digest

Where a single-object receipt or MCP target carries `source_id`, it is
`SHA-256(length || first 16 KiB || last 16 KiB)` of the content unit it identifies
(`rxd_core::identity::input_id`), not a whole-file SHA-256. Hashing every byte cost one
pass over the input on every query — 5.5 ms on a 16 MB assembly — to answer a question
that only needs one input told apart from another. A copy or rename is the same identity.
The weakening is explicit: an edit that preserves length and both ends is not detected,
and detecting a substituted artifact is not this tool's job.

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

A coordinate printed in `at`, `parent_at`, `context_at` or `detail.*` is one the backend's
`--at` accepts; a row whose `at` is absent or null is an occurrence (a reference site, a
literal, an archive entry with no FETCH) rather than an addressable entity.

## Consequences

- JSONL streams lose the header/summary lines and every stream-level field they carried.
- Every backend, test, demo and MCP projection follows the data-row shape.
- `schema_version` remains a field on single-object receipts; streams do not repeat it.
- `--perf` and stderr diagnostics are unchanged; success is exit 0.
- Performance contracts are unchanged (ADR 0002 and the 150 ms requirement).
