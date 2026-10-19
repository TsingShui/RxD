#!/usr/bin/env python3
"""Gate a complete Release asset set, optionally downloaded anonymously from GitHub."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
PLATFORMS = {
    'linux-x86_64': ('x86_64-unknown-linux-gnu', 'tar.gz'),
    'linux-arm64': ('aarch64-unknown-linux-gnu', 'tar.gz'),
    'macos-arm64': ('aarch64-apple-darwin', 'tar.gz'),
    'windows-x86_64': ('x86_64-pc-windows-msvc', 'zip'),
}


def digest(path):
    with path.open('rb') as source:
        checksum = hashlib.sha256()
        for block in iter(lambda: source.read(1024 * 1024), b''):
            checksum.update(block)
        return checksum.hexdigest()


def version(tag):
    package = (ROOT / 'Cargo.toml').read_text().split('[package]', 1)[1].split('\n[', 1)[0]
    expected = re.search(r'^version = "([^"]+)"$', package, re.MULTILINE).group(1)
    if not re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+', tag) or tag != 'v' + expected:
        raise ValueError(f'tag {tag!r} must match Cargo package version v{expected}')
    return expected


def inspect(archive):
    files = {}
    if archive.suffix == '.zip':
        with zipfile.ZipFile(archive) as package:
            for entry in package.infolist():
                if entry.is_dir():
                    continue
                mode = entry.external_attr >> 16
                if mode & 0o170000 == 0o120000:
                    raise ValueError(f'archive link: {entry.filename}')
                if entry.filename in files:
                    raise ValueError('duplicate archive entry')
                files[entry.filename] = package.read(entry)
    else:
        with tarfile.open(archive, 'r:gz') as package:
            for entry in package:
                if entry.isdir():
                    continue
                if not entry.isfile() or entry.name in files:
                    raise ValueError(f'unsupported/duplicate archive entry: {entry.name}')
                files[entry.name] = package.extractfile(entry).read()
    for name in files:
        path = PurePosixPath(name)
        if path.is_absolute() or '..' in path.parts or '\\' in name:
            raise ValueError(f'unsafe archive entry: {name}')
    return files


def verify(assets, tag, commit):
    expected_version = version(tag)
    expected_assets = {'install.sh', 'install.sh.sha256'}
    installer = assets / 'install.sh'
    checksum = assets / 'install.sh.sha256'
    if not installer.is_file() or not checksum.is_file():
        raise ValueError('missing installer or checksum')
    if checksum.read_text(encoding='ascii') != f'{digest(installer)}  install.sh\n':
        raise ValueError('installer checksum mismatch')
    for name, (target, extension) in PLATFORMS.items():
        archive = assets / f'rxd-{name}.{extension}'
        checksum = Path(str(archive) + '.sha256')
        expected_assets.update((archive.name, checksum.name))
        if not archive.is_file() or not checksum.is_file():
            raise ValueError(f'missing archive or checksum: {archive.name}')
        expected_checksum = f'{digest(archive)}  {archive.name}\n'
        if checksum.read_text(encoding='ascii') != expected_checksum:
            raise ValueError(f'checksum mismatch: {archive.name}')
        archived = inspect(archive)
        prefix = f'rxd-{name}/'
        if any(not path.startswith(prefix) for path in archived):
            raise ValueError(f'wrong package root: {archive.name}')
        files = {path[len(prefix):]: data for path, data in archived.items()}
        metadata = json.loads(files['build-info.json'])
        if (metadata['rxd_version'] != expected_version or metadata['target'] != target
                or metadata['rxd_commit'] != commit
                or set(metadata['features']) != {'bundled-kuna', 'bundled-ilspy', 'mcp'}):
            raise ValueError(f'version, target, commit or feature mismatch: {archive.name}')
        listed = set()
        for entry in metadata['files']:
            path = entry['path']
            data = files[path]
            if (path in listed or len(data) != entry['bytes']
                    or hashlib.sha256(data).hexdigest() != entry['sha256']):
                raise ValueError(f'build manifest mismatch: {archive.name}: {path}')
            listed.add(path)
        if listed != set(files) - {'build-info.json'}:
            raise ValueError(f'unmanifested package contents: {archive.name}')
        binary = 'rxd.exe' if extension == 'zip' else 'rxd'
        if not files.get(binary) or not any(path.startswith('licenses/') for path in files):
            raise ValueError(f'missing executable or notices: {archive.name}')
        for manifest in ('ilspy-manifest.json', 'kuna-manifest.json'):
            if json.loads(files[manifest])['target'] != target:
                raise ValueError(f'engine target mismatch: {archive.name}')
        print(f'{tag} {name} SHA-256 {digest(archive)} verified')
    if {path.name for path in assets.iterdir()} != expected_assets:
        raise ValueError('unexpected release assets; expected four archives, installer and their five checksums')
    print(f'{tag}: 4 platforms, checksums and build identities verified')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--repo', default='TsingShui/RxD', help='Public distribution owner/repository')
    parser.add_argument('--commit')
    parser.add_argument('--check-version', action='store_true', help='Only validate tag and print package version')
    parser.add_argument('--assets', type=Path, help='Local complete asset directory')
    parser.add_argument('--public', action='store_true', help='Download all assets without credentials')
    args = parser.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', args.repo):
        parser.error('--repo must be owner/repository')
    expected = version(args.tag)
    if args.check_version:
        print(expected)
        return
    if not args.commit:
        parser.error('--commit is required for asset verification')
    if args.public == bool(args.assets):
        parser.error('choose exactly one of --assets or --public')
    if args.public:
        with tempfile.TemporaryDirectory(prefix='rxd-public-release-') as temporary:
            assets = Path(temporary)
            base = f'https://github.com/{args.repo}/releases/download/{args.tag}/'
            filenames = ['install.sh', 'install.sh.sha256']
            filenames += [f'rxd-{name}.{extension}{suffix}'
                          for name, (_, extension) in PLATFORMS.items()
                          for suffix in ('', '.sha256')]
            for filename in filenames:
                subprocess.run(['curl', '--fail', '--silent', '--show-error', '--location',
                                '--proto', '=https', '--proto-redir', '=https',
                                '--retry', '3', '--max-time', '600',
                                '--output', str(assets / filename), base + filename], check=True)
            verify(assets, args.tag, args.commit)
    else:
        verify(args.assets, args.tag, args.commit)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError, tarfile.TarError, zipfile.BadZipFile,
            subprocess.CalledProcessError) as error:
        sys.exit(f'release verification failed: {error}')
