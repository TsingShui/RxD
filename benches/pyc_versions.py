#!/usr/bin/env python3
"""Offline final-CPython static format verification and fresh-process regression.

Consumes independently saved compile/dis/marshal oracles. Never imports or
executes analyzed source/PYC. Keep every failure, incomplete source and slow sample.
"""
import argparse
import gzip
import hashlib
import json
import math
import os
import platform
import statistics
import subprocess
import time
from pathlib import Path

VERSIONS = ['3.8.20', '3.9.6', '3.10.16', '3.11.11', '3.12.14', '3.13.2']
LABELS = ['ast', 'configparser', 'email-policy', 'email-parser', 'dataclasses']
NAMES = ['node', 'section', 'value', 'fp', 'field']
OPERATIONS = {
    'LOAD_NAME': 'name_read', 'STORE_NAME': 'name_write', 'DELETE_NAME': 'name_delete',
    'LOAD_GLOBAL': 'global_read', 'STORE_GLOBAL': 'global_write', 'DELETE_GLOBAL': 'global_delete',
    'LOAD_FAST': 'local_read', 'LOAD_FAST_CHECK': 'local_read', 'STORE_FAST': 'local_write',
    'DELETE_FAST': 'local_delete', 'LOAD_FAST_AND_CLEAR': 'local_read_clear',
    'MAKE_CELL': 'cell_create', 'LOAD_CLOSURE': 'closure_capture',
    'LOAD_DEREF': 'closure_read', 'STORE_DEREF': 'closure_write', 'DELETE_DEREF': 'closure_delete',
    'LOAD_CLASSDEREF': 'class_closure_read', 'LOAD_ATTR': 'attribute_read',
    'LOAD_SUPER_ATTR': 'attribute_read', 'LOAD_METHOD': 'attribute_read',
    'STORE_ATTR': 'attribute_write', 'DELETE_ATTR': 'attribute_delete',
    'IMPORT_NAME': 'import_name', 'IMPORT_FROM': 'import_from',
    'LOAD_FROM_DICT_OR_GLOBALS': 'dict_or_global_read', 'LOAD_FROM_DICT_OR_DEREF': 'dict_or_closure_read',
    'LOAD_FAST_LOAD_FAST': 'local_read', 'STORE_FAST_STORE_FAST': 'local_write',
}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def run(binary, command, path, arguments=()):
    argv = [str(binary), 'pyc', command, str(path), *arguments]
    start = time.perf_counter_ns()
    process = subprocess.run(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120)
    duration = time.perf_counter_ns()-start
    return process, {'argv': argv, 'duration_ns': duration, 'exit_code': process.returncode,
                     'stdout_bytes': len(process.stdout), 'stdout_sha256': sha(process.stdout),
                     'stderr': process.stderr.decode('utf-8', 'replace')}


def stream(process):
    assert process.returncode == 0, process.stderr.decode('utf-8', 'replace')
    rows = [json.loads(line) for line in process.stdout.splitlines()]
    assert rows[-1]['complete'] is True, rows[-1]
    return rows


def literal_values(value):
    if value['type'] == 'str':
        return [('str', value['utf8_surrogatepass_hex'])]
    if value['type'] == 'bytes':
        return [('bytes', value['hex'])]
    if value['type'] in ('tuple', 'frozenset'):
        return [leaf for child in value['items'] for leaf in literal_values(child)]
    return []


def names(ins):
    value = ins['argval']
    return [item['value'] for item in value['items']] if value['type'] == 'tuple' else [value['value']]


def name_hits(oracle, name):
    hits = []
    for owner, code in enumerate(oracle['codes']):
        for ins in code['dis_instructions']:
            operation = OPERATIONS.get(ins['opname'])
            if operation is None and ins['opname'] != 'STORE_FAST_LOAD_FAST':
                continue
            for position, value in enumerate(names(ins)):
                if value == name:
                    category = operation if operation else ['local_write', 'local_read'][position]
                    hits.append((owner, ins['offset'], ins['opcode'], ins['arg'], category, position))
    return hits


def validate_view(view, code, ranges):
    detail = view['detail']
    assert detail['complete'] is True
    assert detail['bytecode_hex'] == code['code_hex']
    assert view['start'] == ranges['pyc_marshal_start'] and view['end'] == ranges['pyc_marshal_end']
    units = {unit['offset']: unit for unit in detail['instructions']}
    assert len(units) == len(code['code_hex'])//4
    for ins in code['dis_instructions']:
        unit = units[ins['offset']]
        assert (unit['opcode'], unit['name'], unit['raw_operand']) == (ins['opcode'], ins['opname'], ins['raw_arg_byte'])
        if unit['role'] == 'instruction':
            assert unit['operand'] == ins['arg'], (code['name'], ins)
            if unit['jump_target'] is not None:
                assert unit['jump_target'] == ins['argval']['value']
            if unit['targets'] and unit['targets'][0]['table'] != 'consts':
                values = {value['id']: value['value'] for value in detail['values']}
                assert [values[target['value_id']] for target in unit['targets']] == names(ins)
    for cache in code['cache_units']:
        unit = units[cache['offset']]
        assert (unit['role'], unit['owner_offset'], unit['raw_hex']) == ('cache', cache['owner_offset'], cache['raw_hex'])
    for table in ['consts', 'names', 'varnames', 'freevars', 'cellvars']:
        assert len(detail['tables'][table]['entries']) == len(code[table])
    exceptions = [{key: entry[key] for key in ['start', 'end', 'target', 'depth', 'lasti']} for entry in detail['exceptions']]
    assert exceptions == code['exceptions']
    for offset, unit in units.items():
        if code['positions'] is not None:
            location = next(entry for entry in detail['line_mapping'] if entry['start'] <= offset < entry['end'])
            assert [location[key] for key in ['line', 'end_line', 'column', 'end_column']] == code['positions'][offset//2]
        elif code['line_ranges'] is not None:
            line = next(line for start, end, line in code['line_ranges'] if start <= offset < end)
            assert unit['line'] == line
        else:
            line = next(line for start, line in reversed(code['line_starts']) if start <= offset)
            assert unit['line'] == line


def measure(binary, command, path, arguments, samples, prepared, output):
    records = []
    failures = []
    for index in range(samples):
        try:
            process, record = run(binary, command, path, arguments)
            records.append(record)
            if process.returncode != prepared.returncode or process.stdout != prepared.stdout:
                failures.append({'sample': index, 'reason': 'exit/stdout differs from independently checked preparation'})
                (output/f'failed-{index}.stdout').write_bytes(process.stdout)
            if process.returncode != 0:
                failures.append({'sample': index, 'reason': 'nonzero exit', 'stderr': record['stderr']})
        except subprocess.TimeoutExpired as error:
            records.append({'sample': index, 'duration_ns': 120_000_000_000, 'exit_code': None, 'timeout': True})
            failures.append({'sample': index, 'reason': str(error)})
    durations = [record['duration_ns'] for record in records]
    return {'samples': records, 'median_ms': statistics.median(durations)/1e6,
            'p95_ms': sorted(durations)[math.ceil(len(durations)*.95)-1]/1e6,
            'over_150ms_count': sum(duration > 150_000_000 for duration in durations),
            'over_150ms_proportion': sum(duration > 150_000_000 for duration in durations)/len(durations),
            'failures': failures}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--corpus', type=Path, default=Path('tests/corpus/pyc/matrix'))
    parser.add_argument('--output', type=Path, default=Path('target/benchmarks/pyc-ticket38/measurements'))
    parser.add_argument('--samples', type=int, default=20)
    parser.add_argument('--cpu-model', required=True)
    parser.add_argument('--memory-gb', type=int, required=True)
    args = parser.parse_args()
    assert args.samples >= 20
    args.output.mkdir(parents=True, exist_ok=True)
    report = {'binary': str(args.binary.resolve()), 'binary_sha256': sha(args.binary.read_bytes()),
              'baseline': str(args.baseline.resolve()), 'baseline_sha256': sha(args.baseline.read_bytes()),
              'hardware': {'platform': platform.platform(), 'cpu_model': args.cpu_model, 'memory_gb': args.memory_gb, 'memory_unit': 'GiB'},
              'cache_conditions': 'warm OS file caches from preparation; no cache flush or CPU isolation; fresh CLI and worker each call',
              'samples_per_workload': args.samples, 'analyzed_code_executed': False,
              'git_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'git_status': subprocess.check_output(['git', 'status', '--short'], text=True),
              'corpus_audit': [], 'inputs': [], 'failures': [], 'slow_workloads': []}
    for version in VERSIONS:
        directory = args.corpus/version
        manifest = json.loads((directory/'manifest.json').read_text())
        audited = 0
        for manifest_name in ['manifest.json', 'semantics.manifest.json']:
            packing = json.loads((directory/manifest_name).read_text())
            for name, metadata in packing['artifacts'].items():
                data = (directory/metadata.get('storage', name)).read_bytes()
                if 'storage' in metadata:
                    assert sha(data) == metadata['storage_sha256']
                    data = gzip.decompress(data)
                assert len(data) == metadata['bytes'] and sha(data) == metadata['sha256'], (version, name)
                audited += 1
        report['corpus_audit'].append({'version': version, 'verified_artifacts': audited, 'compiler': manifest['version_info'], 'magic': manifest['magic_hex']})
        for label, name in zip(LABELS, NAMES):
            path = directory/(label+'.timestamp.pyc')
            oracle = json.loads(gzip.decompress((directory/(label+'.oracle.json.gz')).read_bytes()))
            output = args.output/version/label
            output.mkdir(parents=True, exist_ok=True)
            row = {'version': version, 'label': label, 'input_bytes': path.stat().st_size, 'input_sha256': sha(path.read_bytes()),
                   'provenance': next(item for item in manifest['inputs'] if item['label'] == label), 'workloads': {}, 'all_code_fetch': {}}
            report['inputs'].append(row)
            try:
                prepared, _ = run(args.binary, 'outline', path)
                outline = stream(prepared)
                codes = [entry for entry in outline if entry['record'] == 'code']
                assert len(codes) == len(oracle['codes'])
                digest = sha(path.read_bytes())
                for index, (entry, code) in enumerate(zip(codes, oracle['codes'])):
                    assert entry['id'] == f'rxd1:pyc:{digest}:code:{index}'
                    assert (entry['detail']['name'], entry['detail']['qualname']) == (code['name'], code['qualname'])
                preparations = {'outline': (prepared, [])}
                prepared, _ = run(args.binary, 'strings', path)
                strings = stream(prepared)
                literals = [entry for entry in strings if entry['record'] == 'string']
                for owner, code in enumerate(oracle['codes']):
                    for slot, constant in enumerate(code['consts']):
                        observed = [(entry['detail']['kind'], entry['detail']['raw_bytes']) for entry in literals if entry['detail']['code_index'] == owner and entry['detail']['constant_slot'] == slot]
                        assert sorted(observed) == sorted(literal_values(constant))
                preparations['strings'] = (prepared, [])
                prepared, _ = run(args.binary, 'refs', path, ['--name', name])
                refs = stream(prepared)
                hits = [entry for entry in refs if entry['record'] == 'reference']
                wanted = name_hits(oracle, name)
                assert wanted and len(hits) == len(wanted), (version, label, name)
                for hit, expected in zip(hits, wanted):
                    d = hit['detail']
                    assert (d['code_index'], d['offset'], d['opcode'], d['operand'], d['operation']) == expected[:5]
                preparations['refs'] = (prepared, ['--name', name])
                selected = min(hits, key=lambda hit: len(oracle['codes'][hit['detail']['code_index']]['code_hex']))
                target = selected['detail']['code_index']
                row['selected_target'] = {'index': target, 'id': selected['context_id'], 'name': oracle['codes'][target]['name'],
                                          'bytecode_bytes': len(oracle['codes'][target]['code_hex'])//2}
                prepared, _ = run(args.binary, 'get-code', path, [selected['context_id']])
                assert prepared.returncode == 0, prepared.stderr
                selected_view = json.loads(prepared.stdout)
                validate_view(selected_view, oracle['codes'][target], oracle['marshal_ranges']['code_ranges'][target])
                preparations['get-code'] = (prepared, [selected['context_id']])
                # Verify every original body from the actual released module once,
                # independently; these are correctness probes, separate from latency.
                checked = 0
                for index, (entry, code) in enumerate(zip(codes, oracle['codes'])):
                    actual, evidence = run(args.binary, 'get-code', path, [entry['id']])
                    assert actual.returncode == 0, (index, evidence)
                    validate_view(json.loads(actual.stdout), code, oracle['marshal_ranges']['code_ranges'][index])
                    checked += 1
                row['all_code_fetch'] = {'verified_codes': checked, 'total_codes': len(codes), 'complete': True}
                for command, (prepared, arguments) in preparations.items():
                    folder = output/command
                    folder.mkdir(exist_ok=True)
                    (folder/'prepared.stdout').write_bytes(prepared.stdout)
                    workload = measure(args.binary, command, path, arguments, args.samples, prepared, folder)
                    workload.update(output_bytes=len(prepared.stdout), output_sha256=sha(prepared.stdout))
                    row['workloads'][command] = workload
                    if workload['failures']:
                        report['failures'].append({'version': version, 'label': label, 'command': command, 'failures': workload['failures']})
                    if workload['over_150ms_count']:
                        report['slow_workloads'].append({'version': version, 'label': label, 'command': command, 'count': workload['over_150ms_count']})
            except (AssertionError, KeyError, StopIteration, subprocess.TimeoutExpired) as error:
                row['static_failure'] = repr(error)
                report['failures'].append({'version': version, 'label': label, 'failure': repr(error)})
            # Exact prior-runtime source-output compatibility, including preexisting
            # incomplete or failed outcomes; no semantic equivalence claim.
            before, before_record = run(args.baseline, 'decompile', path, ['--json'])
            after, after_record = run(args.binary, 'decompile', path, ['--json'])
            compatible = before.returncode == after.returncode and before.stdout == after.stdout
            (output/'decompile-before.stdout').write_bytes(before.stdout)
            (output/'decompile-after.stdout').write_bytes(after.stdout)
            compatibility = {'compatible': compatible, 'before': before_record, 'after': after_record}
            if after.returncode == 0:
                compatibility['complete'] = json.loads(after.stdout)['detail']['complete']
            row['decompile_compatibility'] = compatibility
            if not compatible:
                report['failures'].append({'version': version, 'label': label, 'command': 'decompile', 'reason': 'prior runtime exit/stdout changed'})
            if after.returncode == 0:
                folder = output/'decompile';folder.mkdir(exist_ok=True)
                workload = measure(args.binary, 'decompile', path, ['--json'], args.samples, after, folder)
                workload.update(output_bytes=len(after.stdout), output_sha256=sha(after.stdout), source_complete=compatibility['complete'])
                row['workloads']['decompile'] = workload
                if workload['failures']:
                    report['failures'].append({'version': version, 'label': label, 'command': 'decompile', 'failures': workload['failures']})
                if workload['over_150ms_count']:
                    report['slow_workloads'].append({'version': version, 'label': label, 'command': 'decompile', 'count': workload['over_150ms_count']})
            (args.output/'results.json').write_text(json.dumps(report, indent=2, sort_keys=True)+'\n')
            print(json.dumps({'version': version, 'label': label, 'static_failure': row.get('static_failure'),
                              'source_compatible': compatible, 'source_complete': compatibility.get('complete'),
                              'max_p95_ms': max((w['p95_ms'] for w in row['workloads'].values()), default=None)}), flush=True)
    raise SystemExit(bool(report['failures'] or report['slow_workloads']))

if __name__ == '__main__':
    main()
