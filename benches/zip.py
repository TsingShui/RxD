#!/usr/bin/env python3
"""Fresh release CLI processes with complete atomic output writes; no input execution."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import struct
import subprocess
import time
import zipfile
import zlib

ROOT = Path(__file__).resolve().parents[1]
os.chdir(ROOT)
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--skip-build", action="store_true")
parser.add_argument("--samples", type=int, default=20)
options = parser.parse_args()
if options.samples < 1:
    parser.error("--samples must be positive")
if not options.skip_build:
    env = {**os.environ, "CARGO_BUILD_JOBS": os.environ.get("CARGO_BUILD_JOBS", "2")}
    subprocess.run(["cargo", "build", "--release", "-p", "rxd"], env=env, check=True)
OUT = ROOT / 'target/benchmarks/zip'
INPUTS = OUT / 'inputs'
BINARY = ROOT / 'target/release/rxd'
INPUTS.mkdir(exist_ok=True)

def digest(path):
    state = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            state.update(chunk)
    return state.hexdigest()

def checked(args):
    result = subprocess.run(list(map(str, args)), capture_output=True, timeout=30)
    if result.returncode:
        raise RuntimeError((args, result.returncode, result.stderr.decode(errors='replace')))
    return result

def metadata(args):
    return [json.loads(line) for line in checked([BINARY, *args]).stdout.splitlines()]

def cmd(args):
    return subprocess.check_output(args, text=True).strip()

report = {
    'binary': str(BINARY), 'binary_sha256': digest(BINARY),
    'build': 'Existing release binary (--skip-build)' if options.skip_build else 'cargo build --release -p rxd; default all-feature, no optional Kuna/ILSpy engines',
    'git_head': cmd(['git', 'rev-parse', 'HEAD']),
    'source_sha256': {str(path): digest(path) for path in [
        Path('Cargo.toml'), Path('Cargo.lock'), Path('src/cli/input.rs'), Path('src/skills/rxd/SKILL.md'),
        *sorted(Path('crates/rxd-zip').rglob('*.rs')), *sorted(Path('src/cli/zip').glob('*.rs')),
        Path('src/cli/js/mod.rs'), Path('src/cli/html/mod.rs'), Path('src/cli/wasm/mod.rs'),
    ]},
    'hardware': {'cpu':cmd(['sysctl','-n','machdep.cpu.brand_string']), 'memory_bytes':cmd(['sysctl','-n','hw.memsize']),
                 'platform':platform.platform(), 'cpu_count':os.cpu_count()},
    'toolchain': cmd(['rustc','-V']),
    'cache_conditions': 'Warm OS file caches following hashing and untimed correctness checks; no cache eviction, CPU isolation or concurrent owned builds/tests; shared interactive workstation.',
    'timing_boundary': 'perf_counter_ns immediately around a fresh subprocess.run through exit and complete stdout/stderr capture. Every query uses -o with actual CLI-owned atomic output writes. Includes parent/worker startup, container detection/indexing, reading/decoding, analysis, serialization, output writes/publication. Post-run hash/parity checks excluded.',
    'scope': 'Published AndroidX AAR, Red Hat YAML VSIX and Gson JAR; direct JS uses the published VSIX payload. HTML ZIP/ASAR carriers are synthetic with maintained website payloads. WASM payloads are handwritten fixtures, not production-wide WASM evidence. Sparse/index-scale carriers are synthetic and separately labeled. ASAR rows are current acceptance measurements, not before/after comparisons.',
    'started_unix_ns':time.time_ns(), 'load_start':os.getloadavg(), 'workloads':[],
}

def measure(name, backend, query, carrier, tail=(), *, entry_bytes=None, artifact_bytes=None, provenance=None):
    result_path = OUT / ('artifact.bin' if artifact_bytes is not None else 'result.jsonl')
    args = [BINARY, backend, query, carrier, *tail, '-o', result_path]
    oracle = checked(args)
    expected_stdout = oracle.stdout
    expected_output = result_path.read_bytes()
    if artifact_bytes is not None:
        assert expected_output == artifact_bytes, name
    samples = []
    for _ in range(options.samples):
        started = time.perf_counter_ns()
        result = checked(args)
        duration = time.perf_counter_ns() - started
        assert result.stdout == expected_stdout and not result.stderr, name
        assert result_path.read_bytes() == expected_output, name
        samples.append({'duration_ns':duration, 'exit_code':result.returncode,
                        'stdout_bytes':len(result.stdout), 'artifact_bytes':len(expected_output)})
    times = sorted(sample['duration_ns'] for sample in samples)
    over = sum(t > 150_000_000 for t in times)
    row = {'name':name, 'argv':list(map(str,args)), 'carrier_bytes':carrier.stat().st_size,
           'carrier_sha256':digest(carrier), 'entry_bytes':entry_bytes,
           'provenance':provenance, 'median_ms':statistics.median(times)/1e6,
           'p95_ms':times[math.ceil(.95*len(times))-1]/1e6,
           'over_150ms_count':over, 'over_150ms_proportion':over/len(times),
           'output_bytes':len(expected_output), 'output_sha256':hashlib.sha256(expected_output).hexdigest(),
           'stdout_bytes':len(expected_stdout), 'samples':samples}
    report['workloads'].append(row)
    (OUT/'results.json').write_text(json.dumps(report,indent=2)+'\n')
    print(f'{name}: median={row["median_ms"]:.2f}ms p95={row["p95_ms"]:.2f}ms >150={over}/{options.samples} bytes={len(expected_output)}',flush=True)
    return expected_output

corpus = ROOT / 'tests/corpus/zip'
aar = corpus/'core-1.15.0.aar'
vsix = corpus/'vscode-yaml-2.0.0-400.vsix'
for expected in json.loads((corpus/'expectations.json').read_text()):
    assert digest(corpus/expected['file']) == expected['sha256']
with zipfile.ZipFile(aar) as z:
    classes = z.read('classes.jar')
with zipfile.ZipFile(vsix) as z:
    source = z.read('extension/dist/extension.js')
plain_js = INPUTS/'extension.js'
plain_js.write_bytes(source)
measure('production/aar-entries','zip','entries',aar,provenance='AndroidX Core 1.15.0; tests/corpus/zip/README.md')
measure('production/aar-extract-classes-jar','zip','extract',aar,['--at','classes.jar'],entry_bytes=len(classes),artifact_bytes=classes,provenance='Published AAR; explicit selected classes.jar, no nested analysis')
measure('production/vsix-entries','zip','entries',vsix,provenance='Red Hat YAML 2.0.0 build 400; tests/corpus/zip/README.md')
measure('production/vsix-extract-extension','zip','extract',vsix,['--at','extension/dist/extension.js'],entry_bytes=len(source),artifact_bytes=source,provenance='Published VSIX; selected extension.js')
direct = measure('production/vsix-js-functions','js','functions',vsix,['--entry','extension/dist/extension.js'],entry_bytes=len(source),provenance='Published VSIX, 745879-byte shipped JS')
ordinary = measure('production/plain-js-functions-control','js','functions',plain_js,entry_bytes=len(source),provenance='Byte-identical decoded VSIX JS control')
assert direct == ordinary
measure('production/vsix-js-find','js','find',vsix,['--entry','extension/dist/extension.js','-f','yaml'],entry_bytes=len(source),provenance='Published VSIX, decoded literal search')
function = next(row for row in metadata(['js','functions',vsix,'--entry','extension/dist/extension.js']) if row['end']-row['start'] < 2000)
measure('production/vsix-js-slice','js','slice',vsix,['--entry','zip:18','--at',function['at'],'--json'],entry_bytes=len(source),provenance={'published_payload':'extension/dist/extension.js','selected_at':function['at'],'selected_source_bytes':function['end']-function['start']})
gson = ROOT/'tests/corpus/jvm/gson-2.11.0.jar'
measure('production/gson-jar-zip-entries','zip','entries',gson,provenance='Published Gson 2.11.0 JAR; tests/corpus/jvm/README.md')

html = (ROOT/'site/index.html').read_bytes()
wasm_path = ROOT/'tests/fixtures/wasm/fixture.wasm'
wasm = wasm_path.read_bytes()
web = INPUTS/'web.zip'
with zipfile.ZipFile(web,'w',compression=zipfile.ZIP_DEFLATED) as z:
    z.writestr('index.html',html)
    z.writestr('core.wasm',wasm)
measure('synthetic-zip/maintained-html-resources','html','resources',web,['--entry','index.html'],entry_bytes=len(html),provenance='Synthetic ZIP carrier; maintained site/index.html payload')
measure('synthetic-zip/handwritten-wasm-functions','wasm','functions',web,['--entry','core.wasm'],entry_bytes=len(wasm),provenance='Synthetic ZIP carrier; handwritten WASM fixture')
at = metadata(['wasm','functions',web,'--entry','core.wasm'])[0]['at']
measure('synthetic-zip/handwritten-wasm-raw-fetch','wasm','get-function',web,['--entry','core.wasm','--at',at,'--raw'],entry_bytes=len(wasm),provenance='Synthetic ZIP carrier; handwritten WASM fixture')

small = b'<main><script>const value=7;</script></main>'
indexed = INPUTS/'index5000.zip'
with zipfile.ZipFile(indexed,'w',compression=zipfile.ZIP_DEFLATED) as z:
    z.writestr('selected.html',small)
    for i in range(5000): z.writestr(f'assets/file-{i:05d}',b'')
measure('synthetic-scale/index5000-small-html','html','entries',indexed,['--entry','selected.html'],entry_bytes=len(small),provenance='5000 unrelated empty entries, one 44-byte HTML target')

sparse = INPUTS/'sparse256MiB.zip'
padding = 256*1024*1024
crc = 0
chunk = bytes(1024*1024)
for _ in range(256): crc = zlib.crc32(chunk,crc)
central = []
with sparse.open('wb') as f:
    for name,data,size,checksum in [('selected.html',small,len(small),zlib.crc32(small)),('large.bin',None,padding,crc)]:
        name=name.encode(); offset=f.tell()
        f.write(struct.pack('<IHHHHHIIIHH',0x04034b50,20,0,0,0,0,checksum,size,size,len(name),0));f.write(name)
        if data is None: f.seek(size,1)
        else: f.write(data)
        central.append(struct.pack('<IHHHHHHIIIHHHHHII',0x02014b50,20,20,0,0,0,0,checksum,size,size,len(name),0,0,0,0,0,offset)+name)
    start=f.tell(); directory=b''.join(central);f.write(directory)
    f.write(struct.pack('<IHHHHIIH',0x06054b50,0,0,2,2,len(directory),start,0))
with zipfile.ZipFile(sparse) as z: assert z.read('selected.html') == small
measure('synthetic-scale/sparse256MiB-small-html','html','entries',sparse,['--entry','selected.html'],entry_bytes=len(small),provenance='Valid sparse Stored ZIP with 256MiB unrelated zero entry and a 44-byte HTML target')

def asar(name, entry, data):
    path = INPUTS / name
    header = json.dumps({'files':{entry:{'size':len(data),'offset':'0'}}},separators=(',',':')).encode()
    payload = (len(header)+7)//4*4
    path.write_bytes(struct.pack('<IIII',4,payload+4,payload,len(header)) +
                     header + bytes(payload-4-len(header)) + data)
    return path

site_asar = asar('site.asar','index.html',html)
wasm_asar = asar('wasm.asar','core.wasm',wasm)
measure('asar-acceptance/maintained-html','html','entries',site_asar,['--entry','index.html'],entry_bytes=len(html),provenance='Synthetic ASAR carrier; maintained site/index.html')
measure('asar-acceptance/handwritten-wasm','wasm','functions',wasm_asar,['--entry','core.wasm'],entry_bytes=len(wasm),provenance='Synthetic ASAR carrier; handwritten fixture')
case=json.loads((ROOT/'tests/corpus/manifest.json').read_text())['cases'][0]
js_source = ROOT/'tests/corpus'/case['file']
assert digest(js_source) == case['sha256']
js_asar = asar('production-js.asar','bundle.js',js_source.read_bytes())
measure('asar-acceptance/production-js','js','find',js_asar,['--entry','bundle.js','-f',case['needle']],entry_bytes=js_source.stat().st_size,provenance={'synthetic_container':True,'production_payload':case['file']})
report.update(finished_unix_ns=time.time_ns(),load_end=os.getloadavg(),script_sha256=digest(Path(__file__)))
(OUT/'results.json').write_text(json.dumps(report,indent=2)+'\n')
for name in ['result.jsonl','artifact.bin','vsix-functions.jsonl']:
    (OUT/name).unlink(missing_ok=True)
print('workloads',len(report['workloads']),'samples',sum(len(row['samples']) for row in report['workloads']))
