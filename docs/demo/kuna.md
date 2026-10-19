# Directly linked Kuna CLI

```sh
git submodule update --init third_party/kuna
python3 scripts/build-bundled.py --jobs 1
# Complete single-file product including ILSpy:
python3 scripts/build-ilspy.py osx-arm64
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

rxd pins Kuna `v1.876` (`a219af9ead94fd58bdb07f602f888498b1eb08e5`). The default
`kuna catalog` comes from the linked engine in-process and needs neither `decomp_dbg` nor
a specs cache; an explicitly selected external console keeps upstream handling.
`scripts/check-kuna-latest.py` compares the gitlink with upstream formal tags, so a new
tag requires an explicit reviewed upgrade rather than a build-time update.

## Offline native Frida source

```sh
# Plain name or explicit hexadecimal static VMA; never a file offset/runtime pointer.
target/release/rxd hook-gen frida native ./libexample.so observe --module libexample.so > target/native.js
target/release/rxd hook-gen frida native ./app 0x402000 --main-module --lang ts > target/native.ts
```

Known entries support cheap JS/raw API-typed TS without Kuna. Only `bundled-kuna` enables
linked discovery/selected TS semantic analysis; `backend-kuna` alone does not link it.
The adapter lazily prepares explicit specs for discovery or linked TS; it never starts a
Java bridge or another process. Missing required resources/engine fail explicitly rather
than fabricating entry types. The default module is derived from the basename/main
executable; verify it against the intended live module.

Accepted mapping: linked little-endian ELF32/64, PE32/PE32+, thin Mach-O32/64 on
ia32/x64/ARM32/arm64 with supported subtype/mapping; not every combination has an
independent fixture. ELF uses header-backed PT_LOAD/load bias, PE image-base/RVA, and
Mach-O actual segment coordinates; ARM/Thumb mode is retained. Relocatable, universal,
big-endian and unsupported subtypes (including arm64e) are rejected. Structured signature
information is debug-derived/inferred/unknown, never authoritative merely because it
type-checks. Supported GP integers/pointers use real conversions; 64-bit stays
Int64/UInt64, pointers are not dereferenced, and unsupported ABI/storage/float/vector/
aggregate/variadic cases remain raw/unavailable. Module/target/attach logs and runtime
guards do not prove live identity, ABI or attach success. No analyzed input/generated hook/
Frida/device executes. Budgets are finite; bootstrap/listing checks are cooperative, not
hard preemption or kernel RSS enforcement.

Build the actual module driver with explicitly prepared resources:

```sh
cargo build --release -p rxd-hook-gen --no-default-features --features native,kuna --example module-driver
RXD_HOOK_TEST_SPECS=/absolute/path/to/prepared/specs target/release/examples/module-driver \
  frida native ./libexample.so observe --lang ts > target/native.ts
node devtools/hook-gen/validate.mjs ts target/native.ts
```

This measures a fresh module-driver process, not product CLI startup or runtime hooks.
See the [native design](../design/external-tools-design.md#offline-native-frida-hook) and
[offline validation pins](../../devtools/hook-gen/README.md).

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. In a linked MCP build, `kuna_decompile` selects one function with
exactly one `name` or `address` string (for example, a preceding `kuna_functions` entry's
`address_hex`). The native worker has a nominal 5 ms sampled RSS guard;
`--max-native-worker-bytes` defaults to and is capped at 256 MiB, and startup may lower
it. Observed excess is an explicit `RESOURCE_LIMIT` with worker cleanup, not an
instantaneous OS cap.
