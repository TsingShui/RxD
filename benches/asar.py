#!/usr/bin/env python3
"""Fresh-process ASAR benchmarks over repackaged, pinned production JS.

The application wrapper is synthetic; this is not a shipped Electron app corpus.
Never execute the packaged JavaScript or start Electron/Node.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import struct
import subprocess
import sys

import cli as base

ROOT = Path(__file__).resolve().parents[1]


def pack(path, files, extra_entries=None):
    tree = {"files": {}}
    data = bytearray()
    for entry, source in sorted(files.items()):
        node = tree
        parts = entry.split("/")
        for part in parts[:-1]:
            node = node["files"].setdefault(part, {"files": {}})
        node["files"][parts[-1]] = {"size": len(source), "offset": str(len(data))}
        data.extend(source)
    tree["files"].update(extra_entries or {})
    header = json.dumps(tree, separators=(",", ":")).encode()
    payload_size = (len(header) + 7) // 4 * 4
    blob = (struct.pack("<IIII", 4, payload_size + 4, payload_size, len(header))
            + header + bytes(payload_size - 4 - len(header)) + data)
    path.write_bytes(blob)
    return len(blob)


def rows(payload):
    return [json.loads(line) for line in payload.splitlines()]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--cpu-model")
    parser.add_argument("--output", type=Path, default=ROOT / "target/benchmarks/asar")
    options = parser.parse_args()
    if options.samples < 1:
        parser.error("--samples must be positive")
    if hasattr(signal, "SIGALRM"):
        signal.signal(signal.SIGALRM, base.timeout_handler)
    if options.binary is None:
        subprocess.run(["cargo", "build", "--release", "--locked"], cwd=ROOT, check=True)
    binary = (options.binary or ROOT / "target/release/rxd").resolve()
    output = options.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    corpus = ROOT / "tests/corpus/manifest.json"
    manifest = json.loads(corpus.read_text())
    cases = manifest["cases"][:3]
    files = {"main.cjs": b"const entry=()=>42;", "addon.node": b"\x7fELF\x00\xff\x80native"}
    for case in cases:
        source = (corpus.parent / case["file"]).read_bytes()
        if hashlib.sha256(source).hexdigest() != case["sha256"]:
            raise RuntimeError(f"corpus hash mismatch: {case['name']}")
        files[f"renderer/{case['name']}.js"] = source
    archive = output / "app.asar"
    archive_bytes = pack(archive, files)
    decoys = {f"{i:060x}": {"size": 1 << 30, "offset": str((1 << 53) - 1 - (1 << 30))}
              for i in range(100)}
    protected = output / "bloat.asar"
    protected_bytes = pack(protected, files, decoys)
    prepared = output / "prepared"
    prepared.mkdir(exist_ok=True)
    tasks = []

    def command(name, target, args):
        return [str(binary), "asar", name, str(target), *args]

    def listing(payload, protected=False):
        result = rows(payload)
        assert result[0]["backend"] == "asar" and result[0]["analysis"] == "metadata"
        assert result[-1]["complete"] and result[-1]["records"] == len(files) + (len(decoys) if protected else 0)
        assert result[-1]["invalid_ranges"] == (len(decoys) if protected else 0)
        assert [r["path"] for r in result[1:-1]] == sorted([*files, *(decoys if protected else {})])
        for row in result[1:-1]:
            assert row["range_valid"] == (row["path"] not in decoys)

    def validate_receipt(payload, destination, source, entry):
        receipt = json.loads(payload)
        assert receipt["backend"] == "asar" and receipt["command"] == "extract" and receipt["complete"]
        assert receipt["origin"]["entry"] == entry and receipt["path"] == str(destination)
        assert receipt["source_id"] == hashlib.sha256(source).hexdigest()
        assert receipt["file_bytes"] == len(source) and destination.read_bytes() == source
        return receipt

    for kind, target in [("asar", archive), ("asar_bloat", protected)]:
        tasks.append((f"{kind}/list", command("list", target, []), lambda payload, kind=kind: listing(payload, kind == "asar_bloat"), None))
        for entry, source in files.items():
            destination = prepared / f"{kind}-{Path(entry).name}"
            argv = command("extract", target, ["--file", entry, "-o", str(destination)])
            validate = lambda payload, destination=destination, source=source, entry=entry: validate_receipt(payload, destination, source, entry)
            tasks.append((f"{kind}/{entry}/extract", argv, validate, len(source)))

    report = {
        "benchmark_version": 1, "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "environment": {"os": platform.platform(), "machine": platform.machine(), "cpu_count": os.cpu_count(),
                        "cpu_model": options.cpu_model or base.tool_output(["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"]),
                        "cpu_model_source": "caller observation" if options.cpu_model else "host query",
                        "python": sys.version, "rustc": base.tool_output(["rustc", "--version"]),
                        "load_average_start": os.getloadavg() if hasattr(os, "getloadavg") else None},
        "build": {"profile": "release" if options.binary is None else "custom", "binary": str(binary),
                  "binary_sha256": base.digest(binary), "cargo_lock_sha256": base.digest(ROOT / "Cargo.lock"),
                  "git_head": base.tool_output(["git", "rev-parse", "HEAD"]), "git_status": base.tool_output(["git", "status", "--porcelain"])},
        "method": {"samples": options.samples, "warmups": 1, "profile_samples": 3,
                   "process_mode": "fresh CLI/worker every invocation", "file_cache": "primed by generation/setup; not flushed",
                   "sink": "temporary stdout/stderr and actual atomic artifact writes", "p95": "nearest rank", "latency_target_ms": 150},
        "input": {"kind": "synthetic ASAR wrappers over pinned production JS and handcrafted binary bytes",
                  "archive": str(archive), "archive_bytes": archive_bytes, "archive_sha256": base.digest(archive),
                  "bloat_archive_bytes": protected_bytes, "bloat_archive_sha256": base.digest(protected),
                  "decoys": len(decoys), "declared_decoy_bytes": 100 * (1 << 30),
                  "decoy_reference": "https://github.com/sleeyax/asarmor/blob/master/src/patch.ts",
                  "provenance": [c["provenance"] for c in cases], "corpus_sha256": base.digest(corpus),
                  "limitation": "not a shipped application or protector-generated package; no binary execution"},
        "workloads": [],
    }
    for name, argv, validate, source_bytes in tasks:
        initial, payload = base.checked(argv, 120)
        validate(payload)
        base.checked(argv, 120)
        samples, profiles = [], []
        for _ in range(options.samples):
            sample, payload = base.checked(argv, 120)
            validate(payload)
            assert sample["stdout_sha256"] == initial["stdout_sha256"]
            samples.append(sample)
        for _ in range(3):
            sample, payload = base.checked([*argv, "--perf"], 120)
            validate(payload)
            assert len(sample.get("perf", [])) == 2
            assert sample["stdout_sha256"] == initial["stdout_sha256"]
            profiles.append(sample)
        report["workloads"].append({"name": name, "argv": argv, "source_bytes": source_bytes,
                                    "initial": initial, "samples": samples, "stats": base.stats(samples),
                                    "profiles": profiles, "profile_stats": base.profile_stats(profiles)})

    # A separate workflow includes receipt decoding, routing and a second fresh CLI.
    for case in cases:
        entry = f"renderer/{case['name']}.js"
        source = files[entry]
        destination = prepared / f"workflow-{case['name']}.js"
        argv = command("extract", protected, ["--file", entry, "-o", str(destination)])
        def workflow():
            started = base.time.perf_counter_ns()
            first, payload = base.checked(argv, 120)
            receipt = json.loads(payload)
            second, found = base.checked([str(binary), "js", "find", receipt["path"], "-f", case["needle"]], 120)
            duration = base.time.perf_counter_ns() - started
            validate_receipt(payload, destination, source, entry)
            result = base.jsonl(found, receipt["source_id"])
            assert len(result)-2 == len(case["expected"]["matches"])
            sample = dict(second)
            sample.update(duration_ns=duration, commands=[first, second], command_sum_ns=first["duration_ns"]+second["duration_ns"])
            rss = [s["wait4_max_process_rss_bytes"] for s in (first, second)
                   if s["wait4_max_process_rss_bytes"] is not None]
            sample["wait4_max_process_rss_bytes"] = max(rss) if rss else None
            return sample
        initial = workflow()
        workflow()
        samples = [workflow() for _ in range(options.samples)]
        report["workloads"].append({"name": f"asar_bloat/{case['name']}/extract_find", "argv": argv,
            "source_bytes": len(source), "initial": initial, "samples": samples, "stats": base.stats(samples),
            "profiles": [], "profile_stats": []})
    report["environment"]["load_average_end"] = os.getloadavg() if hasattr(os, "getloadavg") else None
    (output / "results.json").write_text(json.dumps(report, indent=2)+"\n")
    lines = ["# ASAR CLI benchmark", "", report["input"]["kind"], "",
             f"Hardware: {report['environment']['cpu_model']}; {report['environment']['os']}",
             f"ASAR: {archive_bytes} bytes; bloat ASAR: {protected_bytes} bytes; {len(decoys)} synthetic 1-GiB decoys.",
             "Fresh processes, primed OS file cache. Extraction includes real atomic file writes.", "",
             "| Workload | Source bytes | Median ms | P95 ms | >150 ms |", "|---|---:|---:|---:|---:|"]
    for w in report["workloads"]:
        s = w["stats"]
        lines.append(f"| {w['name']} | {w['source_bytes']} | {s['median_ns']/1e6:.3f} | {s['p95_ns']/1e6:.3f} | {s['over_150ms_count']}/{s['samples']} ({s['over_150ms_proportion']:.1%}) |")
    lines.extend(["", "extract_find includes two fresh CLI calls and routing; validation occurs after timing.",
                  "Synthetic containers do not establish acceptance for shipped applications or all protections."])
    (output / "results.md").write_text("\n".join(lines)+"\n")
    print(output / "results.md")


if __name__ == "__main__":
    main()
