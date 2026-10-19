# EVM bytecode fixtures

Copied unchanged from EVMole 0.9.4's published crate (`agent/tests/fixtures`), source commit `bc8a56c0a5928c7846f46e9a6f224bd98df08d90`. Upstream: https://github.com/cdump/evmole . MIT copyright (c) 2023 Maxim Andreev; license in `crates/rxd-evm/EVMOLE-LICENSE`.

- `storage-runtime.hex`: manual flat dispatcher for selector 11223344; SLOAD slot 0, SSTORE slot 1.
- `transient-storage-runtime.hex`: same layout with TLOAD/TSTORE.
- `metadata-runtime.hex`: STOP followed by a terminal CBOR map `solc: h'00081a'` and its 10-byte length suffix.

CLI tests additionally use upstream's documented two-selector ABI argument example, an independently hand-written PUSH1/JUMP/JUMPDEST/STOP stream, and explicit CBOR/invalid byte arrays. No analyzed contract is deployed or run on a chain.
