#!/usr/bin/env python3
"""Fresh-process shipped NativeAOT C# acceptance; never execute analyzed inputs."""
import argparse
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
REFERENCE_COMMIT = "3028b1ab92ec1b0ad15f046b3b70d5ee82336a00"
VERSION = "11.1.0.9782"
PACKAGE_SHA256 = "1f76df4d35193ba4eeb50ffcd84eae381a2c48c67f95c1104dc406759448a811"
COMMIT = "a6909b2e1062d2404a52711926fa74030bd96119"
SUGAR = {
    "AsyncAwait": ["async ", "await "], "Iterator": ["yield return "],
    "ClosureAndLocalFunction": ["=>", "AddSeed("], "Linq": ["from ", "where ", "select "],
    "Initializers": ["Value ="], "Using": ["using "], "Lock": ["lock ("],
    "NullPropagation": ["?."], "Interpolation": ['$"'],
}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def native_identity(library):
    return {"package_version": VERSION, "package_sha256": PACKAGE_SHA256,
            "source_commit": COMMIT, "path": str(library.resolve()),
            "dylib_sha256": digest(library), "dylib_bytes": library.stat().st_size,
            "framework_reference_directories": os.environ.get("RXD_REFDIR", "").split(os.pathsep)}


def invoke(binary, args):
    start = time.perf_counter_ns()
    result = subprocess.run([str(binary), *map(str, args)], capture_output=True, timeout=30)
    elapsed = (time.perf_counter_ns() - start) / 1_000_000
    if result.returncode:
        raise RuntimeError(f"{args}: {result.stderr.decode(errors='replace')}")
    return result, elapsed


def method_id(binary, file, type_name, name, parameters=None):
    result, _ = invoke(binary, ["dotnet", "classes", file, "--class", type_name, "--method", name])
    records = [json.loads(line) for line in result.stdout.splitlines()]
    candidates = [row for row in records if row.get("record") == "method"]
    if parameters is not None:
        candidates = [row for row in candidates if row["detail"]["parameters"] == parameters]
    if len(candidates) != 1:
        raise RuntimeError(f"expected one selected method: {file}/{type_name}/{name}")
    return candidates[0]["id"]


def metrics(samples):
    return {"samples_ms": samples, "median_ms": statistics.median(samples),
            "p95_ms": sorted(samples)[math.ceil(.95 * len(samples)) - 1],
            "max_ms": max(samples), "over_150_count": sum(value > 150 for value in samples),
            "over_150_proportion": sum(value > 150 for value in samples) / len(samples)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--native-library", type=Path, default=Path(os.environ.get(
        "RXD_DOTNET_NATIVE_LIBRARY", ROOT / "target/dotnet-native/osx-arm64/librxd_dotnet_native.dylib")))
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--case", action="append", help="Measure only named workloads; report the selection explicitly")
    parser.add_argument("--references", type=Path, default=ROOT / "target/dotnet-research/references/dotscope")
    parser.add_argument("--framework", type=Path, default=Path.home() / ".dotnet10/shared/Microsoft.NETCore.App/10.0.12")
    parser.add_argument("--output", type=Path, default=ROOT / "target/benchmarks/dotnet")
    parser.add_argument("--cpu-model", required=True)
    parser.add_argument("--memory-gb", type=int, required=True)
    args = parser.parse_args()
    if args.samples < 20:
        parser.error("at least 20 samples are required")
    if not args.native_library.is_file():
        parser.error("build the shipped engine first: benches/ilspy/build-native.sh osx-arm64")
    os.environ["RXD_DOTNET_NATIVE_LIBRARY"] = str(args.native_library.resolve())
    if args.binary is None:
        subprocess.run(["cargo", "build", "--release", "--locked", "--features", "csharp-decompiler"], cwd=ROOT, check=True)
    binary = (args.binary or ROOT / "target/release/rxd").resolve()
    production = args.references / "dotscope/tests/samples"
    corelib = production / "dotnet_10.0/System.Private.CoreLib.dll"
    windowsbase = production / "WindowsBase.dll"
    linq = args.framework / "System.Linq.dll"
    textjson = args.framework / "System.Text.Json.dll"
    pinned_hashes = {
        windowsbase: "d6712d9472c4e92cfa847d0ba64671deb41d92357d41320d9d6dd88bec960e6e",
        corelib: "a6acd5d87523255556bc17d964779aae71da1809623710aa4c6d091061b3246a",
        linq: "8a868d709f942f2d8de60bac937027ed85b67b0596776f83d03088695ef86204",
        textjson: "5e8419f146b0fd12343fdd2d203fa3362b637b62b69a5a6d0acb6e065797382b",
    }
    for path, expected in pinned_hashes.items():
        if not path.is_file() or digest(path) != expected:
            parser.error(f"missing or mismatched pinned corpus: {path}; see tests/corpus/dotnet/README.md")
    args.output.mkdir(parents=True, exist_ok=True)
    scope = json.loads((ROOT / "tests/corpus/dotnet/corelib-performance-scope.json").read_text())
    workloads = []

    def get(name, file, owner, method, parameters=None, synthetic=False, evidence=()):
        identity = method_id(binary, file, owner, method, parameters)
        workloads.append((name, file, ["dotnet", "get-method", file, identity], synthetic, True, evidence))

    sugar = ROOT / "tests/fixtures/dotnet/sugar/Sugar.dll"
    for method, evidence in SUGAR.items():
        get(f"sugar_{method}", sugar, "Rxd.Sugar.Probe", method, synthetic=True, evidence=evidence)
    for name, file, owner, method, parameters in [
        ("corelib_get_Abs_int32", corelib, "System.Math", "Abs", ["int32"]),
        ("linq_get_Count", linq, "System.Linq.Enumerable", "Count", ["genericinst(class(0x01000011)<!!0>)"]),
        ("textjson_get_GetString", textjson, "System.Text.Json.JsonElement", "GetString", []),
    ]:
        get(name, file, owner, method, parameters)
    # WindowsBase needs the non-framework Accessibility sibling; correct scoped
    # degradation is correctness evidence only, never C# latency acceptance.
    for owner, method in [("System.Windows.Point", "get_X"),
                          ("System.Windows.Threading.Dispatcher", "CheckAccess")]:
        identity = method_id(binary, windowsbase, owner, method, [])
        workloads.append((f"windowsbase_scoped_degrade_{method}", windowsbase,
                          ["dotnet", "get-method", windowsbase, identity], False, True,
                          ["cross_assembly_reference:Accessibility"]))
    workloads += [
        ("windowsbase_outline", windowsbase, ["dotnet", "classes", windowsbase, "--class", "System.Windows.Point"], False, False, ()),
        ("corelib_outline", corelib, ["dotnet", "classes", corelib, "--class", "System.Math"], False, False, ()),
        ("windowsbase_strings", windowsbase, ["dotnet", "strings", windowsbase, "--contains", "Dispatcher"], False, False, ()),
        ("fixture_exact_refs", ROOT / "tests/fixtures/dotnet/Fixture.dll",
         ["dotnet", "refs", ROOT / "tests/fixtures/dotnet/Fixture.dll", "--token", "0x06000006"], True, False, ()),
    ]
    identity = method_id(binary, corelib, "System.Math", "Abs", ["int32"])
    workloads += [
        ("corelib_get_il", corelib, ["dotnet", "get-method", corelib, identity, "--il"], False, False, ()),
        ("corelib_get_file", corelib, ["dotnet", "get-method", corelib, identity, "-o", args.output / "method.json"], False, True, ()),
    ]
    # Explicit unaffected-backend checks, not pooled into .NET latency statistics.
    js_case = json.loads((ROOT / "tests/corpus/manifest.json").read_text())["cases"][0]
    js = ROOT / "tests/corpus" / js_case["file"]
    if digest(js) != js_case["sha256"]:
        raise RuntimeError("JS corpus hash mismatch")
    from asar import pack
    archive = args.output / "production-js.asar"
    pack(archive, {"main.js": js.read_bytes()})
    jar = ROOT / "tests/corpus/jvm/gson-2.11.0.jar"
    workloads += [
        ("unaffected_js_entities", js, ["js", "entities", js], False, False, ()),
        ("unaffected_asar_entries", archive, ["asar", "entries", archive], True, False, ()),
        ("unaffected_jar_outline", jar, ["jar", "methods", jar, "--class", "com/google/gson/GsonBuilder"], False, False, ()),
    ]
    if args.case:
        available = {case[0] for case in workloads}
        if set(args.case) - available:
            parser.error(f"unknown workloads: {sorted(set(args.case) - available)}")
        workloads = [case for case in workloads if case[0] in args.case]
    report = {
        "reference_commit": REFERENCE_COMMIT,
        "engine": native_identity(args.native_library),
        "corpus_provenance": "Pinned upstream runtime assemblies and Microsoft.NETCore.App/10.0.12 osx-arm64 SDK runtime artifacts; not broad independent production acceptance. ASAR is a synthetic wrapper of pinned production JS; sugar is synthetic.",
        "hardware": {"cpu": args.cpu_model, "memory_gb": args.memory_gb, "platform": platform.platform(), "machine": platform.machine()},
        "binary_sha256": digest(binary), "binary_bytes": binary.stat().st_size,
        "cache": "warm OS cache; prepared IDs and one initial query per workload; fresh CLI/worker per sample; no cache clearing",
        "measurement_conditions": "No CPU isolation; wall time includes startup, worker initialization, all reads, analysis, dlopen/NativeAOT initialization, serialization and pipe/atomic file writes. Output validation is outside timed region.",
        "requested_cases": args.case, "workloads": [], "acceptance": "pending",
        "limitations": ["C# rendering and lexical recovery evidence are not semantic equivalence or recompilation proofs", "Coverage survey failures cannot count as latency successes", "Exact-reference scan timing uses the labelled handcrafted fixture; runtime-wide default-budget scan failures remain outside these selected latency workloads", "DEX functional non-regression is covered by CLI E2E; latency not measured here (no authorized representative input)"],
    }
    report_path = args.output / "results.json"
    for name, file, command, synthetic, csharp, evidence in workloads:
        destination = Path(command[command.index("-o") + 1]) if "-o" in command else None
        initial, initial_ms = invoke(binary, command)
        data = destination.read_bytes() if destination else initial.stdout
        item = {"name": name, "command": list(map(str, command)), "synthetic": synthetic,
                "input_bytes": file.stat().st_size, "input_sha256": digest(file),
                "output_bytes": len(data), "initial_ms": initial_ms}
        scoped_degrade = name.startswith("windowsbase_scoped_degrade_")
        item["acceptance_role"] = "scoped_degrade_correctness" if scoped_degrade else "latency"
        if command[0] in ("dotnet", "js", "asar", "jar"):
            rows = [json.loads(line) for line in data.splitlines()]
            if not (rows[0]["detail"].get("complete") if command[:2] == ["dotnet", "get-method"] else rows[-1].get("complete")):
                raise RuntimeError(f"incomplete workload {name}")
            if csharp:
                row = rows[0]["detail"]
                recovered = (row.get("representation") == "csharp" and row.get("decompiled") is True
                             and bool(row.get("code")) and row.get("reason") is None
                             and all(text in row["code"] for text in evidence))
                if scoped_degrade:
                    recovered = (row["representation"] == "il" and row["decompiled"] is False
                                 and row["code"] is None and row["reason"] == evidence[0])
                item.update(representation=row["representation"], decompiled=row["decompiled"],
                            reason=row["reason"], code=row["code"], recovery_evidence=list(evidence),
                            recovery_passed=None if scoped_degrade else recovered,
                            scoped_degrade_passed=recovered if scoped_degrade else None)
                if not recovered:
                    item["latency_success"] = False
                    report["workloads"].append(item)
                    report["acceptance"] = "failed_recovery"
                    report_path.write_text(json.dumps(report, indent=2) + "\n")
                    raise SystemExit(f"STOP: {name} did not recover C#; not latency evidence")
                raw, _ = invoke(binary, ["dotnet", "get-method", file, command[3], "--il"])
                raw = json.loads(raw.stdout)
                if rows[0]["source_id"] != raw["source_id"]:
                    raise RuntimeError(f"original source_id mismatch: {name}")
                raw = raw["detail"]
                for field in ["il_hex", "instructions", "handlers"]:
                    if row[field] != raw[field]:
                        raise RuntimeError(f"original {field} mismatch: {name}")
        timings = []
        for _ in range(args.samples):
            result, elapsed = invoke(binary, command)
            actual = destination.read_bytes() if destination else result.stdout
            if actual != data:
                raise RuntimeError(f"nondeterministic output in {name}")
            timings.append(elapsed)
        item.update(metrics(timings))
        large_scope = (name in scope["workloads"]
                       and file.stat().st_size == scope["input"]["bytes"]
                       and digest(file) == scope["input"]["sha256"])
        bound = scope["upper_bound_ms"] if large_scope else 150
        item["latency_limit_ms"] = bound
        item["latency_scope"] = scope if large_scope else None
        item["over_limit_count"] = sum(value > bound for value in timings)
        item["over_limit_proportion"] = item["over_limit_count"] / len(timings)
        item["latency_success"] = None if scoped_degrade else not any(value > bound for value in timings)
        report["workloads"].append(item)
        print(f"{name}: n={len(timings)} median={item['median_ms']:.2f} P95={item['p95_ms']:.2f} >150ms={item['over_150_proportion']:.0%} bound={bound}ms", flush=True)
        report["acceptance"] = "failed_latency" if item["latency_success"] is False else "pending"
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        if item["latency_success"] is False:
            raise SystemExit(f"STOP: {name} exceeded {bound} ms; retained all samples in {report_path}")
    report["acceptance"] = "passed_selected_workloads"
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
