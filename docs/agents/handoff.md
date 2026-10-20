# Contract Unification: Handoff

State: **finished.** Every backend's CLI surface *and* the MCP tool surface are unified and
green. `cargo test --test cli` reports `410 passed / 0 failed` (twice in a row),
`cargo build` has no warnings, `cargo clippy -p rxd --all-targets` is clean,
`scripts/format-workspace.py --check` passes and `scripts/mcp-smoke.py` verifies the
relocated stdio server. The rules are in
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
   `SOURCE_MISMATCH` when the hash differs. Positional ids and `--id` are gone. `xref` is
   not an exception: `--at` anchors on the identity a row prints (dex/jar member and type
   coordinates, `symbol:6:9`, `func:42`, `const:0:3`), while `-f PATTERN --kind KIND` is the
   content search. js keeps `--byte` beside `--at` because a position *inside* a multi-byte
   identifier is not a coordinate any row prints, and pyc keeps `--name` because an exact
   bytecode name is not a coordinate either.
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
   (offsets, opcodes, resolution status, generic arguments, ...). Nothing was dropped, and
   `detail.target` is a *coordinate*: dex/jar print `Lx/y;->name(proto)` (methods) and
   `Lx/y;->name:type` (fields), which `--at` takes back unchanged. The prototype/type was
   added for exactly that round trip; jar's old dotted `owner.name(desc)` spelling and
   quoted string targets were replaced so one spelling reaches both faces.
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
- `63c359a`/`84fb398` dex `--class` became a substring like every other backend, and the
  per-entry pre-check became a cost heuristic (`bounded_by_class`) instead of "names one
  class"; release measurement recorded in `dex-design.md`.
- `xref` two faces: `src/cli/xref.rs` (shared surface) + dex/jar projection, `detail.target`
  became a full coordinate, js `--at`/`--byte`, wasm `xref --at func:N`, pyc `xref --at`,
  and the MCP subcommand-selector machinery removed.

## MCP surface (done, merge `a3068e1`)

The tool catalogue follows the CLI by renovation, not by list maintenance:

- `src/query/catalog.rs` no longer appends `--max-*` to worker argv. The whole `ceilings()`
  table is gone; `bound_threads()` sets `threads = min(requested, server budget)` only for
  commands whose reflected schema declares `--threads`. A ceiling can no longer be invented
  as a backend flag, because there is no code path that writes one.
- `src/query/reflect.rs` builds argv from the clap tree alone; it also fixed a real
  reflection bug (clap reported `possible_values = ["true","false"]` for `SetTrue` flags,
  so every boolean was advertised as a string enum).
- `capability()` is still a deliberate hand-written semantic table
  (`ENUMERATE`/`SEARCH`/`XREF`/`FETCH`), now spelled with the unified names. It is the one
  remaining list to update when a *new* command appears; `tests/mcp/**` asserts the
  catalogue shape so a missed entry fails a test rather than silently mislabeling a tool.
- Tool renames ripple automatically: `js_entities/functions/classes/methods`,
  `asar_entries`, `html_entries`, `pyc_codes`, `evm_blocks`, the nine `wasm_*` listings.
  Selectors are `at`, filters `filter`, original bytes `raw`.
- MCP still spawns one disposable worker process per query
  (`--internal-mcp-worker`, JSON over stdio, deadline + cancellation + RSS monitor) and the
  worker calls the same `query::execute` the CLI uses, so results cannot drift.

## Per-backend gates (final run on this commit)

| Filter | Result |
| --- | --- |
| `--test cli dex::` | 11 passed, 0 failed |
| `--test cli jvm::` | 11 passed, 0 failed |
| `--test cli dotnet::` | 68 passed, 0 failed |
| `--test cli wasm::` | 19 passed, 0 failed |
| `--test cli wasm_wabt::` | 4 passed, 0 failed |
| `--test cli pyc::` | 70 passed, 0 failed |
| `--test cli evm::` | 71 passed, 0 failed |
| `--test cli html::` | 16 passed, 0 failed |
| `--test cli mcp::` | 60 passed, 0 failed |
| full `--test cli` | 410 passed, 0 failed |

`js` and `asar` have no test module: their tests live at the crate root in `tests/cli.rs`
(`js`/`asar`/`refs_*`/`symbols_*`/`local_contexts_*`/`outline_*`/`recursive_*`/`corpus_*`
and the `asar_*` families), so the whole-suite run is their gate — `-- js` matches 14 of
them by name, which is why the module filter is not used as evidence for those two.

## Decisions (all resolved)

1. **pyc work ceiling.** Resolved: pyc stays on the shared `Budget::work = 1,000,000`; no
   larger line was added. The exact-work boundary tests in `tests/pyc/boundaries.rs` call
   the `rxd_pyc` library directly with explicit `Limits`, so the accounting invariant is
   still covered without a CLI knob.
2. **Access flags.** Resolved: `--with-flags` is gone and no member row repeats
   `access_flags` (dex `methods`/`fields` and jar `methods`/`fields`). The authoritative
   spelling of "what is this member" is the Java rendering: `get-class --at` prints
   `public abstract class` / `private static final` members and `get-method --at` prints one
   member's modifiers. On the 5.19M-method dex corpus the omission also saves ~52 MB. jar
   `classes` rows keep `access_flags`, because that listing is the only place that says
   whether a class is an interface, annotation or enum without fetching every class.
3. **MCP dead status branch.** Resolved: `status == 3 || 4 → NOT_FOUND/AMBIGUOUS` and its
   `candidates` payload were removed from `src/query/mod.rs`, together with the unused
   `rxd_dex::apk::lookup_status` and its unit test. Nothing observable changed — the branch
   was unreachable because `lookup_status` had no caller — but a missing DEX selector now has
   exactly one producer (`src/cli/dex/mod.rs` maps the "not found" message to `NOT_FOUND`),
   and the MCP surface no longer documents a candidate payload the DEX path cannot produce.
   `QueryError::candidates` stays because kuna's ambiguous native query still fills it.

4. **dex instruction offsets.** dex `xref` rows carry `entry` but no instruction offset or
   mnemonic; jar/dotnet/pyc/evm all keep theirs. Adding them means threading the offset
   through the dex scan path, which is the performance-critical one. User decision: do not
   add.

5. **`xref` target surface.** Resolved: one `xref` command with two faces, never a
   per-kind subcommand. `--at` names one identity exactly and infers its kind from the
   coordinate (`Lx/y;` type, `Lx/y;->m()V` method, `Lx/y;->f:T` field); `-f/--filter` is a
   case-sensitive substring and requires `--kind` because it cannot say what it looks for; a
   literal anchor is the one case where `--kind string` is required. Contradicting the
   coordinate is an argument error naming both kinds (user decision: accurate errors, not an
   empty result), and an anchor no table carries is `NOT_FOUND`. The shared implementation
   is `src/cli/xref.rs`; dex and jar both project it, and the MCP subcommand-selector
   machinery in `src/query/reflect.rs` was deleted because no command has subcommands any
   more (`dex_xref`/`jar_xref` are one tool each, down from four).
6. **Fuzzy by default.** Resolved: every `xref` content search is a case-sensitive
   substring, including the member name and `--class` (dex previously compared both
   exactly). `--fuzzy-class` is gone; there is no exact-search mode, because exactness is
   what `--at` is for.

## Known follow-ups (not in this goal)

- `benches/evm.py` was migrated to `interface`/`blocks`/`xref`/`--at` and `[at]` keys and
  compiles, but it has not been re-run against the release binary; EVM has no latency gate.
- `src/query/mod.rs` still carries the MCP `status == 3 || 4 → NOT_FOUND/AMBIGUOUS`
  branch, which the unified DEX surface can no longer produce; `candidates` is documented as
  nullable rather than removed. Decide whether to delete it when the DEX `lookup_status`
  field is removed from the library.
- `tests/mcp/**` gates backend-specific tests on their `backend-*` feature so the js-only
  acceptance command stays green; the all-backends run (`mcp,all-backends`: 59 passed) is
  the one used for the final gate.

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
