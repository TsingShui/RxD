# Pinned language corpora

Ordinary E2E uses saved artifacts and independently generated expectations; it does
not access the network or execute sample code. Handwritten small inputs still isolate
boundary behavior. Each backend keeps its provenance, licenses, versions and hashes
in its own subdirectory or README.

## JVM bytecode corpus

Pinned Gson/Rhino JARs, licenses and independent javap expectations are in
[jvm/README.md](jvm/README.md). The native JVM frontend reads them statically and never
executes their code.

## EVM pinned compiler corpus

`evm/` retains solc 0.8.26 authored viaIR/legacy and OpenZeppelin 5.1.0 ERC20 runtime,
independent raw-instruction/selector expectations, package integrity, source/runtime
hashes, generators and licenses. It is an offline compiler corpus, not a chain download
or execution result; see [EVM corpus README](evm/README.md).

## WASM and PYC corpora

`wasm/` and `pyc/` retain pinned inputs, generators and independent expectations used
by their feature-scoped CLI tests. Sample code is never executed.

## ZIP family corpus

`zip/` retains a published AAR/VSIX with independent metadata and payload
expectations; see [zip/README.md](zip/README.md).

## wxapkg fixtures

`tests/fixtures/wxapkg/` contains small normal and `V1MMWX`-encrypted packages
used by the unpack CLI tests; see [wxapkg/README.md](../fixtures/wxapkg/README.md).

## PyInstaller corpus

`pyinstaller/` retains a one-file Mach-O built with PyInstaller 6.22.3 and Python
3.9.6, one-file and one-dir launcher handling, plus its pinned expected output; see
[pyinstaller/README.md](pyinstaller/README.md). The executable is unpacked
statically and never run.
