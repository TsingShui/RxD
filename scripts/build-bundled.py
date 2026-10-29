#!/usr/bin/env python3
"""Compile Kuna specs, then directly link its adapted CLI into rxd."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET

from runtime_bundle import write_bundle
from zig_toolchain import prepare, cargo_environment

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'third_party/kuna'
BUILD = ROOT / 'target/kuna'
PROFILE = {'OPT_LEVEL': '3', 'LTO': 'thin', 'CODEGEN_UNITS': '1', 'STRIP': 'symbols'}
TARGET_RIDS = {
    'x86_64-unknown-linux-musl': 'linux-musl-x64',
    'aarch64-unknown-linux-musl': 'linux-musl-arm64',
    'x86_64-pc-windows-msvc': 'win-x64',
    'aarch64-apple-darwin': 'osx-arm64',
}


def capture(args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def available_memory():
    if sys.platform == 'darwin':
        value = capture(['memory_pressure', '-Q'])
        match = re.search(r'memory free percentage: (\d+)%', value)
        return int(match.group(1)) if match else None
    if sys.platform.startswith('linux'):
        values = {}
        for line in Path('/proc/meminfo').read_text().splitlines():
            fields = line.split()
            if fields[0] in ('MemTotal:', 'MemAvailable:'):
                values[fields[0]] = int(fields[1])
        return 100 * values['MemAvailable:'] / values['MemTotal:']
    return None


def run(args, cwd=ROOT, env=None):
    print('+ ' + ' '.join(map(str, args)), flush=True)
    child = subprocess.Popen(list(map(str, args)), cwd=cwd, env=env,
                             start_new_session=os.name == 'posix')
    try:
        while child.poll() is None:
            free = available_memory()
            if free is not None and free <= 10:
                raise RuntimeError('Stopped build: available system memory reached 10%')
            time.sleep(1)
        if child.returncode:
            raise subprocess.CalledProcessError(child.returncode, args)
    finally:
        if child.poll() is None:
            if os.name == 'posix':
                os.killpg(child.pid, signal.SIGTERM)
            else:
                child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                if os.name == 'posix':
                    os.killpg(child.pid, signal.SIGKILL)
                else:
                    child.kill()
                child.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--jobs', type=int, default=1)
    parser.add_argument('--target', choices=TARGET_RIDS, help='Rust target (Linux defaults to musl)')
    parser.add_argument('--zig', help='Pinned Zig executable for musl targets')
    parser.add_argument('--offline', action='store_true')
    parser.add_argument('--bundle-only', action='store_true')
    parser.add_argument('--features', default='', help='Additional rxd Cargo features')
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error('--jobs must be positive')
    if not (SOURCE / 'decompiler/Cargo.toml').exists():
        run(['git', 'submodule', 'update', '--init', 'third_party/kuna'])
    revision = capture(['git', 'rev-parse', 'HEAD'], SOURCE)
    index = capture(['git', 'ls-files', '--stage', 'third_party/kuna']).split()
    if len(index) < 2 or index[0] != '160000' or revision != index[1]:
        raise RuntimeError('Kuna checkout must match the parent repository gitlink')
    if capture(['git', 'status', '--porcelain', '--untracked-files=no'], SOURCE):
        raise RuntimeError('Kuna tracked source must be unmodified')
    major = (SOURCE / 'VERSION').read_text().strip()
    if capture(['git', 'rev-parse', '--is-shallow-repository'], SOURCE) == 'true':
        raise RuntimeError('Kuna versioning needs full submodule history; fetch --unshallow first')
    bump = capture(['git', 'log', '-1', '--format=%H', '--', 'VERSION'], SOURCE)
    version = major + '.' + capture(['git', 'rev-list', '--count', bump + '..HEAD'], SOURCE)
    rustc = capture(['rustc', '-vV'])
    host = next(line[6:] for line in rustc.splitlines() if line.startswith('host: '))
    target = args.target or os.environ.get('CARGO_BUILD_TARGET') or host.replace('-linux-gnu', '-linux-musl')
    if target not in TARGET_RIDS:
        raise RuntimeError('Unsupported distribution target: ' + target)
    output = BUILD / target
    output.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.pop('CARGO_BUILD_TARGET', None)
    for key in list(env):
        if key.startswith('CARGO_PROFILE_RELEASE_') or key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS'):
            del env[key]
    env.update({'CARGO_TARGET_DIR': str(output / 'cargo'), 'CARGO_INCREMENTAL': '0',
                'CARGO_BUILD_JOBS': str(args.jobs), 'KUNA_VERSION': version})
    env.update({'CARGO_PROFILE_RELEASE_' + key: value for key, value in PROFILE.items()})
    cargo = ['cargo', 'build', '--release', '--locked', '-j', str(args.jobs)]
    if args.offline:
        cargo.append('--offline')
    run(cargo + ['-p', 'kuna-slacomp', '--bin', 'slacomp'],
        SOURCE / 'decompiler', env)
    suffix = '.exe' if os.name == 'nt' else ''
    binaries = output / 'cargo/release'
    spec_sources = output / 'specs-build'
    stamp = output / 'specs-commit'
    if not stamp.exists() or stamp.read_text() != revision:
        if spec_sources.exists():
            shutil.rmtree(spec_sources)
        shutil.copytree(SOURCE / 'specs', spec_sources,
                        ignore=shutil.ignore_patterns('*.sla'))
        run([binaries / ('slacomp' + suffix), '-a', spec_sources])
        stamp.write_text(revision)
    search_dirs = list(spec_sources.glob('Ghidra/Processors/*/data/languages'))
    search_dirs += list(spec_sources.glob('Ghidra/contrib/*/data/languages'))
    search_dirs += [p for directory in list(search_dirs) for p in directory.iterdir() if p.is_dir()]
    for definition in spec_sources.rglob('*.ldefs'):
        for language in ET.parse(definition).getroot().findall('language'):
            names = [language.get('slafile'), language.get('processorspec')]
            names += [c.get('spec') for c in language.findall('compiler')]
            for name in names:
                if name and not any((directory / name).is_file() for directory in search_dirs):
                    raise RuntimeError('Missing runtime spec: ' + str(definition.parent / name))

    with tempfile.TemporaryDirectory(prefix='stage-', dir=output) as staging:
        stage = Path(staging)
        for name in ('LICENSE', 'NOTICE'):
            shutil.copy2(SOURCE / name, stage / name)
        for source in spec_sources.rglob('*'):
            if source.is_file() and source.suffix in ('.sla', '.pspec', '.cspec', '.ldefs'):
                dest = stage / 'specs' / source.relative_to(spec_sources)
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, dest)
        metadata = write_bundle(stage, output, {
            'revision': revision, 'version': version, 'target': target, 'build_host': host,
            'rustc': rustc, 'profile': PROFILE, 'executable': None,
            'integration': 'linked-kuna-cli',
        })
    metadata_path = output / 'manifest.json'
    embed = output / 'embed.rs'
    files = metadata['files']
    if capture(['git', 'status', '--porcelain', '--untracked-files=no'], SOURCE):
        raise RuntimeError('Build changed tracked Kuna source')
    print(json.dumps({'manifest': str(metadata_path), 'runtime_bytes': sum(x['size'] for x in files),
                      'compressed_bytes': metadata['bundle_bytes']}, indent=2))
    if not args.bundle_only:
        env['RXD_KUNA_EMBED_RS'] = str(embed)
        env['CARGO_TARGET_DIR'] = str(ROOT / 'target')
        target_args = []
        if target.endswith('-musl'):
            env.update(cargo_environment(target, prepare(target, args.zig), env))
            target_args = ['--target', target]
        features = ','.join(x for x in ('bundled-kuna', args.features) if x)
        if 'bundled-ilspy' in features.split(','):
            embed = env.get('RXD_ILSPY_EMBED_RS')
            if not embed:
                rid = TARGET_RIDS.get(target)
                candidate = ROOT / 'target/dotnet-native' / rid / 'embed.rs' if rid else None
                if candidate is not None and candidate.is_file():
                    env['RXD_ILSPY_EMBED_RS'] = str(candidate)
                else:
                    raise RuntimeError(
                        'bundled-ilspy requires a native engine; run '
                        'python3 scripts/build-ilspy.py ' + (rid or '<rid>') +
                        ' or set RXD_ILSPY_EMBED_RS to its embed.rs')
            elif not Path(embed).is_file():
                raise RuntimeError('Explicit RXD_ILSPY_EMBED_RS does not exist')
        run(['cargo', 'build', '--release', '--locked', '--features', features, '-j', str(args.jobs)]
            + target_args + (['--offline'] if args.offline else []), env=env)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
