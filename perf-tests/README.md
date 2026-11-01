# Performance tests and instrumentation

**Scope:** the 150 ms gate applies only to decompilation/analysis backend
queries. Installation, configuration, skill printing, help/version, standalone
MCP discovery/lifecycle, maintenance and build/test/release orchestration are not
gated. Functional, safety, completeness and resource-budget checks are never
exempt. See `AGENTS.md` for the authoritative scope.

Performance tests start the real CLI; in-memory parser timings are not substitutes. They
never execute analyzed input.

## MCP and CLI runners

`mcp.py` measures the typed MCP service against the same public CLI for the pinned
JVM, PYC, DEX, .NET and WASM fixtures. It checks independent facts
before timing.

```sh
cargo build --release
python3 perf-tests/mcp.py
```

The runner writes under ignored `target/perf-tests/` by default and reports each
workload separately: input size, samples, median, P95, maximum and the count over
150 ms. Do not pool unrelated commands or average away large inputs.

## Ark backend

`ark.py` measures the fixed dynamic-ABC sample and its HAP packaging with a
release binary, cold process per sample, per workload:

```sh
cargo build --release
python3 perf-tests/ark.py --abc tests/.../modules.abc --hap app.hap --entry ets/modules.abc
```

It reports samples, median, P95, maximum, the count over 150 ms, and optional
`--perf` phase records; output is written to `target/perf-tests/ark.json`.
Small and large methods/records are separate workloads so large targets are not
hidden by averaging.

## 150 ms design gate

Ordinary representative backend queries should complete within 150 ms in release
builds, measured end-to-end with a fresh process. Whole-function/whole-class or
whole-module decompilation may exceed it only for the two grounds in `AGENTS.md`:
an unmodified third-party engine, or a genuinely large selected target with
target-scale evidence and an ordinary small target from the same input inside the
gate. An eligible ground still needs an explicit recorded scope and bound.

## Reports

Keep machine-readable samples and inputs. A report states hardware, OS, release
profile, cache conditions, input sizes and per-workload statistics; never invent
or extrapolate measurements. Slow-workload counts are reported, not hidden.
