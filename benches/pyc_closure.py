#!/usr/bin/env python3
"""Fixed six-format PYC acceptance: retained fresh processes and returned-ID workflows.

Run with a frozen release executable. No analyzed source/code is imported or
executed. Timing never includes oracle checking, hashes, persistence or tokens.
"""
import argparse
from collections import Counter
import datetime
import gzip
import hashlib
import importlib.metadata
import json
import math
import os
from pathlib import Path
import platform
import signal
import statistics
import subprocess
import sys
import tempfile
import time
import traceback

from pyc_versions import VERSIONS, literal_values, name_hits, validate_view

LABEL_QUERIES = {'ast': 'node', 'configparser': 'option', 'dataclasses': 'field',
                 'email-parser': 'read', 'email-policy': 'linesep'}
META = {'record', 'id', 'source_id', 'parent_id', 'context_id', 'start', 'end', 'detail'}
HEADER = {'record', 'backend', 'command', 'source_id', 'input_bytes', 'schema_version', 'query', 'detail'}
SUMMARY = {'record', 'records', 'complete', 'detail'}
DETAIL = {
    'code': 'index name qualname filename name_bytes qualname_bytes filename_bytes kind arg_count posonly_arg_count kwonly_arg_count n_locals stack_size flags first_line body_status bytecode_bytes consts_count names_count varnames_count freevars_count cellvars_count localsplus_count line_table_bytes exception_table_bytes fields incoming incoming_complete ownership',
    'string': 'code_index constant_slot constant_path kind value raw_bytes definition_start definition_end payload_start payload_end occurrence_start occurrence_end reference aliases',
    'reference': 'code_index offset opcode name raw_operand operand target operation constant_id constant_slot constant_path literal_kind relation',
    'code_view': 'input_bytes python_version representation complete query metadata bytecode_hex instructions tables values line_table_hex line_mapping exception_table_hex exceptions',
    'source': 'input_bytes python_version representation complete source',
}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def identity(path):
    data = path.read_bytes()
    return {'path': str(path.resolve()), 'bytes': len(data), 'sha256': sha(data)}


def save_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')


def stats(samples, key='duration_ns', threshold=150_000_000):
    values = sorted(s[key] for s in samples)
    rss = [s.get('wait4_max_process_rss_bytes') for s in samples if s.get('wait4_max_process_rss_bytes') is not None]
    return {'sample_count': len(values), 'median_ns': statistics.median(values),
            'p95_ns': values[math.ceil(len(values) * .95) - 1], 'min_ns': values[0], 'max_ns': values[-1],
            'bound_ns': threshold, 'over_bound_count': None if threshold is None else sum(v > threshold for v in values),
            'over_bound_proportion': None if threshold is None else sum(v > threshold for v in values) / len(values),
            'max_individual_process_peak_rss_bytes': max(rss) if rss else None}


def timeout_handler(_signum, _frame):
    raise TimeoutError('CLI timeout')


def run(argv, timeout):
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        start = time.perf_counter_ns()
        child = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, start_new_session=True)
        usage, timed_out = None, False
        try:
            signal.setitimer(signal.ITIMER_REAL, timeout)
            try:
                _, status, usage = os.wait4(child.pid, 0)
                child.returncode = os.waitstatus_to_exitcode(status)
            finally:
                signal.setitimer(signal.ITIMER_REAL, 0)
        except TimeoutError:
            timed_out = True
            os.killpg(child.pid, signal.SIGKILL)
            _, status, usage = os.wait4(child.pid, 0)
            child.returncode = os.waitstatus_to_exitcode(status)
        duration = time.perf_counter_ns() - start
        stdout.seek(0); stderr.seek(0)
        # Capture/read is included in workflows, excluded from the CLI duration.
        output, diagnostics = stdout.read(), stderr.read()
    rss = int(usage.ru_maxrss) * (1024 if sys.platform == 'linux' else 1) if usage is not None and sys.platform in ('darwin', 'linux') else None
    return {'argv': argv, 'duration_ns': duration, 'exit_code': child.returncode,
            'timed_out': timed_out, 'wait4_max_process_rss_bytes': rss}, output, diagnostics


def persist(directory, tag, call):
    metrics, output, diagnostics = call
    directory.mkdir(parents=True, exist_ok=True)
    base = directory / tag
    base.with_suffix('.stdout').write_bytes(output)
    base.with_suffix('.stderr').write_bytes(diagnostics)
    metrics.update(stdout_bytes=len(output), stderr_bytes=len(diagnostics), stdout_sha256=sha(output), stderr_sha256=sha(diagnostics),
                   stdout_file=str(base.with_suffix('.stdout')), stderr_file=str(base.with_suffix('.stderr')))
    save_json(base.with_suffix('.json'), metrics)
    # Always journal the full outcome before any validation (including failed routes).
    with (directory / 'journal.jsonl').open('a') as file:
        file.write(json.dumps(metrics, ensure_ascii=False) + '\n')
    return metrics


def parsed(call):
    return [json.loads(line) for line in call[1].splitlines()]


def code_id(digest, owner):
    return f'rxd1:pyc:{digest}:code:{owner}'


def selector(oracle, query):
    candidates = []
    for owner, code in enumerate(oracle['codes']):
        if not owner:
            continue
        for slot, constant in enumerate(code['consts']):
            if constant['type'] != 'str' or not constant['value'] or len(constant['utf8_surrogatepass_hex']) > 2000:
                continue
            # Surrogates cannot be passed as a UTF-8 command argument.
            try:
                constant['value'].encode('utf-8')
            except UnicodeEncodeError:
                continue
            hits = [i for i in code['dis_instructions'] if i['opname'] in ('LOAD_CONST', 'RETURN_CONST', 'KW_NAMES') and i['arg'] == slot]
            if hits:
                candidates.append((len(code['code_hex']) // 2, owner, slot, constant['value']))
    body, owner, slot, text = min(candidates)
    name_owner = min(name_hits(oracle, query), key=lambda h: (len(oracle['codes'][h[0]]['code_hex']), h[0], h[1]))[0]
    largest = max(range(1, len(oracle['codes'])), key=lambda i: (len(oracle['codes'][i]['code_hex']), -i))
    return {'constant_owner': owner, 'constant_slot': slot, 'text': text, 'constant_body_bytes': body,
            'name_query': query, 'name_owner': name_owner, 'largest_owner': largest}


def target_indexes(ins, code, minor, categories):
    op, arg, number = ins['opname'], ins['arg'], ins['opcode']
    if arg is None:
        return []
    if number in categories['hasconst']:
        return [('consts', arg)]
    if number in categories['hasname']:
        index = arg >> 2 if op == 'LOAD_SUPER_ATTR' else arg >> 1 if (op == 'LOAD_GLOBAL' and minor >= 11) or (op == 'LOAD_ATTR' and minor >= 12) else arg
        return [('names', index)]
    if number in categories['haslocal'] or number in categories['hasfree']:
        if minor >= 11:
            indexes = [arg >> 4, arg & 15] if op in ('LOAD_FAST_LOAD_FAST', 'STORE_FAST_LOAD_FAST', 'STORE_FAST_STORE_FAST') else [arg]
            return [('localsplus', i) for i in indexes]
        if number in categories['haslocal']:
            return [('varnames', arg)]
        cells = len(code['cellvars'])
        return [('cellvars', arg)] if arg < cells else [('freevars', arg - cells)]
    return []


def verify_view(row, owner, oracle, digest, data, minor, categories):
    code = oracle['codes'][owner]
    ranges = oracle['marshal_ranges']['code_ranges'][owner]
    validate_view(row, code, ranges)
    assert row['id'] == code_id(digest, owner) and row['source_id'] == digest
    assert set(row) == META | {'schema_version', 'backend', 'command'}
    assert set(row['detail']) == set(DETAIL['code_view'].split())
    detail = row['detail']
    assert detail['query'] == {'code_id': row['id'], 'version': None}
    assert data[ranges['pyc_bytecode_start']:ranges['pyc_bytecode_start'] + len(code['code_hex']) // 2].hex() == code['code_hex']
    assert detail['line_table_hex'] == (code['linetable_hex'] or code['lnotab_hex'])
    assert detail['exception_table_hex'] == code['exceptiontable_hex']
    values = {v['id']: v for v in detail['values']}
    for value in values.values():
        if value['kind'] in ('str', 'bytes') and value['payload_start'] is not None:
            payload = data[value['payload_start']:value['payload_end']]
            assert value['raw_hex'] == payload.hex()

    def value_matches(identifier, expected):
        actual = values[identifier]
        kind = expected['type']
        if kind == 'code':
            assert actual['code_id'] == code_id(digest, expected['index'])
        elif kind in ('tuple', 'frozenset'):
            assert actual['kind'] == kind and len(actual['children']) == len(expected['items'])
            if kind == 'tuple':
                for child, wanted in zip(actual['children'], expected['items']):
                    value_matches(child['value_id'], wanted)
            # Frozenset order is explicitly outside the saved JSON oracle's scope.
        elif kind in ('str', 'bytes'):
            assert actual['kind'] == kind
            assert actual['raw_hex'] == expected['utf8_surrogatepass_hex' if kind == 'str' else 'hex']
        elif kind == 'int':
            assert actual['kind'] == 'int' and actual['value'] == str(expected['value'])
        elif kind == 'bool':
            assert actual['kind'] == 'bool' and actual['value'] == ('true' if expected['value'] else 'false')
        elif kind == 'NoneType':
            assert actual['kind'] == 'none'
        elif kind == 'ellipsis':
            assert actual['kind'] == 'ellipsis'
        else:
            raise ValueError(('unvalidated table value kind', kind))
    for table in ('consts', 'names', 'varnames', 'freevars', 'cellvars'):
        for entry, expected in zip(detail['tables'][table]['entries'], code[table]):
            if table == 'consts':
                value_matches(entry['value_id'], expected)
            else:
                assert values[entry['value_id']]['value'] == expected
    if minor >= 11:
        for entry in detail['tables']['localsplus']['entries']:
            assert values[entry['value_id']]['value'] == ranges['localsplus_names'][entry['index']]
    for ins in code['dis_instructions']:
        unit = next(u for u in detail['instructions'] if u['offset'] == ins['offset'])
        if unit['role'] != 'instruction':
            continue
        wanted = target_indexes(ins, code, minor, categories)
        assert [(t['table'], t['index']) for t in unit['targets']] == wanted, (owner, ins, wanted)
        assert unit['start'] == ranges['pyc_bytecode_start'] + ins['offset'] and unit['end'] == unit['start'] + 2
        assert unit['raw_hex'] == data[unit['start']:unit['end']].hex()
        if wanted and wanted[0][0] == 'consts':
            constant = code['consts'][wanted[0][1]]
            actual = values[unit['targets'][0]['value_id']]
            if constant['type'] == 'str':
                assert actual['raw_hex'] == constant['utf8_surrogatepass_hex']
            elif constant['type'] == 'bytes':
                assert actual['raw_hex'] == constant['hex']
    return row


def check_stream(rows, command, query, digest, data):
    assert len(rows) >= 2 and set(rows[0]) == HEADER and set(rows[-1]) == SUMMARY
    assert rows[0]['record'] == 'header' and rows[-1]['record'] == 'summary'
    assert rows[0]['backend'] == 'pyc' and rows[0]['command'] == command and rows[0]['schema_version'] == 2
    assert rows[0]['source_id'] == digest and rows[0]['input_bytes'] == len(data)
    assert rows[0]['query'] == query and rows[-1]['complete'] is True
    header_detail = {'analysis', 'python_version', 'code_count', 'instruction_validation'}
    summary_detail = {'analysis', 'code_count', 'instruction_validation'}
    if command == 'strings':
        header_detail.add('containers')
    if command == 'refs':
        header_detail.add('scope')
        summary_detail |= {'scanned_code_count', 'instructions', 'unsupported_count', 'unsupported'}
    assert set(rows[0]['detail']) == header_detail and set(rows[-1]['detail']) == summary_detail
    assert rows[0]['detail']['instruction_validation'] == ('scanned_scope' if command == 'refs' else 'not_scanned')
    if command == 'refs':
        assert rows[-1]['detail']['unsupported_count'] == 0 and rows[-1]['detail']['unsupported'] == []
    assert rows[-1]['records'] == len(rows) - 2
    for row in rows[1:-1]:
        assert set(row) == META and row['source_id'] == digest
        assert set(row['detail']) == set(DETAIL[row['record']].split())
    return rows[1:-1]


def check_call(call, command, arguments, oracle, selection, digest, data, views, minor, categories, source_complete=None):
    metric, output, stderr = call
    assert not metric['timed_out'], 'timeout'
    if command == 'decompile':
        if '--json' in arguments:
            assert metric['exit_code'] == 0 and not stderr
            row = json.loads(output)
            assert set(row) == META | {'schema_version', 'backend', 'command'}
            assert set(row['detail']) == set(DETAIL['source'].split())
            assert row['source_id'] == digest and row['schema_version'] == 2 and row['detail']['input_bytes'] == len(data)
            assert row['detail']['complete'] is source_complete
            return 'source_complete' if source_complete else 'source_incomplete'
        if source_complete:
            assert metric['exit_code'] == 0 and not stderr and output
            return 'source_complete'
        assert metric['exit_code'] == 1 and not output and b'INCOMPLETE_DECOMPILATION' in stderr
        return 'expected_incomplete_source_error'
    assert metric['exit_code'] == 0 and not stderr, (metric['exit_code'], stderr.decode('utf-8', 'replace'))
    if command == 'get-code':
        owner = int(arguments[0].rsplit(':', 1)[-1])
        verify_view(json.loads(output), owner, oracle, digest, data, minor, categories)
        return 'raw_complete'
    keys = {'outline': ('name', 'index', 'version'), 'strings': ('contains', 'contains_hex', 'version'), 'refs': ('name', 'const_id', 'within', 'version')}[command]
    query = {k: None for k in keys}
    for i in range(0, len(arguments), 2):
        key = arguments[i][2:].replace('-', '_')
        query[key] = int(arguments[i+1]) if key == 'index' else arguments[i+1]
    records = check_stream(parsed(call), command, query, digest, data)
    if command == 'outline':
        indexes = [i for i, code in enumerate(oracle['codes']) if (query['index'] is None or i == query['index']) and (query['name'] is None or code['name'] == query['name'])]
        assert len(records) == len(indexes)
        for row, index in zip(records, indexes):
            code, ranges = oracle['codes'][index], oracle['marshal_ranges']['code_ranges'][index]
            assert row['id'] == code_id(digest, index) and row['detail']['index'] == index
            assert (row['detail']['name'], row['detail']['qualname'], row['detail']['bytecode_bytes']) == (code['name'], code['qualname'], len(code['code_hex'])//2)
            assert (row['start'], row['end']) == (ranges['pyc_marshal_start'], ranges['pyc_marshal_end'])
        return 'metadata_complete'
    if command == 'strings':
        wanted = []
        for owner, code in enumerate(oracle['codes']):
            for slot, value in enumerate(code['consts']):
                for kind, hexdata in literal_values(value):
                    raw = bytes.fromhex(hexdata)
                    if query['contains'] is not None:
                        try:
                            text = raw.decode('utf-8')
                        except UnicodeDecodeError:
                            continue
                        if kind != 'str' or query['contains'] not in text:
                            continue
                    if query['contains_hex'] is not None and bytes.fromhex(query['contains_hex']) not in raw:
                        continue
                    wanted.append((owner, slot, kind, hexdata))
        observed = [(r['detail']['code_index'], r['detail']['constant_slot'], r['detail']['kind'], r['detail']['raw_bytes']) for r in records]
        assert Counter(observed) == Counter(wanted)
        assert len({r['id'] for r in records}) == len(records)
        for row in records:
            d = row['detail']; owner = d['code_index']
            assert row['context_id'] == code_id(digest, owner)
            assert row['id'] == f"rxd1:pyc:{digest}:const:{owner}:" + '.'.join(map(str, d['constant_path']))
            assert data[d['payload_start']:d['payload_end']].hex() == d['raw_bytes']
        return 'literal_complete_multiset'
    if query['name'] is not None:
        wanted = name_hits(oracle, query['name'])
        if query['within'] is not None:
            wanted = [h for h in wanted if code_id(digest, h[0]) == query['within']]
    else:
        owner, slot = selection['constant_owner'], selection['constant_slot']
        wanted = [(owner, i['offset'], i['opcode'], i['arg'], {'LOAD_CONST':'constant_load', 'RETURN_CONST':'constant_return', 'KW_NAMES':'keyword_names'}[i['opname']], 0)
                  for i in oracle['codes'][owner]['dis_instructions'] if i['opname'] in ('LOAD_CONST','RETURN_CONST','KW_NAMES') and i['arg'] == slot]
        if query['within'] is not None and query['within'] != code_id(digest, owner):
            wanted = []
    assert len(records) == len(wanted)
    for row, hit in zip(records, wanted):
        owner, offset, opcode, arg, operation, position = hit
        d = row['detail']
        assert (d['code_index'], d['offset'], d['opcode'], d['operand'], d['operation']) == hit[:5]
        unit = next(u for u in views[owner]['detail']['instructions'] if u['offset'] == offset)
        assert row['context_id'] == code_id(digest, owner) and d['target'] == unit['targets'][position]
        assert (row['start'], row['end'], d['raw_operand']) == (unit['start'], unit['end'], unit['raw_operand'])
        if query['const_id'] is not None:
            assert d['constant_id'] == query['const_id'] and d['constant_path'] == [selection['constant_slot']] and d['relation'] == 'direct_constant'
    return 'explicit_refs_complete'


def workload_specs(path, selection, oracle, digest):
    c, n, l = (selection[k] for k in ('constant_owner', 'name_owner', 'largest_owner'))
    cid, nid = code_id(digest, c), code_id(digest, n)
    const = f"rxd1:pyc:{digest}:const:{c}:{selection['constant_slot']}"
    return [
        ('outline-all','outline',[]), ('outline-index','outline',['--index',str(c)]),
        ('outline-name','outline',['--name',oracle['codes'][c]['name']]),
        ('strings-all','strings',[]), ('strings-text','strings',['--contains',selection['text']]),
        ('strings-hex','strings',['--contains-hex',selection['text'].encode().hex()]),
        ('refs-name-all','refs',['--name',selection['name_query']]),
        ('refs-name-within','refs',['--name',selection['name_query'],'--within',nid]),
        ('refs-const-owner','refs',['--const-id',const]), ('refs-const-within','refs',['--const-id',const,'--within',cid]),
        ('get-code-root','get-code',[code_id(digest,0)]), ('get-code-constant','get-code',[cid]),
        ('get-code-name','get-code',[nid]), ('get-code-largest','get-code',[code_id(digest,l)]),
        ('decompile-json','decompile',['--json']), ('decompile-source','decompile',[])]


def workflow(binary, path, kind, selection, timeout):
    calls, routes, ids = [], [], []
    start = time.perf_counter_ns()
    error = None
    try:
        if kind == 'outline-fetch':
            command, args = 'outline', ['--index', str(selection['constant_owner'])]
        elif kind == 'strings-refs-fetch':
            command, args = 'strings', ['--contains', selection['text']]
        else:
            command, args = 'refs', ['--name', selection['name_query']]
        while True:
            call = run([str(binary), 'pyc', command, str(path), *args], timeout)
            calls.append((command, args, call))
            route_start = time.perf_counter_ns()
            if call[0]['exit_code'] or call[0]['timed_out']:
                raise ValueError('route input failed')
            rows = parsed(call)
            if command == 'get-code':
                routes.append({'segment':'final-fetch-json', 'duration_ns':time.perf_counter_ns()-route_start})
                break
            if not rows[-1]['complete']:
                raise ValueError('route input incomplete')
            if command == 'outline':
                identifier = next(r['id'] for r in rows if r['record'] == 'code' and r['detail']['index'] == selection['constant_owner'])
                command, args = 'get-code', [identifier]
            elif command == 'strings':
                identifier = next(r['id'] for r in rows if r['record'] == 'string' and r['detail']['code_index'] == selection['constant_owner'] and r['detail']['constant_path'] == [selection['constant_slot']] and r['detail']['value'] == selection['text'])
                command, args = 'refs', ['--const-id', identifier]
            else:
                owner = selection['constant_owner'] if kind == 'strings-refs-fetch' else selection['name_owner']
                identifier = next(r['context_id'] for r in rows if r['record'] == 'reference' and r['detail']['code_index'] == owner)
                command, args = 'get-code', [identifier]
            ids.append(identifier)
            routes.append({'segment':'parse-select-route', 'duration_ns':time.perf_counter_ns()-route_start})
    except (ValueError, KeyError, StopIteration, json.JSONDecodeError) as exception:
        error = repr(exception)
    duration = time.perf_counter_ns()-start
    command_ns = sum(c[2][0]['duration_ns'] for c in calls)
    return {'kind':kind, 'duration_ns':duration, 'command_duration_ns':command_ns,
            'route_json_duration_ns':sum(r['duration_ns'] for r in routes), 'route_json_segments':routes,
            'workflow_minus_command_ns':duration-command_ns,
            'residual_definition':'aggregate minus Popen/wait4 durations; includes stdout/stderr read/capture, JSON/selection, tempfiles and harness overhead, not pure routing',
            'actual_returned_ids':ids, 'route_error':error}, calls


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--build-provenance', type=Path, required=True)
    parser.add_argument('--corpus', type=Path, default=Path('tests/corpus/pyc/matrix'))
    parser.add_argument('--output', type=Path, default=Path('target/benchmarks/pyc-closure/default'))
    parser.add_argument('--samples', type=int, default=20)
    parser.add_argument('--timeout', type=float, default=30)
    parser.add_argument('--cpu-model', required=True)
    parser.add_argument('--memory-bytes', type=int, required=True)
    parser.add_argument('--tokenizer-provenance', type=Path, required=True)
    parser.add_argument('--smoke', action='store_true', help='One actual module per format; scope stays separate from full default acceptance')
    args = parser.parse_args()
    if args.samples < 20:
        parser.error('at least 20 fresh processes per workload are required')
    args.binary = args.binary.resolve(); args.corpus = args.corpus.resolve(); args.output = args.output.resolve()
    if args.output.exists():
        parser.error('output must be a new directory; previous failures are never overwritten')
    args.output.mkdir(parents=True)
    signal.signal(signal.SIGALRM, timeout_handler)
    provenance = json.loads(args.build_provenance.read_text())
    assert provenance['binary']['sha256'] == identity(args.binary)['sha256']
    report = {'started_at':datetime.datetime.now(datetime.timezone.utc).isoformat(), 'binary':identity(args.binary),
              'build_provenance':provenance, 'runner':identity(Path(__file__)), 'oracle_helper':identity(Path(__file__).with_name('pyc_versions.py')),
              'scope':'six-format actual email-policy smoke' if args.smoke else 'all 30 actual stdlib modules; six verified final formats',
              'hardware':{'cpu_model':args.cpu_model,'memory_bytes':args.memory_bytes,'platform':platform.platform(),'machine':platform.machine()},
              'cache_conditions':'warm OS file caches from excluded preparation; no cache clearing or CPU isolation; every call creates fresh CLI/worker',
              'timing':'Popen+wait4 through real file-backed stdout/stderr writes; oracle/hash/token/report work after timing; no --perf headline',
              'rss_semantics':'wait4 largest individual process high-water mark, not simultaneous process-tree sum',
              'environment':{k:os.environ.get(k) for k in ('CARGO_INCREMENTAL','CARGO_PROFILE_DEV_DEBUG','CARGO_PROFILE_TEST_DEBUG','CARGO_TARGET_DIR','TIKTOKEN_CACHE_DIR','RXD_DOTNET_NATIVE_LIBRARY','RXD_REFDIR')},
              'workflow_bound_ns':None, 'load_average_start':os.getloadavg(), 'tokenizer_provenance':json.loads(args.tokenizer_provenance.read_text()),
              'literal_oracle_scope':'owner/slot/kind/payload multisets and original payload slices; tuple paths + worked-wire frozenset/alias E2Es; sorted frozenset JSON does not prove child occurrence order',
              'inputs':[], 'corpus_audit':[], 'failures':[], 'incomplete_source':[], 'slow_workloads':[]}
    meta = json.loads((args.corpus/'interpreters.json').read_text())
    all_calls = []
    for version in VERSIONS:
        directory = args.corpus/version
        manifest = json.loads((directory/'manifest.json').read_text())
        audit = []
        for filename in ('manifest.json','semantics.manifest.json'):
            packing = json.loads((directory/filename).read_text())
            for name, expected in packing['artifacts'].items():
                path = directory/expected.get('storage', name)
                data = path.read_bytes()
                if 'storage' in expected:
                    assert sha(data) == expected['storage_sha256']
                    data = gzip.decompress(data)
                assert sha(data) == expected['sha256'] and len(data) == expected['bytes'], (version,name)
                audit.append({'name':name, **identity(path)})
        report['corpus_audit'].append({'version':version,'compiler':manifest['version_info'],'manifest':identity(directory/'manifest.json'),'artifacts':audit})
        categories = next(m['opcode_categories'] for m in meta if '.'.join(map(str,m['version_info'][:3])) == version)
        minor = int(version.split('.')[1])
        labels = ['email-policy'] if args.smoke else list(LABEL_QUERIES)
        for label in labels:
            folder = args.output/version/label
            path = directory/(label+'.timestamp.pyc'); data = path.read_bytes(); digest = sha(data)
            oracle = json.loads(gzip.decompress((directory/(label+'.oracle.json.gz')).read_bytes()))
            selection = selector(oracle, LABEL_QUERIES[label])
            source_complete = True
            entry = {'version':version,'label':label,'input':identity(path),'oracle':identity(directory/(label+'.oracle.json.gz')),
                     'provenance':next(i for i in manifest['inputs'] if i['label'] == label), 'selectors':selection,
                     'target_scales':{str(i):{'name':c['name'],'body_bytes':len(c['code_hex'])//2,'raw_units':len(c['code_hex'])//4,'consts':len(c['consts']),'names':len(c['names']),'members':1} for i,c in enumerate(oracle['codes'])},
                     'code_count':len(oracle['codes']), 'workloads':[], 'workflows':[], 'oracle_probes':[]}
            report['inputs'].append(entry)
            views = {}
            # Independent correctness probes retained outside headline sampling.
            for owner in range(len(oracle['codes'])):
                call = run([str(args.binary),'pyc','get-code',str(path),code_id(digest,owner)],args.timeout)
                metric = persist(folder/'oracle',str(owner),call); all_calls.append(metric)
                try:
                    assert metric['exit_code'] == 0 and not metric['timed_out'] and not call[2]
                    views[owner] = verify_view(json.loads(call[1]),owner,oracle,digest,data,minor,categories)
                    metric['contract']='independently_verified_raw_body'
                except (AssertionError,KeyError,ValueError,StopIteration) as error:
                    metric['contract_error']=traceback.format_exc()
                    report['failures'].append({'version':version,'label':label,'oracle_code':owner,'error':traceback.format_exc()})
                entry['oracle_probes'].append(metric)
            specs = workload_specs(path,selection,oracle,digest)
            dedup = {}
            for name,command,arguments in specs:
                key = (command,tuple(arguments))
                if key in dedup:
                    dedup[key]['labels'].append(name)
                    continue
                workload = {'labels':[name],'command':command,'arguments':arguments,'samples':[]}
                dedup[key]=workload; entry['workloads'].append(workload)
                directory_out = folder/name
                prepared = run([str(args.binary),'pyc',command,str(path),*arguments],args.timeout)
                workload['preparation_excluded'] = persist(directory_out,'preparation',prepared)
                all_calls.append(workload['preparation_excluded'])
                try:
                    workload['outcome']=check_call(prepared,command,arguments,oracle,selection,digest,data,views,minor,categories,source_complete)
                except (AssertionError,KeyError,ValueError,StopIteration) as error:
                    workload['contract_error']=traceback.format_exc()
                    report['failures'].append({'version':version,'label':label,'workload':name,'error':traceback.format_exc()})
                for index in range(args.samples):
                    call=run([str(args.binary),'pyc',command,str(path),*arguments],args.timeout)
                    metric=persist(directory_out,f'sample-{index:03}',call);all_calls.append(metric)
                    metric['matches_preparation']=call[1:] == prepared[1:] and metric['exit_code'] == prepared[0]['exit_code'] and not metric['timed_out']
                    if not metric['matches_preparation']:
                        report['failures'].append({'version':version,'label':label,'workload':name,'sample':index,'reason':'stdout/full-stderr/exit/timeout mismatch'})
                    workload['samples'].append(metric)
                workload['statistics']=stats(workload['samples'])
                if workload['statistics']['over_bound_count']:
                    report['slow_workloads'].append({'version':version,'label':label,'workload':name,'statistics':workload['statistics']})
                if command=='decompile' and not source_complete:
                    report['incomplete_source'].append({'version':version,'label':label,'mode':'json' if arguments else 'source','outcome':workload.get('outcome'),'preparation':workload['preparation_excluded']})
                profiled=run([str(args.binary),'pyc',command,str(path),*arguments,'--perf'],args.timeout)
                workload['profile_excluded']=persist(directory_out,'profile',profiled);all_calls.append(workload['profile_excluded'])
                profiles=[]
                for line in profiled[2].splitlines():
                    try:
                        row=json.loads(line)
                        if row.get('record')=='perf': profiles.append(row)
                    except (ValueError,AttributeError): pass
                workload['perf']=profiles
                if command!='decompile':
                    analyses=[p for p in profiles if p.get('scope')=='analysis']
                    if len(analyses)!=1 or analyses[0]['counters']['ast_depth'] is not None or {'decompile','generate'} & set(analyses[0]['stages_ns']):
                        report['failures'].append({'version':version,'label':label,'workload':name,'reason':'static perf route is not AST-free'})
                save_json(args.output/'results.json',report)
            for kind in ('outline-fetch','strings-refs-fetch','name-refs-fetch'):
                flow={'kind':kind,'samples':[]};entry['workflows'].append(flow)
                for index in range(args.samples):
                    metric,calls=workflow(args.binary,path,kind,selection,args.timeout)
                    metric['calls']=[]
                    for step,(command,arguments,call) in enumerate(calls):
                        saved=persist(folder/'workflows'/kind/f'{index:03}',str(step),call);all_calls.append(saved)
                        metric['calls'].append(saved)
                    # All responses and route failure are preserved before checking.
                    save_json(folder/'workflows'/kind/f'{index:03}'/'workflow.json',metric)
                    try:
                        assert metric['route_error'] is None, metric['route_error']
                        for command,arguments,call in calls:
                            check_call(call,command,arguments,oracle,selection,digest,data,views,minor,categories,source_complete)
                        metric['contract']='verified_actual_returned_ids'
                    except (AssertionError,KeyError,ValueError,StopIteration) as error:
                        metric['contract_error']=traceback.format_exc()
                        report['failures'].append({'version':version,'label':label,'workflow':kind,'sample':index,'error':traceback.format_exc()})
                    flow['samples'].append(metric)
                flow['statistics']=stats(flow['samples'],threshold=None)
                flow['route_json_statistics']=stats(flow['samples'],key='route_json_duration_ns',threshold=None)
                flow['residual_statistics']=stats(flow['samples'],key='workflow_minus_command_ns',threshold=None)
            source_rows = {label:w for w in entry['workloads'] for label in w['labels'] if label in ('decompile-json','decompile-source')}
            try:
                reconstructed = json.loads(Path(source_rows['decompile-json']['preparation_excluded']['stdout_file']).read_bytes())['detail']['source'].encode('utf-8')
                source_output = Path(source_rows['decompile-source']['preparation_excluded']['stdout_file']).read_bytes()
                assert source_output == (reconstructed if source_complete else b'')
                entry['source_modes_consistent'] = True
            except (AssertionError, KeyError, ValueError) as error:
                report['failures'].append({'version':version,'label':label,'reason':'source/JSON contract mismatch','error':traceback.format_exc()})
            assert sha(path.read_bytes())==digest, 'input changed'
            save_json(args.output/'results.json',report)
            print(version,label,'workloads',len(entry['workloads']),'failures',len(report['failures']),'slow',len(report['slow_workloads']),flush=True)
    # Count complete exact UTF-8 response after ALL process and workflow timing.
    import tiktoken
    assert importlib.metadata.version('tiktoken')=='0.12.0'
    encoding=tiktoken.get_encoding('o200k_base'); token_counts={}
    for metric in all_calls:
        digest=metric['stdout_sha256']
        if digest not in token_counts:
            payload=Path(metric['stdout_file']).read_bytes()
            token_counts[digest]=len(encoding.encode_ordinary(payload.decode('utf-8')))
        metric['stdout_o200k_base_tokens']=token_counts[digest]
    for entry in report['inputs']:
        for flow in entry['workflows']:
            for sample in flow['samples']:
                sample['stdout_bytes']=sum(c['stdout_bytes'] for c in sample['calls'])
                sample['stdout_o200k_base_tokens']=sum(c['stdout_o200k_base_tokens'] for c in sample['calls'])
    report['token_accounting']='tiktoken0.12.0/o200k_base encode_ordinary of entire UTF-8 stdout, including envelope/newlines; named accounting only, not Codex tokenizer/billing'
    report['binary_after']=identity(args.binary)
    assert report['binary']==report['binary_after'], 'frozen binary changed'
    report['finished_at']=datetime.datetime.now(datetime.timezone.utc).isoformat();report['load_average_end']=os.getloadavg()
    report['accepted']=not (report['failures'] or report['slow_workloads'])
    save_json(args.output/'results.json',report)
    save_json(args.output/'token-counts.json',token_counts)
    return int(not report['accepted'])


if __name__=='__main__':
    raise SystemExit(main())
