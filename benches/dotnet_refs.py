#!/usr/bin/env python3
"""Verify and measure complete raw/semantic metadata XREF with fresh real CLI writes."""
import argparse
import hashlib
import json
import math
import os
import platform
import statistics
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def digest(data):
    return hashlib.sha256(data).hexdigest()

def sites_digest(rows):
    sites = [{key: row['detail'][key] for key in ('method_token', 'il_offset', 'opcode', 'target_token')}
             for row in rows[1:-1]]
    return digest(json.dumps(sites, sort_keys=True, separators=(',', ':')).encode())

def metrics(samples):
    values = [sample['duration_ms'] for sample in samples]
    over = sum(value > 150 for value in values)
    return dict(samples=samples, median_ms=statistics.median(values),
                p95_ms=sorted(values)[math.ceil(.95 * len(values)) - 1],
                over_150ms_count=over, over_150ms_proportion=over / len(values))

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, default=ROOT / 'tests/corpus/dotnet/method-refs-expectations.json')
    parser.add_argument('--samples', type=int, default=20)
    parser.add_argument('--production', type=Path, default=ROOT / 'target/dotnet-closure-corpus/newtonsoft-json-13.0.3/Newtonsoft.Json.dll')
    parser.add_argument('--framework', type=Path, default=Path.home() / '.dotnet10/shared/Microsoft.NETCore.App/10.0.12')
    parser.add_argument('--upstream', type=Path, default=ROOT / 'target/dotnet-research/references/dotscope/dotscope/tests/samples')
    parser.add_argument('--cpu-model', required=True)
    parser.add_argument('--memory-gb', type=int, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/benchmarks/method-xref')
    args = parser.parse_args()
    if args.samples < 20:
        parser.error('at least 20 fresh-process samples per workload are required')
    binary = args.binary.resolve()
    manifest = json.loads(args.manifest.read_text())
    paths = {'newtonsoft-json-13.0.3': args.production, 'System.Linq': args.framework / 'System.Linq.dll',
             'System.Text.Json': args.framework / 'System.Text.Json.dll',
             'WindowsBase': args.upstream / 'WindowsBase.dll',
             'CoreLib': args.upstream / 'dotnet_10.0/System.Private.CoreLib.dll'}
    args.output.mkdir(parents=True, exist_ok=True)
    report = dict(binary=str(binary), binary_sha256=digest(binary.read_bytes()), binary_bytes=binary.stat().st_size,
                  git_head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                  git_status=subprocess.check_output(['git', 'status', '--short'], cwd=ROOT, text=True),
                  cargo_lock_sha256=digest((ROOT / 'Cargo.lock').read_bytes()),
                  binary_provenance='Supplied stable binary; caller confirms release build separately through saved gate logs. Native engine is not loaded by XREF.',
                  hardware=dict(cpu=args.cpu_model, memory_gb=args.memory_gb, platform=platform.platform(), machine=platform.machine()),
                  cache='Warm OS file cache from identity/oracle preparation and initial query; no cache clearing; fresh CLI and worker each sample.',
                  measurement='No CPU isolation; includes startup, worker, read/hash, metadata and complete heap/IL scan, semantic proof, serialization and actual pipe/atomic output writes. Validation follows timing.',
                  limits='Unchanged defaults; no overrides. Instructions remain per method, work cumulative.',
                  oracle=manifest, workloads=[], acceptance='pending',
                  limitations=manifest.get('limitations', ['Non-generic production targets independently have no same-named MemberRefs or definition MethodSpecs; alias/instantiation broadening is established by independent synthetic CLI E2E.',
                               'Large default diagnostics retain failures and slow samples, grant no latency exception and do not establish latency acceptance.',
                               'Upstream WindowsBase/CoreLib artifacts are not broad independent production acceptance; no cold-cache claim.']))
    path = args.output / 'results.json'
    passed = True
    for case in manifest['cases']:
        file = paths[case['name']].resolve()
        if digest(file.read_bytes()) != case['input_sha256']:
            raise RuntimeError(f'pinned input mismatch: {file}')
        for semantic in (False, True):
            for to_file in ((False, True) if case['acceptance_role'] == 'ordinary' else (True,)):
                label = f"{case.get('workload_name', case['name'])}_{'semantic' if semantic else 'raw'}_{'file' if to_file else 'stdout'}"
                destination = args.output / (label + '.jsonl') if to_file else None
                command = [str(binary), 'dotnet', 'refs', str(file), '--token', f"0x{case['token']:08x}"]
                if semantic:
                    command.append('--semantic')
                if destination:
                    command += ['-o', str(destination.resolve())]
                item = dict(name=label, command=command, input_bytes=file.stat().st_size,
                            input_sha256=case['input_sha256'], acceptance_role=case['acceptance_role'])
                report['workloads'].append(item)
                samples = []
                for index in range(args.samples + 1):
                    start = time.perf_counter_ns()
                    result = subprocess.run(command, capture_output=True, timeout=60)
                    duration_ms = (time.perf_counter_ns() - start) / 1_000_000
                    sample = dict(duration_ms=duration_ms, returncode=result.returncode)
                    if result.returncode:
                        sample['stderr'] = result.stderr.decode(errors='replace')
                        if result.stdout or case['acceptance_role'] == 'ordinary' or 'RESOURCE_LIMIT' not in sample['stderr']:
                            item['failure'] = sample
                            report['acceptance'] = 'failed'
                            path.write_text(json.dumps(report, indent=2) + '\n')
                            raise RuntimeError(f'{label}: {sample}')
                        item['outcome'] = 'default_resource_failure'
                    else:
                        data = destination.read_bytes() if destination else result.stdout
                        if destination and result.stdout:
                            raise RuntimeError(f'file query emitted stdout: {label}')
                        rows = [json.loads(line) for line in data.splitlines()]
                        if rows[0]['schema_version'] != 2 or rows[-1]['complete'] is not True or rows[-1]['records'] != case['site_count'] or len(rows) != case['site_count'] + 2 or sites_digest(rows) != case['site_sha256']:
                            raise RuntimeError(f'complete independent site oracle mismatch: {label}')
                        if semantic and any(row['detail']['canonical_target_token'] != case['token'] or row['detail']['resolution_status'] != 'resolved' or row['detail']['target_kind'] != case.get('target_kind', 'method') for row in rows[1:-1]):
                            raise RuntimeError(f'unproved semantic identity: {label}')
                        for row in rows[1:-1]:
                            if not row['context_id'].startswith('rxd1:dotnet:' + case['input_sha256'] + ':method:') or row['detail']['declaring_type_id'] != row['parent_id']:
                                raise RuntimeError(f'caller navigation mismatch: {label}')
                        current_hash = digest(data)
                        if 'output_sha256' in item and item['output_sha256'] != current_hash:
                            raise RuntimeError(f'nondeterministic result: {label}')
                        item.update(outcome='complete_success', output_bytes=len(data), output_sha256=current_hash, records=case['site_count'])
                        if not destination and index == 0:
                            (args.output / (label + '.jsonl')).write_bytes(data)
                    if index == 0:
                        item['initial_sample'] = sample
                    else:
                        samples.append(sample)
                item.update(metrics(samples))
                if case['acceptance_role'] == 'ordinary' and item['over_150ms_count']:
                    passed = False
                path.write_text(json.dumps(report, indent=2) + '\n')
                print(label, item['outcome'], round(item['median_ms'], 2), round(item['p95_ms'], 2), flush=True)
    report['acceptance'] = ('ordinary_passed_large_scope_unapproved' if any(case['acceptance_role'] != 'ordinary' for case in manifest['cases']) else 'ordinary_passed') if passed else 'ordinary_failed'
    path.write_text(json.dumps(report, indent=2) + '\n')
    raise SystemExit(0 if passed else 1)

if __name__ == '__main__':
    main()
