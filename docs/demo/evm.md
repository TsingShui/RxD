# EVM analysis and code-unit navigation

`--encoding raw|hex` is mandatory. Inputs are analyzed offline; nothing is executed.

```sh
rxd evm analyze tests/fixtures/evm/storage-runtime.hex --encoding hex
rxd evm interface tests/fixtures/evm/storage-runtime.hex --encoding hex --json
rxd evm functions tests/fixtures/evm/storage-runtime.hex --encoding hex
rxd evm storage tests/fixtures/evm/transient-storage-runtime.hex --encoding hex
rxd evm disassemble tests/fixtures/evm/storage-runtime.hex --encoding hex
rxd evm basic-blocks tests/fixtures/evm/storage-runtime.hex --encoding hex
rxd evm cfg tests/fixtures/evm/storage-runtime.hex --encoding hex
rxd evm metadata tests/fixtures/evm/metadata-runtime.hex --encoding hex

rxd evm interface runtime.bin --encoding raw --include-bytecode -o target/interface.json
rxd evm analyze runtime.hex --encoding hex --perf
rxd evm analyze runtime.hex --encoding hex --max-input-bytes 8388608 --max-records 200000
```

Native commands emit one `evm_analysis` object. Carrier SHA-256, decoded-bytecode
SHA-256, decoded-byte offsets, CFG producer block IDs, `undecoded_tail`,
`abi_verified`/`discovery_complete` and inference properties are distinct. See the
[EVM design](../design/evm-design.md#native-json).

## Basic-block FETCH

```sh
rxd evm outline tests/fixtures/evm/storage-runtime.hex --encoding hex -o target/evm-outline.jsonl
BLOCK_ID=$(jq -r 'select(.record=="basic_block") | .id' target/evm-outline.jsonl | head -n 1)
rxd evm get-block tests/fixtures/evm/storage-runtime.hex "$BLOCK_ID" --encoding hex
rxd evm get-block tests/fixtures/evm/storage-runtime.hex "$BLOCK_ID" --encoding hex --max-work 16777216 --perf
```

Block IDs bind carrier, raw/hex encoding and decoded start/end. `get-block` returns only
the selected block's original bytes/instructions; `bytecode.hex` is null and `raw_hex`
contains only that block. A block end is the last opcode start; the navigation `start/end`
are the carrier's byte-exclusive range. Producer blocks are candidates, not proof of
function boundaries or reachability.

## Function FETCH

```sh
rxd evm outline tests/corpus/evm/authored-navigation.runtime.hex --encoding hex -o target/evm-functions.jsonl
FUNCTION_ID=$(jq -r 'select(.record=="function") | .id' target/evm-functions.jsonl | head -n 1)
rxd evm get-function tests/corpus/evm/authored-navigation.runtime.hex "$FUNCTION_ID" --encoding hex
rxd evm get-function tests/corpus/evm/authored-navigation.runtime.hex "$FUNCTION_ID" --encoding hex --raw
rxd evm get-function tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --selector 1ba64846 --raw --perf
```

`get-function` accepts an ID or a unique `--selector HEX8`; zero candidates is
`NOT_FOUND`, multiple is `AMBIGUOUS_SELECTOR`. Default FETCH reconstructs
language-neutral EVM pseudocode when supported (`status=success`); unsupported/
incomplete targets use `raw_fallback` with a reason and exact raw evidence. `--raw` sets
`not_requested` and skips reconstruction. Function IDs bind carrier/encoding, selector,
ABI/fallback mode and entry. Boundaries, ownership, discovery, reconstruction and
structuring are reported separately; a function is not a proven source-level boundary.

```sh
rxd evm get-function tests/corpus/evm/openzeppelin-erc20.runtime.hex --encoding hex --selector 313ce567 --perf
rxd evm get-function tests/corpus/evm/openzeppelin-erc20.runtime.bin --encoding raw --selector 18160ddd
rxd evm get-function tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --selector e48c8755 --max-reconstruction-work 1048576 --max-values 1048576
```

## Code-unit references

```sh
rxd evm outline tests/corpus/evm/authored-navigation.runtime.hex --encoding hex -o target/evm-ref-outline.jsonl
TARGET_ID=$(jq -r 'select(.record=="function") | .id' target/evm-ref-outline.jsonl | head -n 1)
rxd evm refs tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --id "$TARGET_ID" -o target/evm-code-refs.jsonl
CONTEXT_ID=$(jq -r 'select(.record=="code_reference") | .context_id' target/evm-code-refs.jsonl | head -n 1)
rxd evm get-function tests/corpus/evm/authored-navigation.runtime.hex "$CONTEXT_ID" --encoding hex --raw
```

`refs` reports explicit static control-flow references (`jump`, `branch_true`,
`branch_false`, `fallthrough`, `selector_dispatch`) with concrete function/context
witnesses or block-local stack evidence. It does not claim source-level calls, runtime
dispatch, complete selector discovery or a runtime call graph. Unknown/invalid jumps are
counted separately; zero hits can still be a complete scan.

## PUSH/data search and XREF

```sh
rxd evm strings tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --kind text --query RXD_DATA_NAVIGATION -o target/evm-literals.jsonl
LITERAL_ID=$(jq -r 'select(.record=="literal") | .id' target/evm-literals.jsonl | head -n 1)
rxd evm refs tests/corpus/evm/authored-navigation.runtime.hex --encoding hex --id "$LITERAL_ID" -o target/evm-literal-refs.jsonl
CONTEXT_ID=$(jq -r 'select(.record=="literal_reference") | .context_id' target/evm-literal-refs.jsonl | head -n 1)
rxd evm get-function tests/corpus/evm/authored-navigation.runtime.hex "$CONTEXT_ID" --encoding hex --raw
```

`--kind text|bytes|number` is required. Text matches readable runs of complete PUSH
operands; bytes compares the exact full-width operand; number compares unsigned U256.
The scope is validated-block PUSH immediates, and `excluded_unindexed_pushes` accounts
for PUSH-like bytes outside validated blocks. Separate `data_literal`/`data_reference`
records cover unindexed, metadata and contextual CODECOPY candidates; a text/data hit
does not prove a runtime string or executable use. IDs bind one exact occurrence/range;
zero hits still produce a complete header+summary. See the
[EVM design](../design/evm-design.md#strings-and-data-candidates).

## Budgets and performance

EVM is capability-first and has no numeric latency gate. Input, output and record budgets
still apply; default raw input is 8 GiB and complete output 16 MiB. Navigation adds work/
states/stack-cell budgets, and default function reconstruction has its own
reconstruction work/value limits. Failures are atomic with no partial stdout and preserve
existing `-o` files. `--perf` reports decode/analyze/navigate/reconstruct/structure/
serialize phases separately; measurements are informational.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend IDs, offsets, inference and degradation are in the
[EVM design](../design/evm-design.md#verification-and-mcp).
