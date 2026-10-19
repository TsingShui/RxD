# DEX backend provenance

The DEX analysis implementation and CLI were imported from the rasc repository at commit
`6b832c6be9ce198e5bf990647e9c4404dbd093cc` (Apache-2.0). The original license is
retained in this directory. Maintained dependency forks keep their own licenses, fork
notes and patch notes under the top-level `third_party/`.

`rxd dex` provides the non-MCP analysis CLI. The shared root `rxd skill install` entry
and generic skill replace the retired DEX-only skill. The old rasc MCP transport and
persistent session implementation are not imported. The shipped analysis implementation
lives here; `src/cli/dex/` owns the command-line adapter, and no standalone rasc binary is
shipped. CLI argument/output contracts are preserved except for the namespace; test
identifiers were generalized and contain no private corpus or identity mapping.
