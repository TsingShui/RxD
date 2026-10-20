#!/usr/bin/env python3
"""Fresh-process JVM frontend acceptance workloads; no input code execution."""
import argparse
import datetime
import hashlib
import json
import math
import os
import platform
import statistics
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def command(args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/rxd")
    parser.add_argument("--samples", type=int, default=30)
    parser.add_argument("--output", type=Path, default=ROOT / "target/benchmarks/jvm")
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("--samples must be positive")
    binary = args.binary.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    production = [
        ("gson", ROOT / "tests/corpus/jvm/gson-2.11.0.jar", [
            ("com/google/gson/GsonBuilder", "serializeNulls"),
            ("com/google/gson/JsonPrimitive", "isNumber"),
            ("com/google/gson/JsonArray", "size"),
        ]),
        ("rhino", ROOT / "tests/corpus/jvm/rhino-1.7.15.jar", [
            ("org/mozilla/javascript/Context", "getLanguageVersion"),
            ("org/mozilla/javascript/ScriptRuntime", "isJSLineTerminator"),
        ]),
    ]
    report = {
        "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platform": platform.platform(), "machine": platform.machine(),
        "logical_cpus": os.cpu_count(),
        "cpu": command(["sysctl", "-n", "machdep.cpu.brand_string"]) if platform.system() == "Darwin" else platform.processor(),
        "rustc": command(["rustc", "--version"]),
        "git_head": command(["git", "rev-parse", "HEAD"]),
        "git_status": command(["git", "status", "--porcelain"]),
        "binary": str(binary), "binary_sha256": sha(binary),
        "cargo_lock_sha256": sha(ROOT / "Cargo.lock"),
        "cache": "Warm OS file cache: preparation plus one warm-up per workload; caches not flushed; every invocation uses a fresh CLI and worker.",
        "load_start": os.getloadavg(), "inputs": [], "workloads": [],
    }
    for label, path, methods in production:
        report["inputs"].append({"name": label, "path": str(path), "bytes": path.stat().st_size, "sha256": sha(path), "synthetic": False})
        base = [str(binary), "jar"]
        prepared = subprocess.run(base + ["outline", str(path)], capture_output=True, check=True)
        rows = [json.loads(line) for line in prepared.stdout.splitlines()]
        assert rows[0]["record"] == "header" and rows[-1]["complete"]
        assert rows[-1]["records"] == len(rows) - 2
        workloads = [("outline", base + ["outline", str(path)], None)]
        workloads.append(("outline_filtered", base + ["methods", str(path), "--class", methods[0][0], "--name", methods[0][1]], None))
        for owner, name in methods:
            method = next(row for row in rows if row.get("record") == "method" and row["detail"]["class"] == owner and row["detail"]["name"] == name)
            query = base + ["get-method", str(path), method["id"]]
            workloads.append((name + "/java", query, None))
            workloads.append((name + "/bytecode", query + ["--bytecode"], None))
        destination = args.output / (label + "-method.java")
        workloads.append((methods[0][1] + "/file", workloads[2][1] + ["-o", str(destination)], destination))
        for name, argv, destination in workloads:
            def invoke():
                start = time.perf_counter_ns()
                result = subprocess.run(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
                elapsed = time.perf_counter_ns() - start
                if result.returncode:
                    raise RuntimeError(f"{name}: {result.stderr.decode(errors='replace')}")
                data = destination.read_bytes() if destination else result.stdout
                return elapsed, data

            _, expected = invoke()
            durations = []
            for _ in range(args.samples):
                elapsed, data = invoke()
                assert data == expected, (label, name, "non-deterministic output")
                durations.append(elapsed)
            sorted_samples = sorted(durations)
            over = sum(value > 150_000_000 for value in durations)
            item = {"input": label, "name": name, "argv": argv,
                    "samples_ns": durations, "median_ms": statistics.median(durations) / 1e6,
                    "p95_ms": sorted_samples[math.ceil(len(durations) * .95) - 1] / 1e6,
                    "over_150ms_count": over, "over_150ms_proportion": over / len(durations),
                    "output_bytes": len(expected), "output_sha256": hashlib.sha256(expected).hexdigest()}
            report["workloads"].append(item)
            print(f"{label}/{name}: median {item['median_ms']:.2f} ms, P95 {item['p95_ms']:.2f} ms, >150 ms {over}/{len(durations)}", flush=True)
    report["load_end"] = os.getloadavg()
    (args.output / "results.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    lines = ["# JVM release measurements", "", report["cache"], "",
             "| Input/workload | Median ms | P95 ms | >150 ms | Output bytes |",
             "| --- | ---: | ---: | ---: | ---: |"]
    for item in report["workloads"]:
        lines.append(f"| {item['input']}/{item['name']} | {item['median_ms']:.2f} | {item['p95_ms']:.2f} | {item['over_150ms_count']}/{len(item['samples_ns'])} | {item['output_bytes']} |")
    (args.output / "results.md").write_text("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
