# Pinned Solidity runtime corpus

These are offline analysis fixtures. Generation compiles trusted Solidity source;
neither generation nor tests deploy a contract or execute its bytecode. None of
the expectations were obtained from rxd.

## Artifacts and provenance

- `authored-navigation.runtime.{bin,hex}`: trusted authored `NavigationCorpus.sol`,
  MIT. Eight selectors exercise unchecked calldata arithmetic, a conditional,
  a natural counted loop, persistent/transient storage, memory and terminal
  behavior. This is synthetic compiler output, not a production contract.
- `authored-navigation-legacy.runtime.{bin,hex}`: the same authored source through
  the legacy compiler pipeline. This gives an independently identified 100-byte
  readable runtime data object and a static CODECOPY use, plus shared compiler
  subroutines and indirect return jumps.
- `openzeppelin-erc20.runtime.{bin,hex}`: authored MIT `PinnedToken.sol` wrapper
  compiled over the unchanged npm release `@openzeppelin/contracts@5.1.0` ERC20
  implementation. Production library source is retained with its MIT headers
  under `source/@openzeppelin/contracts/` and its full license is retained.
  The wrapper has no overrides or custom runtime functions; it calls the normal
  ERC20 constructor. This is reproducibly compiled production-library source,
  not downloaded bytecode from a deployed token. Constructor name/symbol strings
  are stored in state and are not claimed to be readable runtime literals.

Package resolved tarballs and SHA-512 integrity are fixed in `package-lock.json`
and recorded in `manifest.json`. Source, runtime, exact hex carrier and generator
SHA-256 values are also recorded. Hex carriers are lower-case hex plus one LF;
raw files contain the corresponding bytes without a prefix. The compiler is
`solc 0.8.26+commit.8a97fa7a.Emscripten.clang`, via npm `solc@0.8.26`.
The solc-js wrapper is MIT; the upstream Solidity compiler is GPL-3.0. These are
generation-only tools and are not linked into rxd or required by ordinary tests.

## Fixed settings

All samples target `cancun`, enable optimization at 200 runs, retain CBOR
metadata, use IPFS metadata hashes with literal source contents, and keep default
revert strings. The authored primary and OpenZeppelin samples use `viaIR: true`;
the legacy data sample explicitly uses `viaIR: false`. Exact standard JSON
settings are in the manifest and generator; no compiler defaults are guessed.

## Independent expectations

`*.oracle.json` combines compiler methodIdentifiers, AST byte ranges, storage
layout, source maps, function debug entries and compiler runtime data with a
small independent byte decoder. Each original byte/instruction offset is
checkable against the `.bin` or `.hex` runtime. Compiler-source-map attribution
is evidence about instructions, not proof of exact function ownership: optimized
Solidity sometimes attributes an entire external wrapper to the contract span.

The restricted static-region oracle resolves only adjacent PUSH jump targets
that are valid JUMPDESTs. It leaves indirect jumps unknown, does not evaluate
input code, and makes no general EVM semantic-equivalence claim. This oracle
fully covers the straightforward primary authored targets and ERC20 decimals /
totalSupply. Legacy/shared-helper and other production targets can retain
unresolved regions; do not turn that restricted oracle into a completeness claim.

`acceptance-expectations.json` is a compact selection of independently inspected
instruction, literal, state and CFG facts for CLI E2E. Tests should obtain actual
IDs from outline/strings/refs and compare FETCH evidence and reconstruction to
these facts; the file deliberately contains no precomputed rxd IDs or pseudocode
snapshots. Assert recovered behaviors, status and raw evidence rather than exact
chosen temporary names, or arbitrary semantic equivalence from text matches.

## Reproduce

Generation requires Node (recorded version: 24.20.0) and npm. Ordinary E2E only
reads the checked-in artifacts and must remain offline.

```sh
npm ci --ignore-scripts --no-audit --no-fund
node generate.mjs
python3 curate-expectations.py
```

The generator also writes full standard JSON inputs and compiler outputs,
assembly and optimized Yul beside the fixtures. They are useful audit evidence;
their inclusion in source control is optional. Review runtime/manifest/oracle
differences when intentionally updating source or tooling. Do not regenerate
expectations using rxd output.

Primary references:
[Solidity 0.8.26 compiler interface](https://docs.soliditylang.org/en/v0.8.26/using-the-compiler.html),
[source mappings](https://docs.soliditylang.org/en/v0.8.26/internals/source_mappings.html),
[compiler license](https://github.com/ethereum/solidity/blob/v0.8.26/LICENSE.txt),
[OpenZeppelin 5.1.0 release](https://github.com/OpenZeppelin/openzeppelin-contracts/releases/tag/v5.1.0),
[pinned ERC20 source](https://github.com/OpenZeppelin/openzeppelin-contracts/blob/v5.1.0/contracts/token/ERC20/ERC20.sol).
