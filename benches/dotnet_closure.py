#!/usr/bin/env python3
"""Replay the delivered .NET query matrix and returned-ID workflows on one release CLI."""
import argparse
import hashlib
import json
import os
import platform
import subprocess
import sys
import time
from pathlib import Path

from dotnet import digest, invoke, metrics, native_identity
from dotnet_class import verify_fetch
from dotnet_xref import sites_digest
from dotnet_strings import verify as verify_strings

ROOT = Path(__file__).resolve().parents[1]
PHASES = ('method-refs', 'field-type-refs', 'strings', 'class-il', 'class-csharp',
          'method-and-other-backends', 'other-backends', 'workflows')


def stream(data, source_id):
    rows = [json.loads(line) for line in data.splitlines()]
    assert rows[0]['record'] == 'header' and rows[0]['schema_version'] == 2
    assert rows[0]['source_id'] == source_id
    assert rows[-1]['record'] == 'summary' and rows[-1]['complete'] is True
    assert rows[-1]['records'] == len(rows) - 2
    return rows


def workflows(args, output):
    manifest = json.loads((ROOT / 'tests/corpus/dotnet/closure-expectations.json').read_text())
    source_id = manifest['provenance']['dll_sha256']
    assert digest(args.production) == source_id
    class_oracle = next(case for case in json.loads((ROOT / 'tests/corpus/dotnet/class-expectations.json').read_text())['workloads']
                        if case['token'] == manifest['caller']['declaring_type_token'])
    caller_token = manifest['caller']['method_token']
    caller_oracle = class_oracle['methods'][f'0x{caller_token:08x}']
    report = {'oracle': manifest, 'workloads': [], 'acceptance': 'pending',
              'measurement': 'All process launches, worker/read/hash/native/serialization/writes and JSON ID routing are timed. Independent correctness checks follow timing.',
              'workflow_contract': 'Totals reported separately; no numeric aggregate bound or exception. Each component query remains subject to 150 ms. These workflows do not select the approved JValue class.'}
    output.mkdir(parents=True, exist_ok=True)
    report_path = output / 'results.json'

    def save():
        report_path.write_text(json.dumps(report, indent=2) + '\n')

    for symbol in (True, False):
        for to_file in (False, True):
            label = ('outline-semantic-refs' if symbol else 'strings') + '-method-class-csharp-' + ('file' if to_file else 'stdout')
            destination = output / (label + '-class.json') if to_file else None

            def workflow():
                start = time.perf_counter_ns()
                data, steps = [], []

                def query(command):
                    result, elapsed = invoke(args.binary, command)
                    captured = destination.read_bytes() if '-o' in command else result.stdout
                    if '-o' in command:
                        assert result.stdout == b''
                    steps.append({'command': list(map(str, command)), 'duration_ms': elapsed,
                                  'output_bytes': len(captured), 'output_sha256': hashlib.sha256(captured).hexdigest()})
                    data.append(captured)
                    return captured

                if symbol:
                    outline = query(['dotnet', 'outline', args.production, '--type', manifest['target']['owner'], '--method', manifest['target']['method']])
                    rows = [json.loads(line) for line in outline.splitlines()]
                    target = next(row for row in rows if row['record'] == 'method' and row['detail']['token'] == manifest['target']['token'])
                    refs = query(['dotnet', 'refs', args.production, '--id', target['id']])
                    rows = [json.loads(line) for line in refs.splitlines()]
                    hit = next(row for row in rows if row['record'] == 'reference' and row['detail']['method_token'] == caller_token)
                else:
                    search = query(['dotnet', 'strings', args.production, '--contains', manifest['string']['contains']])
                    rows = [json.loads(line) for line in search.splitlines()]
                    hit = next(row for row in rows if row['record'] == 'string' and row['detail']['method_token'] == caller_token)
                query(['dotnet', 'get-method', args.production, hit['context_id']])
                command = ['dotnet', 'get-class', args.production, hit['detail']['declaring_type_id']]
                if destination:
                    command += ['-o', destination]
                query(command)
                return data, {'duration_ms': (time.perf_counter_ns() - start) / 1_000_000, 'steps': steps}

            item = {'name': label, 'input_bytes': args.production.stat().st_size, 'input_sha256': source_id,
                    'samples': [], 'component_latency_limit_ms': 150, 'aggregate_latency_limit_ms': None}
            report['workloads'].append(item)
            try:
                initial, initial_sample = workflow()
                item['initial_sample'] = initial_sample
                for index, data in enumerate(initial):
                    (output / f'{label}-step-{index}.json').write_bytes(data)
                if symbol:
                    outline = stream(initial[0], source_id)
                    targets = [row for row in outline[1:-1] if row['record'] == 'method']
                    assert manifest['target']['token'] in [row['detail']['token'] for row in targets]
                    refs = stream(initial[1], source_id)
                    assert refs[-1]['records'] == manifest['target']['site_count'] > 0
                    assert sites_digest(refs) == manifest['target']['site_sha256']
                    target_id = next(row['id'] for row in targets if row['detail']['token'] == manifest['target']['token'])
                    assert all(row['detail']['canonical_target_id'] == target_id and row['detail']['resolution_status'] == 'resolved' for row in refs[1:-1])
                    hit = next(row for row in refs[1:-1] if row['detail']['method_token'] == caller_token)
                else:
                    rows = verify_strings(initial[0], manifest['string'], source_id)
                    hit = next(row for row in rows[1:-1] if row['detail']['method_token'] == caller_token)
                method = json.loads(initial[-2])
                body = method['detail']
                assert method['id'] == hit['context_id'] and method['source_id'] == source_id and body['complete'] is True
                assert body['representation'] == 'csharp' and body['decompiled'] is True and body['code'] and body['reason'] is None
                assert body['method']['detail']['name'] == manifest['caller']['name']
                assert hashlib.sha256(bytes.fromhex(body['il_hex'])).hexdigest() == caller_oracle['il_sha256']
                assert body['handlers'] == caller_oracle['handlers']
                offsets = [hit['detail']['il_offset']] if symbol else hit['detail']['il_offsets']
                token = hit['detail']['target_token'] if symbol else hit['detail']['token']
                assert all(any(instruction['offset'] == offset and instruction['operand'] == {'kind': 'token', 'value': token} for instruction in body['instructions']) for offset in offsets)
                whole = verify_fetch(initial[-1], class_oracle, source_id, csharp=True)
                assert whole['id'] == hit['detail']['declaring_type_id']
                assert any(original['id'] == method['id'] for original in whole['detail']['methods'])
                for _ in range(args.samples):
                    actual, sample = workflow()
                    assert actual == initial, f'nondeterministic workflow: {label}'
                    item['samples'].append(sample)
                item.update(metrics([sample['duration_ms'] for sample in item['samples']]))
                item['component_latency_success'] = all(step['duration_ms'] <= 150 for sample in item['samples'] for step in sample['steps'])
                item['workflow_latency_success'] = None
                item['correctness_success'] = True
                save()
                print(label, f"median={item['median_ms']:.2f} P95={item['p95_ms']:.2f} ms; component bounds passed={item['component_latency_success']}", flush=True)
            except Exception as error:
                item['failure'] = str(error) or type(error).__name__
                report['acceptance'] = 'failed_correctness'
                save()
                raise
    report['acceptance'] = 'passed_components_totals_reported' if all(item['component_latency_success'] for item in report['workloads']) else 'failed_component_latency'
    save()
    if report['acceptance'] != 'passed_components_totals_reported':
        raise RuntimeError('workflow component exceeded ordinary 150 ms bound; all samples retained')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--native-library', type=Path, required=True)
    parser.add_argument('--production', type=Path, required=True)
    parser.add_argument('--framework', type=Path, required=True)
    parser.add_argument('--upstream', type=Path, required=True)
    parser.add_argument('--cpu-model', required=True)
    parser.add_argument('--memory-gb', type=int, required=True)
    parser.add_argument('--samples', type=int, default=20)
    parser.add_argument('--phase', choices=PHASES, action='append',
                        help='Run only the named phases; retain earlier failed reports in a different output directory')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/benchmarks/dotnet-closure')
    args = parser.parse_args()
    if args.samples < 20:
        parser.error('at least 20 fresh-process samples per workload are required')
    for path in (args.binary, args.native_library, args.production):
        if not path.is_file():
            parser.error(f'missing input: {path}')
    if os.environ.get('DOTNET_SYSTEM_GLOBALIZATION_INVARIANT'):
        parser.error('final acceptance must use the rebuilt native engine without a globalization environment override')
    os.environ['RXD_DOTNET_NATIVE_LIBRARY'] = str(args.native_library.resolve())
    os.environ['RXD_REFDIR'] = str(args.framework.resolve())
    args.output.mkdir(parents=True, exist_ok=True)
    report = {'binary': str(args.binary.resolve()), 'binary_sha256': digest(args.binary),
              'binary_bytes': args.binary.stat().st_size, 'engine': native_identity(args.native_library),
              'git_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'git_status': subprocess.check_output(['git', 'status', '--short'], cwd=ROOT, text=True),
              'cargo_lock_sha256': digest(ROOT / 'Cargo.lock'),
              'hardware': {'cpu': args.cpu_model, 'memory_gb': args.memory_gb, 'os': platform.platform(), 'machine': platform.machine(),
                           'start_load_average': os.getloadavg()},
              'conditions': 'One supplied stable release/native-feature binary. Warm OS cache from input/oracle preparation; no cache clearing or CPU isolation. Run the entire orchestrator under the task checked-command lock to exclude coordinated Cargo/test/benchmark overlap.',
              'limits': 'Unchanged query defaults; single-query 150 ms except exactly scoped approved JValue C#200 ms and designated pinned upstream CoreLib outline/Abs queries300 ms. Aggregate workflow timing has no approved numeric bound.',
              'requested_phases': args.phase or [phase for phase in PHASES if phase != 'other-backends'], 'phases': [], 'acceptance': 'pending'}
    report_path = args.output / 'results.json'
    common = ['--binary', args.binary, '--samples', args.samples, '--cpu-model', args.cpu_model, '--memory-gb', args.memory_gb]
    ref_args = ['--production', args.production, '--framework', args.framework, '--upstream', args.upstream]
    phases = [
        ('method-refs', 'dotnet_xref.py', ref_args),
        ('field-type-refs', 'dotnet_xref.py', [*ref_args, '--manifest', ROOT / 'tests/corpus/dotnet/field-type-refs-expectations.json']),
        ('strings', 'dotnet_strings.py', ['--input', args.production, '--probe-input', args.framework / 'System.Private.CoreLib.dll']),
        ('class-il', 'dotnet_class.py', ['--input', args.production]),
        ('class-csharp', 'dotnet_class.py', ['--input', args.production, '--csharp', '--native-library', args.native_library, '--framework', args.framework]),
        ('method-and-other-backends', 'dotnet.py', ['--native-library', args.native_library, '--framework', args.framework, '--references', args.upstream.parents[2]]),
    ]
    if args.phase and 'other-backends' in args.phase:
        phases.append(('other-backends', 'dotnet.py', ['--native-library', args.native_library, '--framework', args.framework, '--references', args.upstream.parents[2],
                      '--case', 'unaffected_js_entities', '--case', 'unaffected_asar_entries', '--case', 'unaffected_jar_methods']))
    for name, script, options in phases:
        if args.phase and name not in args.phase:
            continue
        destination = args.output / name
        destination.mkdir(parents=True, exist_ok=True)
        command = [sys.executable, ROOT / 'benches' / script, *common, *options, '--output', destination]
        with (destination / 'runner.log').open('w') as log:
            result = subprocess.run(list(map(str, command)), stdout=log, stderr=subprocess.STDOUT)
        item = {'name': name, 'command': list(map(str, command)), 'returncode': result.returncode,
                'report': str(destination / 'results.json')}
        report['phases'].append(item)
        if result.returncode:
            report['acceptance'] = 'failed_phase'
            report_path.write_text(json.dumps(report, indent=2) + '\n')
            raise SystemExit(f'Failed {name}; all evidence retained in {destination}')
        item['report_sha256'] = digest(destination / 'results.json')
        report_path.write_text(json.dumps(report, indent=2) + '\n')
        print(f'{name}: passed; retained individual workload reports', flush=True)
    if not args.phase or 'workflows' in args.phase:
        try:
            workflows(args, args.output / 'workflows')
        except Exception:
            report['acceptance'] = 'failed_workflow'
            report_path.write_text(json.dumps(report, indent=2) + '\n')
            raise
        report['phases'].append({'name': 'workflows', 'report': str(args.output / 'workflows/results.json'),
                                 'report_sha256': digest(args.output / 'workflows/results.json')})
    report['acceptance'] = 'passed_requested_phases' if args.phase else 'passed_selected_queries_and_workflow_components'
    report['hardware']['end_load_average'] = os.getloadavg()
    report_path.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
