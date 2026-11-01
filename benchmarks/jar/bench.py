#!/usr/bin/env python3
"""Compare real JAR product queries; reports and downloaded tools stay under target/."""
import argparse
import datetime
import hashlib
import html
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
REPOS = {'jadx': 'skylot/jadx', 'cfr': 'leibnitz27/cfr',
         'jar-analyzer-engine': 'jar-analyzer/jar-analyzer-engine'}
JAVA = ['java', '-XX:ActiveProcessorCount=2', '-Xmx2g']
CASES = [
    ('gson', 'tests/corpus/jvm/gson-2.11.0.jar', 'com/google/gson/JsonPrimitive', 'isNumber', '()Z'),
    ('rhino', 'tests/corpus/jvm/rhino-1.7.15.jar', 'org/mozilla/javascript/ClassShutter', 'visibleToScripts', '(Ljava/lang/String;)Z'),
    ('hadoop', 'target/benchmarks/jar/inputs/hadoop-client-runtime-3.4.1.jar',
     'org/apache/hadoop/shaded/org/apache/commons/lang3/CharUtils', 'isAscii', '(C)Z'),
]


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def identity(path):
    return {'path': str(path), 'bytes': path.stat().st_size, 'sha256': digest(path)}


def command(argv, **kwargs):
    return subprocess.run([str(x) for x in argv], capture_output=True, timeout=180, **kwargs)


def latest_tools(base):
    tools = {}
    for name, repo in REPOS.items():
        result = command(['gh', 'release', 'view', '-R', repo, '--json', 'tagName,assets'])
        result.check_returncode()
        release = json.loads(result.stdout)
        version = release['tagName'].removeprefix('v')
        filename = {'jadx': f'jadx-{version}.zip', 'cfr': f'cfr-{version}.jar',
                    'jar-analyzer-engine': f'jar-analyzer-engine-{version}.jar'}[name]
        asset = next(a for a in release['assets'] if a['name'] == filename)
        archive = base / filename
        if not archive.exists():
            downloaded = command(['gh', 'release', 'download', release['tagName'], '-R', repo,
                                  '-p', filename, '-D', base])
            downloaded.check_returncode()
        expected = asset.get('digest')
        if expected and expected != 'sha256:' + digest(archive):
            raise ValueError(f'release digest mismatch: {archive}')
        extracted = base / f'{name}-{version}-extracted'
        if name == 'jadx' and not extracted.exists():
            import zipfile
            with zipfile.ZipFile(archive) as source:
                for entry in source.infolist():
                    if not (extracted / entry.filename).resolve().is_relative_to(extracted.resolve()):
                        raise ValueError('unsafe release archive path')
                source.extractall(extracted)
        if name == 'jadx':
            executable = next(extracted.rglob('bin/jadx'))
            executable.chmod(0o755)
        else:
            executable = archive
        tools[name] = {'repo': repo, 'release': release['tagName'], 'asset_url': asset['url'],
                       'asset': identity(archive), 'executable': str(executable.resolve())}
    return tools


def oracle(path, owner):
    result = command(['javap', '-p', '-s', '-classpath', path, owner.replace('/', '.')])
    result.check_returncode()
    methods = set()
    previous = ''
    for line in result.stdout.decode().splitlines():
        line = line.strip()
        if line.startswith('descriptor:') and '(' in line:
            name = '<clinit>' if previous == 'static {};' else previous.split('(')[0].split()[-1]
            if name == owner.replace('/', '.'):
                name = '<init>'
            methods.add((name, line.split(':', 1)[1].strip()))
        previous = line
    if not methods:
        raise ValueError('empty independent javap method inventory')
    return methods


def validate_source(source, owner, methods):
    simple = owner.rsplit('/', 1)[-1]
    if not re.search(r'\b(class|interface|enum)\s+' + re.escape(simple) + r'\b', source):
        raise ValueError('wrong or missing reconstructed class')
    missing = [name for name, _ in methods if not name.startswith('<')
               and not re.search(r'\b' + re.escape(name) + r'\s*\(', source)]
    if missing:
        raise ValueError('missing declared method names: ' + str(sorted(set(missing))))
    return {'method_name_inventory': 'verified against javap (not overload/body equivalence)',
            'semantic_equivalence': 'not established',
            'diagnostic_markers': len(re.findall(r'unsupported opcode|INVALID_INPUT|could not decompile|Method not decompiled', source, re.I))}


def summarize(samples):
    values = sorted(s['ms'] for s in samples if s['status'] == 'ok')
    return {'samples': samples,
            'first_sample_ms': samples[0]['ms'] if samples and samples[0]['status'] == 'ok' else None,
            'subsequent_median_ms': statistics.median(s['ms'] for s in samples[1:] if s['status'] == 'ok') if any(s['status'] == 'ok' for s in samples[1:]) else None,
            'success_count': len(values),
            'failure_count': len(samples) - len(values),
            'median_ms': statistics.median(values) if values else None,
            'p95_ms': values[math.ceil(.95 * len(values)) - 1] if values else None,
            'max_ms': max(values) if values else None,
            'over_150ms_count': sum(v > 150 for v in values)}


def chart(report, output):
    rows = []
    colors = {'rxd': '#2563eb', 'jadx': '#d97706', 'cfr': '#059669', 'jar-analyzer-engine': '#9333ea'}
    for case in report['cases']:
        for row in case['queries']:
            if row['operation'] == 'get-class':
                rows.append((case['name'], row))
    maximum = max((row['median_ms'] for _, row in rows if row['median_ms'] is not None), default=1)
    height = 160 + len(rows) * 42
    svg = [f'<svg xmlns="http://www.w3.org/2000/svg" width="1400" height="{height}" viewBox="0 0 1400 {height}">',
           '<rect width="100%" height="100%" fill="white"/>',
           '<g font-family="sans-serif" font-size="14" fill="#111827">',
           '<text x="20" y="30" font-size="22">JAR selected-class latency — median milliseconds</text>',
           '<text x="20" y="58">Fresh CLI processes. Jar Analyzer Engine includes automatic indexing and first class reconstruction, every sample.</text>',
           '<text x="20" y="80">Source bodies are not proven equivalent. See result.json for failures, P95 and initialization costs.</text>']
    for index, (name, row) in enumerate(rows):
        y = 115 + index * 42
        label = f'{name} / {row["tool"]}'
        if row['median_ms'] is None:
            svg.append(f'<text x="20" y="{y + 17}" fill="#b91c1c">{html.escape(label)} — FAILED validation ({row["failure_count"]}/{len(row["samples"])} samples); see result.json</text>')
            continue
        width = max(1, row['median_ms'] / maximum * 600)
        svg += [f'<text x="20" y="{y + 17}">{html.escape(label)}</text>',
                f'<rect x="250" y="{y}" width="{width:.2f}" height="25" fill="{colors[row["tool"]]}"/>',
                f'<text x="{260 + width:.2f}" y="{y + 17}">{row["median_ms"]:.2f} ms (P95 {row["p95_ms"]:.2f}; first {row["first_sample_ms"]:.2f})</text>']
    svg += ['</g></svg>']
    (output / 'comparison.svg').write_text('\n'.join(svg) + '\n')
    (output / 'index.html').write_text('<!doctype html><meta charset="utf-8"><title>JAR benchmark</title>'
                                      '<img src="comparison.svg" alt="Latency comparison">'
                                      '<p><a href="result.json">Full measurements and methodology</a></p>')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/perf-tests/jvm-nested/rxd')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/benchmarks/jar/engine-run')
    parser.add_argument('--samples', type=int, default=10)
    parser.add_argument('--case', action='append', choices=[c[0] for c in CASES])
    args = parser.parse_args()
    if args.samples < 1:
        parser.error('--samples must be positive')
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve()
    frozen = args.output / 'rxd'
    if binary != frozen:
        shutil.copy2(binary, frozen)
    tools = latest_tools(ROOT / 'target/benchmarks/jar/tools')
    report = {'created_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'hardware': platform.platform(), 'processor': subprocess.check_output(['sysctl', '-n', 'machdep.cpu.brand_string'], text=True).strip() if platform.system() == 'Darwin' else platform.processor(),
              'cpu_count': os.cpu_count(), 'java': command(['java', '-version']).stderr.decode(),
              'java_flags': JAVA[1:], 'jadx_threads': 2, 'rxd': identity(frozen),
              'runner': identity(Path(__file__).resolve()),
              'git_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'tools': tools, 'cache': 'filesystem cache not flushed; processes fresh each sample; Jar Analyzer Engine database, extracted classes and source cache rebuilt each sample',
              'scope': 'selected class and method inventory only; other rxd capabilities have no adapters yet, not claimed unsupported by products',
              'cases': []}

    def save():
        (args.output / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
        chart(report, args.output)

    for label, relative, owner, method, descriptor in CASES:
        if args.case and label not in args.case:
            continue
        path = (ROOT / relative).resolve()
        expected_hash = {
            'gson': '57928d6e5a6edeb2abd3770a8f95ba44dce45f3b23b7a9dc2b309c581552a78b',
            'rhino': '2427fdcbc149ca0a25ccfbb7c71b01f39ad42708773a47816cd2342861766b63',
            'hadoop': 'f6a800a159f918670db533606d33560d6c13b7e13f14eda493280ae33b9eeb2f',
        }[label]
        if digest(path) != expected_hash:
            raise ValueError(f'input hash mismatch: {path}')
        methods = oracle(path, owner)
        import zipfile
        with zipfile.ZipFile(path) as archive:
            class_bytes = archive.getinfo(owner + '.class').file_size
            class_entries = sum(n.endswith('.class') for n in archive.namelist())
        case = {'name': label, 'input': identity(path), 'class': owner,
                'class_bytes': class_bytes, 'class_entries': class_entries,
                'input_provenance': 'tests/corpus/jvm/README.md' if label != 'hadoop' else 'https://repo.maven.apache.org/maven2/org/apache/hadoop/hadoop-client-runtime/3.4.1/hadoop-client-runtime-3.4.1.jar',
                'javap_methods': sorted(methods), 'queries': []}
        report['cases'].append(case)
        target = args.output / label
        target.mkdir(exist_ok=True)
        for tool in ['rxd', 'jadx', 'cfr', 'jar-analyzer-engine']:
            samples = []
            for i in range(args.samples):
                dest = target / f'{tool}-{i}'
                if dest.exists():
                    shutil.rmtree(dest)
                dest.mkdir()
                if tool == 'rxd':
                    argv = [frozen, 'jar', 'get-class', path, '--at', 'L' + owner + ';', '-o', dest / 'class.java']
                elif tool == 'jadx':
                    argv = [tools[tool]['executable'], '-j', '2', '-r', '--single-class', owner.replace('/', '.'),
                            '--single-class-output', dest / 'class.java', path]
                elif tool == 'jar-analyzer-engine':
                    argv = [*JAVA, '-jar', tools[tool]['executable'], '--jar', path,
                            '--decompile', owner.replace('/', '.'), '--log-level', 'ERROR']
                else:
                    argv = [*JAVA, '-jar', tools[tool]['executable'], path, '--jarfilter',
                            '^' + re.escape(owner.replace('/', '.')) + '$', '--outputdir', dest, '--silent', 'true']
                env = dict(os.environ)
                if tool == 'jadx':
                    env['JAVA_OPTS'] = ' '.join(JAVA[1:])
                sample = {'argv': [str(a) for a in argv]}
                started = time.perf_counter_ns()
                try:
                    result = command(argv, env=env, cwd=dest)
                    sample['ms'] = (time.perf_counter_ns() - started) / 1e6
                    (dest / 'stderr.log').write_bytes(result.stderr)
                    result.check_returncode()
                    if tool == 'jar-analyzer-engine':
                        (dest / 'stdout.log').write_bytes(result.stdout)
                        source = result.stdout.decode()
                        marker = source.find('//\n// Jar Analyzer Engine')
                        if marker < 0:
                            raise ValueError('missing Jar Analyzer Engine source block')
                        (dest / 'class.java').write_text(source[marker:])
                        sample['index_created'] = (dest / 'jar-analyzer.db').is_file()
                        sample['database_bytes'] = (dest / 'jar-analyzer.db').stat().st_size if sample['index_created'] else 0
                    files = [dest / 'class.java'] if tool == 'jar-analyzer-engine' else list(dest.rglob('*.java'))
                    if len(files) != 1:
                        raise ValueError(f'expected one source file, got {len(files)}')
                    source = files[0].read_text()
                    sample['verification'] = validate_source(source, owner, methods)
                    sample.update(status='ok', output=identity(files[0]))
                except (OSError, ValueError, subprocess.SubprocessError) as error:
                    sample.update(status='failure', failure=str(error), ms=(time.perf_counter_ns()-started)/1e6)
                samples.append(sample)
                if i:
                    shutil.rmtree(dest)
            row = {'tool': tool, 'operation': 'get-class', 'boundary': 'fresh CLI process, input read, reconstruction and file/pipe writes; Jar Analyzer Engine automatic index construction included', **summarize(samples)}
            case['queries'].append(row)
            print(label, tool, row['median_ms'], 'failures', row['failure_count'], flush=True)
            save()
    print('Report:', args.output / 'result.json')
    print('Chart:', args.output / 'index.html')


if __name__ == '__main__':
    main()
