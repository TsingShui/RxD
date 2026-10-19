#!/usr/bin/env python3
"""Frozen-binary EVM closure evidence and fresh-process measurements; never executes inputs."""
import argparse
import datetime
import hashlib
import json
import math
import os
import platform
import shutil
import statistics
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "tests/corpus/evm"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def command(argv, cwd=ROOT):
    return subprocess.check_output(argv, cwd=cwd, text=True).strip()


def rows(data):
    result = [json.loads(line) for line in data.splitlines()]
    assert result[0]["record"] == "header" and result[-1]["record"] == "summary"
    assert result[-1]["complete"] and result[-1]["records"] == len(result) - 2
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--output", type=Path, default=ROOT / "target/benchmarks/evm-final-ticket49")
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("--samples must be positive")
    binary, baseline, output = args.binary.resolve(), args.baseline.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.pop("RXD_CACHE_DIR", None)
    report = {
        "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platform": platform.platform(), "machine": platform.machine(), "logical_cpus": os.cpu_count(),
        "cpu": command(["sysctl", "-n", "machdep.cpu.brand_string"]) if platform.system() == "Darwin" else platform.processor(),
        "ram_bytes": int(command(["sysctl", "-n", "hw.memsize"])) if platform.system() == "Darwin" else None,
        "rustc": command(["rustc", "--version"]), "source_head": command(["git", "rev-parse", "HEAD"]),
        "source_status": command(["git", "status", "--porcelain"]),
        "binary": str(binary), "binary_sha256": digest(binary.read_bytes()),
        "baseline": str(baseline), "baseline_sha256": digest(baseline.read_bytes()),
        "baseline_source": "a19c7a566842c721f765fcef962070c4135a04a5", "samples": args.samples,
        "cache": "Warm OS file cache after tests/preparation and one warm-up per workload; caches not flushed. Each invocation is a fresh CLI and worker; no resident session. RXD_CACHE_DIR removed.",
        "load_start": os.getloadavg(), "evm_numeric_gate": None,
        "limitations": ["macOS arm64 only; other platform CI contracts inspected, not executed", "Authored EVM inputs are synthetic solc output; OZ is compiled published library source, not deployed mainnet bytecode", "No .NET production corpus was present or measured", "DEX functional gates only; no representative DEX latency claim", "Stages/counters are separate profile calls, not substituted for fresh-process wall time", "Behavior assertions establish declared source/byte/CFG facts, not arbitrary semantic equivalence"],
        "inputs": [], "workloads": [], "flows": [], "native_regression": [], "selected_scale_control": {},
    }
    files = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=ROOT).split(b"\0")
    source = {os.fsdecode(p): digest((ROOT / os.fsdecode(p)).read_bytes()) for p in files if p and (ROOT / os.fsdecode(p)).is_file()}
    (output / "source-files-sha256.json").write_text(json.dumps(source, indent=2) + "\n")
    report["source_manifest_sha256"] = digest((output / "source-files-sha256.json").read_bytes())
    report["cargo_lock_sha256"] = digest((ROOT / "Cargo.lock").read_bytes())
    report["kuna_pin"] = command(["git", "submodule", "status", "third_party/kuna"])
    (output / "source-diff.patch").write_bytes(subprocess.check_output(["git", "diff", "HEAD", "--"], cwd=ROOT))

    def invoke(argv, cwd=ROOT):
        start = time.perf_counter_ns()
        result = subprocess.run(argv, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
        elapsed = time.perf_counter_ns() - start
        if result.returncode:
            raise RuntimeError(f"{argv}: {result.stderr.decode(errors='replace')}")
        return elapsed, result

    def statistics_for(samples):
        return {"samples_ns": samples, "median_ms": statistics.median(samples) / 1e6,
                "p95_ms": sorted(samples)[math.ceil(len(samples) * .95) - 1] / 1e6,
                "over_150ms_count": sum(v > 150_000_000 for v in samples),
                "over_150ms_proportion": sum(v > 150_000_000 for v in samples) / len(samples)}

    def measure(name, argv, kind, selected=None):
        _, warm = invoke(argv)
        durations = []
        for _ in range(args.samples):
            elapsed, result = invoke(argv)
            assert result.stdout == warm.stdout and result.stderr == warm.stderr, (name, "output changed")
            durations.append(elapsed)
        perf = None
        if argv[1] != "jar":
            _, profile = invoke(argv + ["--perf"])
            assert profile.stdout == warm.stdout
            perf = [json.loads(line) for line in profile.stderr.splitlines() if line.startswith(b'{"record":"perf"')]

        item = {"name": name, "kind": kind, "argv": argv, "output_bytes": len(warm.stdout),
                "output_sha256": digest(warm.stdout), "perf": perf, "perf_unavailable_reason": "JVM CLI does not expose --perf" if perf is None else None, **statistics_for(durations)}
        if selected:
            item["selected"] = selected
        report["workloads"].append(item)
        print(f"{name}: median={item['median_ms']:.3f}ms P95={item['p95_ms']:.3f}ms >150={item['over_150ms_count']}/{args.samples}", flush=True)
        return warm.stdout

    def evm(path, encoding, verb, *extra):
        return [str(binary), "evm", verb, str(path), "--encoding", encoding, *extra]

    def context(path, encoding, id_, raw=False):
        verb = "get-function" if id_.split(":")[4] == "function" else "get-block"
        return evm(path, encoding, verb, id_, *(["--raw"] if raw and verb == "get-function" else []))

    def original(row, path, sample, encoding):
        carrier, decoded = path.read_bytes(), (CORPUS / f"{sample}.runtime.bin").read_bytes()
        assert row["record"] == "function_code"
        for segment in row["detail"]["segments"]:
            s, e = segment["decoded_start"], segment["decoded_end"]
            assert segment["raw_hex"] == decoded[s:e].hex()
            part = carrier[segment["carrier_start"]:segment["carrier_end"]]
            assert part == (decoded[s:e] if encoding == "raw" else decoded[s:e].hex().encode())
        for instruction in row["detail"]["instructions"]:
            s, e = instruction["decoded_offset"], instruction["decoded_end"]
            assert instruction["opcode"] == decoded[s] and instruction["operand_hex"] == decoded[s+1:e].hex()

    expected_status = {
        "authored-navigation": {s: "success" for s in ["1ba64846", "0bb0b13a", "188b85b4", "3acddfc1", "82da2e57", "ae3da75a", "e21f37ce", "e48c8755"]},
        "authored-navigation-legacy": {s: "success" for s in ["1ba64846", "0bb0b13a", "188b85b4", "3acddfc1", "82da2e57", "ae3da75a", "e21f37ce", "e48c8755"]},
        "openzeppelin-erc20": {"313ce567": "success", "18160ddd": "success", "06fdde03": "success", "095ea7b3": "raw_fallback"},
    }
    for sample, selectors in expected_status.items():
        oracle = json.loads((CORPUS / f"{sample}.oracle.json").read_text())
        for encoding, suffix in [("raw", "bin"), ("hex", "hex")]:
            path = CORPUS / f"{sample}.runtime.{suffix}"
            decoded = (CORPUS / f"{sample}.runtime.bin").read_bytes()
            report["inputs"].append({"sample": sample, "encoding": encoding, "path": str(path), "bytes": path.stat().st_size, "sha256": digest(path.read_bytes()), "decoded_bytes": len(decoded), "decoded_sha256": digest(decoded), "provenance": "synthetic authored compiler output" if sample.startswith("authored") else "compiled published OZ5.1.0 library source"})
            outlined = rows(measure(f"{sample}/{encoding}/outline", evm(path, encoding, "outline"), "evm_informational"))
            for selector, status in selectors.items():
                assert selector in oracle["method_identifiers"].values()
                unit = next(r for r in outlined if r["record"] == "function" and r["detail"]["selector"] == selector)
                argv = context(path, encoding, unit["id"])
                row = json.loads(invoke(argv)[1].stdout)
                original(row, path, sample, encoding)
                rec = row["detail"]["reconstruction"]
                assert rec["status"] == status and row["detail"]["complete"]
                if status == "raw_fallback":
                    assert rec["reason"] == "unsupported_opcode" and rec["text"] is None and not rec["complete"]
                elif selector == "188b85b4":
                    assert rec["control_flow_status"] == "structured_loops" and rec["structured_loop_count"] == 1
                elif selector == "06fdde03":
                    assert rec["control_flow_status"] == "contextual_gotos" and rec["complete"] and "goto" in rec["text"]
                else:
                    assert rec["complete"] and rec["control_flow_status"] == "structured_acyclic"
                selected = {"id": unit["id"], "selector": selector, "entry": row["detail"]["decoded_entry"], "decoded_bytes": sum(s["decoded_end"]-s["decoded_start"] for s in row["detail"]["segments"]), "physical_instructions": row["detail"]["instruction_count"], "contexts": row["detail"]["context_count"], "reconstruction": {k: v for k, v in rec.items() if k != "text"}}
                measure(f"{sample}/{encoding}/{selector}/default", argv, "evm_informational", selected)
                raw = json.loads(measure(f"{sample}/{encoding}/{selector}/raw", context(path, encoding, unit["id"], True), "evm_informational", selected))
                assert raw["detail"]["instructions"] == row["detail"]["instructions"] and raw["detail"]["reconstruction"]["status"] == "not_requested"
            if sample.startswith("authored"):
                query = ["--kind", "text", "--query", "RXD_DATA_NAVIGATION"]
                found = rows(measure(f"{sample}/{encoding}/search-text", evm(path, encoding, "strings", *query), "evm_informational"))
                kind = "data_literal" if sample.endswith("legacy") else "literal"
                hit = next(r for r in found if r["record"] == kind)
                refs = rows(measure(f"{sample}/{encoding}/refs-text", evm(path, encoding, "refs", "--id", hit["id"]), "evm_informational"))
                reference = next(r for r in refs if r["record"] == ("data_reference" if kind == "data_literal" else "literal_reference"))
                if kind == "data_literal":
                    assert hit["detail"]["decoded_start"] == 492 and hit["detail"]["decoded_end"] == 592
                    assert reference["detail"]["instruction_offset"] == 397 and reference["detail"]["length_hex"] == f"{100:064x}"
                else:
                    assert hit["detail"]["instruction_offset"] == 213
                target = next(r for r in outlined if r["record"] == "function" and r["detail"]["selector"] == "1ba64846")
                measure(f"{sample}/{encoding}/refs-code", evm(path, encoding, "refs", "--id", target["id"]), "evm_informational")
                for query_kind, needle in [("number", "7"), ("bytes", "07")]:
                    probe = rows(measure(f"{sample}/{encoding}/search-{query_kind}", evm(path, encoding, "strings", "--kind", query_kind, "--query", needle), "evm_informational"))
                    occurrence = next(r for r in probe if r["record"] == "literal")
                    measure(f"{sample}/{encoding}/refs-{query_kind}", evm(path, encoding, "refs", "--id", occurrence["id"]), "evm_informational")
                for flow_kind in ["outline", "search", "search_number", "search_bytes", "code_refs"]:
                    durations, command_sums, evidence = [], [], None
                    for _ in range(args.samples):
                        start = time.perf_counter_ns()
                        process_ns = 0
                        if flow_kind == "outline":
                            ns, res = invoke(evm(path, encoding, "outline")); process_ns += ns
                            route = next(r for r in rows(res.stdout) if r["record"] == "function" and r["detail"]["selector"] == "1ba64846")["id"]
                        elif flow_kind.startswith("search"):
                            flow_query = query if flow_kind == "search" else ["--kind", "number" if flow_kind == "search_number" else "bytes", "--query", "7" if flow_kind == "search_number" else "07"]
                            ns, res = invoke(evm(path, encoding, "strings", *flow_query)); process_ns += ns
                            literal = next(r for r in rows(res.stdout) if r["record"] == (kind if flow_kind == "search" else "literal"))
                            ns, res = invoke(evm(path, encoding, "refs", "--id", literal["id"])); process_ns += ns
                            route = next(r for r in rows(res.stdout) if r["record"] in ["literal_reference", "data_reference"])["context_id"]
                        else:
                            ns, res = invoke(evm(path, encoding, "outline")); process_ns += ns
                            unit = next(r for r in rows(res.stdout) if r["record"] == "function" and r["detail"]["selector"] == "1ba64846")
                            block = next(r for r in unit["detail"]["blocks"] if r["detail"]["shared"])
                            assert decoded[block["detail"]["decoded_start"]] == 0x5b
                            ns, res = invoke(evm(path, encoding, "refs", "--id", block["id"])); process_ns += ns
                            route = next(r for r in rows(res.stdout) if r["record"] == "code_reference" and r["detail"]["context_kind"] == "function")["context_id"]
                        ns, fetched = invoke(context(path, encoding, route)); process_ns += ns
                        row = json.loads(fetched.stdout); original(row, path, sample, encoding)
                        assert row["detail"]["reconstruction"]["status"] == "success"
                        durations.append(time.perf_counter_ns()-start); command_sums.append(process_ns)
                        evidence = {"context_id": route, "fetch_sha256": digest(fetched.stdout), "fetch_bytes": len(fetched.stdout), "status": row["detail"]["reconstruction"]["status"]}
                    report["flows"].append({"sample": sample, "encoding": encoding, "kind": flow_kind, "fresh_processes_per_flow": 2 if flow_kind == "outline" else 3, "includes_route_parsing_and_evidence_validation": True, "command_sum_ns": command_sums, "last_evidence": evidence, **statistics_for(durations)})
            for verb in ["analyze", "interface", "storage", "disassemble", "basic-blocks", "cfg", "metadata"]:
                argv = evm(path, encoding, verb)
                _, final = invoke(argv)
                _, old = invoke([str(baseline), *argv[1:]])
                assert (final.returncode, final.stdout, final.stderr) == (old.returncode, old.stdout, old.stderr), (sample, encoding, verb)
                report["native_regression"].append({"sample": sample, "encoding": encoding, "command": verb, "stdout_bytes": len(final.stdout), "stdout_sha256": digest(final.stdout), "equal_exit_stdout_stderr": True})

    # Scale contrast: add unrelated authored instruction regions, keep selected body/counters identical.
    original_path = CORPUS / "authored-navigation.runtime.bin"
    padded = output / "authored-unrelated-padding.bin"
    padded.write_bytes(original_path.read_bytes() + b"\0"*1024 + bytes.fromhex("5b60aa5f5500")*4000)
    scale = []
    for path in [original_path, padded]:
        outline = rows(invoke(evm(path, "raw", "outline"))[1].stdout)
        unit = next(r for r in outline if r["record"] == "function" and r["detail"]["selector"] == "1ba64846")
        data = measure(f"scale/{path.name}/addSeven", context(path, "raw", unit["id"]), "evm_informational")
        row = json.loads(data)
        _, prof = invoke(context(path, "raw", unit["id"]) + ["--perf"])
        analysis = next(json.loads(l) for l in prof.stderr.splitlines() if json.loads(l)["scope"] == "analysis")
        scale.append({"input_bytes": path.stat().st_size, "input_sha256": digest(path.read_bytes()), "instructions": row["detail"]["instructions"], "reconstruction": row["detail"]["reconstruction"], "counters": analysis["counters"], "output_bytes": len(data)})
    assert scale[0]["instructions"] == scale[1]["instructions"] and scale[0]["reconstruction"] == scale[1]["reconstruction"]
    for key in ["reconstructed_instructions", "reconstructed_blocks", "reconstructed_contexts", "reconstruction_values", "reconstruction_work", "structured_contexts", "structured_edges", "structuring_work"]:
        assert scale[0]["counters"][key] == scale[1]["counters"][key]
    report["selected_scale_control"] = scale

    # Existing production/stdlib controls, sampled separately for baseline and final binaries.
    js = ROOT / "tests/corpus/artifacts/lodash-4.18.1-production.js"
    jar = ROOT / "tests/corpus/jvm/gson-2.11.0.jar"
    pyc = ROOT / "tests/corpus/pyc/matrix/3.13.2/email-policy.hash-checked.pyc"
    controls = []
    jrows = rows(invoke([str(binary), "js", "find", str(js), "--string", "Expected a function"])[1].stdout)
    js_id = next(r for r in jrows if r["record"] == "match")["context_id"]
    controls.extend([("js/find", ["js", "find", str(js), "--string", "Expected a function"]), ("js/get", ["js", "get", str(js), "--id", js_id])])
    jrows = rows(invoke([str(binary), "jar", "outline", str(jar), "--class", "com/google/gson/GsonBuilder", "--method", "serializeNulls"])[1].stdout)
    method_id = next(r for r in jrows if r["record"] == "method")["id"]
    controls.extend([("jvm/outline", ["jar", "outline", str(jar), "--class", "com/google/gson/GsonBuilder", "--method", "serializeNulls"]), ("jvm/get-method", ["jar", "get-method", str(jar), method_id])])
    prows = rows(invoke([str(binary), "pyc", "outline", str(pyc)])[1].stdout)
    small = min((r for r in prows if r["record"] == "code"), key=lambda r: r["detail"]["bytecode_bytes"])
    controls.extend([("pyc/outline", ["pyc", "outline", str(pyc)]), ("pyc/get-code", ["pyc", "get-code", str(pyc), small["id"]])])
    for path in [js, jar, pyc]:
        report["inputs"].append({"path": str(path), "bytes": path.stat().st_size, "sha256": digest(path.read_bytes()), "provenance": "pinned published library" if path != pyc else "actual final CPython3.13.2 stdlib"})
    for name, tail in controls:
        old = measure(f"baseline/{name}", [str(baseline), *tail], "ordinary_backend_control")
        new = measure(f"final/{name}", [str(binary), *tail], "ordinary_backend_control")
        assert old == new, (name, "baseline result changed")
    isolated = output / "isolated-single-executable"
    isolated.mkdir(exist_ok=True)
    moved = isolated / "rxd"
    shutil.copy2(binary, moved)
    argv = [str(moved), "evm", "get-function", str(CORPUS / "authored-navigation.runtime.bin"), "--encoding", "raw", "--selector", "188b85b4"]
    _, relocation = invoke(argv, cwd=isolated)
    assert json.loads(relocation.stdout)["detail"]["reconstruction"]["control_flow_status"] == "structured_loops"
    report["single_executable"] = {"copied_binary_sha256": digest(moved.read_bytes()), "argv": argv, "cwd": str(isolated), "success": True, "linkage": command(["otool", "-L", str(moved)]) if platform.system() == "Darwin" else None}
    report["load_end"] = os.getloadavg()
    assert report["binary_sha256"] == digest(binary.read_bytes()), "measured binary changed"
    (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Saved {len(report['workloads'])} workloads, {len(report['flows'])} flows and {len(report['native_regression'])} native comparisons to {output}/results.json", flush=True)


if __name__ == "__main__":
    main()
