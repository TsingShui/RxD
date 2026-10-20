# Contract Unification: Handoff

State: every backend's CLI surface is unified and green. `cargo test --test cli` reports
`374 passed / 35 failed`, and all 35 failures are `mcp::` (the MCP tool surface is being
migrated in a separate lane; it is the only work item left). The rules are in
`AGENTS.md` (`## Query Surface`), the budget table is `rxd_core::Budget`, and no backend
exposes `--max-*`.

## The contract, as implemented

1. **One command, one object kind.** No listing command mixes kinds. js has
   `entities` (the full catalogue, file root included) plus `functions`/`classes`/`methods`;
   wasm has one command per kind (`types`, `imports`, `exports`, `functions`, `tables`,
   `memories`, `globals`, `data`, `elements`, `strings`, `xref`); html/asar have
   `entries`; pyc has `codes`; evm has `blocks` and `interface` (the content-bound function
   listing), and the old `outline` is gone everywhere.
2. **Listing selectors.** Every listing command takes `-f/--filter` (case-insensitive
   substring) plus its own exact selectors (`--kind`, `--name`, `--class`, `--func`,
   `--selector`, ...). Single-object analysis commands (`manifest`, `decompile`,
   `analyze`, `storage`, `metadata`, `cfg`, `disassemble`, `extract`) take no filter.
3. **`--at` selects a row.** Every fetch/extract command takes `--at` with the coordinate a
   row prints; the short form expands against the header identity inside the backend
   library. A full `rxd1:<backend>:<sha256>:...` id is still accepted and still fails with
   `SOURCE_MISMATCH` when the hash differs. Positional ids and `--id` are gone.
4. **`--raw` is the only spelling** for the original instructions/bytes view of a fetch
   (`get-method`, `get-function`, `get-code`). Backends with no second representation
   (dex, pyc) have no `--raw` because there is nothing to switch away from.
5. **Record shape.** Identity in `at`, caller in `context_at`, extras in `detail`, and no
   per-record `source_id` (the header carries it; the shared envelope still serializes
   `source_id: null`). Ids nested inside `detail` are the same copy-only coordinate, so any
   coordinate a caller copies is one `--at` accepts.
6. **`strings` rows carry `{value,index}`** plus the platform fields. `index` is the
   backend's own identifier for that string: DEX `string_ids`, JVM constant pool, pyc
   constant slot, .NET literal inventory position, wasm/evm inventory position.
7. **`xref` rows are `context_at` + `detail.target`** plus the backend's own evidence
   (offsets, opcodes, resolution status, generic arguments, ...). Nothing was dropped.
8. **Budgets** live only in `rxd_core::Budget` (input, entry, metadata, entries, entities,
   instructions, work, records, output, AST depth, semantic nodes) and every backend
   projects it into its own `Limits`. The CLI exposes no ceiling flag; a `--max-*` argument
   is an unknown argument (exit 2). Tests that used to trip a ceiling from the CLI now
   assert either the refusal or the backend-internal failure path.

## Commits

- `f48f4c5` html, `dedf385` asar — `entries`, `--at`, `-f`, per-row `source_id` dropped.
- `994023c` js — `entities`/`functions`/`classes`/`methods`, `get --at`, nested ids short,
  `ast_depth`/`semantic_nodes` moved into `Budget`.
- `2f0e4e9`/`d088666`/`794e8c4` wasm / pyc / evm — each backend's command surface, rows,
  coordinates, budgets, tests and docs.
- `069ac54` `strings.index` everywhere + nested detail ids short.
- `706f125` docs/skill/site synchronisation.

## Remaining: the MCP surface (35 tests)

The tool catalogue is derived from the clap tree, so it followed the renames, but
`src/query/catalog.rs` still appends `--max-*` argv to worker commands and classifies some
renamed commands from a stale list; `tests/mcp/**` and `benches/mcp.py` still use
`js_outline`, `{"id": ...}` and the old row keys. A lane is working in
`/Users/bytedance/zhome/rxd-mcp` (branch `lane/mcp`) on exactly that, with the rule that
the reflection stays the single source of truth and MCP results must still equal CLI
results. Merge it when `cargo test ... --test cli mcp::` is 0 failures.

## Rebuilding the prebuilt bundles

`prebuilt/linux-arm64`, `prebuilt/linux-x64` and `prebuilt/android-ilspy/aarch64-linux-android`
predate every one of these surface changes and must be rebuilt from the final commit
(`prebuilt/README.md` has the manifest/licence contract).

## Notes for future work

- `--from <jsonl|->` exists for content-bound re-reads on the backends that have it; the
  unified surface makes it optional rather than required, because `at` is file-scoped and
  the header carries the identity.
- Directory scans, semantic index sizes and AST depth are bounded by the shared budget, so
  a caller cannot tighten them per invocation any more. If that becomes necessary, add the
  ceiling to `Budget` (one table) rather than a CLI flag.
- `benches/cli.py` no longer accepts or forwards budget flags; it reports the shared
  budget instead of an override.
