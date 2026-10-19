#!/usr/bin/env python3
"""Measure complete fresh-process skill installation against isolated user homes."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / "src/skills/rxd/SKILL.md"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def command_output(args):
    result = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return result.stdout.decode(errors="replace").strip() if result.returncode == 0 else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--output", type=Path, default=ROOT / "target/benchmarks/skill")
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("--samples must be positive")
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    content = SKILL.read_bytes()
    stale = b"---\nname: rxd\n---\nSynthetic previous skill version.\n"
    sources = ["Cargo.toml", "Cargo.lock", "src/main.rs", "src/cli/mod.rs",
               "src/cli/skill.rs", "src/skills/rxd/SKILL.md", "benches/skill.py"]
    report = {
        "binary": str(binary), "binary_bytes": binary.stat().st_size,
        "binary_sha256": sha256(binary.read_bytes()),
        "build": "caller-supplied release executable; this runner does not build",
        "git_head": command_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"]),
        "git_status": command_output(["git", "-C", str(ROOT), "status", "--short"]),
        "source_sha256": {path: sha256((ROOT / path).read_bytes()) for path in sources},
        "skill_bytes": len(content), "skill_sha256": sha256(content),
        "stale_content": {"provenance": "synthetic previous skill text",
                          "bytes": len(stale), "sha256": sha256(stale)},
        "hardware": {"platform": platform.platform(), "machine": platform.machine(),
                     "cpu_count": os.cpu_count(),
                     "cpu": command_output(["sysctl", "-n", "machdep.cpu.brand_string"])
                     if platform.system() == "Darwin" else platform.processor(),
                     "memory_bytes": command_output(["sysctl", "-n", "hw.memsize"])
                     if platform.system() == "Darwin" else None},
        "cache": "OS caches are not flushed; skill/hash preparation warms file caches. "
                 "Every timed call launches a new process with a new isolated HOME. "
                 "Target initial states are prepared outside each timer. The executable may "
                 "have run previously; this is not cold-executable or cold-OS-cache evidence.",
        "boundary": "External perf_counter_ns around subprocess.run through exit and "
                    "complete stdout/stderr pipe capture; all installer file writes included. "
                    "Preparation, content validation and report persistence excluded.",
        "load_start": os.getloadavg() if hasattr(os, "getloadavg") else None,
        "started_unix_ns": time.time_ns(), "workloads": [],
    }
    for name in ["installed", "updated", "unchanged", "print", "all"]:
        samples = []
        workload = {"name": name, "samples": samples}
        report["workloads"].append(workload)
        for index in range(args.samples):
            with tempfile.TemporaryDirectory(prefix=f"{name}-{index}-", dir=output) as temporary:
                home = Path(temporary)
                target = home / ".pi/agent/skills/rxd/SKILL.md"
                targets = [target]
                invocation = [str(binary), "skill", "install", "-a", "pi"]
                state = "absent"
                if name in ("updated", "unchanged"):
                    target.parent.mkdir(parents=True)
                    target.write_bytes(stale if name == "updated" else content)
                    state = "different synthetic bytes" if name == "updated" else "identical bytes"
                if name == "print":
                    invocation = [str(binary), "skill", "install", "--print"]
                    targets = []
                    state = "no target directories"
                if name == "all":
                    invocation[-1] = "all"
                    targets += [home / ".codex/skills/rxd/SKILL.md",
                                home / ".claude/skills/rxd/SKILL.md",
                                home / ".agents/skills/rxd/SKILL.md"]
                    state = "four absent targets"
                env = os.environ.copy()
                env.update(HOME=str(home), USERPROFILE=str(home), RXD_CACHE_DIR=str(home / "cache"))
                env.pop("CODEX_HOME", None)
                started = time.perf_counter_ns()
                try:
                    result = subprocess.run(invocation, env=env, stdout=subprocess.PIPE,
                                            stderr=subprocess.PIPE, timeout=30)
                except subprocess.TimeoutExpired as error:
                    sample = {"duration_ns": time.perf_counter_ns() - started,
                              "argv": invocation, "initial_state": state, "timed_out": True,
                              "stdout_bytes": len(error.stdout or b""),
                              "stderr": (error.stderr or b"").decode(errors="replace")}
                    samples.append(sample)
                    report["failure"] = {"workload": name, "sample": sample}
                    (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
                    raise
                elapsed = time.perf_counter_ns() - started
                sample = {"duration_ns": elapsed, "argv": invocation, "home": str(home),
                          "initial_state": state, "exit_code": result.returncode,
                          "stdout_bytes": len(result.stdout), "stderr_bytes": len(result.stderr),
                          "stdout_sha256": sha256(result.stdout),
                          "stderr": result.stderr.decode(errors="replace")}
                samples.append(sample)
                if name == "print":
                    valid = result.stdout == content and not result.stderr and not list(home.iterdir())
                else:
                    verb = "installed" if name == "all" else name
                    expected = "".join(f"rxd skill {verb} at {path}\n" for path in targets).encode()
                    try:
                        valid = not result.stdout and result.stderr == expected and all(
                            path.read_bytes() == content for path in targets)
                    except OSError as error:
                        sample["validation_error"] = str(error)
                        valid = False
                if result.returncode != 0 or not valid or (home / "cache").exists():
                    report["failure"] = {"workload": name, "sample": sample}
                    (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
                    raise RuntimeError(f"{name}: installer contract failed; see results.json")
        durations = sorted(sample["duration_ns"] for sample in samples)
        over = sum(value > 150_000_000 for value in durations)
        workload.update(median_ns=statistics.median(durations),
                        p95_ns=durations[math.ceil(len(durations) * 0.95) - 1],
                        over_150ms_count=over,
                        over_150ms_proportion=over / len(durations))
    report.update(finished_unix_ns=time.time_ns(),
                  load_end=os.getloadavg() if hasattr(os, "getloadavg") else None)
    (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    lines = ["# Skill installer CLI measurements", "", report["cache"], "", report["boundary"], "",
             f"Skill: {len(content)} bytes; SHA-256 `{sha256(content)}`.", "",
             "| Workload | Samples | Median ms | P95 ms | >150 ms |",
             "| --- | ---: | ---: | ---: | ---: |"]
    for item in report["workloads"]:
        lines.append(f"| {item['name']} | {len(item['samples'])} | "
                     f"{item['median_ns'] / 1e6:.3f} | {item['p95_ns'] / 1e6:.3f} | "
                     f"{item['over_150ms_count']} ({item['over_150ms_proportion']:.1%}) |")
    lines += ["", "All raw samples, argv, target states, output diagnostics, hardware, load and source/binary identities are in results.json.",
              "A successful run validates contracts; assess latency for each workload separately."]
    (output / "results.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
