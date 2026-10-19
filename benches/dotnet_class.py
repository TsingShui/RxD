#!/usr/bin/env python3
"""Fresh-process aggregate IL/C# acceptance against pinned independent production expectations."""
import argparse
import hashlib
import json
import platform
import os
from pathlib import Path
from dotnet import digest, invoke, metrics

ROOT = Path(__file__).resolve().parents[1]
JVALUE_CSHARP_SCOPE = {
    'package': 'Newtonsoft.Json', 'version': '13.0.3', 'representation': 'csharp',
    'input_sha256': '22c649f75fce5be7c7ccda8880473b634ef69ecf33f5d1ab8ad892caf47d5a07',
    'input_bytes': 712464, 'type_token': 0x020000c6,
    'selected_types': ['Newtonsoft.Json.Linq.JValue', 'Newtonsoft.Json.Linq.JValue+JValueDynamicProxy'],
    'methods': 66, 'instructions': 2064, 'upper_bound_ms': 200,
    'commands': ['get_class_csharp', 'get_class_csharp_file'],
}


def latency_policy(name, input_bytes, source_id, row):
    scope = JVALUE_CSHARP_SCOPE
    if row is not None and name in scope['commands']:
        body = row['detail']
        if (source_id == scope['input_sha256'] and row['source_id'] == source_id
                and input_bytes == scope['input_bytes']
                and body['representation'] == 'csharp' and body['decompiled'] is True
                and body['type']['detail']['token'] == scope['type_token']
                and sorted(ty['detail']['full_name'] for ty in body['types']) == scope['selected_types']
                and len(body['methods']) == scope['methods']
                and sum(len(method['detail']['instructions'] or []) for method in body['methods']) == scope['instructions']):
            return scope['upper_bound_ms'], scope
    return 150, None


def verify_fetch(data, expected, source_id, csharp=False):
    row = json.loads(data)
    assert row['record'] == 'class_body' and row['source_id'] == source_id
    body = row['detail']
    assert body['complete'] is True and body['decompiled'] is csharp
    if csharp:
        assert body['representation'] == 'csharp' and body['reason'] is None
        assert body['code'] and expected['name'].rsplit('.', 1)[-1] in body['code']
        lines = len(body['code'].splitlines())
        by_id = {method['id']: method['detail'] for method in body['methods']}
        for point in body['mapping']:
            method = by_id[point['method_id']]
            assert point['method_token'] == method['method']['detail']['token']
            assert 1 <= point['line'] <= lines
            assert any(instruction['offset'] == point['il_offset'] for instruction in method['instructions'])
    else:
        assert body['representation'] == 'il' and body['code'] is None
    assert sorted(ty['detail']['token'] for ty in body['types']) == expected['type_tokens']
    assert sorted(field['detail']['token'] for field in body['fields']) == expected['field_tokens']
    methods = {f"0x{item['detail']['method']['detail']['token']:08x}": item['detail'] for item in body['methods']}
    assert methods.keys() == expected['methods'].keys()
    for token, oracle in expected['methods'].items():
        method = methods[token]
        assert method['body_present'] == (not oracle['no_body'])
        if oracle['no_body']:
            assert method['il_hex'] is None
        else:
            assert hashlib.sha256(bytes.fromhex(method['il_hex'])).hexdigest() == oracle['il_sha256']
            assert method['handlers'] == oracle['handlers']
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/benchmarks/dotnet-class')
    parser.add_argument('--samples', type=int, default=20)
    parser.add_argument('--csharp', action='store_true', help='Require successful selected-class C# recovery')
    parser.add_argument('--native-library', type=Path)
    parser.add_argument('--framework', type=Path)
    parser.add_argument('--cpu-model', required=True)
    parser.add_argument('--memory-gb', type=int, required=True)
    args = parser.parse_args()
    if args.csharp:
        if not args.native_library or not args.native_library.is_file():
            parser.error('--csharp requires a trusted --native-library')
        os.environ['RXD_DOTNET_NATIVE_LIBRARY'] = str(args.native_library.resolve())
        if args.framework:
            os.environ['RXD_REFDIR'] = str(args.framework.resolve())
    if args.samples < 20:
        parser.error('at least 20 samples are required')
    manifest = json.loads((ROOT / 'tests/corpus/dotnet/class-expectations.json').read_text())
    source_id = manifest['provenance']['dll_sha256']
    if digest(args.input) != source_id:
        parser.error('input hash does not match pinned Newtonsoft.Json 13.0.3 net6.0 assembly')
    args.output.mkdir(parents=True, exist_ok=True)
    report = {
        'binary': str(args.binary.resolve()), 'binary_sha256': digest(args.binary),
        'binary_bytes': args.binary.stat().st_size, 'provenance': manifest['provenance'],
        'hardware': {'cpu': args.cpu_model, 'memory_gb': args.memory_gb, 'platform': platform.platform(), 'machine': platform.machine()},
        'cache': 'warm OS file cache; an initial validation query; no cache clearing; each sample starts a fresh CLI and worker',
        'measurement_conditions': 'No CPU isolation. End-to-end wall time includes startup, worker initialization, immutable read/hash, parse, cumulative class traversal/decode, serialization and stdout pipe or atomic file writes. Independent verification is outside timed region.',
        'limitations': ['Selected two production classes; no assembly-wide or cold-cache claim', 'Selected-class C# recovery required' if args.csharp else 'IL retrieval only; no C# decompiler latency claim', 'Concurrent host work is not excluded'],
        'workloads': [], 'acceptance': 'pending',
    }
    if args.csharp:
        report['engine'] = {'path': str(args.native_library.resolve()), 'bytes': args.native_library.stat().st_size, 'sha256': digest(args.native_library), 'framework_reference_directories': os.environ.get('RXD_REFDIR', '').split(os.pathsep), 'globalization_environment_override': os.environ.get('DOTNET_SYSTEM_GLOBALIZATION_INVARIANT')}
    for expected in manifest['workloads']:
        identity = f"rxd1:dotnet:{source_id}:type:{expected['token']:08x}"
        representation = 'csharp' if args.csharp else 'il'
        mode = [] if args.csharp else ['--il']
        commands = [('outline', ['dotnet', 'outline', args.input, '--type', expected['name']]),
                    ('get_class_' + representation, ['dotnet', 'get-class', args.input, identity, *mode]),
                    ('get_class_' + representation + '_file', ['dotnet', 'get-class', args.input, identity, *mode, '-o', args.output / 'class.json'])]
        for name, command in commands:
            destination = Path(command[command.index('-o')+1]) if '-o' in command else None
            try:
                initial, initial_ms = invoke(args.binary, command)
            except RuntimeError as error:
                report['acceptance'] = 'failed_query'
                report['workloads'].append({'name': f"{expected['name']}/{name}", 'command': list(map(str, command)), 'failure': str(error), 'input_bytes': args.input.stat().st_size, 'input_sha256': source_id})
                (args.output / 'results.json').write_text(json.dumps(report, indent=2)+'\n')
                raise SystemExit(f'Query failed; evidence retained: {error}')
            data = destination.read_bytes() if destination else initial.stdout
            fetch_row = None
            if name == 'outline':
                rows = [json.loads(line) for line in data.splitlines()]
                assert rows[-1]['complete'] is True
                assert next(row for row in rows if row['record'] == 'type')['id'] == identity
            else:
                try:
                    fetch_row = verify_fetch(data, expected, source_id, args.csharp)
                except AssertionError as error:
                    (args.output / f"{expected['token']:08x}-{name}-failed.json").write_bytes(data)
                    report['acceptance'] = 'failed_correctness'
                    report['workloads'].append({'name': f"{expected['name']}/{name}", 'command': list(map(str, command)), 'failure': str(error) or 'Recovery/original-scope mismatch'})
                    (args.output / 'results.json').write_text(json.dumps(report, indent=2)+'\n')
                    raise SystemExit('Recovery correctness failed; evidence retained')
            (args.output / f"{expected['token']:08x}-{name}.json").write_bytes(data)
            samples = []
            for _ in range(args.samples):
                result, elapsed = invoke(args.binary, command)
                actual = destination.read_bytes() if destination else result.stdout
                assert actual == data, f'nondeterministic output: {name}'
                samples.append(elapsed)
            item = {'name': f"{expected['name']}/{name}", 'command': list(map(str, command)),
                    'input_bytes': args.input.stat().st_size, 'input_sha256': source_id,
                    'output_bytes': len(data), 'output_sha256': hashlib.sha256(data).hexdigest(),
                    'initial_ms': initial_ms, **metrics(samples)}
            if fetch_row is not None:
                scope = fetch_row['detail']
                item['class_scope'] = {'types': len(scope['types']), 'fields': len(scope['fields']), 'methods': len(scope['methods']), 'instructions': sum(len(method['detail']['instructions'] or []) for method in scope['methods']), 'csharp_bytes': len(scope['code'].encode()) if scope['code'] is not None else None}
            upper_bound, latency_scope = latency_policy(name, args.input.stat().st_size, source_id, fetch_row)
            item['latency_limit_ms'] = upper_bound
            item['latency_scope'] = latency_scope
            item['over_limit_proportion'] = sum(value > upper_bound for value in samples) / len(samples)
            item['latency_success'] = item['over_limit_proportion'] == 0
            report['workloads'].append(item)
            report['acceptance'] = 'failed_latency' if not item['latency_success'] else 'pending'
            (args.output / 'results.json').write_text(json.dumps(report, indent=2)+'\n')
            print(f"{item['name']}: median={item['median_ms']:.2f} P95={item['p95_ms']:.2f} >150ms={item['over_150_proportion']:.0%} bound={upper_bound}ms over_bound={item['over_limit_proportion']:.0%}", flush=True)
    report['acceptance'] = 'passed_selected_workloads' if all(item['latency_success'] for item in report['workloads']) else 'failed_latency'
    (args.output / 'results.json').write_text(json.dumps(report, indent=2)+'\n')
    if report['acceptance'] != 'passed_selected_workloads':
        raise SystemExit('Applicable latency bound failed; exact samples and scope retained')


if __name__ == '__main__':
    main()
