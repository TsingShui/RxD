#!/usr/bin/env python3
"""Offline process E2E checks for the release asset publication gate (authored fixtures)."""
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
# Cargo is the independent source of truth for the package under test.
PACKAGE = (ROOT / 'Cargo.toml').read_text().split('[package]', 1)[1].split('\n[', 1)[0]
VERSION = next(line.split('=')[1].strip().strip('"')
               for line in PACKAGE.splitlines() if line.startswith('version ='))
PLATFORMS = {
    'linux-x86_64': ('x86_64-unknown-linux-gnu', 'tar.gz'),
    'linux-arm64': ('aarch64-unknown-linux-gnu', 'tar.gz'),
    'macos-arm64': ('aarch64-apple-darwin', 'tar.gz'),
    'windows-x86_64': ('x86_64-pc-windows-msvc', 'zip'),
}


def fixture(directory):
    installer = directory / 'install.sh'
    installer.write_bytes(b'authored installer fixture')
    (directory / 'install.sh.sha256').write_text(
        hashlib.sha256(installer.read_bytes()).hexdigest() + '  install.sh\n')
    for name, (target, extension) in PLATFORMS.items():
        # Deliberately authored data, never executed or presented as a real build.
        files = {'rxd.exe' if extension == 'zip' else 'rxd': b'authored binary fixture',
                 'kuna-manifest.json': json.dumps({'target': target}).encode(),
                 'ilspy-manifest.json': json.dumps({'target': target}).encode(),
                 'licenses/NOTICE': b'authored notice'}
        files['build-info.json'] = json.dumps({
            'rxd_version': VERSION, 'rxd_commit': 'abc123', 'target': target,
            'features': ['bundled-kuna', 'bundled-ilspy', 'mcp'],
            'files': [{'path': path, 'bytes': len(data),
                       'sha256': hashlib.sha256(data).hexdigest()}
                      for path, data in files.items()],
        }).encode()
        archive = directory / f'rxd-{name}.{extension}'
        if extension == 'zip':
            with zipfile.ZipFile(archive, 'w') as package:
                for path, data in files.items():
                    package.writestr(f'rxd-{name}/{path}', data)
        else:
            with tarfile.open(archive, 'w:gz') as package:
                for path, data in files.items():
                    info = tarfile.TarInfo(f'rxd-{name}/{path}')
                    info.size = len(data)
                    package.addfile(info, io.BytesIO(data))
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        Path(str(archive) + '.sha256').write_text(f'{checksum}  {archive.name}\n')


def run(directory, success, *extra):
    result = subprocess.run([sys.executable, ROOT / 'scripts/verify-release.py',
                             '--tag', 'v' + VERSION, '--commit', 'abc123', '--assets', directory,
                             *extra], capture_output=True, text=True)
    assert (result.returncode == 0) == success, result
    return result


def main():
    with tempfile.TemporaryDirectory(prefix='release-test-') as temporary:
        directory = Path(temporary)
        fixture(directory)
        result = run(directory, True)
        assert '4 platforms' in result.stdout, result
        run(directory, False, '--tag', 'v999999.0.0')
        archive = directory / 'rxd-linux-arm64.tar.gz'
        archive.write_bytes(b'corrupted archive')
        result = run(directory, False)
        assert 'checksum' in result.stderr, result
        fixture(directory)
        archive.unlink()
        result = run(directory, False)
        assert 'missing' in result.stderr, result
    print('Release process E2E: complete set accepted; version, corrupt and missing assets rejected')


if __name__ == '__main__':
    main()
