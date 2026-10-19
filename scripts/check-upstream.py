#!/usr/bin/env python3
"""Read-only upstream discovery. JSON stdout; 0=current, 1=action, 2=incomplete."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
SHA = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})\Z")


def capture(args, cwd, timeout):
    env = dict(os.environ, GIT_TERMINAL_PROMPT="0", GCM_INTERACTIVE="Never")
    result = subprocess.run(
        args, cwd=cwd, env=env, text=True, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, timeout=timeout, check=True,
    )
    return result.stdout.strip()


def revision(value):
    if not SHA.fullmatch(value):
        raise ValueError("expected a full Git object ID")
    return value


def local_pin(source, root, timeout, require_checkout):
    pin = source["pin"]
    if "gitlink" in pin:
        path = pin["gitlink"]
        config = capture([
            "git", "config", "--file", ".gitmodules", "--get-regexp",
            r"^submodule\..*\.(path|url)$",
        ], root, timeout)
        modules = dict(line.split(None, 1) for line in config.splitlines())
        urls = [
            modules.get(key[:-5] + ".url") for key, value in modules.items()
            if key.endswith(".path") and value == path
        ]
        if urls != [source["url"]]:
            raise ValueError("registry URL differs from .gitmodules or submodule path is ambiguous")
        row = capture(["git", "ls-files", "--stage", "--", path], root, timeout)
        fields = row.split()
        if len(fields) != 4 or fields[0] != "160000" or fields[2] != "0":
            raise ValueError("pin is not one unconflicted index gitlink")
        sha = revision(fields[1])
        checkout = root / path
        if (checkout / ".git").exists():
            head = capture(["git", "rev-parse", "HEAD"], checkout, timeout)
            dirty = capture(
                ["git", "status", "--porcelain", "--untracked-files=no"],
                checkout, timeout,
            )
            if head != sha or dirty:
                raise ValueError("submodule checkout differs from index gitlink or has tracked edits")
            state = "verified"
        else:
            if require_checkout:
                raise ValueError("submodule is not initialized")
            state = "not-initialized"
        return {"revision": sha, "checkout": state}
    evidence = (root / source["evidence"]).read_text()
    if "revision" in pin:
        sha = revision(pin["revision"])
        if sha not in evidence:
            raise ValueError("pin revision is absent from its provenance document")
        return {"revision": sha}
    if "tag" in pin:
        if pin["tag"] not in evidence:
            raise ValueError("pin tag is absent from its provenance document")
        return {"tag": pin["tag"]}
    raise ValueError("pin requires revision, tag, or gitlink")


def fetch_refs(url, patterns, root, timeout):
    try:
        # No --refs: annotated tags need their peeled ^{} commit, not the tag object.
        output = capture(["git", "ls-remote", url, *sorted(patterns)], root, timeout)
        refs = {}
        for row in output.splitlines():
            sha, name = row.split()
            refs[name] = revision(sha)
        return {"refs": refs}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        return {"error": error_message(error)}


def tag_revision(refs, tag):
    name = "refs/tags/" + tag
    if name not in refs:
        raise ValueError("pinned tag is missing from upstream: " + tag)
    return refs.get(name + "^{}", refs[name])


def latest(source, refs):
    policy = source["track"]
    if policy["kind"] == "head":
        if "HEAD" not in refs:
            raise ValueError("upstream did not advertise HEAD")
        return {"ref": "HEAD", "revision": refs["HEAD"]}
    prefix = policy["prefix"]
    candidates = []
    for name in refs:
        if not name.startswith("refs/tags/"):
            continue
        tag = name[len("refs/tags/"):]
        if re.fullmatch(re.escape(prefix) + r"[0-9]+(?:\.[0-9]+)*", tag):
            key = tuple(int(part) for part in tag[len(prefix):].split("."))
            candidates.append((key, tag))
    if not candidates:
        raise ValueError("upstream has no formal numeric version tags for this policy")
    _, tag = max(candidates)
    return {"ref": "refs/tags/" + tag, "revision": tag_revision(refs, tag)}


def error_message(error):
    if isinstance(error, subprocess.TimeoutExpired):
        return "git command timed out"
    if isinstance(error, subprocess.CalledProcessError):
        # Do not echo credentials or arbitrary remote diagnostics into reports.
        return "git command failed (exit %s); check connectivity, access and refs" % error.returncode
    return str(error)


def compare_url(url, before, after):
    if url.startswith("https://github.com/"):
        return url.removesuffix(".git") + "/compare/" + before + "..." + after
    return None


def check(args):
    root = args.root.resolve()
    registry = json.loads(args.registry.read_text())
    reviews = json.loads(args.reviews.read_text())
    if registry["schema_version"] != 1 or reviews["schema_version"] != 1:
        raise ValueError("unsupported registry/reviews schema")
    sources = registry["sources"]
    if not isinstance(sources, list) or not sources or not all(isinstance(s, dict) for s in sources):
        raise ValueError("sources must be a nonempty array of objects")
    if not isinstance(reviews["reviews"], dict):
        raise ValueError("reviews must be an object keyed by source ID")
    if set(reviews["reviews"]) - {s["id"] for s in sources}:
        raise ValueError("review ledger contains unknown source IDs")
    ids = [s["id"] for s in sources]
    if len(set(ids)) != len(ids):
        raise ValueError("duplicate source IDs")
    if set(args.source) - set(ids):
        raise ValueError("unknown source ID(s): " + ", ".join(sorted(set(args.source) - set(ids))))
    sources = [s for s in sources if not args.source or s["id"] in args.source]
    results, requests = [], {}
    for source in sources:
        result = {key: source[key] for key in ("id", "relationship", "local_paths", "evidence")}
        result.update(url=source.get("url"), track=source.get("track"), pin=source.get("pin"))
        results.append(result)
        if source.get("blocked"):
            result.update(status="blocked", reason=source["blocked"])
            continue
        try:
            if not isinstance(source["url"], str) or not source["url"]:
                raise ValueError("source URL must be a nonempty string")
            if source["track"]["kind"] not in ("head", "formal-tags"):
                raise ValueError("unknown tracking policy")
            result["pin"] = local_pin(source, root, args.timeout, args.require_checkout)
            patterns = requests.setdefault(source["url"], set())
            patterns.add("HEAD" if source["track"]["kind"] == "head" else "refs/tags/*")
            if "tag" in source["pin"]:
                patterns.add("refs/tags/*")
        except (OSError, KeyError, TypeError, ValueError, subprocess.SubprocessError) as error:
            result.update(status="error", reason=error_message(error))
    with ThreadPoolExecutor(max_workers=4) as pool:
        urls = list(requests)
        fetched = dict(zip(urls, pool.map(
            lambda url: fetch_refs(url, requests[url], root, args.timeout), urls,
        )))
    for source, result in zip(sources, results):
        if "status" in result:
            continue
        try:
            remote = fetched[source["url"]]
            if "error" in remote:
                raise ValueError(remote["error"])
            refs = remote["refs"]
            pin = result["pin"]
            if "tag" in pin:
                pin["revision"] = tag_revision(refs, pin["tag"])
            target = result["latest"] = latest(source, refs)
            before, after = pin["revision"], target["revision"]
            result["compare_url"] = compare_url(source["url"], before, after)
            review = reviews["reviews"].get(source["id"])
            result["followups"] = []
            if review is not None:
                revision(review["revision"])
                revision(review["baseline_revision"])
                report = (root / review["report"]).read_text()
                if review["revision"] not in report or review["baseline_revision"] not in report:
                    raise ValueError("review report does not contain both recorded revisions")
                if not isinstance(review["followups"], list):
                    raise ValueError("review followups must be a list")
                result["followups"] = review["followups"]
                result["review"] = review
            if before == after:
                result["status"] = "current"
            elif review and review["baseline_revision"] == before and review["revision"] == after:
                result["status"] = "reviewed"
            else:
                # A changed HEAD could be a rewrite, not a fast-forward. Discovery does not fetch history.
                result["status"] = "changed"
        except (OSError, KeyError, TypeError, ValueError) as error:
            result.update(status="error", reason=error_message(error))
    incomplete = any(r["status"] in ("blocked", "error") for r in results)
    action = any(
        r["status"] == "changed" or r.get("followups")
        or (args.require_current and r["status"] == "reviewed")
        for r in results
    )
    return {
        "schema_version": 1,
        "complete": not incomplete,
        "action_required": action,
        "sources": results,
    }, 2 if incomplete else (1 if action else 0)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", action="append", default=[], help="source ID (repeatable)")
    parser.add_argument("--root", type=Path, default=ROOT, help="repository root")
    parser.add_argument("--registry", type=Path, default=ROOT / "devtools/upstream/sources.json")
    parser.add_argument("--reviews", type=Path, default=ROOT / "devtools/upstream/reviews.json")
    parser.add_argument("--timeout", type=int, default=30, help="timeout per Git command, seconds")
    parser.add_argument("--require-checkout", action="store_true", help="require selected gitlinks to be initialized")
    parser.add_argument("--require-current", action="store_true", help="fail for reviewed but unadopted upstream revisions too")
    args = parser.parse_args(argv)
    try:
        if args.timeout <= 0:
            raise ValueError("timeout must be positive")
        report, code = check(args)
    except (OSError, KeyError, TypeError, ValueError, subprocess.SubprocessError) as error:
        report, code = {"schema_version": 1, "complete": False, "error": error_message(error)}, 2
    print(json.dumps(report, ensure_ascii=True, sort_keys=True, indent=2))
    return code


if __name__ == "__main__":
    sys.exit(main())
