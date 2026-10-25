#!/usr/bin/env python3
"""Build pinned WABT fixtures and expected decompilation; run from root with <wat2wasm>."""
import sys, os, re, json, hashlib, subprocess, tempfile

WAT2WASM = sys.argv[1] if len(sys.argv) > 1 else "wat2wasm"
HERE = os.path.dirname(os.path.abspath(__file__))
WASM_DIR = os.path.join(HERE, "wasm")

def split_case(text):
    wat = text.split("(;; STDOUT ;;;")[0]
    wat = "\n".join(l for l in wat.splitlines() if not l.strip().startswith(";;; TOOL"))
    m = re.search(r"\(;; STDOUT ;;;(.*?);;; STDOUT ;;\)", text, re.S)
    expected = m.group(1).strip("\n") if m else ""
    return wat, expected

def expected_funcs(expected):
    funcs = {}
    lines = expected.split("\n")
    i = 0
    hdr = re.compile(r'^(export )?function ([A-Za-z0-9_]+)\(')
    while i < len(lines):
        m = hdr.match(lines[i])
        if m:
            name = m.group(2)
            depth = lines[i].count("{") - lines[i].count("}")
            body = [lines[i]]
            j = i + 1
            while j < len(lines) and depth > 0:
                depth += lines[j].count("{") - lines[j].count("}")
                body.append(lines[j])
                j += 1
            funcs[name] = "\n".join(body)
            i = j
        else:
            i += 1
    return funcs

cases = []
tool_version = subprocess.run([WAT2WASM, "--version"], capture_output=True).stdout.decode().strip()

for fn in sorted(os.listdir(WASM_DIR)):
    if not fn.endswith(".txt"):
        continue
    name = fn[:-4]
    path = os.path.join(WASM_DIR, fn)
    raw = open(path, "rb").read()
    txt_sha = hashlib.sha256(raw).hexdigest()
    wat, expected = split_case(raw.decode())
    watpath = os.path.join(tempfile.gettempdir(), name + ".wat")
    wasmpath = os.path.join(WASM_DIR, name + ".wasm")
    open(watpath, "w").write(wat)
    r = subprocess.run([WAT2WASM, "--debug-names", watpath, "-o", wasmpath], capture_output=True)
    if r.returncode != 0:
        print(f"skip {name}: wat2wasm failed ({r.stderr.decode().strip()[:80]})")
        continue
    wasm_sha = hashlib.sha256(open(wasmpath, "rb").read()).hexdigest()
    cases.append({
        "name": name,
        "source_txt": fn,
        "source_txt_sha256": txt_sha,
        "wasm": name + ".wasm",
        "wasm_sha256": wasm_sha,
        "expected_functions": expected_funcs(expected),
    })
    print(f"ok {name}: {len(cases[-1]['expected_functions'])} expected functions")

manifest = {
    "corpus_version": 1,
    "oracle": "WABT wasm-decompile official test suite (test/decompile/*.txt)",
    "upstream": "https://github.com/WebAssembly/wabt",
    "upstream_version": "1.0.36",
    "license": "Apache-2.0 (see ../licenses/wabt-1.0.36-Apache-2.0.txt)",
    "generator": "generate-wasm.py",
    "wat2wasm_version": tool_version,
    "note": "Expected text is wasm-decompile output; rxd output is compared structurally, not byte-for-byte. Known-divergence features (struct detection, offset-after-index folding, full multi-value stack flushing) are documented in the test module.",
    "cases": cases,
}
out = os.path.join(HERE, "wasm-expectations.json")
json.dump(manifest, open(out, "w"), indent=2, ensure_ascii=False)
print(f"wrote {out} with {len(cases)} cases")
