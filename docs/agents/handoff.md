# Contract Unification: Handoff

State when this note was last updated: `dotnet:: 68 passed`, `dex:: 11 passed`,
`jvm:: 11 passed`, `html:: 16 passed`, `asar 6/7` (the seventh asserts the not-yet-migrated
js `get --at`). The rules are in `AGENTS.md` (`## Query Surface`), the budget table
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

- `html` (`f48f4c5`): `list` → `entries`, `extract --at` (short `kind:start:end` expands
  against the header hash; only a full id can report `SOURCE_MISMATCH`), `-f/--filter`
  over tag/content, five `--max-*` flags dropped, per-row `source_id` dropped.
- `asar` (`dedf385`): `list` → `entries`, every file row prints its archive-relative
  path in `at`, `extract --at`, `-f/--filter` over paths, five `--max-*` flags dropped.

## Deferred: pyc (blocked on a budget-semantics decision)

`pyc outline` → `codes` and the flag removal were implemented and then reverted, because
`tests/pyc/boundaries.rs` asserts **exact** work accounting: `--max-work` defaults to
16,777,216 while `rxd_core::Budget::work` is 1,000,000, so moving the ceiling into the
table changes the reported counter and breaks five exact-value tests (plus
`--max-ast-depth` 128 has no `Budget` field at all). Finishing pyc needs one of:

1. raise `Budget::work` to 16,777,216 and add an `ast_depth` field so the shared table
   reproduces today's numbers, or
2. accept the smaller shared ceilings and rewrite the boundary tests around them.

That is a product decision about how tight the shared default may be, so it is recorded
here instead of guessed. `--contains`/`--contains-hex` on `pyc strings` are also not yet
`-f/--filter`/`--filter-hex`.

## Baseline: the uncommitted test changes are the target contract

The working tree carries a scripted `id→at` migration in `tests/{evm,html,mcp,pyc,wasm}`
plus tests/cli.rs. Full suite: 203 passed, 202 failed, grouped as evm 61, mcp 45, root
40 (mostly js), pyc 33, wasm 12, html 8, wasm_wabt 4.

Decision: **keep them**. They already encode the target contract - short `at`
coordinates, `--at` selectors, `--max-*` gone - while the backends they exercise have
not moved yet. Each failure group maps onto a pending step below, so the migration is
finished by making the backend conform, not by reverting the tests:

- `unexpected argument '--at'` (js get/locate, wasm get-function): the command still
  takes a positional id or `--id`.
- `INVALID_ID` / `expected canonical content-bound ... ID` (evm): validation still wants
  the long id while records print the short coordinate.
- `starts_with("rxd1:js:...")` (js, wasm, identifiers): assertions still expect the
  content-hash prefix that `at_of` strips.
- js header key set gained `declaration`/`declaration_ids`/`resolution`/`selected`:
  either the header or the assertion is stale - decide when js is touched.
- mcp timeouts and the two benchmark failures: to be diagnosed with those steps.

Nothing is reverted; the tree stays red only until each backend step lands.

## What the js attempt taught us (read this before retrying)

The split was implemented and reverted: `entities` (every structural kind except the
program record) plus `functions`/`classes`/`methods` grouped commands, driven by
`Query::Outline { group: Option<&'static str>, contains: Option<String> }` in
`crates/rxd-js`, with `group_kinds()` owning the closed kind sets and
`JsString::contains_ci` for the filter. `normalize_id(source, id)` prefixes a short
`kind:start:end` coordinate with `rxd1:js:<source id>:` so `get --at` and `--within`
accept what a row prints, and a `program` id means "whole file" (`is_program_id`).

Three test-side obstacles made it far more than a rename:

1. **Budget-driven tests.** 17 functions in `tests/cli.rs` pass `--max-*` and assert an
   exit-1 `RESOURCE_LIMIT`: `budgets_reject_results_without_partial_output`,
   `deep_and_wide_inputs_are_controlled_failures`,
   `get_symbol_validates_identity_budgets_and_atomic_outputs_without_changing_structure_get`,
   `symbols_is_bounded_atomic_single_file_and_rejects_invalid_*`,
   `refs_is_single_file_bounded_atomic_and_semantic_only_on_demand`,
   `recursive_budgets_and_late_errors_leave_output_untouched`,
   `local_context_ids_are_content_bound_parser_only_bounded_and_atomic`,
   `perf_reports_failed_stages_without_partial_stdout`,
   `benchmark_saves_failed_budget_results_and_returns_nonzero`,
   `asar_selected_reads_survive_bloat_and_preserve_atomic_outputs`, plus single-flag
   assertions in the ts/typed/recursive tests. They must become "the flag is unknown
   (exit 2)" or lose the budget half of the test; the atomicity half can use a failing
   selector instead.
2. **The program record.** Ten assertions count entities or index rows assuming the
   program record is present (for example `outline_entities_have_exact_spans`,
   `empty_files_zero_matches_and_invalid_offsets`, `filtering_preserves_absolute_depth`,
   `asar_extracted_production_sources_match_independent_acorn`). Each needs one less.
3. **Command naming.** `Query::Refs` reports `name() == "refs"` while the CLI command is
   `xref`; `assert_js_stream` only extended the header keys for `"xref"`, so refs streams
   failed the key assertion. Accept both names (or fix `name()` to `xref`).

## asar and html

Both `extract` take a positional `id` (`src/cli/html/args.rs:54`), while their rows print
a short `at` such as `doctype:0:15`; the tests already pass `--at` and the short value, so
the failures are "unexpected argument '--at'" plus `INVALID_ID: malformed HTML record ID`.
Give both a `--at` selector that accepts the short coordinate (prefix the input's content
id, exactly like `normalize_id` in js), rename `list` to `entries`, and add `-f/--filter`.

## pyc

`outline` becomes `codes`. `strings` still exposes `--contains` (pyc-specific) and
`--contains-hex`; rename the former to `-f/--filter`, keep the hex form. The 33 pyc
failures are the unmigrated `id→at` assertions plus `--contains`.

## evm

61 failures, all `INVALID_ID: expected a canonical content-bound EVM block ID`: the CLI
still validates the long id while rows print the short coordinate. Do the walk before the
rename: accept the short form, then `--id` → `--at`, drop `outline` (duplicate of
`basic-blocks`), and add `-f/--filter`.

## mcp

45 timeouts, not assertion failures: the MCP server stops answering. Diagnose with a
single `mcp::` test after the backend surfaces settle - the server forwards CLI argv, so a
removed flag can make a tool fail before it answers.

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
