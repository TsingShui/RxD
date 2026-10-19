#!/usr/bin/env python3
"""Fresh-process DEX CLI benchmarks over explicitly supplied authorized inputs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import time
import cli as base

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, action="append", required=True)
    parser.add_argument("--class", dest="classes", action="append", required=True,
                        help="One real class per input, in matching order")
    parser.add_argument("--source", action="append", help="Optional provenance, one per input")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--baseline", type=Path, help="Original rasc release binary")
    parser.add_argument("--cpu-model")
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--output", type=Path, default=ROOT / "target/benchmarks/dex")
    options = parser.parse_args()
    if len(options.input) != len(options.classes) or (options.source and len(options.source) != len(options.input)):
        parser.error("provide one --class and, if used, one --source per --input")
    if options.samples < 1 or options.timeout <= 0:
        parser.error("samples and timeout must be positive")
    if hasattr(signal, "SIGALRM"):
        signal.signal(signal.SIGALRM, base.timeout_handler)
    if options.binary is None:
        subprocess.run(["cargo", "build", "--release", "--locked"], cwd=ROOT, check=True)
    binary = (options.binary or ROOT / "target/release/rxd").resolve()
    baseline = options.baseline.resolve() if options.baseline else None
    output = options.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    report = {
        "benchmark_version": 1,
        "started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "environment": {"os": platform.platform(), "machine": platform.machine(),
                        "cpu_model": options.cpu_model or platform.processor() or None,
                        "logical_cpus": os.cpu_count(), "load_average_start": os.getloadavg() if hasattr(os, "getloadavg") else None},
        "cache": "Prepared queries and one warmup per workload prime OS file caches; caches are not purged. Every sample starts a new process; no resident session.",
        "binary": {"path": str(binary), "sha256": base.digest(binary), "build": "custom" if options.binary else "cargo build --release --locked"},
        "baseline": {"path": str(baseline), "sha256": base.digest(baseline)} if baseline else None,
        "cargo_lock_sha256": base.digest(ROOT / "Cargo.lock"),
        "git_head": base.tool_output(["git", "rev-parse", "HEAD"]),
        "git_status": base.tool_output(["git", "status", "--short"]),
        "inputs": [], "workloads": [], "not_applicable": [],
        "validation": "Handcrafted E2E supplies independent boundary expectations. Benchmark compares deterministic payloads and verdicts with original rasc when supplied; this is a migration oracle, not an independent decompiler correctness proof.",
    }

    def command(executable, args):
        return [str(executable), *(["dex"] if executable == binary else []), *args]

    def run(executable, args):
        metrics, payload = base.run_command(command(executable, args), options.timeout)
        if metrics["timed_out"] or metrics["exit_code"] not in (0, 3, 4):
            raise base.CommandFailure(command(executable, args), metrics)
        return metrics, payload

    def verdict(metrics, payload):
        return metrics["exit_code"], hashlib.sha256(payload).hexdigest(), metrics.get("diagnostic")

    try:
        for index, (input_path, class_name) in enumerate(zip(options.input, options.classes)):
            target = input_path.resolve()
            path = str(target)
            case = f"input-{index + 1}"
            report["inputs"].append({"name": case, "path": path, "input_bytes": target.stat().st_size,
                                     "sha256": base.digest(target), "class": class_name,
                                     "provenance": options.source[index] if options.source else "Caller-supplied authorized input; provenance not supplied"})
            tasks = [
                ("entries", ["entries", path]),
                ("classes", ["classes", path]),
                ("classes_filter", ["classes", path, "-f", class_name]),
                ("strings_page", ["strings", path, "--filter", "Authorization", "--limit", "20"]),
                ("findrefs_string", ["findrefs", path, "string", "Authorization"]),
                ("findrefs_type", ["findrefs", path, "type", "Activity"]),
                ("findrefs_method", ["findrefs", path, "method", "onCreate", "--class", class_name]),
                ("findrefs_field", ["findrefs", path, "field", "--class", class_name]),
                ("get-class", ["get-class", path, class_name]),
                ("getclass_members", ["get-class", path, class_name, "--members"]),
                ("fields_plan", ["fields-plan", "--apk", path, "--descriptor", class_name]),
                ("fields_plan_ambiguous", ["fields-plan", "--apk", path, path, "--descriptor", class_name]),
                ("getclass_file", ["get-class", path, class_name, "-o", str(output / f"{case}.java")]),
            ]
            _, members = run(binary, ["get-class", path, class_name, "--members"])
            method = re.search(rb"^# members methods: method_ids=(\d+)", members, re.M)
            field = re.search(rb"^# members fields: field_ids=\d+ slot=(\d+)", members, re.M)
            for kind, match in [("method", method), ("field", field)]:
                if match:
                    args = ["member-by-index", "--apk", path, "--descriptor", class_name, f"--{kind}-index", match[1].decode()]
                    tasks.append((f"member_{kind}", args))
                    if kind == "method":
                        tasks.append(("member_method_ambiguous", ["member-by-index", "--apk", path, path, "--descriptor", class_name, "--method-index", match[1].decode()]))
                else:
                    report["not_applicable"].append({"name": f"{case}/member_{kind}", "reason": "Selected class declares no corresponding member"})
            with target.open("rb") as source:
                bare = source.read(4) == b"dex\n"
            if not bare:
                tasks.append(("manifest", ["manifest", path]))
            else:
                report["not_applicable"].append({"name": f"{case}/manifest", "reason": "Standalone DEX has no Manifest"})
            for name, args in tasks:
                first, payload = run(binary, args)
                expected = verdict(first, payload)
                baseline_initial = None
                if baseline:
                    baseline_initial, old_payload = run(baseline, args)
                    if verdict(baseline_initial, old_payload) != expected:
                        raise RuntimeError(f"Migration output/verdict mismatch: {case}/{name}")
                samples, old_samples = [], []
                for _ in range(options.samples):
                    # Alternate order to reduce systematic drift between implementations.
                    executables = [binary, baseline] if len(samples) % 2 == 0 else [baseline, binary]
                    for executable in executables:
                        if executable is None:
                            continue
                        sample, payload = run(executable, args)
                        if verdict(sample, payload) != expected:
                            raise RuntimeError(f"Nondeterministic output/verdict: {case}/{name}")
                        if "-o" in args:
                            written = Path(args[args.index("-o") + 1]).read_bytes()
                            if written != payload:
                                raise RuntimeError("File output differs from stdout")
                        (samples if executable == binary else old_samples).append(sample)
                workload = {"name": f"{case}/{name}", "input_bytes": target.stat().st_size,
                            "argv": command(binary, args), "initial": first, "samples": samples,
                            "stats": base.stats(samples), "baseline_initial": baseline_initial,
                            "baseline_samples": old_samples, "baseline_stats": base.stats(old_samples),
                            "migration_match": True if baseline else None}
                report["workloads"].append(workload)
                stats = workload["stats"]
                print(f"{workload['name']}: median={stats['median_ns']/1e6:.2f} ms, >150={stats['over_150ms_count']}/{stats['samples']}", flush=True)
    except Exception as error:
        report["failure"] = str(error)
        raise
    finally:
        report["environment"]["load_average_end"] = os.getloadavg() if hasattr(os, "getloadavg") else None
        (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        lines = ["# DEX CLI benchmark", "", report["cache"], "",
                 f"Hardware: {report['environment']['cpu_model']}; {report['environment']['os']}", "",
                 "| Workload | Input bytes | Median ms | P95 ms | >150 ms | Baseline median ms |",
                 "|---|---:|---:|---:|---:|---:|"]
        for workload in report["workloads"]:
            stats, old = workload["stats"], workload["baseline_stats"]
            previous = f"{old['median_ns']/1e6:.3f}" if old else "n/a"
            lines.append(f"| {workload['name']} | {workload['input_bytes']} | {stats['median_ns']/1e6:.3f} | {stats['p95_ns']/1e6:.3f} | {stats['over_150ms_count']}/{stats['samples']} ({stats['over_150ms_proportion']:.1%}) | {previous} |")
        lines += ["", "Full cold-process startup, reading, analysis, serialization and real file-backed output are measured. Validation runs after timing. Exit 3/4 verdicts are checked and retained.",
                  "A successful script exit confirms execution and contract checks, not the 150 ms performance acceptance requirement."]
        if report.get("failure"):
            lines += ["", "Failure: " + report["failure"]]
        (output / "results.md").write_text("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
