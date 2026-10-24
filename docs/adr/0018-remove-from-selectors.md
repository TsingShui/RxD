# ADR 0018: Remove the `--from` selector and stream identity check

Status: accepted.

## Context

ADR 0003 replaced content-bound handles with copy-only coordinates and moved the input
identity into the stream header. FETCH commands also accepted `--from <jsonl|->`, which
read the first matching unit record's coordinate from a previous stream and compared the
stream header's `source_id` (or, for DEX/ASAR, `input_bytes`) with the current input.

That design did not earn its surface:

- `--from` never selected a row; it took the first unit record. A listing that matched
  several rows still had to be narrowed to one before the stream was useful.
- The check only accepted the exact same input. Any rebuild, repack or edit changed
  `source_id`, so the dangerous cross-version case was rejected rather than checked; a
  coordinate copied from another input had to use `--at` anyway.
- ASAR entries carried no `source_id` or `input_bytes`, and DEX carried no `source_id`, so
  those backends degraded to a path or size guard.
- An Agent already has the listing and its `at` string in context; copying it costs no
  tool call. MCP reflected the CLI selector but had no separate stream workflow; its query
  path used `at` only.

## Decision

Remove `--from` from every CLI selector. `--at` is the only way to name an existing row.
The shared selector machinery (`selector_stream`, `Expect`, header identity checks and
`SOURCE_MISMATCH` for streams) is deleted.

Coordinates remain copy-only and file-scoped. A coordinate copied from a different input
is not detected; callers re-run the listing for the input they are fetching.

Stream headers keep their self-describing fields, including `source_id` where a backend
has a content identity. Extract receipts keep their artifact `source_id`. Neither is a
FETCH binding.

## Consequences

- The CLI has one anchor spelling. Pipeline convenience (`listing | fetch --from -`) is
  gone; scripts extract `at` with a JSON parser and pass `--at`.
- `SOURCE_MISMATCH` is no longer produced by CLI selectors. The failure class remains for
  backend-internal unit/carrier mismatches.
- DEX and ASAR no longer pretend to check a foreign stream by size or path.
- Documentation and tests for `--from` are removed.

## Alternatives rejected

- **Keep `--from` as pure convenience, without identity checking.** This retains the
  selector surface for no safety gain and still requires selecting the first row.
- **Keep `--from` and fill identity in every backend.** This preserves an exact-input
  check that rejects legitimate cross-version use while adding a per-stream hash and a
  second anchor spelling.
