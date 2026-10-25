# EVM analysis and code-unit navigation

`--encoding raw|hex` is mandatory. Inputs are analyzed offline; nothing is executed.
Every command lists one object kind; `-f/--filter` narrows a row listing by the value
each row is named by. Rows print a short, copy-only `at` coordinate that `--at` accepts.

```sh
rxd evm analyze tests/fixtures/evm/storage-runtime.hex --encoding hex
rxd evm storage tests/fixtures/evm/transient-storage-runtime.hex --encoding hex
rxd evm disassemble tests/fixtures/evm/storage-runtime.hex --encoding hex
rxd evm basic-blocks tests/fixtures/evm/storage-runtime.hex --encoding hex
rxd evm cfg tests/fixtures/evm/storage-runtime.hex --encoding hex
rxd evm metadata tests/fixtures/evm/metadata-runtime.hex --encoding hex

rxd evm analyze runtime.bin --encoding raw --include-bytecode -o target/analyze.json
rxd evm analyze runtime.hex --encoding hex --perf
```

`analyze`, `storage`, `disassemble`, `basic-blocks`, `cfg` and `metadata` emit one
`evm_analysis` object. Carrier SHA-256, decoded-bytecode SHA-256, decoded-byte offsets,
CFG producer block IDs, `undecoded_tail`, `abi_verified`/`discovery_complete` and
inference properties are distinct.

## Function listing and FETCH

```sh
rxd evm interface tests/corpus/evm/authored-navigation.runtime.hex --encoding hex -o target/evm-interface.jsonl
rxd evm interface tests/corpus/evm/authored-navigation.runtime.hex --encoding hex -f 1ba64846
rxd evm interface runtime.hex --encoding hex --include-bytecode -o target/interface.jsonl
FUNCTION_AT=$(jq -r 'select(.record=="function") | .at' target/evm-interface.jsonl | head -n 1)
rxd evm get-function tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --at "$FUNCTION_AT"
rxd evm get-function tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --at "$FUNCTION_AT" --raw
rxd evm get-function tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --selector 1ba64846 --raw --perf
```

`interface` lists function candidates as JSONL, and each `function` row
carries EVMole's own `arguments` and `stateMutability` next to the navigation evidence
(selector, dispatch, decoded entry, blocks, boundary/ownership status). `get-function`
takes the row's `at` or a unique `--selector HEX8`; zero candidates is `NOT_FOUND`,
multiple is `AMBIGUOUS_SELECTOR`. Default FETCH reconstructs language-neutral EVM
pseudocode when supported (`status=success`); unsupported/incomplete targets use
`raw_fallback` with a reason and exact raw evidence. `--raw` sets `not_requested` and
skips reconstruction. Boundaries, ownership, discovery, reconstruction and structuring
are reported separately; a function is not a proven source-level boundary.

```sh
rxd evm get-function tests/corpus/evm/openzeppelin-erc20.runtime.hex --encoding hex --selector 313ce567 --perf
rxd evm get-function tests/corpus/evm/openzeppelin-erc20.runtime.bin --encoding raw --selector 18160ddd
rxd evm get-function tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --selector e48c8755
```

## Basic-block listing and FETCH

```sh
rxd evm blocks tests/fixtures/evm/storage-runtime.hex --encoding hex -o target/evm-blocks.jsonl
rxd evm blocks tests/fixtures/evm/storage-runtime.hex --encoding hex -f block:0:
BLOCK_AT=$(jq -r 'select(.record=="basic_block") | .at' target/evm-blocks.jsonl | head -n 1)
rxd evm get-block tests/fixtures/evm/storage-runtime.hex --encoding hex --at "$BLOCK_AT"
rxd evm get-block tests/fixtures/evm/storage-runtime.hex --encoding hex --at "$BLOCK_AT" --perf
```

`blocks` lists basic blocks; `get-block` returns only the selected block's
original bytes/instructions (`bytecode.hex` is null, `raw_hex` covers that block). A block
end is the last opcode start; the row `start/end` are the carrier's byte-exclusive range.
Producer blocks are candidates, not proof of function boundaries or reachability. The
native `basic-blocks` command stays the raw EVMole range array.

## Code-unit references

```sh
rxd evm interface tests/corpus/evm/authored-navigation.runtime.hex --encoding hex -o target/evm-ref-interface.jsonl
TARGET_AT=$(jq -r 'select(.record=="function") | .at' target/evm-ref-interface.jsonl | head -n 1)
rxd evm xref tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --at "$TARGET_AT" -o target/evm-code-xref.jsonl
rxd evm xref tests/corpus/evm/authored-navigation.runtime.hex --encoding hex -m 1ba64846 --kind function
CONTEXT_AT=$(jq -r 'select(.record=="code_reference") | .context_at' target/evm-code-xref.jsonl | head -n 1)
rxd evm get-function tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --at "$CONTEXT_AT" --raw
```

`xref` reports explicit static control-flow references (`jump`, `branch_true`,
`branch_false`, `fallthrough`, `selector_dispatch`) with concrete function/context
witnesses or block-local stack evidence. Each row keeps the caller in `context_at`, the
queried object in `detail.target`, and the instruction/stack evidence in `detail`;
`-f … --kind …` resolves one target by selector, block coordinate or literal value; zero
hits are `NOT_FOUND` and multiple matches are `AMBIGUOUS`. It does not claim source-level
calls, runtime dispatch, complete selector discovery or a runtime call graph. Unknown/invalid jumps are
counted separately; zero hits can still be a complete scan.

## PUSH/data search and XREF

```sh
rxd evm strings tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --kind text --query RXD_DATA_NAVIGATION -o target/evm-literals.jsonl
rxd evm strings tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --kind text -f NAVIGATION
LITERAL_AT=$(jq -r 'select(.record=="literal") | .at' target/evm-literals.jsonl | head -n 1)
rxd evm xref tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --at "$LITERAL_AT" -o target/evm-literal-xref.jsonl
CONTEXT_AT=$(jq -r 'select(.record=="literal_reference") | .context_at' target/evm-literal-xref.jsonl | head -n 1)
rxd evm get-function tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --at "$CONTEXT_AT" --raw
```

`--kind text|bytes|number` is required. Text matches readable runs of complete PUSH
operands; bytes compares the exact full-width operand; number compares unsigned U256.
Literal rows carry `detail.value` (decoded text when the operand is printable, otherwise
its lowercase hex) and `detail.index` (position in the decoded inventory), so `-f`
matches the value the row is named by. The scope is validated-block PUSH immediates, and
`excluded_unindexed_pushes` accounts for PUSH-like bytes outside validated blocks.
Separate `data_literal`/`data_reference` records cover unindexed, metadata and contextual
CODECOPY candidates; a text/data hit does not prove a runtime string or executable use.
IDs bind one exact occurrence/range; zero hits still succeed with no data rows beyond the analysis record.

## Budgets and performance

EVM is capability-first and has no numeric latency gate. Ordinary CLI has no
input-derived navigation/reconstruction work quota; there is no `--max-*` flag or
fixed record/output quota. Interpreter guards remain 100,000 states and 1,048,576
stack cells/arena entries. MCP service quotas remain separate.
Failures are atomic with no partial stdout and preserve existing `-o` files. `--perf`
reports decode/analyze/navigate/reconstruct/structure/serialize phases separately and
echoes the effective ceilings; measurements are informational.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend-specific coordinates, offsets and fallback semantics are
preserved by the same query implementation.
