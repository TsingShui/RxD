#!/usr/bin/env python3
"""Fresh-process release latency for rxd PYC queries."""

import argparse
import hashlib
import json
import math
import platform
import statistics
import subprocess
import time
from pathlib import Path


def invoke(binary: Path, path: Path, command: str, query_args: list[str]) -> tuple[int, bytes]:
    started = time.perf_counter_ns()
    result = subprocess.run(
        [str(binary), "pyc", command, str(path)] + (["--json"] if command == "decompile" else []) + query_args,
        capture_output=True,
        check=False,
    )
    elapsed = time.perf_counter_ns() - started
    if result.returncode != 0:
        raise RuntimeError(f"{path}: {result.stderr.decode(errors='replace')}")
    if command == "decompile":
        payload = json.loads(result.stdout)
        if payload["schema_version"] != 2 or payload["record"] != "source":
            raise RuntimeError(f"{path}: unexpected output envelope")
        complete = payload["detail"]["complete"]
    else:
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        if (rows[0]["schema_version"] != 2 or rows[0]["record"] != "header"
                or rows[-1]["record"] != "summary" or rows[-1]["records"] != len(rows) - 2):
            raise RuntimeError(f"{path}: unexpected static-query envelope")
        complete = rows[-1]["complete"]
    if complete is not True:
        raise RuntimeError(f"{path}: query incomplete")
    return elapsed, result.stdout


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, action="append", required=True)
    parser.add_argument("--command", choices=["decompile", "outline", "strings", "refs"], default="decompile")
    parser.add_argument("--query-arg", action="append", default=[], help="repeat exact CLI query arguments, e.g. --query-arg=--contains --query-arg=URL")
    parser.add_argument("--source", default="caller-provided PYC inputs")
    parser.add_argument("--cpu-model", default="not provided")
    parser.add_argument("--binary", type=Path, default=Path("target/release/rxd"))
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--output", type=Path, default=Path("target/benchmarks/pyc"))
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("--samples must be positive")
    binary = args.binary.resolve()
    rows = []
    for path in args.input:
        path = path.resolve()
        raw = path.read_bytes()
        _, expected = invoke(binary, path, args.command, args.query_arg)  # preparation also warms the file cache
        samples = []
        for _ in range(args.samples):
            elapsed, output = invoke(binary, path, args.command, args.query_arg)
            if output != expected:
                raise RuntimeError(f"{path}: inconsistent output")
            samples.append(elapsed)
        ordered = sorted(samples)
        median_ms = statistics.median(samples) / 1e6
        p95_ms = ordered[math.ceil(0.95 * len(ordered)) - 1] / 1e6
        over = sum(value > 150_000_000 for value in samples)
        rows.append(
            {
                "path": str(path),
                "input_bytes": len(raw),
                "input_sha256": hashlib.sha256(raw).hexdigest(),
                "output_bytes": len(expected),
                "output_sha256": hashlib.sha256(expected).hexdigest(),
                "samples_ns": samples,
                "median_ms": median_ms,
                "p95_ms": p95_ms,
                "over_150ms_count": over,
                "over_150ms_proportion": over / len(samples),
            }
        )
        print(f"{path.name}: median {median_ms:.2f} ms, P95 {p95_ms:.2f} ms, >150 ms {over}/{len(samples)}")
    report = {
        "source": args.source,
        "command": args.command,
        "query_args": args.query_arg,
        "binary": str(binary),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "hardware": platform.platform(),
        "cpu": args.cpu_model,
        "cache_conditions": "one preparation call per input; OS cache not flushed",
        "measurement": "fresh CLI and worker process, JSON serialization and captured stdout included",
        "workloads": rows,
    }
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
