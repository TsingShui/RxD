#!/usr/bin/env python3
"""Package rxd with its linked Kuna CLI, compressed specs, and embedded ILSpy."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify(stage, kuna):
    binary = stage / ('rxd.exe' if os.name == 'nt' else 'rxd')
    with tempfile.TemporaryDirectory(prefix='rxd-package-') as temporary:
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(('RXD_', 'KUNA_', 'DOTNET_')) and key != 'SLEIGHHOME'}
        env['RXD_CACHE_DIR'] = str(Path(temporary) / 'cache')

        def run(*args):
            return subprocess.check_output([binary, *map(str, args)], env=env,
                                           cwd=temporary, timeout=120)

        expected_version = re.search(r'^version = "([^"]+)"$',
                                     (ROOT / 'Cargo.toml').read_text().split('[package]', 1)[1]
                                     .split('\n[', 1)[0], re.MULTILINE).group(1)
        assert run('--version').decode().strip() == 'rxd ' + expected_version
        run('--help')
        subprocess.check_call([sys.executable, str(ROOT / 'scripts/mcp-smoke.py'),
                               '--binary', str(binary), '--expect-native'],
                              env=env, cwd=temporary, timeout=120)
        assert not (Path(temporary) / 'cache/rxd-ilspy').exists()
        evm_input = Path(temporary) / 'runtime.hex'
        evm_input.write_text('0x6003565b00\n', encoding='ascii')
        evm_result = json.loads(run('evm', 'analyze', evm_input, '--encoding', 'hex'))
        assert evm_result['detail']['engine'] == {'name': 'evmole', 'version': '0.9.4'}
        assert evm_result['detail']['result']['disassembled'][-1] == [4, 'STOP']
        assert run('kuna', '--version').decode().strip() == 'kuna ' + kuna['version']
        assert json.loads(run('kuna', 'catalog', '--json', '--option', 'loweredswitch'))
        assert not (Path(temporary) / 'cache/rxd-kuna').exists()
        sample = ROOT / 'third_party/kuna/decompiler/crates/kuna-analysis/tests/fixtures/fauxware'
        recovered = json.loads(run('kuna', 'decompile', sample, 'main', '--json'))
        assert recovered['functions'][0]['code']
        kuna_cache = next(path for path in (Path(temporary) / 'cache/rxd-kuna').iterdir()
                          if (path / '.complete').is_file())
        assert not (kuna_cache / 'kuna').exists() and not (kuna_cache / 'kuna.exe').exists()
        assert kuna_cache.name == kuna['bundle_sha256']
        run('html', 'entries', ROOT / 'tests/fixtures/html/catalog.html')
        sample = ROOT / 'tests/fixtures/dotnet/sugar/Sugar.dll'
        rows = [json.loads(line) for line in run('dotnet', 'outline', sample,
                '--type', 'Rxd.Sugar.Probe', '--method', 'AsyncAwait').splitlines()]
        method = next(row['id'] for row in rows if row['record'] == 'method')
        assert not (Path(temporary) / 'cache/rxd-ilspy').exists()
        result = json.loads(run('dotnet', 'get-method', sample, method))['detail']
        assert result['decompiled'] and result['representation'] == 'csharp', result
        assert 'await ' in result['code'], result
        sample = ROOT / 'tests/fixtures/dotnet/closure/Closure.dll'
        rows = [json.loads(line) for line in run('dotnet', 'outline', sample,
                '--type', 'Rxd.Closure.Root`1').splitlines()]
        type_id = next(row['id'] for row in rows if row['record'] == 'type')
        result = json.loads(run('dotnet', 'get-class', sample, type_id))['detail']
        assert result['decompiled'] and 'Root<T>' in result['code'], result

        # Both runtimes must come from the single executable, not the source tree.
        caches = list((Path(temporary) / 'cache/rxd-ilspy').iterdir())
        runtime = next(path for path in caches if (path / '.complete').is_file())
        native = json.loads((stage / 'ilspy-manifest.json').read_text(encoding='utf-8'))
        assert runtime.name == native['bundle_sha256']
        library = runtime / native['executable']
        assert (runtime / 'dotnet-refs/System.Runtime.dll').is_file()
        library.write_bytes(b'corrupt')
        run('--help')
        run('dotnet', 'get-class', sample, type_id, '--il')
        result = subprocess.run([binary, 'dotnet', 'get-class', sample, type_id],
                                env=env, cwd=temporary, capture_output=True, timeout=120)
        assert result.returncode != 0 and not result.stdout, result
        assert b'integrity verification' in result.stderr, result.stderr


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    parser.add_argument('--rid', required=True)
    parser.add_argument('--name', required=True)
    parser.add_argument('--ilspy-dir', type=Path, help='Prepared ILSpy bundle directory override')
    args = parser.parse_args()
    kuna_dir = ROOT / 'target/kuna' / args.target
    kuna = json.loads((kuna_dir / 'manifest.json').read_text(encoding='utf-8'))
    native_dir = (args.ilspy_dir or ROOT / 'target/dotnet-native' / args.rid).resolve()
    native = json.loads((native_dir / 'manifest.json').read_text(encoding='utf-8'))
    assert kuna['target'] == native['target'] == args.target
    output = ROOT / 'target/dist'
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='release stage ', dir=ROOT / 'target') as temporary:
        stage = Path(temporary) / ('rxd-' + args.name)
        stage.mkdir()
        binary_name = 'rxd.exe' if os.name == 'nt' else 'rxd'
        shutil.copy2(ROOT / 'target/release' / binary_name, stage / binary_name)
        shutil.copy2(native_dir / 'manifest.json', stage / 'ilspy-manifest.json')
        shutil.copy2(kuna_dir / 'manifest.json', stage / 'kuna-manifest.json')
        shutil.copy2(ROOT / 'README.md', stage / 'README.md')
        licenses = stage / 'licenses'
        shutil.copytree(ROOT / 'target/dotnet-native/licenses', licenses / 'dotnet')
        (licenses / 'kuna').mkdir(parents=True)
        for name in ('LICENSE', 'NOTICE'):
            shutil.copy2(ROOT / 'third_party/kuna' / name, licenses / 'kuna' / name)
        shutil.copy2(ROOT / 'crates/rxd-dotnet/native/NOTICE', licenses / 'ILSpy-NOTICE.txt')
        shutil.copy2(ROOT / 'crates/rxd-evm/EVMOLE-LICENSE', licenses / 'EVMole-LICENSE.txt')
        verify(stage, kuna)
        commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
        package_version = re.search(r'^version = "([^"]+)"$',
                                    (ROOT / 'Cargo.toml').read_text().split('[package]', 1)[1]
                                    .split('\n[', 1)[0], re.MULTILINE).group(1)
        metadata = {'rxd_commit': commit, 'rxd_version': package_version, 'target': args.target,
                    'features': ['bundled-kuna', 'bundled-ilspy', 'mcp'],
                    'files': [{'path': path.relative_to(stage).as_posix(),
                               'bytes': path.stat().st_size, 'sha256': digest(path)}
                              for path in sorted(stage.rglob('*')) if path.is_file()]}
        (stage / 'build-info.json').write_text(json.dumps(metadata, indent=2) + '\n', encoding='utf-8')
        if os.name == 'nt':
            archive = output / (stage.name + '.zip')
            with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as package:
                for path in sorted(stage.rglob('*')):
                    if path.is_file():
                        package.write(path, path.relative_to(stage.parent))
        else:
            archive = output / (stage.name + '.tar.gz')
            with tarfile.open(archive, 'w:gz', compresslevel=9) as package:
                package.add(stage, arcname=stage.name)
        checksum = digest(archive)
        archive.with_suffix(archive.suffix + '.sha256').write_text(
            checksum + '  ' + archive.name + '\n', encoding='utf-8')
        summary = (f'{archive.name}: {archive.stat().st_size / 1048576:.2f} MiB\n'
                   f'rxd: {(stage / binary_name).stat().st_size / 1048576:.2f} MiB; '
                   f'embedded ILSpy archive: {native["bundle_bytes"] / 1048576:.2f} MiB\n'
                   f'SHA-256: {checksum}\n')
        print(summary)
        if os.environ.get('GITHUB_STEP_SUMMARY'):
            with open(os.environ['GITHUB_STEP_SUMMARY'], 'a', encoding='utf-8') as stream:
                stream.write('```text\n' + summary + '```\n')


if __name__ == '__main__':
    main()
