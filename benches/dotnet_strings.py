#!/usr/bin/env python3
"""Complete string-navigation acceptance, timed as fresh CLI and worker processes."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import time

from dotnet import digest, invoke, metrics
from dotnet_strings_oracle import canonical_hash

ROOT = Path(__file__).resolve().parents[1]


def entity_token(identity, source_id, kind):
    prefix = f'rxd1:dotnet:{source_id}:{kind}:'
    assert identity.startswith(prefix)
    encoded = identity[len(prefix):]
    assert len(encoded) == 8
    return int(encoded, 16)


def verify(data, expected, source_id):
    rows = [json.loads(line) for line in data.splitlines()]
    assert rows[0]['record'] == 'header' and rows[0]['schema_version'] == 2
    assert rows[0]['source_id'] == source_id
    assert rows[-1]['record'] == 'summary' and rows[-1]['complete'] is True
    assert rows[-1]['records'] == len(rows) - 2 == expected['records']
    canonical = []
    uses = 0
    for row in rows[1:-1]:
        assert row['source_id'] == source_id
        detail = row['detail']
        method = entity_token(row['context_id'], source_id, 'method') if row['context_id'] else None
        owner = entity_token(detail['declaring_type_id'], source_id, 'type') if detail['declaring_type_id'] else None
        assert detail['method_token'] == method
        if expected['string'] is not None:
            assert row['record'] == 'reference' and detail['opcode'] == 'ldstr'
            assert method is not None and owner is not None
            assert row['end'] == row['start'] + 5
            canonical.append([method, detail['il_offset'], detail['target_token'], owner])
            uses += 1
        else:
            assert row['record'] == 'string' and row['start'] is None and row['end'] is None
            assert row['parent_id'] == detail['declaring_type_id']
            assert detail['status'] == ('referenced' if method else 'unreferenced')
            assert detail['il_offsets'] == sorted(set(detail['il_offsets']))
            canonical.append([detail['offset'], detail['token'], detail['value'], detail['utf16_units'], method, owner, detail['il_offsets']])
            uses += len(detail['il_offsets'])
    assert canonical_hash(canonical) == expected['canonical_sha256']
    assert uses == expected['uses'] and uses > 0
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/benchmarks/dotnet-strings')
    parser.add_argument('--samples', type=int, default=20)
    parser.add_argument('--cpu-model', required=True)
    parser.add_argument('--memory-gb', type=int, required=True)
    parser.add_argument('--probe-input', type=Path, action='append', default=[])
    args = parser.parse_args()
    if args.samples < 20:
        parser.error('at least 20 fresh-process samples are required')
    manifest = json.loads((ROOT / 'tests/corpus/dotnet/string-expectations.json').read_text())
    source_id = manifest['provenance']['dll_sha256']
    if digest(args.input) != source_id:
        parser.error('input does not match pinned Newtonsoft.Json 13.0.3 net6.0 assembly')
    args.output.mkdir(parents=True, exist_ok=True)
    report = {'binary': str(args.binary.resolve()), 'binary_sha256': digest(args.binary),
              'binary_bytes': args.binary.stat().st_size, 'provenance': manifest['provenance'],
              'oracle': manifest['oracle'], 'metadata_oracle_sha256': manifest['metadata_oracle_sha256'],
              'native_engine_used': False, 'budgets': 'Shipped default budgets; no max-* override in any command',
              'hardware': {'cpu': args.cpu_model, 'memory_gb': args.memory_gb, 'platform': platform.platform(), 'machine': platform.machine()},
              'cache': 'warm OS file cache; initial correctness queries; no cache clearing; every timed query starts fresh CLI + worker',
              'measurement_conditions': 'No CPU isolation. Includes startup, worker, immutable input read/hash, metadata and complete heap/IL validation, ownership grouping, serialization, stdout pipes/atomic files. Correctness checks run outside timed single-query regions.',
              'limitations': ['One independent published production assembly; no cold-cache claim', 'Whole-input probes are retained correctness diagnostics, not latency acceptance', 'Workflow has three fresh CLI/worker launches and ID routing; reported separately from the ordinary 150 ms query contract', 'Original IL only; no native C# latency claim'],
              'workloads': [], 'whole_input_probes': [], 'acceptance': 'pending'}

    def save():
        (args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')

    for probe in args.probe_input:
        command = ['dotnet', 'strings', probe, '--contains', 'Unexpected']
        start = time.perf_counter_ns()
        result = subprocess.run([str(args.binary), *map(str, command)], capture_output=True, timeout=30)
        elapsed = (time.perf_counter_ns() - start) / 1_000_000
        (args.output / f'{probe.stem}-probe.stdout').write_bytes(result.stdout)
        (args.output / f'{probe.stem}-probe.stderr').write_bytes(result.stderr)
        item = {'input': str(probe.resolve()), 'input_bytes': probe.stat().st_size, 'input_sha256': digest(probe),
                'command': list(map(str, command)), 'exit_code': result.returncode, 'elapsed_ms': elapsed,
                'stdout_bytes': len(result.stdout), 'stderr': result.stderr.decode(errors='replace'),
                'acceptance_role': 'whole-input correctness diagnostic; not a timed successful workload'}
        if result.returncode:
            assert not result.stdout, 'failed whole-input query leaked partial stdout'
        else:
            rows = [json.loads(line) for line in result.stdout.splitlines()]
            assert rows[-1]['complete'] is True
        report['whole_input_probes'].append(item)
        save()

    for expected in manifest['workloads']:
        command = ['dotnet', 'refs' if expected['string'] is not None else 'strings', args.input]
        if expected['string'] is not None:
            command += ['--string', expected['string']]
        elif expected['contains'] is not None:
            command += ['--contains', expected['contains']]
        for file_output in (False, True):
            name = expected['name'] + ('-file' if file_output else '-stdout')
            destination = args.output / f'{name}.jsonl'
            argv = command + (['-o', destination] if file_output else [])
            try:
                result, initial_ms = invoke(args.binary, argv)
                data = destination.read_bytes() if file_output else result.stdout
                verify(data, expected, source_id)
                destination.write_bytes(data)
                samples = []
                for _ in range(args.samples):
                    result, elapsed = invoke(args.binary, argv)
                    actual = destination.read_bytes() if file_output else result.stdout
                    assert actual == data
                    samples.append(elapsed)
            except (RuntimeError, AssertionError) as error:
                report['acceptance'] = 'failed_query_or_correctness'
                report['workloads'].append({'name': name, 'command': list(map(str, argv)), 'failure': str(error)})
                save()
                raise
            item = {'name': name, 'command': list(map(str, argv)), 'input_bytes': args.input.stat().st_size,
                    'input_sha256': source_id, 'output_bytes': len(data), 'output_sha256': hashlib.sha256(data).hexdigest(),
                    'records': expected['records'], 'verified_uses': expected['uses'], 'initial_ms': initial_ms,
                    'over_150_count': sum(sample > 150 for sample in samples), **metrics(samples)}
            item['latency_success'] = not any(sample > 150 for sample in samples)
            report['workloads'].append(item)
            save()
            print(f"{name}: median={item['median_ms']:.2f} P95={item['p95_ms']:.2f} >150ms={item['over_150_count']}/{args.samples}", flush=True)

    expected = next(item for item in manifest['workloads'] if item['name'] == 'url')
    search = ['dotnet', 'strings', args.input, '--contains', expected['contains']]
    def workflow():
        start = time.perf_counter_ns()
        search_result, _ = invoke(args.binary, search)
        rows = [json.loads(line) for line in search_result.stdout.splitlines()]
        hit = next(row for row in rows if row['record'] == 'string' and row['context_id'])
        method_args = ['dotnet', 'get-method', args.input, hit['context_id'], '--il']
        class_args = ['dotnet', 'get-class', args.input, hit['detail']['declaring_type_id'], '--il']
        method, _ = invoke(args.binary, method_args)
        whole_class, _ = invoke(args.binary, class_args)
        elapsed = (time.perf_counter_ns() - start) / 1_000_000
        return (search_result.stdout, method.stdout, whole_class.stdout), elapsed, (method_args, class_args)
    data, initial_ms, route = workflow()
    verify(data[0], expected, source_id)
    hit = next(row for row in [json.loads(line) for line in data[0].splitlines()] if row['record'] == 'string' and row['context_id'])
    method, whole_class = map(json.loads, data[1:])
    assert method['id'] == hit['context_id'] and method['detail']['complete'] is True
    assert whole_class['id'] == hit['detail']['declaring_type_id'] and whole_class['detail']['complete'] is True
    assert any(item['id'] == method['id'] for item in whole_class['detail']['methods'])
    for offset in hit['detail']['il_offsets']:
        assert any(instruction['offset'] == offset and instruction['mnemonic'] == 'ldstr' and instruction['operand'] == {'kind': 'token', 'value': hit['detail']['token']} for instruction in method['detail']['instructions'])
    samples = []
    for _ in range(args.samples):
        actual, elapsed, _ = workflow()
        assert actual == data
        samples.append(elapsed)
    for name, output in zip(['search', 'method', 'class'], data):
        (args.output / f'workflow-{name}.json').write_bytes(output)
    report['workflow'] = {'name': 'strings-to-method-and-class-il', 'commands': [list(map(str, command)) for command in (search, *route)],
                          'input_bytes': args.input.stat().st_size, 'input_sha256': source_id,
                          'output_bytes': list(map(len, data)), 'initial_ms': initial_ms, **metrics(samples),
                          'latency_contract': 'Three-process workflow measured separately; no numeric aggregate exception claimed'}
    report['acceptance'] = 'passed_selected_queries' if all(item['latency_success'] for item in report['workloads']) else 'failed_latency'
    save()
    if report['acceptance'] != 'passed_selected_queries':
        raise SystemExit('ordinary 150 ms query requirement failed; all exact samples retained')


if __name__ == '__main__':
    main()
