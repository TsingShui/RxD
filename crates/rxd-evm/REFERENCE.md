# EVMole reference

- Native Rust crate: `evmole = 0.9.4`, exact Cargo version, only `serde` enabled.
- Upstream: https://github.com/cdump/evmole
- Published crate source commit: `bc8a56c0a5928c7846f46e9a6f224bd98df08d90`.
- Cargo.lock records the crates.io checksum. The upstream implementation is unchanged.
- License: MIT, copyright (c) 2023 Maxim Andreev; unchanged text in `EVMOLE-LICENSE`, included in release notices.
- Public interface: `contract_info(ContractInfoArgs)`; native `Contract` serde representation is retained under `detail.result`.
- Features: selectors, arguments, state mutability, persistent/transient storage, disassembly, basic blocks, CFG and CBOR metadata. No JavaScript, WASM or Python binding is compiled.

`source_id` is SHA-256 of the selected carrier; `detail.bytecode.sha256` is SHA-256 of decoded bytecode. Neither is Ethereum's Keccak code hash.
Function/storage/CFG data is inferred. Names, verified ABI, deployment authenticity, runtime/init identity, hardfork and discovery completeness are not established.
CFG destinations name upstream block IDs; block ends identify the last opcode start. Preserve the producer's representation, including unknown dynamic edges and encoded CBOR values.

Native contextual function navigation is rxd-owned Rust code; it does not use or modify the producer CFG resolver as a source boundary oracle. Exact constant control-flow arithmetic uses directly pinned `alloy-primitives = 1.7.3` (already locked transitively by EVMole), MIT OR Apache-2.0, native U256 values only. No new executable, interpreter or runtime dependency is required.


Selected-function reconstruction is rxd-owned native Rust (`src/reconstruction.rs`,
crate version 0.1.0, Apache-2.0); it directly lowers the bounded selected contextual
graph to an EVM-only value arena, statements, block parameters and explicit edges.
There is no additional engine dependency or runtime. EVMole remains unchanged;
its selector/block producer results are navigation evidence, not the reconstruction
implementation. `alloy-primitives = 1.7.3` preserves full 256-bit immediate words.
Runtime analysis is offline; the single rxd executable contains the emitter.

The word-intrinsic contract follows the primary Ethereum execution specifications:
[arithmetic](https://ethereum.github.io/execution-specs/src/ethereum/forks/cancun/vm/instructions/arithmetic.py.html),
[bitwise](https://ethereum.github.io/execution-specs/src/ethereum/forks/cancun/vm/instructions/bitwise.py.html),
[memory](https://ethereum.github.io/execution-specs/src/ethereum/forks/cancun/vm/instructions/memory.py.html),
[storage](https://ethereum.github.io/execution-specs/src/ethereum/forks/cancun/vm/instructions/storage.py.html).
These are semantic references, not code copied into rxd or a claim to a full Cancun
execution engine. Input hardfork, gas/exceptional execution equivalence, original
source names, Solidity/ABI and source-level calls remain unknown.
