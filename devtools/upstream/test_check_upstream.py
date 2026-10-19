"""Offline contracts through the real discovery CLI and local Git remotes."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/check-upstream.py"


class DiscoveryCli(unittest.TestCase):
    def setUp(self):
        scratch = ROOT / "target/upstream-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "rxd"
        self.remote = Path(self.temp.name) / "remote"
        for path in (self.root, self.remote):
            path.mkdir()
            self.git(path, "init", "-q", "--initial-branch=main")
            self.git(path, "config", "user.email", "fixture@example.invalid")
            self.git(path, "config", "user.name", "Offline Fixture")
        self.before = self.commit("before")
        self.git(self.remote, "tag", "v1.9")
        (self.root / "NOTICE").write_text(self.before + "\n")
        self.source = {
            "id": "engine", "relationship": "maintained-fork",
            "local_paths": ["vendor"], "evidence": "NOTICE",
            "url": str(self.remote), "pin": {"revision": self.before},
            "track": {"kind": "formal-tags", "prefix": "v"},
        }
        self.sources = [self.source]
        self.reviews = {}

    def git(self, cwd, *args):
        return subprocess.check_output(
            ["git", *args], cwd=cwd, text=True, stderr=subprocess.PIPE,
        ).strip()

    def snapshot(self, repository, message):
        """Construct fixture history, not developer commits or their hook lifecycle."""
        tree = self.git(repository, "write-tree")
        parent = self.git(repository, "for-each-ref", "--format=%(objectname)", "refs/heads/main")
        command = ["commit-tree", tree, "-m", message]
        if parent:
            command += ["-p", parent]
        commit = self.git(repository, *command)
        self.git(repository, "update-ref", "refs/heads/main", commit)
        return commit

    def commit(self, value):
        (self.remote / "source.txt").write_text(value)
        self.git(self.remote, "add", "source.txt")
        return self.snapshot(self.remote, value)

    def discover(self, *args):
        registry, reviews = self.root / "sources.json", self.root / "reviews.json"
        registry.write_text(json.dumps({"schema_version": 1, "sources": self.sources}))
        reviews.write_text(json.dumps({"schema_version": 1, "reviews": self.reviews}))
        command = [
            sys.executable, str(SCRIPT), "--root", str(self.root),
            "--registry", str(registry), "--reviews", str(reviews), *args,
        ]
        result = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertEqual(result.stderr, "")
        return result.returncode, json.loads(result.stdout)

    def test_numeric_formal_tags_peel_annotations_ignore_prereleases_and_do_not_mutate(self):
        code, report = self.discover()
        self.assertEqual(code, 0)
        self.assertEqual(report["sources"][0]["status"], "current")
        after = self.commit("release")
        self.git(self.remote, "tag", "-am", "formal release", "v1.10")
        self.commit("unreleased")
        self.git(self.remote, "tag", "v999.0-rc1")
        self.git(self.root, "add", ".")
        self.snapshot(self.root, "registry")
        local_state = self.git(self.root, "status", "--porcelain")
        remote_state = self.git(self.remote, "show-ref")
        code, report = self.discover()
        self.assertEqual(code, 1)
        row = report["sources"][0]
        self.assertEqual(row["latest"], {"ref": "refs/tags/v1.10", "revision": after})
        self.assertEqual(row["status"], "changed")
        self.assertEqual(self.git(self.root, "status", "--porcelain"), local_state)
        self.assertEqual(self.git(self.remote, "show-ref"), remote_state)

    def test_head_policy_tracks_head_not_release_tags(self):
        after = self.commit("head")
        self.sources.append(dict(self.source, id="release-only"))
        self.source["track"] = {"kind": "head"}
        code, report = self.discover()
        self.assertEqual(code, 1)
        self.assertEqual(report["sources"][0]["latest"], {"ref": "HEAD", "revision": after})
        self.assertEqual(report["sources"][1]["status"], "current")

    def test_index_gitlink_does_not_require_initialized_checkout_or_local_tags(self):
        self.source["pin"] = {"gitlink": "vendor"}
        (self.root / ".gitmodules").write_text(
            '[submodule "engine"]\n\tpath = vendor\n\turl = ' + str(self.remote) + "\n"
        )
        self.git(self.root, "update-index", "--add", "--cacheinfo", "160000," + self.before + ",vendor")
        self.snapshot(self.root, "old gitlink")
        after = self.commit("new release")
        self.git(self.remote, "tag", "v1.10")
        self.git(self.root, "update-index", "--cacheinfo", "160000," + after + ",vendor")
        code, report = self.discover()
        self.assertEqual(code, 0)
        self.assertEqual(report["sources"][0]["pin"], {"revision": after, "checkout": "not-initialized"})
        code, report = self.discover("--require-checkout")
        self.assertEqual(code, 2)
        self.assertIn("not initialized", report["sources"][0]["reason"])
        self.git(self.root, "clone", "-q", str(self.remote), "vendor")
        code, _ = self.discover("--require-checkout")
        self.assertEqual(code, 0)
        self.git(self.root, "update-index", "--cacheinfo", "160000," + self.before + ",vendor")
        code, _ = self.discover()
        self.assertEqual(code, 2)
        self.git(self.root, "update-index", "--cacheinfo", "160000," + after + ",vendor")
        (self.root / "vendor/source.txt").write_text("local tracked edit")
        code, _ = self.discover()
        self.assertEqual(code, 2)
        (self.root / ".gitmodules").write_text(
            '[submodule "engine"]\n\tpath = vendor\n\turl = incorrect-remote\n'
        )
        code, report = self.discover()
        self.assertEqual(code, 2)
        self.assertIn("registry URL differs", report["sources"][0]["reason"])

    def test_review_cursor_does_not_move_pin_or_hide_pending_actions(self):
        after = self.commit("review target")
        self.git(self.remote, "tag", "v1.10")
        (self.root / "review.md").write_text(self.before + "\n" + after)
        self.reviews["engine"] = {
            "baseline_revision": self.before, "revision": after,
            "report": "review.md", "followups": [],
        }
        code, report = self.discover()
        self.assertEqual(code, 0)
        row = report["sources"][0]
        self.assertEqual(row["status"], "reviewed")
        self.assertEqual(row["pin"]["revision"], self.before)
        code, _ = self.discover("--require-current")
        self.assertEqual(code, 1)
        self.reviews["engine"]["followups"] = ["adapt validation fix"]
        code, report = self.discover()
        self.assertEqual(code, 1)
        self.assertTrue(report["action_required"])
        self.commit("another release")
        self.git(self.remote, "tag", "v1.11")
        code, report = self.discover()
        self.assertEqual(code, 1)
        self.assertEqual(report["sources"][0]["status"], "changed")
        self.assertEqual(report["sources"][0]["followups"], ["adapt validation fix"])
        (self.root / "review.md").unlink()
        code, _ = self.discover()
        self.assertEqual(code, 2)

    def test_network_and_provenance_failures_preserve_other_actionable_rows(self):
        self.commit("release")
        self.git(self.remote, "tag", "v1.10")
        bad = dict(self.source, id="unreachable", url=str(self.root / "missing-remote"))
        blocked = dict(self.source, id="unknown", blocked="Upstream identity unknown", url=None)
        self.sources.extend([bad, blocked])
        code, report = self.discover()
        self.assertEqual(code, 2)
        self.assertFalse(report["complete"])
        self.assertTrue(report["action_required"])
        self.assertEqual([row["status"] for row in report["sources"]], ["changed", "error", "blocked"])
        code, report = self.discover("--source", "engine")
        self.assertEqual(code, 1)
        self.assertTrue(report["complete"])
        code, _ = self.discover("--source", "typo")
        self.assertEqual(code, 2)

    def test_malformed_review_ledger_is_json_failure_not_traceback_or_success(self):
        self.reviews = []
        code, report = self.discover()
        self.assertEqual(code, 2)
        self.assertFalse(report["complete"])
        self.assertIn("reviews must be an object", report["error"])
        self.reviews = {"engine": {}}
        code, report = self.discover()
        self.assertEqual(code, 2)
        self.assertEqual(report["sources"][0]["status"], "error")

    def test_missing_formal_tags_pinned_tags_and_stale_provenance_are_errors(self):
        self.git(self.remote, "tag", "-d", "v1.9")
        self.git(self.remote, "tag", "v2-preview")
        code, report = self.discover()
        self.assertEqual(code, 2)
        self.assertIn("no formal", report["sources"][0]["reason"])
        self.source["pin"] = {"tag": "v1.9"}
        (self.root / "NOTICE").write_text("v1.9")
        code, report = self.discover()
        self.assertEqual(code, 2)
        self.assertIn("pinned tag is missing", report["sources"][0]["reason"])
        self.source["pin"] = {"revision": self.before}
        code, report = self.discover()
        self.assertEqual(code, 2)
        self.assertIn("provenance", report["sources"][0]["reason"])


if __name__ == "__main__":
    unittest.main()
