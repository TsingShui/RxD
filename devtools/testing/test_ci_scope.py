"""Offline CLI contracts for impact selection; no Cargo/build invocation."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/ci-scope.py"


class ScopeCli(unittest.TestCase):
    def setUp(self):
        scratch = ROOT / "target/testing"
        scratch.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "repository"
        self.root.mkdir()
        self.env = dict(os.environ, GIT_AUTHOR_NAME="Fixture", GIT_COMMITTER_NAME="Fixture",
                        GIT_AUTHOR_EMAIL="fixture@example.invalid", GIT_COMMITTER_EMAIL="fixture@example.invalid")
        self.git("init", "-q", "--initial-branch=main")
        self.put("README.md")
        self.put("Cargo.lock")
        self.base = self.snapshot()

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root, env=self.env, text=True).strip()

    def put(self, name, data="authored fixture"):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(data)

    def snapshot(self):
        self.git("add", ".")
        commit = self.git("commit-tree", self.git("write-tree"), "-m", "fixture")
        self.git("update-ref", "HEAD", commit)
        return commit

    def discover(self, *args):
        output = Path(self.temp.name) / "github-output"
        result = subprocess.run([
            sys.executable, str(SCRIPT), "--root", str(self.root), "--base", self.base,
            "--github-output", str(output), *args,
        ], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout), output.read_text()

    def scopes(self, report):
        return [row["scope"] for row in report["matrix"]["include"]]

    def test_docs_and_tooling_never_trigger_product_builds(self):
        self.put(".agents/skills/example/SKILL.md")
        self.put("AGENTS.md")
        report, outputs = self.discover()
        self.assertFalse(report["rust"])
        self.assertFalse(report["full"])
        self.assertIn("rust=false\n", outputs)
        self.put("scripts/check-upstream.py")
        report, _ = self.discover()
        self.assertTrue(report["tools"])
        self.assertFalse(report["rust"])
        self.assertFalse(report["full"])
        # Markdown embedded in the executable is not just project documentation.
        self.put("src/skills/rxd/SKILL.md")
        report, _ = self.discover()
        self.assertEqual(self.scopes(report), ["shared"])
        report, _ = self.discover("--full", "--head", self.base)
        self.assertTrue(report["full"])
        self.assertTrue(report["tools"])
        self.assertTrue(report["rust"])
        self.assertEqual(self.scopes(report), ["shared"])

    def test_backend_forks_and_unknown_paths_widen_scope(self):
        self.put("crates/rxd-dotnet/src/metadata.rs")
        report, _ = self.discover()
        self.assertEqual(self.scopes(report), ["dotnet"])
        self.put("third_party/radguard/src/render.rs")
        report, _ = self.discover()
        self.assertEqual(self.scopes(report), ["dex", "dotnet", "jvm"])
        self.put("src/cli/dotnet/mod.rs")
        self.put("new-input.unknown")
        report, _ = self.discover()
        self.assertEqual(self.scopes(report), ["dex", "dotnet", "jvm", "shared"])
        self.assertFalse(report["full"])
        self.put("crates/rxd-dotnet/native/Exports.cs")
        report, _ = self.discover()
        self.assertTrue(report["full"])
        self.put("prebuilt/android-ilspy/aarch64-linux-android/manifest.json")
        report, _ = self.discover()
        self.assertTrue(report["full"])
        self.assertTrue(report["tools"])
        report, _ = self.discover("--full")
        self.assertEqual(self.scopes(report), ["dex", "dotnet", "jvm", "shared"])

    def test_archived_arxd_has_no_product_checks(self):
        self.put("apps/arxd/README.md")
        self.put("apps/arxd/AGENTS.md")
        self.put("third_party/termux-exec/ExecIntercept.c")
        self.put("apps/arxd/src/agent/poc.ts")
        report, outputs = self.discover()
        self.assertFalse(report["rust"])
        self.assertFalse(report["full"])
        self.assertFalse(report["tools"])
        self.assertNotIn("apps/arxd/README.md", [row["scope"] for row in report["matrix"]["include"]])
        self.assertTrue(all("archived Arxd" in value for value in report["reasons"].values()))
        self.assertNotIn("arxd=", outputs)

    def test_deletions_renames_and_explicit_head_do_not_lose_inputs(self):
        (self.root / "docs").mkdir(exist_ok=True)
        self.git("mv", "Cargo.lock", "docs/moved-lock.md")
        self.put("tests/wasm_wabt/example.rs")
        head = self.snapshot()
        self.put("src/mcp/new.rs")
        report, _ = self.discover("--head", head)
        self.assertTrue(report["full"])
        self.assertIn("Cargo.lock", report["paths"])
        self.assertNotIn("src/mcp/new.rs", report["paths"])
        self.assertEqual(self.scopes(report), ["wasm"])
        report, _ = self.discover()
        self.assertIn("src/mcp/new.rs", report["paths"])
        result = subprocess.run([
            sys.executable, str(SCRIPT), "--root", str(self.root), "--base", "missing-revision",
        ], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
