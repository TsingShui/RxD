#!/usr/bin/env python3
"""End-to-end CLI benchmarks; standard library only, never execute input JS."""

import argparse
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
VERSION = 1
NEEDLE = "/__rxd_bench__/needle"


class CommandFailure(RuntimeError):
    def __init__(self, argv, sample):
        super().__init__(f"command failed: {argv!r}: {sample.get('diagnostic', '')}")
        self.sample = sample
        self.argv = argv


def digest(path):
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            hasher.update(block)
    return hasher.hexdigest()


def tool_output(args):
    try:
        result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
        return result.stdout.strip() if result.returncode == 0 else None
    except OSError:
        return None


def generate(path, requested_bytes, needle, syntax="auto"):
    """Deterministic 2-KiB functions: one outer function and one nested arrow."""
    count = max(1, requested_bytes // 2048)
    target = count // 2
    offset = 0
    target_span = None
    xref_byte = None
    definition_expected = None
    hasher = hashlib.sha256()
    with path.open("wb") as output:
        for i in range(count):
            typed = syntax in ("ts", "tsx")
            prefix = f'function f{i}(x{":number" if typed else ""}){{const data{":string" if typed else ""}="'
            marker = json.dumps(needle if i == target else "padding", ensure_ascii=True)
            result = '<div data-marker={marker}>{step(data.length)}</div>' if syntax in ("jsx", "tsx") else 'step(data.length)+marker.length'
            suffix = f'";const marker={marker};const step=(y{":number" if typed else ""})=>y+x;return {result};}}\n'
            padding = 2048 - len(prefix.encode()) - len(suffix.encode())
            if padding < 0:
                raise ValueError("needle is too large for the synthetic fixture")
            block = (prefix + "p" * padding + suffix).encode()
            output.write(block)
            hasher.update(block)
            if i == target:
                target_span = [offset, offset + len(block) - 1]
                xref_byte = offset + len(f"function f{i}(")
                declaration_start = offset + block.index(b"marker=")
                initializer_start = declaration_start + len(b"marker=")
                declaration_end = initializer_start + len(marker.encode())
                definition_expected = {
                    "binding_name": "marker", "identifier": {"start": declaration_start, "end": declaration_start + 6},
                    "declaration": {"kind": "variable_declarator", "start": declaration_start, "end": declaration_end,
                                    "variable_kind": "const", "initializer": {"start": initializer_start, "end": declaration_end}},
                }
            offset += len(block)
        remainder = max(0, requested_bytes - offset)
        tail = (b"/*" + b"p" * (remainder - 4) + b"*/") if remainder >= 4 else b" " * remainder
        output.write(tail)
        hasher.update(tail)
    return {
        "kind": "synthetic", "generator_version": VERSION,
        "requested_bytes": requested_bytes, "input_bytes": path.stat().st_size,
        "source_id": hasher.hexdigest(), "expected_entities": count * 2 + 1,
        "target_span": target_span, "path": str(path),
        "xref_byte": xref_byte, "xref_identifier": "x",
        "definition_expected": definition_expected,
        "context_expected": {"kind": "variable_declarator", "start": definition_expected["declaration"]["start"],
                             "end": definition_expected["declaration"]["end"],
                             "parent": {"kind": "variable_declaration", "start": definition_expected["declaration"]["start"] - 6,
                                        "end": definition_expected["declaration"]["end"] + 1}},
    }


def timeout_handler(_signum, _frame):
    raise TimeoutError("benchmark command exceeded its timeout")


def run_command(argv, timeout):
    # File-backed redirection measures real serialization and writes without a
    # slow Python pipe reader imposing backpressure on the measured process.
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        started = time.perf_counter_ns()
        child = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=stdout,
                                 stderr=stderr, start_new_session=(os.name == "posix"))
        usage = None
        timed_out = False
        try:
            if hasattr(os, "wait4") and hasattr(signal, "setitimer"):
                signal.setitimer(signal.ITIMER_REAL, timeout)
                try:
                    _, status, usage = os.wait4(child.pid, 0)
                    child.returncode = os.waitstatus_to_exitcode(status)
                finally:
                    signal.setitimer(signal.ITIMER_REAL, 0)
            else:
                child.wait(timeout=timeout)
        except (TimeoutError, subprocess.TimeoutExpired):
            timed_out = True
            if os.name == "posix":
                os.killpg(child.pid, signal.SIGKILL)
            else:
                child.kill()
            child.wait()
        duration = time.perf_counter_ns() - started
        stdout.seek(0)
        stderr.seek(0)
        payload = stdout.read()
        diagnostics = stderr.read()
    rss = None
    if usage is not None and sys.platform in ("darwin", "linux"):
        rss = int(usage.ru_maxrss) * (1024 if sys.platform == "linux" else 1)
    metrics = {
        "duration_ns": duration, "exit_code": child.returncode,
        "timed_out": timed_out, "stdout_bytes": len(payload),
        "stderr_bytes": len(diagnostics), "stdout_sha256": hashlib.sha256(payload).hexdigest(),
        "wait4_max_process_rss_bytes": rss,
    }
    reports = []
    for line in diagnostics.splitlines():
        try:
            item = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            continue
        if isinstance(item, dict) and item.get("record") == "perf":
            reports.append(item)
    if reports:
        metrics["perf"] = reports
    if child.returncode != 0 or timed_out:
        metrics["diagnostic"] = diagnostics.decode("utf-8", errors="replace")[-4096:]
    return metrics, payload


def checked(argv, timeout):
    sample, payload = run_command(argv, timeout)
    if sample["timed_out"] or sample["exit_code"] != 0:
        raise CommandFailure(argv, sample)
    return sample, payload


def jsonl(payload, source_id):
    records = [json.loads(line) for line in payload.splitlines()]
    if (len(records) < 2 or records[0].get("record") != "header"
            or records[0].get("source_id") != source_id
            or records[0].get("schema_version") != 2
            or records[-1].get("record") != "summary"
            or records[-1].get("complete") is not True
            or records[-1].get("records") != len(records) - 2):
        raise RuntimeError("invalid or incomplete JSONL result")
    return records


def stats(samples):
    values = sorted(sample["duration_ns"] for sample in samples)
    if not values:
        return None
    rss = [s["wait4_max_process_rss_bytes"] for s in samples
           if s["wait4_max_process_rss_bytes"] is not None]
    return {
        "samples": len(values), "median_ns": statistics.median(values),
        "p95_ns": values[math.ceil(len(values) * 0.95) - 1],
        "min_ns": values[0], "max_ns": values[-1],
        "over_150ms_count": sum(value > 150_000_000 for value in values),
        "over_150ms_proportion": sum(value > 150_000_000 for value in values) / len(values),
        "max_process_peak_rss_bytes": max(rss) if rss else None,
        "stdout_bytes": samples[0]["stdout_bytes"],
    }


def profile_stats(samples):
    grouped = {}
    for sample in samples:
        for report in sample.get("perf", []):
            key = (report["scope"], report["command"])
            grouped.setdefault(key, []).append(report)
    summaries = []
    for (scope, command), reports in sorted(grouped.items()):
        stage_names = sorted({stage for report in reports for stage in report["stages_ns"]})
        rss = [r["peak_rss_bytes"] for r in reports if r["peak_rss_bytes"] is not None]
        summaries.append({
            "scope": scope, "command": command, "samples": len(reports),
            "stage_median_ns": {stage: statistics.median(r["stages_ns"].get(stage, 0) for r in reports)
                                for stage in stage_names},
            "total_median_ns": statistics.median(r["total_ns"] for r in reports),
            "peak_rss_bytes": max(rss) if rss else None,
            "counters": reports[0]["counters"], "limits": reports[0]["limits"],
        })
    return summaries


def benchmark_fixture(manifest, binary, rg, options, extra):
    path = Path(manifest["path"])
    needle = manifest.get("needle", options.needle)
    def command(name, args=(), perf=False):
        # Ceilings are internal (rxd_core::Budget); the CLI takes no budget flag.
        return [str(binary), "js", name, str(path), *args, *extra, *(["--perf"] if perf else [])]

    # Untimed setup obtains and validates real IDs. It explicitly primes the file cache.
    _, payload = checked(command("find", ["-f", needle]), options.timeout)
    found = jsonl(payload, manifest["source_id"])
    matches = [r for r in found if r.get("record") == "match"]
    if not matches:
        if manifest.get("plaintext_find") is not False:
            raise RuntimeError("no decoded literal matches the requested needle")
        _, payload = checked(command("entities"), options.timeout)
        structures = jsonl(payload, manifest["source_id"])
        fallback = manifest["fallback_entity"]
        selected = next((e for e in structures[1:-1]
                         if [e["detail"]["kind"], e["start"], e["end"]] == fallback), None)
        if selected is None:
            raise RuntimeError("entities does not contain the independent fallback entity")
        entity_id = selected["at"]
        locate_byte = selected["start"]
    else:
        if manifest.get("plaintext_find") is False:
            raise RuntimeError("encrypted corpus unexpectedly exposes the plaintext needle")
        entity_id = matches[0]["parent_at"]
        locate_byte = matches[0]["start"]
    _, payload = checked(command("slice", ["--at", entity_id, "--json"]), options.timeout)
    extracted = json.loads(payload)
    entity = extracted["detail"]["entity"]
    span = [entity["start"], entity["end"]]
    if manifest.get("target_span") is not None and span != manifest["target_span"]:
        raise RuntimeError("find selected an unexpected synthetic function")
    with path.open("rb") as source:
        source.seek(span[0])
        original = source.read(span[1] - span[0])
    if extracted["detail"].get("complete") is not True or extracted["detail"]["source"].encode() != original:
        raise RuntimeError("get did not extract exact original source bytes")
    manifest.update(target_id=entity_id, selected_span=span, matches=len(matches),
                    setup_queries_prime_file_cache=True)
    cases = [
        ("entities", command("entities")),
        ("find", command("find", ["-f", needle])),
        ("locate", command("locate", ["--byte", str(locate_byte)])),
        ("slice", command("slice", ["--at", entity_id])),
        ("symbols", command("symbols")),
        ("symbols_within", command("symbols", ["--within", entity_id])),
    ]
    definition_expected = manifest.get("definition_expected")
    definition_original = None
    parent_original = None
    if definition_expected is not None:
        identifier = definition_expected["identifier"]
        declaration = definition_expected["declaration"]
        symbol_id = f"rxd1:js:{manifest['source_id']}:symbol:{identifier['start']}:{identifier['end']}"
        _, payload = checked(command("slice", ["--at", symbol_id, "--json"]), options.timeout)
        binding = json.loads(payload)
        with path.open("rb") as source:
            source.seek(declaration["start"])
            definition_original = source.read(declaration["end"] - declaration["start"])
        if (binding["detail"].get("complete") is not True or binding["detail"]["source"].encode() != definition_original
                or binding["detail"]["declaration"] != declaration or binding["detail"]["symbol"]["detail"]["name"] != definition_expected["binding_name"]):
            raise RuntimeError("binding extraction differs from independent declaration expectation")
        parent_id = binding["detail"]["symbol"]["parent_at"]
        _, payload = checked(command("slice", ["--at", parent_id, "--json"]), options.timeout)
        parent = json.loads(payload)
        with path.open("rb") as source:
            source.seek(parent["detail"]["entity"]["start"])
            parent_original = source.read(parent["detail"]["entity"]["end"] - parent["detail"]["entity"]["start"])
        if parent["detail"]["source"].encode() != parent_original:
            raise RuntimeError("containing structure extraction changed source bytes")
        manifest["symbol_extraction"] = {
            "symbol_id": symbol_id, "parent_id": parent_id, "binding_name": binding["detail"]["symbol"]["detail"]["name"],
            "declaration_bytes": len(definition_original), "parent_bytes": len(parent_original),
            "output_reduction_bytes": len(parent_original) - len(definition_original),
            "output_reduction_proportion": 1 - len(definition_original) / len(parent_original),
            "measurement": "raw UTF-8 source bytes; no token estimate",
        }
        cases.extend([
            ("slice_symbol", command("slice", ["--at", symbol_id])),
            ("slice_symbol_json", command("slice", ["--at", symbol_id, "--json"])),
            ("slice_symbol_parent", command("slice", ["--at", parent_id])),
        ])
    else:
        manifest.setdefault("cases", []).append({"name": "slice_symbol", "status": "not_applicable",
            "reason": "No independently selected source declaration for this input."})
    context_expected = manifest.get("context_expected")
    context_id = matches[0].get("context_at") if matches else None
    if context_id is None and context_expected is not None:
        context_id = (f"fragment.{context_expected['kind']}:"
                      f"{context_expected['start']}:{context_expected['end']}")
    context_original = None
    context_metadata = None
    if context_id:
        _, payload = checked(command("slice", ["--at", context_id, "--json"]), options.timeout)
        context_metadata = json.loads(payload)
        fragment = context_metadata["detail"]["fragment"]
        with path.open("rb") as source:
            source.seek(fragment["start"])
            context_original = source.read(fragment["end"] - fragment["start"])
        if context_metadata["detail"].get("complete") is not True or context_metadata["detail"]["source"].encode() != context_original:
            raise RuntimeError("local context extraction changed original source bytes")
        if context_expected is not None:
            if any((fragment["detail"][key] if key == "kind" else fragment[key]) != context_expected[key] for key in ("kind", "start", "end")):
                raise RuntimeError("local context differs from independent AST expectation")
            parent = context_expected["parent"]
            parent_id = (f"fragment.{parent['kind']}:{parent['start']}:{parent['end']}"
                         if parent else None)
            if fragment["parent_at"] != parent_id:
                raise RuntimeError("local context parent differs from independent AST expectation")
        _, payload = checked(command("slice", ["--at", fragment["detail"]["entity_id"]]), options.timeout)
        manifest["context_extraction"] = {
            "context_id": context_id, "kind": fragment["detail"]["kind"], "context_bytes": len(context_original),
            "entity_bytes": len(payload), "output_reduction_proportion": 1 - len(context_original) / len(payload),
            "expectation": "independent AST/generator" if context_expected is not None else "CLI span and byte consistency only",
            "selection": "first supported literal context, otherwise independent parameter",
        }
        cases.extend([("slice_context", command("slice", ["--at", context_id])),
                      ("slice_context_json", command("slice", ["--at", context_id, "--json"]))])
        if matches and matches[0].get("context_id"):
            cases.append(("find_context_slice", None))
    else:
        manifest.setdefault("cases", []).append({"name": "slice_context", "status": "not_applicable",
            "reason": "The selected position has no supported local syntax context."})
    if manifest.get("xref_byte") is not None:
        cases.append(("xref_byte", command("xref", ["--byte", str(manifest["xref_byte"])])))
    else:
        manifest.setdefault("cases", []).append({"name": "xref_byte", "status": "not_applicable",
            "reason": "No independently selected binding offset for this input."})
    if matches:
        cases.append(("find_slice", None))
    else:
        manifest.setdefault("cases", []).append({"name": "find_slice", "status": "not_applicable",
            "reason": "The needle is encrypted; static literal search cannot recover its runtime value. Structural queries use an independently selected entity."})
    if rg:
        cases.append(("rg", [rg, "--no-config", "--color", "never", "--no-heading", "-boF", "--", needle, str(path)]))
    else:
        manifest.setdefault("cases", []).append({"name": "rg", "status": "unavailable"})
    for name, argv in cases:
        entry = {"name": name, "argv": argv, "status": "pending", "samples": [], "profiles": []}
        manifest.setdefault("cases", []).append(entry)

        def execute(profiled=False):
            if name not in ("find_slice", "find_context_slice"):
                args = argv + (["--perf"] if profiled and name != "rg" else [])
                sample, output = run_command(args, options.timeout)
                if sample["timed_out"] or sample["exit_code"] not in ([0, 1] if name == "rg" else [0]):
                    raise CommandFailure(args, sample)
                return sample, output
            started = time.perf_counter_ns()
            first, found_payload = checked(command("find", ["-f", needle], profiled), options.timeout)
            records = jsonl(found_payload, manifest["source_id"])
            selected = next(r["context_at" if name == "find_context_get" else "parent_at"]
                            for r in records if r.get("record") == "match")
            second, output = checked(command("slice", ["--at", selected], profiled), options.timeout)
            elapsed = time.perf_counter_ns() - started
            peaks = [s["wait4_max_process_rss_bytes"] for s in [first, second]
                     if s["wait4_max_process_rss_bytes"] is not None]
            return {
                "duration_ns": elapsed, "exit_code": 0, "timed_out": False,
                "stdout_bytes": first["stdout_bytes"] + second["stdout_bytes"],
                "stderr_bytes": first["stderr_bytes"] + second["stderr_bytes"],
                "stdout_sha256": hashlib.sha256(found_payload + b"\0" + output).hexdigest(),
                "wait4_max_process_rss_bytes": max(peaks) if peaks else None,
                "command_duration_ns": first["duration_ns"] + second["duration_ns"],
                "steps": [first, second], "perf": first.get("perf", []) + second.get("perf", []),
            }, output

        try:
            initial, output = execute()
            entry["initial_case_sample"] = initial
            if name in ("find", "entities", "xref_byte", "symbols", "symbols_within"):
                records = jsonl(output, manifest["source_id"])
                if name == "xref_byte":
                    if records[0]["detail"].get("resolution") != "resolved" or records[0]["detail"]["selected"]["name"] != manifest["xref_identifier"]:
                        raise RuntimeError("independent binding query did not resolve")
                    expected = manifest.get("refs_expected")
                    if expected is not None:
                        keys = ("start", "end", "read", "write", "member_write")
                        if any("type_reference" in row for row in expected):
                            keys += ("type_reference", "value_reference")
                        actual = [{key: row[key] if key in ("start", "end") else row["detail"][key] for key in keys} for row in records[1:-1]]
                        if actual != expected:
                            raise RuntimeError("refs mismatch against independent Acorn expectations")
                if name in ("symbols", "symbols_within"):
                    if records[0]["detail"].get("analysis") != "lexical" or records[0]["detail"].get("resolution") not in ("static", "dynamic"):
                        raise RuntimeError("symbol discovery did not report lexical resolution")
                    if records[0]["detail"]["resolution"] == "dynamic" and len(records) != 2:
                        raise RuntimeError("dynamic symbol discovery enumerated bindings")
                    positions = [(row["start"], row["end"]) for row in records[1:-1]]
                    if positions != sorted(positions):
                        raise RuntimeError("symbol locations are not ordered")
                    ids = [row["at"] for row in records[1:-1] if row["record"] == "symbol"]
                    if len(ids) != len(set(ids)):
                        raise RuntimeError("symbol identities are not unique")
                    if name == "symbols" and manifest.get("xref_byte") is not None:
                        symbol = next(row for row in records[1:-1]
                                      if row["record"] == "symbol" and row["start"] == manifest["xref_byte"])
                        if symbol["detail"]["name"] != manifest["xref_identifier"] or symbol["detail"]["relation"] != "declared":
                            raise RuntimeError("symbol mismatch against independently selected binding")
                if name == "entities":
                    manifest["observed_entities"] = len(records) - 2
                if name == "entities" and manifest.get("expected_entities") is not None:
                    if len(records) - 2 != manifest["expected_entities"]:
                        raise RuntimeError("entities returned an unexpected structure count")
            elif name in ("slice_context", "find_context_slice") and output != context_original:
                raise RuntimeError("local context extraction changed original bytes")
            elif name == "get_context_json":
                if json.loads(output) != context_metadata:
                    raise RuntimeError("local context metadata changed from validated setup")
            elif name == "slice_symbol" and output != definition_original:
                raise RuntimeError("binding declaration extraction changed original bytes")
            elif name == "get_symbol_parent" and output != parent_original:
                raise RuntimeError("binding parent extraction changed original bytes")
            elif name == "slice_symbol_json":
                binding = json.loads(output)
                if (binding["detail"].get("complete") is not True or binding["detail"]["declaration"] != definition_expected["declaration"]
                        or binding["detail"]["source"].encode() != definition_original):
                    raise RuntimeError("binding metadata differs from independent declaration expectation")
            elif name in ("slice", "find_slice") and output != original:
                raise RuntimeError("source extraction changed original bytes")
            elif name == "locate" and json.loads(output)["detail"]["entity"]["at"] != entity_id:
                raise RuntimeError("locate disagrees with find/get")
            signature = (initial["stdout_sha256"], initial["stdout_bytes"], initial["exit_code"])
            del output
            def verified(profiled=False):
                sample, _output = execute(profiled)
                if (sample["stdout_sha256"], sample["stdout_bytes"], sample["exit_code"]) != signature:
                    raise RuntimeError("repeated or profiled query changed output")
                return sample
            for _ in range(options.warmups):
                verified()
            entry["samples"] = [verified() for _ in range(options.samples)]
            if name != "rg":
                entry["profiles"] = [verified(True) for _ in range(options.profile_samples)]
                for sample in entry["profiles"]:
                    expected = 4 if name in ("find_slice", "find_context_slice") else 2
                    if (len(sample.get("perf", [])) != expected
                            or any(r.get("perf_version") != 1 or r.get("status") != "ok" for r in sample["perf"])):
                        raise RuntimeError("missing or invalid perf sidecar")
            entry.update(status="ok", summary=stats(entry["samples"]),
                         profile_summary=profile_stats(entry["profiles"]))
            print(f"{path.name}: {name}: {entry['summary']['median_ns'] / 1e6:.2f} ms", file=sys.stderr)
        except (RuntimeError, ValueError, KeyError, StopIteration, OSError) as error:
            entry.update(status="failed", error=str(error))
            if isinstance(error, CommandFailure):
                entry.update(failed_sample=error.sample, failed_argv=error.argv)


def render_markdown(report):
    lines = ["# CLI Benchmark", "", f"Generated: {report['created_at']}", "",
             "All headline latency samples run without perf instrumentation. All invocations start fresh processes; file caches are primed by setup and are not flushed.", "",
             "| Input | Case | Median ms | P95 ms | >150 ms | Output bytes | Max process RSS MiB |", "| --- | --- | ---: | ---: | ---: | ---: | ---: |"]
    for fixture in report["fixtures"]:
        label = f"{fixture.get('name', Path(fixture.get('path', 'unknown')).name)} ({fixture.get('input_bytes', 0) / 1024 / 1024:.2f} MiB)"
        for case in fixture.get("cases", []):
            summary = case.get("summary")
            if not summary:
                lines.append(f"| {label} | {case['name']} ({case['status']}) | — | — | — | — | — |")
                continue
            rss = summary["max_process_peak_rss_bytes"]
            rss = f"{rss / 1024 / 1024:.2f}" if rss is not None else "unavailable"
            exceedance = f"{summary['over_150ms_count']}/{summary['samples']} ({summary['over_150ms_proportion']:.1%})"
            lines.append(f"| {label} | {case['name']} | {summary['median_ns'] / 1e6:.2f} | {summary['p95_ns'] / 1e6:.2f} | {exceedance} | {summary['stdout_bytes']} | {rss} |")
    lines += ["", "## Selected declaration output sizes", "",
              "Raw original bytes, without JSON metadata. JS corpus queries maximize the enclosing-structure/declarator size ratio among initialized declarators of 32-2048 bytes; typed corpus uses the first initialized identifier declarator. These are selected examples, not average workload or token savings. Synthetic queries use a known marker declaration.", "",
              "| Input | Binding | Declaration bytes | Containing structure bytes | Reduction |",
              "| --- | --- | ---: | ---: | ---: |"]
    for fixture in report["fixtures"]:
        extraction = fixture.get("symbol_extraction")
        if extraction:
            lines.append(f"| {fixture.get('name', Path(fixture['path']).name)} | {extraction['binding_name']} | {extraction['declaration_bytes']} | {extraction['parent_bytes']} | {extraction['output_reduction_proportion']:.2%} |")
    lines += ["", "## Local context output sizes", "",
              "First supported literal match contexts; when unavailable, corpus cases use an independent parameter. Zero-match inputs never recover encrypted literals. Raw source bytes only; reductions are per selected example, not average task or token savings.", "",
              "| Input | Context kind | Context bytes | Containing structure bytes | Reduction |",
              "| --- | --- | ---: | ---: | ---: |"]
    for fixture in report["fixtures"]:
        context = fixture.get("context_extraction")
        if context:
            lines.append(f"| {fixture.get('name', Path(fixture['path']).name)} | {context['kind']} | {context['context_bytes']} | {context['entity_bytes']} | {context['output_reduction_proportion']:.2%} |")
    lines += ["", "RSS is the largest individual process high-water mark in the waited process tree, not simultaneous total tree memory. Published library artifacts are not a production business application's complete bundle; generated transformations are labeled separately. Synthetic scale inputs are not evidence of real bundle performance.", "", "`rg` searches raw bytes and returns offsets; `find` searches decoded literals and returns structured records. These outputs and capabilities differ. Encrypted needles produce complete zero-match results; a find-then-slice workflow is not applicable and structural slice/locate use an independent entity selection.", "", "## Instrumented phase medians", "", "These diagnostics are collected separately from headline latency samples.", "", "| Input | Case / command | Scope | Stage | Median ms |", "| --- | --- | --- | --- | ---: |"]
    for fixture in report["fixtures"]:
        for case in fixture.get("cases", []):
            for profile in case.get("profile_summary", []):
                for stage, duration in profile["stage_median_ns"].items():
                    lines.append(f"| {fixture.get('name', Path(fixture['path']).name)} | {case['name']} / {profile['command']} | {profile['scope']} | {stage} | {duration / 1e6:.3f} |")
    return "\n".join(lines) + "\n"


def positive(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return number


def nonnegative(value):
    number = int(value)
    if number < 0:
        raise argparse.ArgumentTypeError("must be nonnegative")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, help="use an existing binary instead of building release; provenance is marked custom")
    inputs = parser.add_mutually_exclusive_group()
    inputs.add_argument("--sizes", type=float, nargs="+", metavar="MIB", help="explicit synthetic scale inputs")
    inputs.add_argument("--input", type=Path, action="append", default=[], help="authorized real JS input; repeatable")
    inputs.add_argument("--corpus", type=Path, help="pinned production/obfuscated corpus manifest (default: tests/corpus/manifest.json)")
    parser.add_argument("--case", action="append", default=[], help="select a named corpus case; repeatable")
    parser.add_argument("--needle", default=NEEDLE)
    parser.add_argument("--syntax", choices=["auto", "js", "jsx", "ts", "tsx"], default="auto", help="syntax override for CLI and synthetic source generation")
    parser.add_argument("--samples", type=positive, default=7)
    parser.add_argument("--warmups", type=nonnegative, default=1)
    parser.add_argument("--profile-samples", type=nonnegative, default=3)
    parser.add_argument("--timeout", type=positive, default=120, help="seconds per CLI invocation")
    parser.add_argument("--output", type=Path, default=ROOT / "target/benchmarks/latest")

    options = parser.parse_args()
    if not options.needle or any(not math.isfinite(size) or size <= 0 for size in options.sizes or []):
        parser.error("needle must be nonempty and sizes must be finite positive numbers")
    if not options.input and options.sizes is None and options.corpus is None:
        options.corpus = ROOT / "tests/corpus/manifest.json"
    if options.case and options.corpus is None:
        parser.error("--case requires corpus inputs")
    corpus = None
    refs_queries = {}
    definition_queries = {}
    context_queries = {}
    if options.corpus is not None:
        try:
            corpus = json.loads(options.corpus.read_text())
            if corpus.get("corpus_version") != 1 or not corpus.get("cases"):
                raise ValueError("unsupported or empty corpus manifest")
            refs_path = options.corpus.resolve().parent / "refs-expectations.json"
            if refs_path.is_file():
                oracle = json.loads(refs_path.read_text())
                if oracle.get("refs_version") != 1:
                    raise ValueError("unsupported refs expectations")
                refs_queries = {c["name"]: c for c in oracle["cases"]}
            definitions_path = options.corpus.resolve().parent / "definitions-expectations.json"
            if definitions_path.is_file():
                oracle = json.loads(definitions_path.read_text())
                if oracle.get("definitions_version") != 1:
                    raise ValueError("unsupported source declaration expectations")
                definition_queries = {c["name"]: c for c in oracle["cases"]}
            contexts_path = options.corpus.resolve().parent / "contexts-expectations.json"
            if contexts_path.is_file():
                oracle = json.loads(contexts_path.read_text())
                if oracle.get("contexts_version") != 1:
                    raise ValueError("unsupported local context expectations")
                context_queries = {c["name"]: c for c in oracle["cases"]}
            unknown = set(options.case) - {c["name"] for c in corpus["cases"]}
            if unknown:
                raise ValueError(f"unknown corpus cases: {sorted(unknown)}")
        except (OSError, ValueError) as error:
            parser.error(str(error))
    if options.binary is None:
        subprocess.run(["cargo", "build", "--release", "--locked"], cwd=ROOT, check=True)
    binary = (options.binary or ROOT / "target/release/rxd").resolve()
    if not binary.is_file():
        parser.error(f"binary does not exist: {binary}")
    directory = options.output.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    if hasattr(signal, "SIGALRM"):
        signal.signal(signal.SIGALRM, timeout_handler)
    rg = shutil.which("rg")
    extra = ["--syntax", options.syntax] if options.syntax != "auto" else []
    report = {
        "benchmark_version": VERSION,
        "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "environment": {"os": platform.platform(), "machine": platform.machine(),
                        "processor": platform.processor(), "cpu_count": os.cpu_count(),
                        "cpu_model": tool_output(["sysctl", "-n", "machdep.cpu.brand_string"]) if platform.system() == "Darwin" else platform.processor(),
                        "load_average_start": os.getloadavg() if hasattr(os, "getloadavg") else None,
                        "python": sys.version, "rustc": tool_output(["rustc", "--version"]),
                        "cargo": tool_output(["cargo", "--version"]),
                        "rg": tool_output([rg, "--version"]) if rg else None},
        "build": {"profile": "release" if options.binary is None else "custom",
                  "command": "cargo build --release --locked" if options.binary is None else None,
                  "binary": str(binary), "binary_sha256": digest(binary),
                  "binary_version": tool_output([str(binary), "--version"]),
                  "cargo_lock_sha256": digest(ROOT / "Cargo.lock"),
                  "git_head": tool_output(["git", "rev-parse", "HEAD"]),
                  "git_status": tool_output(["git", "status", "--porcelain"])},
        "method": {"samples": options.samples, "warmups": options.warmups,
                   "profile_samples": options.profile_samples, "timeout_seconds": options.timeout,
                   "process_mode": "fresh process on every invocation; no resident AST cache",
                   "file_cache": "primed by generation/setup; not flushed or controlled",
                   "sink": "temporary files; serialization and writes included in child wall time",
                   "memory": "wait4 largest individual process high-water mark; not simultaneous tree RSS",
                   "workflow": "find_slice/find_context_slice include ID decoding, routing and output capture between fresh CLI processes",
                   "p95": "nearest rank; fewer than 20 samples make p95 the maximum",
                   "latency_instrumented": False,
                   # One budget table (rxd_core::Budget) owns every ceiling; benchmarks
                   # cannot override it through the CLI.
                   "budgets": "rxd_core::Budget defaults",
                   "latency_target_ms": 150,
                   "contexts_expectations_sha256": digest(options.corpus.resolve().parent / "contexts-expectations.json") if context_queries else None,
                   "definitions_expectations_sha256": digest(options.corpus.resolve().parent / "definitions-expectations.json") if definition_queries else None,
                   "refs_expectations_sha256": digest(options.corpus.resolve().parent / "refs-expectations.json") if refs_queries else None,
                   "needle": options.needle,
                   "syntax_override": options.syntax,
                   "input_mode": "corpus" if corpus else "provided" if options.input else "synthetic",
                   "corpus_manifest_sha256": digest(options.corpus) if corpus else None},
        "fixtures": [],
    }
    corpus_cases = [c for c in corpus["cases"] if not options.case or c["name"] in options.case] if corpus else []
    paths = ([options.corpus.resolve().parent / c["file"] for c in corpus_cases]
             if corpus else options.input or [None] * len(options.sizes))
    for index, path in enumerate(paths):
        manifest = {"status": "pending"}
        try:
            if corpus:
                case = corpus_cases[index]
                source_id = digest(path)
                if source_id != case["sha256"]:
                    raise ValueError(f"corpus hash mismatch: {case['name']}")
                sampled = case["expected"]["sample_entities"]
                if not sampled:
                    raise ValueError("corpus needs an independent sampled entity")
                fallback = next((e for e in sampled if e["kind"] != "program"), sampled[0])
                manifest.update(name=case["name"], kind=case["provenance"]["kind"],
                                path=str(path), input_bytes=path.stat().st_size, source_id=source_id,
                                provenance=case["provenance"], needle=case["needle"],
                                plaintext_find=case["plaintext_find"], expected_entities=case["expected"]["entity_count"],
                                fallback_entity=[fallback["kind"], fallback["start"], fallback["end"]])
                query = refs_queries.get(case["name"])
                if query is not None:
                    if query["source_id"] != source_id or query["resolution"] != "resolved":
                        raise ValueError("invalid independent refs expectation for corpus")
                    manifest.update(xref_byte=query["byte"], xref_identifier=query["identifier"],
                                    refs_expected=query["references"])
                definition = definition_queries.get(case["name"])
                if definition is not None:
                    if definition["source_id"] != source_id:
                        raise ValueError("source declaration oracle hash mismatch")
                    manifest["definition_expected"] = definition
                contexts = context_queries.get(case["name"])
                if contexts is not None:
                    if contexts["source_id"] != source_id:
                        raise ValueError("local context oracle hash mismatch")
                    manifest["context_expected"] = (contexts["matches"][0] if contexts["matches"] and contexts["matches"][0]["context"] is not None else contexts["selected"])["context"]
            elif path is None:
                size = options.sizes[index]
                path = directory / f"synthetic-{index}-{size:g}MiB.js"
                manifest.update(generate(path, math.ceil(size * 1024 * 1024), options.needle, options.syntax))
            else:
                path = path.resolve()
                manifest.update(kind="provided", path=str(path), input_bytes=path.stat().st_size, source_id=digest(path))
            report["fixtures"].append(manifest)
            benchmark_fixture(manifest, binary, rg, options, extra)
            manifest["status"] = "failed" if any(c["status"] == "failed" for c in manifest["cases"]) else "ok"
        except (RuntimeError, ValueError, KeyError, OSError) as error:
            manifest.update(status="failed", error=str(error))
            if isinstance(error, CommandFailure):
                manifest.update(failed_sample=error.sample, failed_argv=error.argv)
            if manifest not in report["fixtures"]:
                report["fixtures"].append(manifest)
    report["environment"]["load_average_end"] = os.getloadavg() if hasattr(os, "getloadavg") else None
    report["status"] = "failed" if any(f["status"] != "ok" for f in report["fixtures"]) else "ok"
    (directory / "results.json").write_text(json.dumps(report, ensure_ascii=True, indent=2) + "\n")
    (directory / "results.md").write_text(render_markdown(report))
    print(f"benchmark {report['status']}: {directory / 'results.json'}", file=sys.stderr)
    return 0 if report["status"] == "ok" else 1


if __name__ == "__main__":
    sys.exit(main())
