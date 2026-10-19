# Contract Unification: Handoff

State when this note was written: `dotnet:: 68 passed`, `dex:: 11 passed`,
`jvm:: 11 passed`. The rules are in `AGENTS.md` (`## Query Surface`), the budget table
is `rxd_core::Budget`, and no backend exposes `--max-*`.

## Done

- `dotnet`/`jar`/`dex` record shape: `{record, at, context_at, detail}`; identity in
  `at`, extras in `detail`, no per-row `source_id`.
- `jar`: `outline` split into `classes`/`methods`/`fields` + `entries`; coordinates
  `Lx;->f(I)V`; `--bytecode` renamed `--raw`; budget flags dropped.
- `dex`: `fields`/`methods`/`classes` rows carry `at`; `get-method`/`get-class` take
  `--at`; `xref` rows are `context_at` + `detail.target` with the full caller
  prototype.
- `-f/--filter` on every listing command of dex and jar; `--raw` is the one flag for
  original instructions.

## Baseline: the uncommitted test changes are the target contract

The working tree carries a scripted `id→at` migration in `tests/{evm,html,mcp,pyc,wasm}`
plus tests/cli.rs. Full suite: 203 passed, 202 failed, grouped as evm 61, mcp 45, root
40 (mostly js), pyc 33, wasm 12, html 8, wasm_wabt 4.

Decision: **keep them**. They already encode the target contract - short `at`
coordinates, `--at` selectors, `--max-*` gone - while the backends they exercise have
not moved yet. Each failure group maps onto a pending step below, so the migration is
finished by making the backend conform, not by reverting the tests:

- `unexpected argument '--at'` (js get/locate, asar/html extract, wasm get-function):
  the command still takes a positional id or `--id`.
- `INVALID_ID` / `expected canonical content-bound ... ID` (evm): validation still wants
  the long id while records print the short coordinate.
- `starts_with("rxd1:js:...")` (js, wasm, identifiers): assertions still expect the
  content-hash prefix that `at_of` strips.
- js header key set gained `declaration`/`declaration_ids`/`resolution`/`selected`:
  either the header or the assertion is stale - decide when js is touched.
- mcp timeouts and the two benchmark failures: to be diagnosed with those steps.

Nothing is reverted; the tree stays red only until each backend step lands.

## Pending, in order

1. **dotnet split** (code was written and reverted to keep the tree green): replace
   `outline` with `classes`/`fields`/`methods`, add `-f/--filter`, rename `--il` to
   `--raw`, drop the `--max-*` flags. Then migrate `tests/dotnet/**`; 44 call sites
   use `outline` and 12 assertions expect the old per-row keys. A test helper that
   concatenates the three listings into the old mixed view keeps most assertions
   usable; the rest need their keys updated.
2. **js split**: `outline` -> `functions` (`function_declaration`,
   `function_expression`, `arrow_function`), `classes` (`class_declaration`),
   `methods` (`constructor`, `class_method`, `getter`, `setter`, `object_method`).
   The `program` entity disappears: its metadata already sits in the header and its
   span is `0..input_bytes`. Sub-kind stays in `detail.kind`. 38 call sites in
   `tests/cli.rs`.
3. **wasm split**: `outline` -> `types`/`imports`/`exports`/`functions`/`globals`/`data`.
   21 call sites in `tests/wasm/**`.
4. **pyc**: rename `outline` -> `codes` (single record kind).
5. **evm**: delete `outline` (it duplicates `basic-blocks`), `--id` -> `--at`, add
   `-f/--filter`, fold `disassemble` into `--raw`.
6. **asar/html**: `list` -> `entries`, `extract --id` -> `extract --at`, add
   `-f/--filter`.
7. **Cross-backend keys**: `strings` rows become `{value, index}` plus platform
   extras (dotnet keeps `token`/`status`/`context_at`; jar keeps `utf16`); dotnet
   `xref` rows become `context_at` + `detail.target` (drop `il_offset`/`opcode`,
   keep the resolution fields); `--from <JSONL>` is promoted to dex and jar.

## Decisions still open

- **dotnet coordinate**: tokens stay (`0x06000001`). A name-based coordinate
  (`Namespace.Type::Method(System.Int32)`) would be uniform with dex/jar but needs
  the ILSpy signature-rendering cost measured first; only adopt it if `methods` does
  not visibly regress.
- **evm `outline` vs `basic-blocks`**: delete `outline` as planned, unless its record
  shape carries something `basic-blocks` does not.
- **`entries` for dotnet (PE sections/resources)**: not started. PE sections are not
  analysis units, so it is only worth it if embedded resources or single-file
  bundles turn out to matter.

## Process

Each backend is its own change: code, focused `cargo test --test cli <backend>::`,
commit. Nothing is committed red, which is why the dotnet split is not in the tree
right now.
