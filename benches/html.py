#!/usr/bin/env python3
"""Fresh-process HTML lexical-container benchmarks.

Inputs are pinned real npm package pages when `tests/corpus/node_modules` is
installed (`npm ci` in `tests/corpus`, gitignored), plus handcrafted realistic
documents (labeled `authored`) and generated large/markup-dense pages (labeled
`synthetic`). There is no in-repo production HTML corpus and no network access:
when the pinned corpus is absent it is skipped with an explicit note. Nothing is
executed and no external resource is loaded.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import sys

import cli as base

ROOT = Path(__file__).resolve().parents[1]

SCRIPT = "const answer = 40 + 2; function run(v){return v;} export {answer, run};"

# Real published npm package pages shipped inside the gitignored E2E corpus. The
# version comes from tests/corpus/package-lock.json (fixed npm integrity); when the
# corpus is not installed these entries are skipped and reported as absent.
PINNED_PAGES = (
    ("tslib", "tslib/tslib.html"),
    ("terser", "terser/tools/props.html"),
    ("string-template", "string-template/coverage/index.html"),
    ("md5", "md5/demo/index.html"),
)


def locked_version(lock, package):
    entry = lock.get("packages", {}).get(f"node_modules/{package}")
    return entry.get("version") if entry else None


def write_text(path, text):
    path.write_text(text, encoding="utf-8")
    return text.encode("utf-8")


def document_page():
    return ("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Docs</title>"
            "<link rel=\"stylesheet\" href=\"/app.css\"></head>"
            "<body><nav><a href=\"/\">Home</a><a href=\"/docs\">Docs</a></nav>"
            "<main><h1>Guide</h1><p>Install with <code>rxd</code>.</p>"
            "<form action=\"/search\"><input name=\"q\" placeholder=\"query\"></form>"
            "<table><tr><td>a</td><td>b</td></tr></table></main>"
            "<footer><!--generated--></footer></body></html>")


def attribute_page():
    return ("<!DOCTYPE html><!--catalog--><DIV HREF='/a&amp;b' href=x disabled empty=''>"
            "<a href='/x&amp;y'>first</a><a href='/z'>second</a><BR>Hi</DIV>")


def script_page():
    return f'<!DOCTYPE html><script type="module">{SCRIPT}</script><p>after</p>'


def markup_dense_page(count=20000):
    parts = ["<!DOCTYPE html><body>"]
    for i in range(count):
        parts.append(f'<a href="/item/{i}" data-index="{i}">item {i}</a>')
    parts.append("</body>")
    return "".join(parts)


def deep_nested_page(depth=500):
    return "".join(["<div>"] * depth) + "x" + "".join(["</div>"] * depth)


def rows(payload):
    return [json.loads(line) for line in payload.splitlines()]


def validate_list(payload, source_id, expected_nodes=None):
    result = rows(payload)
    assert result[0]["record"] == "header" and result[0]["backend"] == "html"
    assert result[0]["command"] == "list" and result[0]["schema_version"] == 2
    assert result[0]["source_id"] == source_id
    assert result[0]["detail"]["analysis"] == "lexical"
    assert result[0]["detail"]["parser"] == "html5gum"
    assert result[0]["detail"]["parser_version"] == "0.8.4"
    assert result[0]["detail"]["lexical_profile"] == "lexical_naive_v1"
    assert result[-1]["record"] == "summary" and result[-1]["complete"] is True
    assert result[-1]["records"] == len(result) - 2
    if expected_nodes is not None:
        assert result[-1]["detail"]["nodes"] == expected_nodes
    for row in result[1:-1]:
        assert row["record"] in ("element", "text", "comment", "doctype")
        assert row["source_id"] == source_id
        assert row["id"] == (f"rxd1:html:{source_id}:{row['record']}:"
                             f"{row['start']}:{row['end']}")
    return result


def validate_receipt(payload, destination, selected, container, node_id):
    receipt = json.loads(payload)
    assert receipt["record"] == "file" and receipt["backend"] == "html"
    assert receipt["command"] == "extract" and receipt["schema_version"] == 2
    assert receipt["detail"]["complete"] is True
    assert receipt["source_id"] == hashlib.sha256(selected).hexdigest()
    assert receipt["detail"]["file_bytes"] == len(selected)
    assert receipt["start"] == 0 and receipt["end"] == len(selected)
    assert destination.read_bytes() == selected
    origin = receipt["detail"]["origin"]
    assert origin["format"] == "html" and origin["container"] == str(container)
    assert origin["source_id"] == hashlib.sha256(container.read_bytes()).hexdigest()
    assert origin["node_id"] == node_id
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--cpu-model")
    parser.add_argument("--output", type=Path, default=ROOT / "target/benchmarks/html")
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

    pages = {
        "doc.html": document_page(),
        "attribute.html": attribute_page(),
        "script.html": script_page(),
    }
    page_bytes = {}
    for name, text in pages.items():
        page_bytes[name] = write_text(output / name, text)
    script_source = SCRIPT.encode("utf-8")
    script_path = output / "script.html"

    synthetic = {
        "markup-dense.html": markup_dense_page(),
        "deep-nested.html": deep_nested_page(),
    }
    synthetic_bytes = {}
    for name, text in synthetic.items():
        synthetic_bytes[name] = write_text(output / name, text)

    corpus_root = ROOT / "tests/corpus/node_modules"
    lock_path = ROOT / "tests/corpus/package-lock.json"
    lock = json.loads(lock_path.read_text()) if lock_path.is_file() else {}
    corpus_pages = {}
    corpus_absent = []
    for package, relative in PINNED_PAGES:
        version = locked_version(lock, package)
        source = corpus_root / relative
        if version is not None and source.is_file():
            corpus_pages[f"{package}@{version}"] = (relative, source)
        else:
            corpus_absent.append(f"{package}@{version or 'unknown'} ({relative})")

    prepared = output / "prepared"
    prepared.mkdir(exist_ok=True)
    tasks = []

    def command(name, target, args):
        return [str(binary), "html", name, str(target), *args]

    for name in pages:
        target = output / name
        source_id = base.digest(target)
        tasks.append((f"authored/{name}/list",
                      command("list", target, []),
                      lambda payload, source_id=source_id: validate_list(payload, source_id),
                      len(page_bytes[name])))

    # Small inline-script content extraction (single command, one atomic write).
    script_container = output / "script.html"
    script_hash = base.digest(script_container)
    _, listing = base.checked(command("list", script_container, []), 120)
    catalog = validate_list(listing, script_hash)
    script_element = next(r for r in catalog[1:-1]
                          if r["record"] == "element" and r["detail"]["tag"] == "script")
    script_id = script_element["id"]
    start = script_element["detail"]["content_start"]
    end = script_element["detail"]["content_end"]
    script_destination = prepared / "inline.js"

    def extract_argv():
        return command("extract", script_container,
                       ["--id", script_id, "--content", "-o", str(script_destination)])

    def validate_script_extract(payload):
        return validate_receipt(payload, script_destination, script_source,
                                script_container, script_id)

    initial, payload = base.checked(extract_argv(), 120)
    validate_script_extract(payload)
    tasks.append(("authored/script/extract", extract_argv(),
                  validate_script_extract, len(script_source)))

    # Synthetic markup-dense/deep-nested list workloads are separately labeled.
    for name in synthetic:
        target = output / name
        source_id = base.digest(target)
        tasks.append((f"synthetic/{name}/list",
                      command("list", target, []),
                      lambda payload, source_id=source_id: validate_list(payload, source_id),
                      len(synthetic_bytes[name])))

    # Pinned real npm package pages, labeled package@version from the lockfile.
    for label, (relative, source) in corpus_pages.items():
        source_id = base.digest(source)
        tasks.append((f"corpus/{label}/list",
                      command("list", source, []),
                      lambda payload, source_id=source_id: validate_list(payload, source_id),
                      source.stat().st_size))

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
        "input": {
            "kind": ("pinned real npm package pages (when tests/corpus/node_modules is installed), authored ordinary "
                     "pages and generated synthetic large/markup-dense pages"),
            "corpus_pages": {label: {"package": label.split("@")[0], "version": label.split("@")[1],
                                     "relative": relative, "source": str(source),
                                     "bytes": source.stat().st_size, "sha256": base.digest(source)}
                             for label, (relative, source) in corpus_pages.items()},
            "corpus_absent": corpus_absent,
            "corpus_provenance": "real published npm package files pinned by tests/corpus/package-lock.json (npm integrity)",
            "pages": {name: {"bytes": len(page_bytes[name]), "sha256": base.digest(output / name)}
                      for name in pages},
            "synthetic_pages": {name: {"bytes": len(synthetic_bytes[name]), "sha256": base.digest(output / name)}
                                for name in synthetic},
            "inline_script_bytes": len(script_source), "inline_script_sha256": hashlib.sha256(script_source).hexdigest(),
            "provenance": "authored/synthetic pages plus pinned npm package pages when the gitignored corpus is installed; inline script is a fixed authored snippet, not a production bundle",
            "limitation": "no in-repo production HTML corpus and no network access; when tests/corpus/node_modules is absent only authored/synthetic pages are measured; no page execution or external resource loading"},
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

    # Complete list -> returned ID -> exact extract -> rxd js workflow (three fresh CLI calls).
    def workflow():
        started = base.time.perf_counter_ns()
        first, listing = base.checked(command("list", script_container, []), 120)
        catalog = validate_list(listing, script_hash)
        element = next(r for r in catalog[1:-1]
                       if r["record"] == "element" and r["detail"]["tag"] == "script")
        script_id = element["id"]
        second, receipt_payload = base.checked(
            command("extract", script_container,
                    ["--id", script_id, "--content", "-o", str(script_destination)]), 120)
        receipt = validate_script_extract(receipt_payload)
        third, outlined = base.checked([str(binary), "js", "outline", str(script_destination)], 120)
        outline = base.jsonl(outlined, receipt["source_id"])
        assert any(r["detail"]["name"] == "run" for r in outline[1:-1])
        duration = base.time.perf_counter_ns() - started
        sample = dict(third)
        sample.update(duration_ns=duration, commands=[first, second, third],
                      command_sum_ns=first["duration_ns"] + second["duration_ns"] + third["duration_ns"])
        rss = [s["wait4_max_process_rss_bytes"] for s in (first, second, third)
               if s["wait4_max_process_rss_bytes"] is not None]
        sample["wait4_max_process_rss_bytes"] = max(rss) if rss else None
        return sample

    initial_workflow = workflow()
    workflow()
    workflow_samples = [workflow() for _ in range(options.samples)]
    report["workloads"].append({"name": "authored/script/list_extract_js", "argv": command("list", script_container, []),
                                "source_bytes": len(script_source), "initial": initial_workflow,
                                "samples": workflow_samples, "stats": base.stats(workflow_samples),
                                "profiles": [], "profile_stats": []})
    report["environment"]["load_average_end"] = os.getloadavg() if hasattr(os, "getloadavg") else None
    (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    if corpus_pages:
        corpus_line = ("Pinned real npm pages measured (package@version from package-lock.json): "
                       + ", ".join(f"{label} {relative}" for label, (relative, _) in corpus_pages.items()) + ".")
    else:
        corpus_line = ("Pinned real npm pages skipped because tests/corpus/node_modules is absent (run `npm ci` "
                       "in tests/corpus): " + ", ".join(corpus_absent) + ".")
    lines = ["# HTML CLI benchmark", "", report["input"]["kind"], "",
             corpus_line,
             f"Hardware: {report['environment']['cpu_model']}; {report['environment']['os']}",
             "Fresh processes, primed OS file cache. Extract includes a real atomic file write.",
             "Pinned npm pages are real published package files; authored pages are handcrafted; synthetic pages are generated.",
             "", "| Workload | Source bytes | Median ms | P95 ms | >150 ms |", "|---|---:|---:|---:|---:|"]
    for w in report["workloads"]:
        s = w["stats"]
        lines.append(f"| {w['name']} | {w['source_bytes']} | {s['median_ns']/1e6:.3f} | {s['p95_ns']/1e6:.3f} | {s['over_150ms_count']}/{s['samples']} ({s['over_150ms_proportion']:.1%}) |")
    lines.extend(["", "list_extract_js includes three fresh CLI calls, ID routing and one atomic write; validation occurs after timing.",
                  "No in-repo production HTML corpus and no network access; authored/synthetic pages do not establish acceptance for all shipped web pages or markup shapes.",
                  "Pinned npm pages are real package files, not full production sites; they still do not cover every markup shape or protector-generated markup."])
    (output / "results.md").write_text("\n".join(lines) + "\n")
    print(output / "results.md")


if __name__ == "__main__":
    main()
