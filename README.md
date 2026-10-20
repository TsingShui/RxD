<div align="center">

# <img src="site/assets/favicon.png" alt="" width="44" style="vertical-align: middle"> RxD

**Rust x Decompiler: For The Real Reverse Engineer**

An open-source, high-performance analysis CLI built for Agents: one binary for most
reverse-engineering workflows, easy to integrate and ready to use.

Project site: [tsingshui.github.io/RxD](https://tsingshui.github.io/RxD/) · [Examples](https://tsingshui.github.io/RxD/examples.html) · [Arxd](https://tsingshui.github.io/RxD/arxd.html) · [RoadMap](https://tsingshui.github.io/RxD/roadmap.html) · [ChangeLog](https://tsingshui.github.io/RxD/changelog.html) · [中文](https://tsingshui.github.io/RxD/zh/)

[![Build RxD](https://github.com/TsingShui/RxD/actions/workflows/build.yml/badge.svg)](https://github.com/TsingShui/RxD/actions/workflows/build.yml)
[![Rust](https://img.shields.io/badge/rust-1.97%2B-orange?logo=rust)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-blue)](docs/demo/ci.md)
[![Formats](https://img.shields.io/badge/formats-10-success)](#-supported-formats)
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

The shell installer supports pinned and latest-stable installation once a public
release is available; see [Install and update](docs/demo/install.md). After installing
the binary, install the shared Agent skill:

```sh
rxd skill install -a pi
```

The Pi skill is written to `~/.pi/agent/skills/rxd/SKILL.md` with the name `rxd` and
covers every current backend. The skill is embedded in the binary; installation does
not need the network or a source checkout.

Register the native MCP server separately from skill installation, using only roots
that may be analyzed:

```sh
rxd mcp install -a pi --root /absolute/path/to/inputs --preview
rxd mcp install -a pi --root /absolute/path/to/inputs
# Repeat -a for cc and codex, or use -a all. Without -a, detected clients are configured.
```

Reload and verify using the
[Pi, Claude Code and Codex onboarding guide](docs/demo/install.md#agent-onboarding).
Writing configuration does not establish a live session; client opt-outs and
overrides still apply.

## Why RxD

| | |
| --- | --- |
| 🧩 **Broad coverage** | Native, APK, JAR, JavaScript, EVM, ASAR, HTML, .NET, PYC and WASM |
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
| `asar` | `entries` | 9 MB | 3.2 | 3.5 | 3.3 |
| `evm` | `analyze` | 51 B | 3.2 | 3.6 | 3.3 |
| `evm` | `disassemble` | 51 B | 3.2 | 3.5 | 3.2 |
| `wasm` | `functions` | 225 B | 3.2 | 3.7 | 3.3 |
| `wasm` | `get-function` | 225 B | 3.2 | 3.6 | 3.3 |
| `asar` | `entries` | 804 B | 3.3 | 3.6 | 3.3 |
| `wasm` | `functions` | 786 B | 3.3 | 3.5 | 3.3 |
| `pyc` | `codes` | 10.4 KB | 3.5 | 3.9 | 3.7 |
| `pyc` | `decompile` | 1.3 KB | 3.5 | 3.9 | 3.6 |
| `pyc` | `decompile` | 10.4 KB | 3.9 | 4.3 | 3.9 |
| `kuna` | `catalog` | — | 4.9 | 5.4 | 5.0 |
| `dotnet` | `methods` | 4.1 KB | 5.4 | 5.7 | 5.4 |
| `dotnet` | `get-method` | 4.1 KB | 5.4 | 5.6 | 5.4 |
| `js` | `find` | 78.7 KB | 6.1 | 6.4 | 6.1 |
| `dotnet` | `get-class` | 4.1 KB | 6.3 | 6.6 | 6.3 |
| `js` | `entities` | 78.7 KB | 6.7 | 7.2 | 6.8 |
| `js` | `entities` | 73.2 KB | 7.0 | 7.6 | 7.1 |
| `jar` | `strings` | 298.4 KB | 10.2 | 10.6 | 10.3 |
| `js` | `entities` | 168.3 KB | 10.5 | 10.9 | 10.6 |
| `jar` | `get-method` | 298.4 KB | 11.0 | 11.4 | 11.1 |
| `dex` | `xref` | 64 MB | 12.3 | 13.1 | 12.5 |
| `asar` | `extract` | 9 MB | 12.5 | 14.0 | 12.8 |
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
[distribution guide](docs/demo/ci.md) covers checksums and manual installation.

When a public release is available, the planned installer is:

```sh
curl -fsSL https://github.com/TsingShui/RxD/releases/latest/download/install.sh | sh
```

See [Install and update](docs/demo/install.md) for pinned versions, custom
directories, script inspection, PATH and troubleshooting.

**Windows**: download `rxd-windows-x86_64.zip` and its SHA-256 file, verify and
unpack, then run `rxd-windows-x86_64\rxd.exe --version`. The shell installer only
supports the three Unix targets above.

**Build from source**: initialize the Kuna submodule, then a normal Cargo build
enables `all-feature` (all backend namespaces, MCP and embedded ILSpy) and reuses
verified ILSpy bundles without a .NET SDK or extra environment variables. Kuna is
not built or embedded by default. Rust and system linker tools are still required;
the first dependency download needs network access. macOS is ARM64-only; Linux
supports ARM64/x86_64; Windows ILSpy prebuilts still require a producer or an
explicitly prepared bundle.

```sh
git submodule update --init third_party/kuna
cargo build --release
target/release/rxd skill install -a pi
# Reduced builds remain explicit:
cargo build --release --no-default-features --features backend-js
# Backends only; no embedded native engine or MCP:
cargo build --release --no-default-features --features all-backends
```

Prebuilt bundles are documented in [`prebuilt/README.md`](prebuilt/README.md);
the full maintenance and release flow is in
[Build and verification](docs/demo/ci.md#build-and-verification-details).

**Agent skills**: after installing the binary, use the root entry point to select
targets or print the skill:

```sh
rxd skill install -a pi codex claude-code
rxd skill install -a agents
rxd skill install -a all
rxd skill install
rxd skill install --dir /path/to/skills
rxd skill install --print
```

`-a/--agent` accepts multiple Agents; names are case-insensitive, `claude` and
`claude-code` are equivalent, and duplicate targets are installed once. Pi uses
`~/.pi/agent/skills`, Codex uses `skills` under a non-empty `CODEX_HOME` (otherwise
`~/.codex/skills`), Claude Code uses `~/.claude/skills`, and cross-Agent installation
uses `~/.agents/skills`; `all` selects all four locations. Without a target,
installation uses detected Pi, Codex and Claude user directories, falling back to
the cross-Agent directory.

`--dir` is the skill root and writes `DIR/rxd/SKILL.md`; it cannot be combined with
`--agent`. `--print` only emits the embedded skill and cannot be combined with an
installation target. Reinstalling reports installed, updated or unchanged; identical
content is not rewritten. Diagnostics go to stderr. Use the same command to update
the skill; see `rxd skill install --help`. Installation does not initialize analysis
backends or release analysis runtime caches.

## 🧩 Supported formats

| Backend | Entry | Capability | Status |
| --- | --- | --- | --- |
| **Native (Kuna)** | `rxd kuna` | ELF/PE/Mach-O function inventory, single-function and whole-project decompilation; embedded specs, direct CLI | ✅ optional embedded distribution |
| **JavaScript** | `rxd js` | JS/TS/JSX/TSX and recursive directories; structure, strings, byte locations, lexical references, original source extraction | ✅ |
| **ASAR** | `rxd asar` | Electron archive entry enumeration and raw extraction | ✅ |
| **HTML** | `rxd html` | Local-page lexical catalog and node-level raw extraction (element/text/comment/doctype) | ✅ |
| **Android APK/DEX** | `rxd dex` · `rxd apk` | Classes, strings, references, whole-class decompilation, Manifest decoding, field layout | ✅ |
| **JVM class/JAR** | `rxd jar` | Class/field/method metadata, constant-pool strings, cross-class references, single-method/whole-class Java | ✅ |
| **.NET IL** | `rxd dotnet` | Type/member inventory, string navigation, semantic references, original IL or C# reconstruction | ✅ |
| **WebAssembly** | `rxd wasm` | Module structure, instruction decode, explicit reference scanning, structured C-like decompilation | ✅ |
| **CPython bytecode** | `rxd pyc` | CPython 3.8–3.13 navigation and bounded whole-module source reconstruction | ✅ |
| **EVM bytecode** | `rxd evm` | EVMole 0.9.4 native analysis; candidate/literal static navigation, on-demand raw evidence and declared pseudocode reconstruction | ✅ static workflow accepted |

## MCP

Release builds provide a local typed MCP service; Agents can consume
`structuredContent` directly:

```sh
rxd mcp install -a pi -a cc -a codex --root /workspace --preview
rxd mcp install -a pi -a cc -a codex --root /workspace
# Clients without automatic registration can use stdio directly:
rxd mcp serve --base /workspace --root /workspace
# Default source builds include MCP; reduced builds must enable it explicitly.
```

Use `rxd_open` / `rxd_close` to pin inputs, or pass a path directly. Every built-in
backend preserves its original commands, content identities, completeness and
degradation semantics; optional Kuna provides native JSON `functions` and
single-function `decompile`. See [configuration and codemode examples](docs/demo/mcp.md),
the [full contract](docs/design/mcp-design.md) and
[four-platform release builds](docs/demo/ci.md). HTTPS is a later transport milestone.

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
