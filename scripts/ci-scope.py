#!/usr/bin/env python3
"""Conservative CI scope selection; unknown paths widen checks, never skip them."""

import argparse
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
BACKENDS = ("dotnet", "html", "jvm", "dex", "pyc", "evm", "wasm")
FULL_FILES = {
    "Cargo.toml", "Cargo.lock", "build.rs", ".gitmodules", "install.sh",
    ".github/workflows/build.yml", "scripts/build-bundled.py", "scripts/build-ilspy.py",
    "scripts/runtime_bundle.py", "scripts/package-release.py", "scripts/verify-release.py",
    "scripts/test-release.py", "scripts/test-install.py",
    "crates/rxd-dotnet/build.rs", "crates/rxd-dotnet/Cargo.toml",
}
TOOL_FILES = {"scripts/ci-scope.py", "scripts/check-upstream.py", "scripts/check-kuna-latest.py"}


def git(root, *args):
    return subprocess.check_output(["git", *args], cwd=root).decode().split("\0")[:-1]


def plan(paths):
    scopes, reasons = set(), {}
    full, tools = False, False
    for path in paths:
        if path.startswith(("apps/arxd/", "third_party/termux-exec/")):
            # Deleted archive paths still appear in comparisons across the removal.
            reasons[path] = "removed standalone Arxd archive; no product checks"
        elif path.startswith("site/") or path == ".gitignore" or path.startswith(".vscode/"):
            reasons[path] = "static site or local development configuration"
        elif path in FULL_FILES or path.startswith((
            ".cargo/", ".github/workflows/", "crates/rxd-dotnet/native/", "devtools/android-ilspy/",
            "crates/rxd-kuna-cli/", "third_party/kuna", "src/cli/kuna/",
            "src/query/kuna", "src/mcp/", "tests/mcp/", "tests/kuna/",
        )):
            full = True
            reasons[path] = "complete native/packaging validation"
            if path.startswith(".github/"):
                tools = True
        elif path in TOOL_FILES or path.startswith(("devtools/upstream/", "devtools/testing/")):
            tools = True
            reasons[path] = "maintenance tooling contracts"
        elif path.endswith(".md") and (
            path.startswith(("docs/", ".agents/", "benches/"))
            or path in ("AGENTS.md", "README.md", "GLOSSARY.md", "tests/AGENTS.md")
        ):
            reasons[path] = "documentation/development reference only"
        elif path.startswith("third_party/radguard/"):
            scopes.update(("dex", "jvm"))
            reasons[path] = "shared DEX/JVM rendering fork"
        elif path.startswith("third_party/axmldecoder/"):
            scopes.add("dex")
            reasons[path] = "DEX manifest decoder"
        elif path.startswith("third_party/pycdc/"):
            scopes.add("pyc")
            reasons[path] = "Python reconstruction fork"
        else:
            backend = next((name for name in BACKENDS if path.startswith((
                "crates/rxd-" + name + "/", "src/cli/" + name + "/",
                "tests/" + name + "/", "tests/corpus/" + name + "/",
            ))), None)
            if path.startswith(("tests/wasm_wabt/", "tests/corpus/wasm-wabt/")):
                backend = "wasm"
            if backend:
                scopes.add(backend)
                reasons[path] = "backend-scoped public package/CLI checks"
            else:
                # Unpack shares unnamespaced tests/cli.rs; do not guess filters.
                scopes.add("shared")
                reasons[path] = "shared, unnamespaced, or unknown input; conservative full default gates"
    return {
        "paths": paths, "reasons": reasons, "tools": tools, "full": full,
        "rust": bool(scopes),
        "matrix": {"include": [{"scope": scope} for scope in sorted(scopes)]},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True, help="explicit comparison revision")
    parser.add_argument("--head", help="committed comparison endpoint; otherwise include local changes")
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--full", action="store_true", help="force periodic/manual/release acceptance")
    args = parser.parse_args()
    try:
        # --no-renames includes both old and new paths, including deleted inputs.
        paths = git(args.root, "diff", "--name-only", "--no-renames", "-z", args.base,
                    *([args.head] if args.head else []), "--")
        if not args.head:
            paths += git(args.root, "ls-files", "--others", "--exclude-standard", "-z")
        report = plan(sorted(set(paths)))
        if args.full:
            scopes = {row["scope"] for row in report["matrix"]["include"]}
            scopes.add("shared")
            report.update(full=True, tools=True, rust=True,
                          matrix={"include": [{"scope": scope} for scope in sorted(scopes)]})
        if args.github_output:
            with args.github_output.open("a") as output:
                for key in ("tools", "rust", "full", "matrix"):
                    output.write(key + "=" + json.dumps(report[key], separators=(",", ":")) + "\n")
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
