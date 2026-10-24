#!/usr/bin/env python3
"""Frozen-release CLI/stdin/stdout measurements; never execute analyzed inputs.

Every sample, failure, timeout and complete wire result is retained. Timing excludes
oracle validation, hashes and report persistence. This runner does not grant exceptions.
"""
import argparse
import base64
from collections import Counter
import datetime
import gzip
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
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from mcp_client import Client, structured
from pyc_oracle import name_hits, validate_view
from dotnet_strings_oracle import canonical_hash
from input_identity import input_identity


def sha(data):
    return hashlib.sha256(data).hexdigest()


def identity(path):
    path = Path(path).resolve()
    data = path.read_bytes()
    return {'path': str(path), 'bytes': len(data), 'sha256': sha(data)}


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=True, indent=2) + '\n')


def stats(rows):
    values = sorted(row['duration_ns'] for row in rows if 'duration_ns' in row)
    over = sum(value > 150_000_000 for value in values)
    return {'samples': rows, 'sample_count': len(rows), 'timed_sample_count': len(values),
            'failure_count': sum(row.get('failure') is not None for row in rows),
            'median_ms': statistics.median(values) / 1e6 if values else None,
            'p95_ms': values[math.ceil(.95 * len(values)) - 1] / 1e6 if values else None,
            'over_150ms_count': over, 'over_150ms_proportion': over / len(values) if values else None}


def project(raw, kind):
    if kind == 'jsonl':
        rows = [json.loads(line) for line in raw.splitlines()]
        if rows[0]['record'] != 'header' or rows[-1]['record'] != 'summary':
            raise ValueError('missing complete JSONL boundaries')
        if not rows[-1]['complete'] or rows[-1]['records'] != len(rows) - 2:
            raise ValueError('incomplete JSONL result')
        return {'header': rows[0], 'records': rows[1:-1], 'summary': rows[-1]}
    if kind == 'text':
        return raw.decode('utf-8')
    result = json.loads(raw)
    if kind == 'asar-extract':
        result['_original_bytes'] = base64.b64encode(Path(result['detail']['path']).read_bytes()).decode()
    return result


def parity(actual, expected, kind):
    if kind == 'asar-extract':
        if actual['detail']['path'] is not None:
            raise ValueError('MCP extraction exposed a host output path')
        if actual.get('bytes_base64') != expected['_original_bytes']:
            raise ValueError('ASAR extracted bytes differ from actual CLI file writes')
        if actual['child_target']['source_id'] != expected['source_id']:
            raise ValueError('ASAR child identity differs from original extracted bytes')
        receipt = {key: value for key, value in actual.items() if key not in ('child_target', 'bytes_base64')}
        expected = {key: value for key, value in expected.items() if key != '_original_bytes'}
        expected['detail']['path'] = None
        if receipt != expected:
            raise ValueError('ASAR extraction receipt differs beyond declared MCP child/path projection')
    elif kind == 'text':
        if actual.get('source') != expected:
            raise ValueError('unchanged source differs between CLI and MCP')
    elif actual != expected:
        raise ValueError('CLI/MCP content, identity, ordering or completeness differs')


def expand(value, context):
    if isinstance(value, str) and value.startswith('${') and value.endswith('}'):
        result = context
        for key in value[2:-1].split('.'):
            result = result[int(key)] if isinstance(result, list) else result[key]
        return result
    if isinstance(value, dict):
        return {key: expand(item, context) for key, item in value.items()}
    if isinstance(value, list):
        return [expand(item, context) for item in value]
    return value


def verify_oracle(case, data):
    oracle = case.get('oracle')
    if not oracle:
        raise ValueError('each case requires an independent oracle')
    kind = oracle['kind']
    records = data.get('records', [])
    if kind == 'js-find':
        actual = [{'literal_kind': row['detail']['literal_kind'], 'value': row['detail']['value'],
                   'start': row['start'], 'end': row['end'], 'dynamic': row['detail']['dynamic']}
                  for row in records]
        expected = [{key: row[key] for key in actual[0]} for row in oracle['matches']] if actual else []
        if actual != expected or len(records) != len(oracle['matches']):
            raise ValueError('independent Acorn span/value oracle differs')
    elif kind == 'jvm-method':
        methods = [row for row in records if row['record'] == 'method']
        if len(methods) != 1 or methods[0]['detail']['name'] != oracle['name']:
            raise ValueError('published method declaration oracle differs')
    elif kind == 'pyc-refs':
        actual = Counter((row['detail']['code_index'], row['detail']['offset'],
                          row['detail']['opcode'], row['detail']['raw_operand'],
                          row['detail']['operation']) for row in records)
        expected = Counter(tuple(row[:5]) for row in oracle['sites'])
        if actual != expected:
            raise ValueError('independent CPython dis name-reference oracle differs')
    elif kind == 'dotnet-strings':
        canonical = []
        for row in records:
            detail = row['detail']
            canonical.append([detail['offset'], detail['token'], detail['value'],
                              detail['utf16_units'], detail['method_token'],
                              int(detail['declaring_type_id'].split(':')[-1], 16)
                              if detail['declaring_type_id'] else None, detail['il_offsets']])
        if len(records) != oracle['records'] or canonical_hash(canonical) != oracle['canonical_sha256']:
            raise ValueError('independent PEReader strings oracle differs')
    elif kind == 'dotnet-method':
        detail = data['detail']
        if sha(bytes.fromhex(detail['il_hex'])) != oracle['il_sha256']:
            raise ValueError('independent PEReader method IL differs')
        if detail['handlers'] != oracle['handlers']:
            raise ValueError('independent PEReader method exception evidence differs')
        if not detail['decompiled'] or detail['representation'] != 'csharp' or not detail['code']:
            raise ValueError('configured C# workload unexpectedly degraded')
    elif kind == 'records':
        selected = [row for row in records if row['record'] == oracle['record']]
        if len(selected) != len(oracle['details']):
            raise ValueError('independent record inventory differs')
        for actual, expected in zip(selected, oracle['details']):
            if any(actual['detail'].get(key) != value for key, value in expected.items()):
                raise ValueError('independent record fields differ')
    elif kind == 'contains':
        text = json.dumps(data, ensure_ascii=True)
        if not all(value in text for value in oracle['values']):
            raise ValueError('independent literal/producer facts differ')
    else:
        raise ValueError('unknown oracle kind: ' + kind)


def builtin_cases(root, dotnet, csharp):
    cases = []
    manifest = json.loads((root / 'tests/corpus/manifest.json').read_text())
    for case in manifest['cases']:
        if case['provenance']['kind'] != 'published-production':
            continue
        path = root / 'tests/corpus' / case['file']
        cases.append({'name': case['name'], 'input': str(path), 'sha256': case['sha256'],
                      'backend': 'js', 'synthetic': False, 'provenance': case['provenance'],
                      'oracle': {'kind': 'js-find', 'matches': case['expected']['matches']},
                      'query': {'tool': 'js_find', 'arguments': {'string': case['needle']},
                                'cli': ['js', 'find', '${path}', '-f', case['needle']],
                                'projection': 'jsonl'},
                      'workflow': [{'name': 'q'}, {'name': 'fetch', 'tool': 'js_slice',
                                    'arguments': {'at': '${q.records.0.context_at}'},
                                    'cli': ['js', 'get', '${path}', '--at', '${q.records.0.context_at}', '--json'],
                                    'projection': 'json'}]})
    for label, digest, owner, method in [
            ('gson-2.11.0', '57928d6e5a6edeb2abd3770a8f95ba44dce45f3b23b7a9dc2b309c581552a78b',
             'com/google/gson/GsonBuilder', 'serializeNulls'),
            ('rhino-1.7.15', '2427fdcbc149ca0a25ccfbb7c71b01f39ad42708773a47816cd2342861766b63',
             'org/mozilla/javascript/Context', 'getLanguageVersion')]:
        cases.append({'name': label, 'input': str(root / 'tests/corpus/jvm' / (label + '.jar')),
                      'sha256': digest, 'backend': 'jar', 'synthetic': False,
                      'provenance': {'source': 'tests/corpus/jvm/README.md', 'published_method': owner + '.' + method},
                      'oracle': {'kind': 'jvm-method', 'name': method},
                      'query': {'tool': 'jar_methods', 'arguments': {'class': 'L' + owner + ';', 'filter': method},
                                'cli': ['jar', 'methods', '${path}', '--class', 'L' + owner + ';', '--filter', method],
                                'projection': 'jsonl'},
                      'workflow': [{'name': 'q'}, {'name': 'fetch', 'tool': 'jar_get_method',
                                    'arguments': {'at': '${selected_id}', 'raw': True},
                                    'cli': ['jar', 'get-method', '${path}', '--at', '${selected_id}', '--raw'],
                                    'projection': 'json'}]})
    for version in ['3.8.20', '3.9.6', '3.10.16', '3.11.11', '3.12.14', '3.13.2']:
        directory = root / 'tests/corpus/pyc/matrix' / version
        manifest = json.loads((directory / 'manifest.json').read_text())
        name = 'email-parser'
        saved = manifest['artifacts'][name + '.oracle.json']
        stored = (directory / saved['storage']).read_bytes()
        raw = gzip.decompress(stored)
        if sha(stored) != saved['storage_sha256'] or sha(raw) != saved['sha256']:
            raise ValueError('independent PYC oracle hash differs')
        oracle = json.loads(raw)
        sites = name_hits(oracle, 'self')
        selected = min({site[0] for site in sites}, key=lambda index: len(oracle['codes'][index]['code_hex']))
        file = name + '.timestamp.pyc'
        cases.append({'name': 'cpython-' + version + '-email-parser',
                      'input': str(directory / file), 'sha256': manifest['artifacts'][file]['sha256'],
                      'backend': 'pyc', 'synthetic': False,
                      'provenance': {'source': str(directory / 'manifest.json'), 'python': manifest['python'],
                                     'compiler_sha256': manifest['binary_sha256'], 'oracle': identity(directory / saved['storage'])},
                      'oracle': {'kind': 'pyc-refs', 'sites': sites},
                      'source_scale': {'code_objects': len(oracle['codes']), 'selected_code_index': selected,
                                       'selected_bytecode_bytes': len(oracle['codes'][selected]['code_hex']) // 2},
                      'query': {'tool': 'pyc_xref', 'arguments': {'name': 'self'},
                                'cli': ['pyc', 'xref', '${path}', '--name', 'self'], 'projection': 'jsonl'},
                      'workflow': [{'name': 'q'}, {'name': 'select', 'tool': 'pyc_codes',
                                    'arguments': {'index': selected},
                                    'cli': ['pyc', 'codes', '${path}', '--index', selected], 'projection': 'jsonl'},
                                   {'name': 'fetch', 'tool': 'pyc_get_code',
                                    'arguments': {'at': '${select.records.0.at}'},
                                    'cli': ['pyc', 'get-code', '${path}', '--at', '${select.records.0.at}'], 'projection': 'json'}],
                      '_pyc_oracle': oracle, '_selected': selected})
    if dotnet:
        pin = json.loads((root / 'tests/corpus/dotnet/string-expectations.json').read_text())
        expected = next(item for item in pin['workloads'] if item['name'] == 'url')
        cases.append({'name': 'newtonsoft-13.0.3-strings', 'input': str(dotnet),
                      'sha256': pin['provenance']['dll_sha256'], 'backend': 'dotnet', 'synthetic': False,
                      'provenance': pin['provenance'], 'oracle': {'kind': 'dotnet-strings', **expected},
                      'query': {'tool': 'dotnet_strings', 'arguments': {'filter': expected['contains']},
                                'cli': ['dotnet', 'strings', '${path}', '--filter', expected['contains']],
                                'projection': 'jsonl'},
                      'workflow': [{'name': 'q'}, {'name': 'refs', 'tool': 'dotnet_xref',
                                    'arguments': {'at': '${selected_id}'},
                                    'cli': ['dotnet', 'xref', '${path}', '--at', '${selected_id}'], 'projection': 'jsonl'},
                                   {'name': 'fetch', 'tool': 'dotnet_get_method',
                                    'arguments': {'at': '${selected_id}', 'raw': True},
                                    'cli': ['dotnet', 'get-method', '${path}', '--at', '${selected_id}', '--raw'], 'projection': 'json'}]})
    if dotnet and csharp:
        oracle = json.loads((root / 'tests/corpus/dotnet/class-expectations.json').read_text())
        owner = next(case for case in oracle['workloads'] if case['name'] == 'Newtonsoft.Json.JsonReader')
        token, expected = next((token, method) for token, method in owner['methods'].items() if method['name'] == 'get_TokenType')
        digest = oracle['provenance']['dll_sha256']
        # A .NET coordinate is its metadata token, printed as `0x06000001`.
        at = '0x' + token[2:]
        cases.append({'name': 'newtonsoft-13.0.3-token-type-csharp', 'input': str(dotnet),
                      'sha256': digest, 'backend': 'dotnet', 'synthetic': False,
                      'provenance': oracle['provenance'],
                      'oracle': {'kind': 'dotnet-method', **expected},
                      'source_scale': {'method_token': token, 'independent_il_sha256': expected['il_sha256']},
                      'query': {'tool': 'dotnet_get_method', 'arguments': {'at': at},
                                'cli': ['dotnet', 'get-method', '${path}', '--at', at], 'projection': 'json'},
                      'workflow': [{'name': 'q'}, {'name': 'refs', 'tool': 'dotnet_xref',
                                    'arguments': {'at': '${q.at}'},
                                    'cli': ['dotnet', 'xref', '${path}', '--at', '${q.at}'], 'projection': 'jsonl'},
                                   {'name': 'fetch', 'tool': 'dotnet_get_method',
                                    'arguments': {'at': '${q.at}', 'raw': True},
                                    'cli': ['dotnet', 'get-method', '${path}', '--at', '${q.at}', '--raw'], 'projection': 'json'}]})
    return cases


class Recorder:
    def __init__(self, output, binary, roots, options, timeout, env):
        self.output, self.binary, self.roots = output, binary, roots
        self.options, self.timeout, self.env = options, timeout, env
        self.counter, self.failures = 0, []

    def persist(self, tag, metrics, raw=b'', stderr=b''):
        self.counter += 1
        name = f'{self.counter:05d}-{tag}'
        base = self.output / 'samples' / name
        base.parent.mkdir(parents=True, exist_ok=True)
        Path(str(base) + '.stdout').write_bytes(raw)
        if stderr:
            Path(str(base) + '.stderr').write_bytes(stderr)
        metrics.update(stdout_sha256=sha(raw), stdout_bytes=len(raw), stderr_bytes=len(stderr),
                       stdout_file=str(Path(str(base) + '.stdout')))
        save(Path(str(base) + '.json'), metrics)
        with (self.output / 'journal.jsonl').open('a') as stream:
            stream.write(json.dumps({'tag': tag, **metrics}) + '\n')
        if metrics.get('failure'):
            self.failures.append({'tag': tag, **metrics})
        return metrics

    def cli(self, step, context, tag, binary=None, defer=False):
        argv = [str(binary or self.binary), *map(str, expand(step['cli'], context))]
        start = time.perf_counter_ns()
        process = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   env=self.env, cwd=ROOT, start_new_session=os.name == 'posix')
        try:
            raw, stderr = process.communicate(timeout=self.timeout)
            metrics = {'duration_ns': time.perf_counter_ns() - start, 'argv': argv,
                       'exit_code': process.returncode, 'timed_out': False}
            if process.returncode:
                metrics['failure'] = 'CLI exit ' + str(process.returncode)
        except subprocess.TimeoutExpired:
            if os.name == 'posix':
                os.killpg(process.pid, signal.SIGKILL)
            else:
                subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'],
                               capture_output=True, timeout=5, check=False)
                if process.poll() is None:
                    process.kill()
            raw, stderr = process.communicate()
            metrics = {'duration_ns': time.perf_counter_ns() - start, 'argv': argv,
                       'exit_code': process.returncode, 'timed_out': True, 'failure': 'CLI timeout'}
        if defer:
            metrics['_raw'], metrics['_stderr'] = raw, stderr
            return metrics, raw
        return self.persist(tag, metrics, raw, stderr), raw

    def client(self, tag):
        stderr = (self.output / (tag + '.stderr')).open('wb')
        client = Client(self.binary, self.roots, stderr, options=self.options,
                        timeout=self.timeout, env=self.env)
        client.diagnostics = stderr
        return client

    def call(self, client, step, context, tag, reference, defer=False):
        arguments = expand(step['arguments'], context)
        arguments.setdefault('input', reference)
        start = time.perf_counter_ns()
        raw = b''
        try:
            packet, metrics, raw = client.tool(step['tool'], arguments)
            value = structured(packet)
        except Exception as error:
            metrics = {'duration_ns': time.perf_counter_ns() - start,
                       'failure': str(error), 'timed_out': isinstance(error, TimeoutError)}
            value = None
        metrics.update(tool=step['tool'], arguments=arguments)
        if defer:
            metrics['_raw'] = raw
            return metrics, value
        return self.persist(tag, metrics, raw), value

    def finish(self, client, tag):
        try:
            client.close()
        except Exception as error:
            self.persist(tag + '-shutdown', {'failure': str(error)})
        client.diagnostics.close()


def rss_tree(pid):
    # Deliberately reported as a sampled peak, never an exact peak or hard RSS cap.
    if os.name != 'posix':
        return None
    try:
        output = subprocess.check_output(['ps', '-axo', 'pid=,ppid=,rss='], timeout=2, text=True)
        rows = [tuple(map(int, line.split())) for line in output.splitlines() if line.strip()]
        children = {pid}
        while True:
            expanded = children | {child for child, parent, _ in rows if parent in children}
            if children == expanded:
                break
            children = expanded
        return sum(rss * 1024 for child, _, rss in rows if child in children)
    except (OSError, ValueError, subprocess.SubprocessError):
        return None


def validate_catalog(packet, case):
    if 'error' in packet:
        raise RuntimeError(packet['error'])
    tools = packet['result']['tools']
    names = {tool['name'] for tool in tools}
    required = {'rxd_open', 'rxd_close', case['query']['tool']}
    required.update(step['tool'] for step in case['workflow'] if 'tool' in step)
    if required - names:
        raise ValueError('required routes absent from tools/list: ' + repr(sorted(required - names)))
    for tool in tools:
        if tool['name'] in required and ('inputSchema' not in tool or 'outputSchema' not in tool):
            raise ValueError('discovered analysis route lacks its typed contract')
    return len(tools), sorted(required)


def returned_route(state, name, value, backend):
    state[name] = value
    if name == 'q' and backend in ('jar', 'dotnet', 'wasm'):
        state['selected_id'] = next((entry['at'] if backend in ('jar', 'wasm') else entry['context_at']
                                    for entry in value.get('records', [])
                                    if entry['record'] in ('method', 'function') or entry.get('context_at')), None)


def fresh_workflow(recorder, case, context, sample):
    tag = f"{case['name']}-flow-fresh-{sample}"
    start = time.perf_counter_ns()
    client = recorder.client(tag)
    components, raw_frames, results = [], [], []
    state = dict(context)
    failure, elapsed = None, None
    try:
        _, metrics, raw = client.initialize()
        metrics.update(component='startup_initialize', duration_ns=time.perf_counter_ns() - start)
        components.append(metrics); raw_frames.append(raw)
        packet, metrics, raw = client.request('tools/list', {})
        metrics['component'] = 'tools_list'
        components.append(metrics); raw_frames.append(raw)
        count, routes = validate_catalog(packet, case)
        metrics.update(tool_count=count, required_routes=routes)
        packet, metrics, raw = client.tool('rxd_open', {'path': context['path'], 'backend': case['backend']})
        metrics['component'] = 'open'
        components.append(metrics); raw_frames.append(raw)
        opened = structured(packet)
        if opened['source_id'] != case['identity']:
            raise ValueError('fresh workflow open identity differs')
        reference = {'target_id': opened['target_id']}
        children = []
        for declaration in case['workflow']:
            step = case['query'] if 'tool' not in declaration else declaration
            name = declaration['name']
            row, value = recorder.call(client, step, state, tag + '-' + name, reference, defer=True)
            row['component'] = name
            raw_frames.append(row.pop('_raw', b''))
            components.append(row)
            if value is None:
                raise RuntimeError(row.get('failure', 'fresh workflow query failed'))
            returned_route(state, name, value, case['backend'])
            results.append((step, value, dict(state)))
            if value.get('child_target'):
                children.append(value['child_target']['target_id'])
        for target in [*dict.fromkeys(children), opened['target_id']]:
            packet, metrics, raw = client.tool('rxd_close', {'target_id': target})
            metrics['component'] = 'close'
            components.append(metrics); raw_frames.append(raw)
            structured(packet)
        elapsed = time.perf_counter_ns() - start
        # Equivalent CLI output validation is outside the complete fresh workflow timer.
        for index, (step, actual, routed) in enumerate(results):
            metrics, raw = recorder.cli(step, routed, tag + f'-oracle-cli-{index}')
            if metrics.get('failure'):
                raise RuntimeError('fresh workflow CLI verification failed')
            parity(actual, project(raw, step['projection']), step['projection'])
    except Exception as error:
        elapsed = time.perf_counter_ns() - start if elapsed is None else elapsed
        failure = str(error)
    finally:
        recorder.finish(client, tag)
    for index, row in enumerate(components):
        recorder.persist(tag + f'-component-{index}', row, raw_frames[index])
    return recorder.persist(tag, {'duration_ns': elapsed, 'components': components,
                                  'failure': failure, 'request_bytes': client.sent_bytes,
                                  'response_bytes': client.received_bytes,
                                  'timing': 'fresh process startup + initialize + tools/list typed route discovery + open + returned-ID query chain + close, complete responses and client glue; report/CLI verification excluded'},
                            b''.join(raw_frames))


def measure_case(recorder, case, samples, concurrency, baseline):
    path = str(Path(case['input']).resolve())
    query = case['query']
    context = {'path': path, 'destination': str(recorder.output / (case['name'] + '-cli-extracted.bin'))}
    groups = {key: [] for key in ['cli', 'startup_initialize', 'tools_list', 'fresh_path_query',
                                  'startup_initialize_discovery_first_query_total', 'open', 'same_target_query',
                                  'workflow_cli_total', 'workflow_target_total', 'workflow_fresh_mcp_total', 'concurrent_batch']}
    if baseline:
        groups['baseline_cli'] = []
    # CLI/MCP parity and independent oracle checking happen before timed samples.
    preflight, raw = recorder.cli(query, context, case['name'] + '-preflight-cli')
    if preflight.get('failure'):
        return {'preflight_failure': preflight['failure'], 'groups': groups}
    expected = project(raw, query['projection'])
    verify_oracle(case, expected)
    prepared = recorder.client(case['name'] + '-preflight-mcp')
    try:
        prepared.initialize()
        catalog_packet, catalog_metrics, catalog_raw = prepared.request('tools/list', {})
        recorder.persist(case['name'] + '-preflight-tools-list', catalog_metrics, catalog_raw)
        validate_catalog(catalog_packet, case)
        _, result = recorder.call(prepared, query, context, case['name'] + '-preflight-path', {'path': path})
        if result is None:
            return {'preflight_failure': 'MCP preflight failed', 'groups': groups}
        parity(result, expected, query['projection'])
        if case['backend'] == 'jar':
            context['selected_id'] = next(row['at'] for row in result['records'] if row['record'] == 'method')
        elif case['backend'] == 'wasm':
            context['selected_id'] = next(row['at'] for row in result['records'] if row['record'] == 'function')
        elif case['backend'] == 'pyc':
            # Get the ID from actual ENUMERATE; never fabricate the timed workflow route.
            outlined = structured(prepared.tool('pyc_codes', {'input': {'path': path}, 'index': case['_selected']})[0])
            context['selected_id'] = outlined['records'][0]['at']
        else:
            context['selected_id'] = next((row['context_at'] for row in result.get('records', []) if row['context_at']), None)
        context['q'] = result
        scale = []
        for step in case.get('workflow', []):
            if 'tool' not in step:
                continue
            _, value = recorder.call(prepared, step, context, case['name'] + '-preflight-' + step['name'], {'path': path})
            if value is None:
                raise ValueError('workflow preflight failed')
            cli_metrics, cli_raw = recorder.cli(step, context, case['name'] + '-preflight-cli-' + step['name'])
            if cli_metrics.get('failure'):
                raise ValueError('CLI workflow preflight failed')
            parity(value, project(cli_raw, step['projection']), step['projection'])
            if step['tool'] == 'pyc_get_code' and case.get('_pyc_oracle'):
                oracle = case['_pyc_oracle']
                validate_view(value, oracle['codes'][case['_selected']],
                              oracle['marshal_ranges']['code_ranges'][case['_selected']])
            context[step['name']] = value
            detail = value.get('detail', {})
            scale.append({'tool': step['tool'], 'output_json_bytes': len(json.dumps(value).encode()),
                          'representation': value.get('representation', detail.get('representation')),
                          'original_bytecode_bytes': len(detail.get('bytecode_hex', detail.get('il_hex'))) // 2
                          if isinstance(detail.get('bytecode_hex', detail.get('il_hex')), str) else None,
                          'instruction_count': len(detail['instructions']) if isinstance(detail.get('instructions'), list) else None,
                          'selected_original_span_bytes': value['end'] - value['start']
                          if isinstance(value.get('start'), int) and isinstance(value.get('end'), int) else None})
    finally:
        recorder.finish(prepared, case['name'] + '-preflight')
    # Persist exactly selected routes/source scale outside measured query windows.
    selected = {'selected_id': context.get('selected_id'), 'source_scale': case.get('source_scale'), 'fetch_scale': scale}
    for sample in range(samples):
        prefix = f"{case['name']}-{sample}"
        row, raw = recorder.cli(query, context, prefix + '-cli')
        groups['cli'].append(row)
        if not row.get('failure'):
            parity(project(raw, query['projection']), expected, query['projection'])
        if baseline:
            row, raw = recorder.cli(query, context, prefix + '-baseline', baseline)
            groups['baseline_cli'].append(row)
            if not row.get('failure'):
                parity(project(raw, query['projection']), expected, query['projection'])
        start = time.perf_counter_ns()
        client = recorder.client(prefix + '-fresh')
        components, raw_frames = [], []
        phase, failure, elapsed = 'startup_initialize', None, None
        try:
            _, metrics, raw = client.initialize()
            metrics.update(component=phase, duration_ns=time.perf_counter_ns() - start)
            components.append(metrics); raw_frames.append(raw)
            phase = 'tools_list'
            packet, metrics, raw = client.request('tools/list', {})
            metrics['component'] = phase
            components.append(metrics); raw_frames.append(raw)
            count, routes = validate_catalog(packet, case)
            metrics.update(tool_count=count, required_routes=routes)
            phase = 'fresh_path_query'
            packet, metrics, raw = client.tool(query['tool'], {**query['arguments'], 'input': {'path': path}})
            metrics['component'] = phase
            components.append(metrics); raw_frames.append(raw)
            elapsed = time.perf_counter_ns() - start
            parity(structured(packet), expected, query['projection'])
        except Exception as error:
            elapsed = time.perf_counter_ns() - start if elapsed is None else elapsed
            failure = str(error)
            if components and components[-1]['component'] == phase:
                components[-1]['failure'] = failure
            else:
                components.append({'duration_ns': elapsed, 'component': phase, 'failure': failure,
                                   'timed_out': isinstance(error, TimeoutError)})
                raw_frames.append(b'')
        finally:
            recorder.finish(client, prefix + '-fresh')
        for index, row in enumerate(components):
            groups[row['component']].append(recorder.persist(prefix + '-' + row['component'], row, raw_frames[index]))
        groups['startup_initialize_discovery_first_query_total'].append(recorder.persist(prefix + '-discovery-first-total', {
            'duration_ns': elapsed, 'failure': failure, 'components': components,
            'request_bytes': client.sent_bytes, 'response_bytes': client.received_bytes,
            'timing': 'external fresh start + initialize + complete tools/list discovery + first path query; validation/report persistence excluded'},
            b''.join(raw_frames)))
        groups['workflow_fresh_mcp_total'].append(fresh_workflow(recorder, case, context, sample))
    client = recorder.client(case['name'] + '-resident')
    try:
        client.initialize()
        resident_packet, resident_metrics, resident_raw = client.request('tools/list', {})
        recorder.persist(case['name'] + '-resident-tools-list', resident_metrics, resident_raw)
        validate_catalog(resident_packet, case)
        for sample in range(samples):
            start = time.perf_counter_ns()
            raw = b''
            try:
                packet, metrics, raw = client.tool('rxd_open', {'path': path, 'backend': case['backend']})
                opened = structured(packet)
                if opened['source_id'] != case['identity']:
                    raise ValueError('open source identity differs')
                client.tool('rxd_close', {'target_id': opened['target_id']})
            except Exception as error:
                metrics = {'duration_ns': time.perf_counter_ns() - start, 'failure': str(error)}
            groups['open'].append(recorder.persist(f"{case['name']}-open-{sample}", metrics, raw))
        opened = structured(client.tool('rxd_open', {'path': path, 'backend': case['backend']})[0])
        reference = {'target_id': opened['target_id']}
        for sample in range(samples):
            row, result = recorder.call(client, query, context, f"{case['name']}-target-{sample}", reference)
            groups['same_target_query'].append(row)
            if result is not None:
                parity(result, expected, query['projection'])
        for sample in range(samples):
            for mode in ['cli', 'target']:
                start = time.perf_counter_ns()
                state = dict(context)
                components, total, failure = [], 0, None
                for declaration in case.get('workflow', []):
                    step = query if 'tool' not in declaration else declaration
                    name = declaration['name']
                    if mode == 'cli':
                        row, raw = recorder.cli(step, state, f"{case['name']}-flow-cli-{sample}-{name}", defer=True)
                        value = project(raw, step['projection']) if not row.get('failure') else None
                    else:
                        row, value = recorder.call(client, step, state, f"{case['name']}-flow-target-{sample}-{name}", reference, defer=True)
                    components.append(row)
                    if value is None:
                        failure = row.get('failure', 'workflow failure')
                        break
                    returned_route(state, name, value, case['backend'])
                total = time.perf_counter_ns() - start
                for value in state.values():
                    if mode == 'target' and isinstance(value, dict) and value.get('child_target'):
                        client.tool('rxd_close', {'target_id': value['child_target']['target_id']})
                for index, row in enumerate(components):
                    recorder.persist(f"{case['name']}-flow-{mode}-{sample}-component-{index}",
                                     row, row.pop('_raw', b''), row.pop('_stderr', b''))
                group = 'workflow_cli_total' if mode == 'cli' else 'workflow_target_total'
                groups[group].append(recorder.persist(f"{case['name']}-flow-{mode}-{sample}", {
                    'duration_ns': total, 'components': components, 'failure': failure,
                    'timing': 'external workflow wall time, complete writes plus client glue; report persistence excluded'}))
        for sample in range(samples):
            peak, stop = [None], threading.Event()
            def observe():
                while not stop.is_set():
                    rss = rss_tree(client.process.pid)
                    if rss is not None:
                        peak[0] = max(peak[0] or 0, rss)
                    stop.wait(.01)
            observer = threading.Thread(target=observe, daemon=True)
            observer.start()
            start = time.perf_counter_ns()
            elapsed = None
            raw_responses, packets, components, failure = [], [], [], None
            try:
                requests = [client.send('tools/call', {'name': query['tool'],
                             'arguments': {**query['arguments'], 'input': reference}})
                            for _ in range(concurrency)]
                for request in requests:
                    packet, metrics, raw = client.receive(request)
                    components.append(metrics)
                    raw_responses.append(raw)
                    packets.append(packet)
                elapsed = time.perf_counter_ns() - start
                for packet in packets:
                    parity(structured(packet), expected, query['projection'])
            except Exception as error:
                elapsed = time.perf_counter_ns() - start if elapsed is None else elapsed
                failure = str(error)
            finally:
                stop.set(); observer.join(timeout=3)
            groups['concurrent_batch'].append(recorder.persist(f"{case['name']}-concurrent-{sample}", {
                'duration_ns': elapsed, 'concurrency': concurrency, 'components': components,
                'sampled_tree_peak_rss_bytes': peak[0], 'rss_sampling': 'ps process tree at nominal 10 ms; not exact high-water mark; includes server + descendants',
                'failure': failure}, b''.join(raw_responses)))
        client.tool('rxd_close', {'target_id': opened['target_id']})
    finally:
        recorder.finish(client, case['name'] + '-resident')
    return {'selected': selected, 'groups': {key: stats(rows) for key, rows in groups.items()}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--build-info', type=Path, required=True,
                        help='Frozen binary build/source/features/profile/engine provenance JSON; must contain binary_sha256')
    parser.add_argument('--baseline', type=Path)
    parser.add_argument('--corpus-root', type=Path, default=ROOT)
    parser.add_argument('--dotnet-input', type=Path)
    parser.add_argument('--include-csharp', action='store_true', help='Require native engine C# outcome and independent original IL oracle')
    parser.add_argument('--plan', type=Path, action='append', default=[], help='Additional explicitly pinned cases JSON')
    parser.add_argument('--only', action='append', default=[], help='Select explicit case names; repeatable')
    parser.add_argument('--samples', type=int, default=20)
    parser.add_argument('--concurrency', type=int, default=2)
    parser.add_argument('--timeout', type=float, default=35)
    parser.add_argument('--server-option', action='append', default=[], help='Literal extra argv item (use --server-option=...)')
    parser.add_argument('--cpu-model', required=True)
    parser.add_argument('--memory-bytes', type=int, required=True)
    parser.add_argument('--conditions', required=True, help='Cache, CPU load/isolation and measurement conditions')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/benchmarks/mcp')
    args = parser.parse_args()
    if args.samples < 20 or args.concurrency < 1 or args.timeout <= 0 or args.memory_bytes <= 0:
        parser.error('require at least 20 samples and positive concurrency/timeout/memory')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    info = json.loads(args.build_info.read_text())
    actual = identity(args.binary)
    if info.get('binary_sha256') != actual['sha256']:
        raise ValueError('build-info does not match frozen binary SHA-256')
    frozen = output / ('rxd.exe' if os.name == 'nt' else 'rxd')
    shutil.copy2(args.binary, frozen)
    if sha(frozen.read_bytes()) != actual['sha256']:
        raise ValueError('binary changed during freeze')
    save(output / 'build-info.json', info)
    baseline = None
    if args.baseline:
        baseline = output / ('baseline.exe' if os.name == 'nt' else 'baseline')
        shutil.copy2(args.baseline, baseline)
    corpus = args.corpus_root.resolve()
    cases = builtin_cases(corpus, args.dotnet_input, args.include_csharp)
    plans = []
    for path in args.plan:
        extra = json.loads(path.read_text())
        cases.extend(extra['cases'])
        plans.append(identity(path))
    # `sha256` is the pinned provenance digest; `identity` is what rxd prints as
    # `source_id` (a sampled digest of the same bytes).
    for case in cases:
        case.setdefault('identity', input_identity(Path(case['input'])))
    if args.only:
        available = {case['name'] for case in cases}
        if set(args.only) - available:
            raise ValueError('unknown selected cases: ' + repr(set(args.only) - available))
        cases = [case for case in cases if case['name'] in args.only]
    roots = sorted({str(corpus), str(output), *[str(Path(case['input']).resolve().parent) for case in cases]})
    env = dict(os.environ)
    report = {'benchmark_version': 2, 'timestamp_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'platform': platform.platform(), 'machine': platform.machine(), 'cpu': args.cpu_model,
              'memory_bytes': args.memory_bytes, 'logical_cpus': os.cpu_count(),
              'conditions': args.conditions, 'binary': actual, 'build_info': info,
              'runner_cargo_lock': identity(ROOT / 'Cargo.lock'),
              'load_start': os.getloadavg() if hasattr(os, 'getloadavg') else None,
              'baseline': identity(baseline) if baseline else None, 'plans': plans,
              'server_options': args.server_option, 'roots': roots, 'samples_per_workload': args.samples,
              'scope': 'Explicitly listed inputs/queries only; synthetic and production separate; no automatic latency acceptance or new exception.',
              'notes': ['No analyzed artifact is executed or instantiated.', 'CLI fresh process includes complete stdout/stderr pipe writes.',
                        'MCP complete-response read includes JSON parse plus structured/text copies; validation/persistence outside query timings.',
                        'Fresh first-query total includes startup, initialize and full tools/list typed capability discovery; old reports excluded discovery and remain unchanged.',
                        'Fresh MCP workflow includes startup/initialize/discovery/open/returned-ID chain/close; resident target workflow excludes already-paid setup.',
                        'Workflow totals include complete component calls and client glue; first calls/open/reuse and concurrent throughput/RSS are separate.',
                        'MCP workers are new for each call, even with the same immutable target.',
                        'RSS sampled on Unix; unavailable on Windows, never interpreted as zero.'],
              'workloads': [], 'failures': []}
    recorder = Recorder(output, frozen, roots, args.server_option, args.timeout, env)
    for case in cases:
        item = {key: value for key, value in case.items() if not key.startswith('_')}
        try:
            if not case.get('workflow'):
                raise ValueError('each case must declare a nonempty workflow; one-step native/inference workflows are explicit')
            if 'synthetic' not in case or not case.get('provenance'):
                raise ValueError('each case must declare synthetic status and input provenance')
            item['input_identity'] = identity(case['input'])
            if item['input_identity']['sha256'] != case['sha256']:
                raise ValueError('input differs from explicit corpus pin')
            item.update(measure_case(recorder, case, args.samples, args.concurrency, baseline))
            if item.get('preflight_failure'):
                report['failures'].append({'case': case['name'], 'error': item['preflight_failure']})
        except Exception as error:
            item['failure'] = str(error)
            report['failures'].append({'case': case['name'], 'error': str(error)})
        report['workloads'].append(item)
        report['sample_failures'] = recorder.failures
        save(output / 'results.json', report)
        reason = item.get('failure', item.get('preflight_failure'))
        print(case['name'] + ': ' + ('FAILED ' + reason if reason else 'recorded'), flush=True)
    if not cases:
        report['failures'].append({'error': 'no selected workloads'})
    report['sample_failures'] = recorder.failures
    report['load_end'] = os.getloadavg() if hasattr(os, 'getloadavg') else None
    save(output / 'results.json', report)
    lines = ['# MCP release measurements', '', args.conditions, '',
             '| Input / mode | Samples | Median ms | P95 ms | >150 ms | Failures |',
             '| --- | ---: | ---: | ---: | ---: | ---: |']
    for case in report['workloads']:
        for mode, group in case.get('groups', {}).items():
            if not isinstance(group, dict) or group['median_ms'] is None:
                continue
            lines.append(f"| {case['name']} / {mode} | {group['sample_count']} | {group['median_ms']:.2f} | {group['p95_ms']:.2f} | {group['over_150ms_count']} | {group['failure_count']} |")
    (output / 'results.md').write_text('\n'.join(lines) + '\n')
    return 1 if report['failures'] or recorder.failures else 0


if __name__ == '__main__':
    sys.exit(main())
