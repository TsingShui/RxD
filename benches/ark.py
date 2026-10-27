#!/usr/bin/env python3
"""Cold-process Ark backend benchmark; release acceptance runner.

Measures the real CLI against a fixed ABC/HAP. It never executes analyzed code.
Run with a release binary after performance-measurement approval:

    cargo build --release
    python3 benches/ark.py --abc /path/modules.abc --hap /path/app.hap

Reports per-workload samples, median, P95, maximum, the count over 150 ms and
optional `--perf` phase records. Writes JSON under target/benchmarks/.
"""
import argparse
import json
import os
import platform
import statistics
import subprocess
import time
from pathlib import Path

GATE_MS = 150.0


def run(binary, args, samples, perf):
    times = []
    phases = []
    for _ in range(samples):
        command = [binary, *args] + (["--perf"] if perf else [])
        started = time.perf_counter()
        result = subprocess.run(command, capture_output=True, text=True)
        times.append((time.perf_counter() - started) * 1000.0)
        if result.returncode != 0:
            raise SystemExit(f"command failed ({result.returncode}): {' '.join(command)}\n{result.stderr[:600]}")
        if not result.stdout.strip():
            raise SystemExit(f"command produced no output: {' '.join(command)}")
        for line in result.stderr.splitlines():
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            if record.get("record") == "perf":
                phases.append(record)
    ordered = sorted(times)
    return {
        "samples": len(times),
        "median_ms": round(statistics.median(times), 3),
        "p95_ms": round(ordered[max(0, int(len(ordered) * 0.95) - 1)], 3),
        "max_ms": round(max(times), 3),
        "over_gate": sum(1 for value in times if value > GATE_MS),
        "phase_records": phases[:1],
    }


def first_row(binary, args):
    result = subprocess.run([binary, *args], capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(result.stderr[:600])
    for line in result.stdout.splitlines():
        fields = line.split("\t")
        if fields:
            return fields[0]
    raise SystemExit(f"no rows: {' '.join(args)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--abc", type=Path, required=True)
    parser.add_argument("--hap", type=Path)
    parser.add_argument("--entry", default="ets/modules.abc")
    parser.add_argument("--binary", default="target/release/rxd")
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--small-method", help="method:0x... coordinate")
    parser.add_argument("--large-method", help="method:0x... coordinate")
    parser.add_argument("--record", help="record:0x... coordinate")
    parser.add_argument("--large-record", help="record:0x... coordinate")
    args = parser.parse_args()
    binary = args.binary
    if not Path(binary).exists():
        raise SystemExit(f"missing binary: {binary} (build with cargo build --release)")

    record = args.record or first_row(binary, ["ark", "list", "record", str(args.abc)])
    methods = subprocess.run(
        [binary, "ark", "list", "method", str(args.abc), "--at", record],
        capture_output=True, text=True, check=True,
    ).stdout.splitlines()
    rows = [line.split("\t") for line in methods if line]
    if not rows:
        raise SystemExit("selected record has no methods")
    small = args.small_method or rows[0][0]
    large = args.large_method or max(rows, key=lambda row: int(row[2] or 0))[0]

    workloads = [
        ("list_record", ["ark", "list", "record", str(args.abc)], False),
        ("list_method", ["ark", "list", "method", str(args.abc), "--at", record], False),
        ("method_small", ["ark", "decompile", str(args.abc), "--at", small], True),
        ("method_large", ["ark", "decompile", str(args.abc), "--at", large], True),
        ("record", ["ark", "decompile", str(args.abc), "--at", record], True),
    ]
    if args.large_record:
        workloads.append(("record_large", ["ark", "decompile", str(args.abc), "--at", args.large_record], True))
    if args.hap:
        workloads.append(("method_hap", ["ark", "decompile", str(args.hap), "--entry", args.entry, "--at", small], True))

    report = {
        "gate_ms": GATE_MS,
        "binary": str(Path(binary).resolve()),
        "abc": {"path": str(args.abc), "bytes": args.abc.stat().st_size},
        "hap": {"path": str(args.hap), "bytes": args.hap.stat().st_size} if args.hap else None,
        "hardware": platform.platform(),
        "machine": platform.machine(),
        "cpu_count": os.cpu_count(),
        "cache": "cold process per sample; no warm-up",
        "coordinates": {"record": record, "small": small, "large": large},
        "workloads": {},
    }
    for name, command, perf in workloads:
        report["workloads"][name] = run(binary, command, args.samples, perf)
    output = Path("target/benchmarks")
    output.mkdir(parents=True, exist_ok=True)
    target = output / "ark.json"
    target.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
