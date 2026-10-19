#!/usr/bin/env python3
"""Deterministic shipped-native-engine C# survey; never execute inputs.

Coverage here means successful C# rendering, not equivalence to original C#.
Keep every selected method and failure in the report; do not sample by success.
"""
import argparse
import collections
import hashlib
import json
import os
import platform
import re
import subprocess
from pathlib import Path

from dotnet import REFERENCE_COMMIT, ROOT, invoke, native_identity

PINNED = {
    "WindowsBase.dll": "d6712d9472c4e92cfa847d0ba64671deb41d92357d41320d9d6dd88bec960e6e",
    "dotnet_10.0/System.Private.CoreLib.dll": "a6acd5d87523255556bc17d964779aae71da1809623710aa4c6d091061b3246a",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/rxd")
    parser.add_argument("--references", type=Path, default=ROOT / "target/dotnet-research/references/dotscope")
    parser.add_argument("--native-library", type=Path, default=Path(os.environ.get(
        "RXD_DOTNET_NATIVE_LIBRARY", ROOT / "target/dotnet-native/osx-arm64/librxd_dotnet_native.dylib")))
    parser.add_argument("--methods", type=int, default=48)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--cpu-model", required=True)
    parser.add_argument("--memory-gb", type=int, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "target/benchmarks/dotnet-coverage/results.json")
    args = parser.parse_args()
    if args.methods < 2:
        parser.error("--methods must be at least 2")
    binary = args.binary.resolve()
    if not args.native_library.is_file():
        parser.error("build the shipped engine first: benches/ilspy/build-native.sh osx-arm64")
    os.environ["RXD_DOTNET_NATIVE_LIBRARY"] = str(args.native_library.resolve())
    report = {
        "reference_commit": REFERENCE_COMMIT,
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "engine": native_identity(args.native_library),
        "hardware": {"cpu": args.cpu_model, "memory_gb": args.memory_gb,
                     "platform": platform.platform(), "machine": platform.machine()},
        "cache": "Warm OS cache after outline preparation; fresh CLI/worker per method; no cache clearing or CPU isolation",
        "selection": "uniform positions in MethodDef token order among methods with IL; includes first and last; no success-based selection",
        "limitations": ["C# rendering is not proof of semantic equivalence or sugar recovery",
                       "Pinned upstream artifacts; not broad independent production acceptance",
                       "Single invocation per method is a coverage survey, not latency acceptance"],
        "assemblies": [],
    }
    for relative, expected_hash in PINNED.items():
        file = args.references / "dotscope/tests/samples" / relative
        source_id = hashlib.sha256(file.read_bytes()).hexdigest()
        if source_id != expected_hash:
            raise RuntimeError(f"pinned corpus hash mismatch: {file}")
        output, _ = invoke(binary, ["dotnet", "outline", file])
        rows = [json.loads(line) for line in output.stdout.splitlines()]
        if not rows[-1].get("complete"):
            raise RuntimeError(f"incomplete outline: {file}")
        methods = [row for row in rows if row.get("record") == "method" and row["detail"]["has_body"]]
        count = min(args.methods, len(methods))
        selected = [methods[index * (len(methods) - 1) // (count - 1)] for index in range(count)] if count > 1 else methods
        outcomes = []
        for method in selected:
            result = subprocess.run([str(binary), "dotnet", "get-method", str(file), method["id"]], capture_output=True, timeout=30)
            outcome = {"method": method, "exit_code": result.returncode}
            if result.returncode:
                outcome.update(decompiled=False, reason=result.stderr.decode(errors="replace"), complete=False)
            else:
                row = json.loads(result.stdout)
                if row["source_id"] != source_id or not row["detail"]["complete"] or not row["detail"]["il_hex"] or not row["detail"]["instructions"]:
                    raise RuntimeError(f"identity/incomplete original IL: {method['id']}")
                if row["detail"]["reason"] == "engine_unavailable":
                    raise RuntimeError("coverage requires a feature-enabled binary and loadable native library")
                if row["detail"]["decompiled"]:
                    offsets = {item["offset"] for item in row["detail"]["instructions"]}
                    if row["detail"]["representation"] != "csharp" or not row["detail"]["code"] or row["detail"]["reason"] is not None:
                        raise RuntimeError(f"incomplete C# code: {method['id']}")
                    if any(site["il_offset"] not in offsets or not 1 <= site["line"] <= len(row["detail"]["code"].splitlines())
                           for site in row["detail"]["mapping"]):
                        raise RuntimeError(f"invalid source mapping: {method['id']}")
                outcome.update(decompiled=row["detail"]["decompiled"], representation=row["detail"]["representation"],
                               reason=row["detail"]["reason"], complete=row["detail"]["complete"], code=row["detail"]["code"], mapping=row["detail"]["mapping"],
                               instructions=len(row["detail"]["instructions"]), handlers=len(row["detail"]["handlers"]))
            outcomes.append(outcome)
        failures = collections.Counter(re.sub(r"^IL_[0-9a-f]+:", "", item["reason"] or "") for item in outcomes if not item["decompiled"])
        successes = sum(item["decompiled"] for item in outcomes)
        assembly = {"file": relative, "source_id": source_id, "input_bytes": file.stat().st_size,
                    "methods_with_il": len(methods), "selected": count, "csharp_rendered": successes,
                    "csharp_rendered_proportion": successes / count if count else 0, "failures": dict(failures), "outcomes": outcomes}
        report["assemblies"].append(assembly)
        print(f"{relative}: csharp={successes}/{count}; failures={dict(failures)}", flush=True)
    if args.baseline:
        baseline = json.loads(args.baseline.read_text())
        before = {item["method"]["id"]: item for assembly in baseline["assemblies"] for item in assembly["outcomes"]}
        after = {item["method"]["id"]: item for assembly in report["assemblies"] for item in assembly["outcomes"]}
        if before.keys() != after.keys():
            raise RuntimeError("baseline input/selection mismatch")
        report["comparison"] = {
            "baseline_binary_sha256": baseline["binary_sha256"],
            "newly_rendered": [identity for identity in before if not before[identity]["decompiled"] and after[identity]["decompiled"]],
            "regressions": [identity for identity in before if before[identity]["decompiled"] and not after[identity]["decompiled"]],
        }
        print(f"newly rendered={len(report['comparison']['newly_rendered'])}; regressions={len(report['comparison']['regressions'])}")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
