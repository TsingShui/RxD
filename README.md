<div align="center">

# <img src="site/assets/favicon.png" alt="" width="44" style="vertical-align: middle"> RxD

**Rust x Decompiler: For The Real Reverse Engineer**

An open-source, high-performance analysis CLI built for Agents: one binary for most
reverse-engineering workflows, easy to integrate and ready to use.

Project site: [tsingshui.github.io/RxD](https://tsingshui.github.io/RxD/) · [Examples](https://tsingshui.github.io/RxD/examples.html) · [Arxd](https://tsingshui.github.io/RxD/arxd.html) · [RoadMap](https://tsingshui.github.io/RxD/roadmap.html) · [ChangeLog](https://tsingshui.github.io/RxD/changelog.html) · [中文](https://tsingshui.github.io/RxD/zh/)

[![Build RxD](https://github.com/TsingShui/RxD/actions/workflows/build.yml/badge.svg)](https://github.com/TsingShui/RxD/actions/workflows/build.yml)
[![Rust](https://img.shields.io/badge/rust-1.97%2B-orange?logo=rust)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-blue)](docs/ci.md)
[![Formats](https://img.shields.io/badge/formats-11-success)](#-supported-formats)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![No Exec](https://img.shields.io/badge/input-never%20executed-critical)](#safety-model)

</div>

---

## Quick start

Download the artifact for your platform from a successful
[Build RxD](https://github.com/TsingShui/RxD/actions/workflows/build.yml) run, unpack
the artifact and its inner archive, then put `rxd` on your executable path:

```sh
/absolute/path/rxd --version
/absolute/path/rxd --help
```

Use `rxd BACKEND --help` to choose a workflow, then `rxd BACKEND COMMAND --help`
for minimal examples, required argument combinations, coordinate sources and
result routing. Copy returned `at`/`context_at` values rather than inventing IDs.

The shell installer supports pinned and latest-stable installation once a public
release is available; see [Install and update](docs/install.md). After installing
the binary, print the shared Agent skill:

```sh
rxd skill
```

The skill is emitted as the exact `SKILL.md` Markdown on stdout with the name `rxd`,
covers every current backend and needs no network or source checkout. `rxd skill`
writes no files or Agent configuration; place the output where an Agent discovers it.

Start the native MCP stdio server separately and register that command in the
client's own configuration. Inputs use absolute paths; all local paths are allowed
by default. Optionally restrict access with repeatable `--allow-path` grants:

```sh
rxd mcp
rxd mcp --allow-path /absolute/path/to/inputs
```

Reload and verify using the
[Pi, Claude Code and Codex onboarding guide](docs/install.md#agent-onboarding).
Writing configuration does not establish a live session; client opt-outs and
overrides still apply.

## Why RxD

| | |
| --- | --- |
| 🧩 **Broad coverage** | Native, APK, JAR, EVM, .NET, PYC, Lua, Flutter and WASM, plus ASAR/ZIP/PyInstaller/wxapkg unpacking |
| ⚡ **Fast enough to ask freely** | 31 cold-process workloads across RxD-owned backends: median **6.7 ms**, mean **13.6 ms** |
| 🔧 **Precise decompilation** | Carefully selected high-performance backends with explicit degradation contracts |
| 🤖 **CodeMode** | Agent-first output and native CodeMode-compatible records |
| 📦 **One binary, easy integration** | All tools inlined in one executable; unpack and use |

Measured with **37 workloads × 20 cold-process calls = 740 samples** on an AMD
Ryzen 9 7950X, release build, full process invocation and warm file caches.

**RxD-owned backends** — 31 workloads · 620 samples

| Statistic | Value |
| --- | ---: |
| Median (median of the 31 medians) | **6.7 ms** |
| Mean (mean of the 31 medians) | **13.6 ms** |
| Slowest median | **62 ms** (`dex classes`, 126 MB APK) |
| Maximum P95 | 98.7 ms |
| Samples over 150 ms | **0 / 620** |

Inputs around 100 KB are typically **3–10 ms**; a 1 MB JAR enumerates in 56 ms;
ordinary queries on 64–126 MB APKs take 1.9–62 ms.

**Kuna native executable** — 6 workloads · 120 samples

| Statistic | Value |
| --- | ---: |
| Median | 95.5 ms |
| Mean | 115.5 ms |
| Single-function `functions` / `decompile` (23 KB ELF) | 56 / 59 ms |
| Whole-binary `decompile-all` / `decompile-project` (23 KB ELF) | 132 / 187 ms |
| `functions` on a real ELF (`/bin/ls`, 158 KB) | 254 ms |

<details>
<summary><b>All 37 workloads</b> (milliseconds)</summary>

| Backend | Command | Input | Median | P95 | Mean |
| --- | --- | ---: | ---: | ---: | ---: |
| `dex` | `manifest` | 64 MB | 1.9 | 2.3 | 2.0 |
| `evm` | `analyze` | 51 B | 3.2 | 3.6 | 3.3 |
| `evm` | `disassemble` | 51 B | 3.2 | 3.5 | 3.2 |
| `wasm` | `functions` | 225 B | 3.2 | 3.7 | 3.3 |
| `wasm` | `get-function` | 225 B | 3.2 | 3.6 | 3.3 |
| `wasm` | `functions` | 786 B | 3.3 | 3.5 | 3.3 |
| `pyc` | `codes` | 10.4 KB | 3.5 | 3.9 | 3.7 |
| `pyc` | `decompile` | 1.3 KB | 3.5 | 3.9 | 3.6 |
| `pyc` | `decompile` | 10.4 KB | 3.9 | 4.3 | 3.9 |
| `kuna` | `catalog` | — | 4.9 | 5.4 | 5.0 |
| `dotnet` | `methods` | 4.1 KB | 5.4 | 5.7 | 5.4 |
| `dotnet` | `get-method` | 4.1 KB | 5.4 | 5.6 | 5.4 |
| `dotnet` | `get-class` | 4.1 KB | 6.3 | 6.6 | 6.3 |
| `jar` | `strings` | 298.4 KB | 10.2 | 10.6 | 10.3 |
| `jar` | `get-method` | 298.4 KB | 11.0 | 11.4 | 11.1 |
| `dex` | `xref` | 64 MB | 12.3 | 13.1 | 12.5 |
| `dex` | `strings` | 64 MB | 12.5 | 12.8 | 12.5 |
| `jar` | `xref` | 298.4 KB | 14.2 | 14.7 | 14.2 |
| `jar` | `methods` | 298.4 KB | 14.5 | 15.1 | 14.5 |
| `dex` | `classes` | 64 MB | 22.2 | 24.5 | 22.3 |
| `dex` | `strings` | 126 MB | 24.5 | 25.2 | 24.5 |
| `jar` | `get-class` | 298.4 KB | 26.4 | 27.2 | 27.0 |
| `kuna` | `functions` | 23.6 KB ELF | 55.6 | 58.7 | 55.9 |
| `jar` | `methods` | 1 MB | 56.1 | 58.2 | 55.9 |
| `kuna` | `decompile` | 23.6 KB ELF | 59.0 | 59.5 | 59.0 |
| `dex` | `get-class` | 64 MB | 61.5 | 98.7 | 64.9 |
| `dex` | `classes` | 126 MB | 62.1 | 71.1 | 61.0 |
| `kuna` | `decompile-all` | 23.6 KB ELF | 131.9 | 133.0 | 131.9 |
| `kuna` | `decompile-project` | 23.6 KB ELF | 187.3 | 188.8 | 187.6 |
| `kuna` | `functions` | 158.5 KB ELF | 254.0 | 256.4 | 253.8 |

</details>

APK and ELF benchmarks use authorized inputs; see [benches/README.md](benches/README.md)
for method and limits.

## Why RxD is fast and precise

1. Most of RxD is implemented in Rust, without hidden memory or performance surprises.
2. RxD decompiles on demand wherever possible.
3. RxD integrates carefully selected engines such as Kuna, ILSpy, PyCdC and ASC,
   then adapts them to its own navigation and output contracts.
4. RxD is driven by practical reverse-engineering workflows and measured acceptance
   contracts, not by format-count marketing.

## 📦 Installation

**macOS / Linux**: supported targets are macOS arm64 and Linux x86_64/arm64
(glibc ≥ 2.39). Download a package from a successful Actions run; the
[distribution guide](docs/ci.md) covers checksums and manual installation.

When a public release is available, the planned installer is:

```sh
curl -fsSL https://github.com/TsingShui/RxD/releases/latest/download/install.sh | sh
```

See [Install and update](docs/install.md) for pinned versions, custom
directories, script inspection, PATH and troubleshooting.

**Windows**: download `rxd-windows-x86_64.zip` and its SHA-256 file, verify and
unpack, then run `rxd-windows-x86_64\rxd.exe --version`. The shell installer only
supports the three Unix targets above.

**Build from source**: initialize the Kuna submodule, then a normal Cargo build
enables `all-feature` (all backend namespaces and MCP). Desktop ILSpy is not
checked in and is not embedded by default; C# reconstruction is opt-in. Kuna is
not built or embedded by default. Rust and system linker tools are required; the
first dependency download needs network access. macOS is ARM64-only; Linux
supports ARM64/x86_64; Windows x64 is supported.

```sh
git submodule update --init third_party/kuna
cargo build --release
target/release/rxd skill
# Reduced builds remain explicit:
cargo build --release --no-default-features --features backend-wasm
# Backends and MCP, without a native C# engine:
cargo build --release --no-default-features --features all-backends,mcp
# C# reconstruction: produce the engine once on this OS, then embed it:
python3 scripts/build-ilspy.py linux-x64
RXD_ILSPY_EMBED_RS="$PWD/target/dotnet-native/linux-x64/embed.rs" \
  cargo build --release --features bundled-ilspy
```

`backend-kuna` (the `rxd kuna` command namespace, alias `rxd bin`) is a default feature, but the
Kuna engine is linked only with `bundled-kuna`. A plain build exposes the command
and reports `KUNA_NOT_BUNDLED` when it runs; release bundles and
`scripts/build-bundled.py` opt in, while `--no-default-features` removes the
namespace entirely.

No native engine is checked in. Desktop ILSpy is produced per target with
`scripts/build-ilspy.py`; Android App engines are produced from source with the
instructions in [`devtools/android-ilspy/`](devtools/android-ilspy/). The full
maintenance and release flow is in
[Build and verification](docs/ci.md#build-and-verification-details).

**Agent skills**: print the embedded Agent skill to stdout; it writes no files and
configures no Agent:

```sh
rxd skill
```

Redirect or copy the output into the skills directory an Agent discovers; the skill
name is `rxd` and it covers every current backend. The Markdown is embedded in the
binary, needs no network or source checkout, and diagnostics stay on stderr. Start
MCP separately with `rxd mcp` and register it in the client configuration.

## 🧩 Supported formats

| Backend | Entry | Capability | Status |
| --- | --- | --- | --- |
| **Unpack** | `rxd unpack` | Detect ASAR, ZIP, PyInstaller or wxapkg by content and extract every regular file into a new directory; PyInstaller one-dir roots and V1MMWX wxapkg encryption are supported | ✅ |
| **Native (Kuna)** | `rxd kuna` · `rxd bin` | ELF/PE/Mach-O function inventory, single-function and whole-project decompilation; embedded specs, direct CLI | ✅ engine opt-in (`bundled-kuna`) |
| **Android APK/DEX** | `rxd rasc` | Classes, strings, references, whole-class decompilation, Manifest decoding, field layout | ✅ |
| **JVM class/JAR** | `rxd jar` | Class/field/method metadata, constant-pool strings, cross-class references, single-method/whole-class Java | ✅ |
| **.NET IL** | `rxd ilspy` | Type/member inventory, string navigation, semantic references, original IL or C# reconstruction | ✅ |
| **WebAssembly** | `rxd wasm` | Module structure, instruction decode, explicit reference scanning, structured C-like decompilation | ✅ |
| **CPython bytecode** | `rxd pyc` | CPython 3.8–3.13 navigation and bounded whole-module source reconstruction | ✅ |
| **Dart AOT (Flutter)** | `rxd dae` | Snapshot info, function symbols, pool strings and references, whole-library pseudocode for arm64 `libapp.so` | ✅ |
| **Lua bytecode** | `rxd unluac-rs` | PUC Lua 5.1–5.5, LuaJIT 2.1 and Luau navigation, references and whole-chunk reconstruction | ✅ |
| **EVM bytecode** | `rxd evmole` | EVMole 0.9.4 native analysis; candidate/literal static navigation, on-demand raw evidence and declared pseudocode reconstruction | ✅ static workflow accepted |

## MCP

Release builds provide a local typed MCP service; Agents can consume
`structuredContent` directly:

```sh
rxd mcp
# Default source builds include MCP; reduced builds must enable it explicitly.
```

`rxd mcp` is the stdio server; it starts no client and writes no client
configuration. Register the absolute executable and argv in the client yourself.
Local input paths must be absolute. Without `--allow-path`, the server permits any
local input; each supplied file or directory grant narrows that set.

Use `rxd_open` / `rxd_close` to pin inputs, or pass a path directly. Every built-in
backend preserves its original commands, content identities, completeness and
degradation semantics; optional Kuna provides native JSON `functions` and
single-function `decompile`. See [configuration and codemode examples](docs/mcp.md)
and [four-platform release builds](docs/ci.md). HTTPS is a later transport milestone.

## Safety model

Analyzed files are untrusted data. rxd never executes input code, instantiates input
modules or loads them into a runtime. Parsers read one fixed source buffer, IDs are bound
to content and byte ranges, and every query has explicit input, entity, work, record and
output budgets. Failures are atomic: no partial success payload is written and an
existing `-o` target is preserved. Worker isolation limits accidental state sharing but
is not an OS security sandbox.

## 📄 License

RxD is licensed under the [Apache License 2.0](LICENSE).

Component licenses are listed in [NOTICE](NOTICE).

---

<div align="center">

If RxD saved you a toolchain detour, a ⭐ is welcome.

</div>
