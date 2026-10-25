# Directly linked Kuna CLI

`backend-kuna` is a default feature, so the `rxd kuna` namespace (alias
`rxd bin`) exists in an ordinary build. The engine is a separate opt-in: only `bundled-kuna` links the
pinned `third_party/kuna` checkout through the generated adapter. Without it the
command reports `KUNA_NOT_BUNDLED`; `--no-default-features` omits the namespace
entirely. Default and reduced builds therefore never need the upstream toolchain,
while release bundles ship the engine and specs.

```sh
git submodule update --init third_party/kuna
python3 scripts/build-bundled.py --jobs 1
# Complete single-file product including ILSpy:
python3 scripts/build-ilspy.py osx-arm64
RXD_ILSPY_EMBED_RS="$PWD/target/dotnet-native/osx-arm64/embed.rs" \
  python3 scripts/build-bundled.py --jobs 1 --features bundled-ilspy
```

The build-only slacomp compiles specs under `target/kuna/<host>/`. The generated
`rxd-kuna-cli` adapter links upstream CLI modules into rxd without editing the Kuna
submodule. Native code shares the Rust link/LTO step with rxd. Only specs and license
notices are compressed in the Kuna resource archive; no standalone Kuna executable or
`decomp_dbg` is shipped. `--bundle-only` prepares specs without building rxd; `--offline`
uses cached Cargo dependencies. Direct Cargo builds need the initialized submodule for
optional dependency metadata.

```sh
target/release/rxd kuna --version
# `bin` is the same dispatcher under a shorter, tool-neutral name.
target/release/rxd bin --version
target/release/rxd kuna functions ./app --summary --json
target/release/rxd kuna decompile ./app main --json
target/release/rxd kuna decompile ./app main
target/release/rxd kuna decompile-all ./app --json --jobs 2
target/release/rxd kuna decompile-project ./app --stream --jobs 2 -o ./app.kuna
target/release/rxd kuna catalog --json --option loweredswitch
target/release/rxd kuna docs options
```

Only rxd is needed at runtime. Help/version/docs and the default catalog work without a
specs cache. Analysis prepares a private `rxd-kuna/<archive-sha256>` cache under
`RXD_CACHE_DIR`, XDG cache or the platform user cache. Worker commands restart rxd with
the `kuna` prefix and call the same linked CLI. Text conversion captures native JSON
stdout in-process and extracts `functions[].code`.

Default single-function text uses the JSON policy; `--raw`, `--regions` and `--kassert`
require a separately supplied console engine. Explicit `--decomp-dbg` /
`KUNA_DECOMP_DBG` retain native handling. `decompile-project` exports native C/H/ASM and
index files and does not accept `--json`. `kuna specs` / `kuna test` still require their
external helper programs.

Use `python3 scripts/format-workspace.py --check` for formatting; `cargo fmt --all` also
traverses upstream path dependencies and must not be used.

## Pinned release

rxd pins Kuna `v1.978` (`fd71d996be867356394675a42dbadd951f5ce424`). The default
`kuna catalog` comes from the linked engine in-process and needs neither `decomp_dbg` nor
a specs cache; an explicitly selected external console keeps upstream handling.
`scripts/check-kuna-latest.py` compares the gitlink with upstream formal tags, so a new
tag requires an explicit reviewed upgrade rather than a build-time update.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. In a linked MCP build, `kuna_decompile` selects one function with
exactly one `name` or `address` string (for example, a preceding `kuna_functions` entry's
`address_hex`). The native worker has a nominal 5 ms sampled RSS guard;
`--max-native-worker-bytes` defaults to 1 GiB and may be raised or lowered.
Observed excess is an explicit `RESOURCE_LIMIT` with worker cleanup, not an
instantaneous OS cap. RxD no longer injects its input-derived instruction quota;
the unchanged pinned engine retains its own `maxinstruction` default (100,000).
Direct `rxd kuna` calls can adjust it through upstream `--option maxinstruction N`;
MCP does not advertise a per-request `limits` object.
