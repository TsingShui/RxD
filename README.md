<div align="center">

# <img src="site/assets/favicon.png" alt="" width="44" style="vertical-align: middle"> RxD

**Rust x Decompiler**

An open-source, high-performance analysis CLI built for Agents: one binary for most
reverse-engineering workflows, easy to integrate and ready to use.


Project site: [tsingshui.github.io/RxD](https://tsingshui.github.io/RxD/) · [Examples](https://tsingshui.github.io/RxD/examples.html) · [Arxd](https://tsingshui.github.io/RxD/arxd.html) · [RoadMap](https://tsingshui.github.io/RxD/roadmap.html) · [ChangeLog](https://tsingshui.github.io/RxD/changelog.html)

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


## 📦 Installation


```sh
curl -fsSL https://github.com/TsingShui/RxD/releases/latest/download/install.sh | sh
```

**Agent skills**: print the embedded Agent skill to stdout; it writes no files and
configures no Agent:

```sh
rxd skill
```

Redirect or copy the output into the skills directory an Agent discovers; the skill
name is `rxd` and it covers every current backend. The Markdown is embedded in the
binary, needs no network or source checkout, and diagnostics stay on stderr. Start
MCP separately with `rxd mcp` and register it in the client configuration.

## MCP

Release builds provide a local typed MCP service; Agents can consume `structuredContent` directly:

```sh
rxd mcp
# Default source builds include MCP; reduced builds must enable it explicitly.
```

## 📄 License

RxD is licensed under the [Apache License 2.0](LICENSE).

Component licenses are listed in [NOTICE](NOTICE).

---

<div align="center">

If RxD saved you a toolchain detour, a ⭐ is welcome.

</div>
