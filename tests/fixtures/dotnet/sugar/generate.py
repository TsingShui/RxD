#!/usr/bin/env python3
"""Explicit SDK-only regeneration; ordinary Rust tests never invoke this script."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[3]
SDK_ROOT = Path.home() / ".dotnet10"
SDK_VERSION = "10.0.401"
ENV = dict(os.environ, DOTNET_ROOT=str(SDK_ROOT),
           PATH=str(SDK_ROOT) + os.pathsep + os.environ.get("PATH", ""),
           DOTNET_CLI_TELEMETRY_OPTOUT="1", DOTNET_NOLOGO="1")
BUILD = REPO / "target" / "dotnet-sugar"


def dotnet(*args, capture=False):
    result = subprocess.run([str(SDK_ROOT / "dotnet"), *map(str, args)],
                            cwd=ROOT, env=ENV, check=True, text=True,
                            stdout=subprocess.PIPE if capture else None)
    return result.stdout.strip() if capture else None


def build(project):
    output = BUILD / project
    dotnet("build", ROOT / f"{project}.csproj", "-c", "Release",
           f"-p:BaseIntermediateOutputPath={output / 'obj'}/",
           f"-p:OutputPath={output / 'bin'}/", "-p:AppendTargetFrameworkToOutputPath=false",
           f"-p:PathMap={ROOT}=/_/sugar", "-p:NuGetAudit=false",
           "--configfile", ROOT / "NuGet.Config")
    return output / "bin" / f"{project}.dll"


if dotnet("--version", capture=True) != SDK_VERSION:
    raise SystemExit(f"Regeneration requires isolated SDK {SDK_VERSION} at {SDK_ROOT}")
compiler = SDK_ROOT / "sdk" / SDK_VERSION / "Roslyn" / "bincore" / "csc.dll"
compiler_version = dotnet(compiler, "-version", capture=True)
fixture = build("Sugar")
inspector = build("Inspect")
expected = json.loads(dotnet("--fx-version", "10.0.12", inspector, fixture, capture=True))
assets = json.loads((BUILD / "Sugar" / "obj" / "project.assets.json").read_text())
assert not assets["libraries"], "Fixture must not acquire NuGet dependencies"
bundled = ET.parse(SDK_ROOT / "sdk" / SDK_VERSION / "Microsoft.NETCoreSdk.BundledVersions.props")
framework = next(item for item in bundled.iter("KnownFrameworkReference")
                 if item.get("Include") == "Microsoft.NETCore.App"
                 and item.get("TargetFramework") == "net10.0")
expected["provenance"] = {
    "sdk": SDK_VERSION,
    "compiler": compiler_version,
    "compiler_sha256": hashlib.sha256(compiler.read_bytes()).hexdigest(),
    "framework_packages": [{"name": framework.get("TargetingPackName"),
                            "version": framework.get("TargetingPackVersion")}],
    "expectation_reader_runtime": "Microsoft.NETCore.App 10.0.12",
    "package_references": [],
    "source_sha256": hashlib.sha256((ROOT / "Sugar.cs").read_bytes()).hexdigest(),
    "expectation_reader": "System.Reflection.Metadata / System.Reflection.PortableExecutable (SDK framework)",
}
shutil.copyfile(fixture, ROOT / "Sugar.dll")
(ROOT / "expected.json").write_text(json.dumps(expected, indent=2) + "\n")
print(expected["sha256"], expected["bytes"], expected["mvid"], expected["pe_timestamp"])
