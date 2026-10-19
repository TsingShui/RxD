#!/usr/bin/env python3
"""Build the pinned ILSpy NativeAOT shared library for the current OS."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import xml.etree.ElementTree as ET

from runtime_bundle import write_bundle

ROOT = Path(__file__).resolve().parents[1]
BUILD = ROOT / 'target/dotnet-native'
VERSION = '11.1.0.9782'
PACKAGE_SHA256 = '1f76df4d35193ba4eeb50ffcd84eae381a2c48c67f95c1104dc406759448a811'
LIBRARIES = {
    'osx-arm64': 'librxd_dotnet_native.dylib',
    'linux-x64': 'librxd_dotnet_native.so',
    'linux-arm64': 'librxd_dotnet_native.so',
    'win-x64': 'rxd_dotnet_native.dll',
}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('rid', choices=LIBRARIES)
    parser.add_argument('--dotnet', help='Path to the .NET 10 SDK executable')
    args = parser.parse_args()
    host = {'darwin': 'osx', 'linux': 'linux', 'win32': 'win'}.get(sys.platform)
    if args.rid.split('-')[0] != host:
        parser.error('NativeAOT must be built on the target OS')
    executable = 'dotnet.exe' if os.name == 'nt' else 'dotnet'
    dotnet = args.dotnet
    if not dotnet and os.environ.get('DOTNET_ROOT'):
        dotnet = str(Path(os.environ['DOTNET_ROOT']) / executable)
    if not dotnet:
        isolated = Path.home() / '.dotnet10' / executable
        dotnet = str(isolated) if isolated.is_file() else shutil.which(executable)
    if not dotnet:
        parser.error('Install the .NET 10 SDK or pass --dotnet')
    dotnet = Path(dotnet).resolve()
    env = os.environ.copy()
    env.update({'DOTNET_ROOT': str(dotnet.parent), 'DOTNET_CLI_TELEMETRY_OPTOUT': '1',
                'DOTNET_CLI_HOME': str(BUILD / 'sdk-home'),
                'NUGET_PACKAGES': str(BUILD / 'packages')})
    sdk = subprocess.check_output([dotnet, '--version'], env=env, text=True).strip()
    if not sdk.startswith('10.'):
        parser.error('The native project requires the .NET 10 SDK; found ' + sdk)

    feed = BUILD / 'feed'
    feed.mkdir(parents=True, exist_ok=True)
    package_name = 'icsharpcode.decompiler.' + VERSION + '.nupkg'
    package = feed / package_name
    if not package.exists():
        url = 'https://api.nuget.org/v3-flatcontainer/icsharpcode.decompiler/' + VERSION + '/' + package_name
        pending = package.with_suffix('.tmp')
        with urllib.request.urlopen(url, timeout=60) as response, pending.open('wb') as output:
            shutil.copyfileobj(response, output)
        if digest(pending) != PACKAGE_SHA256:
            pending.unlink()
            raise RuntimeError('Downloaded ILSpy package failed SHA-256 verification')
        pending.replace(package)
    if digest(package) != PACKAGE_SHA256:
        raise RuntimeError('Cached ILSpy package failed SHA-256 verification')
    project = ROOT / 'crates/rxd-dotnet/native/rxd-dotnet-native.csproj'
    source_files = [project, project.with_name('NOTICE'), *sorted(project.parent.rglob('*.cs'))]
    source_inputs = {path.relative_to(ROOT).as_posix(): digest(path) for path in source_files}
    # NuGet's Windows command-line source normalization can treat a URL as a
    # filesystem path. An explicit configuration preserves feed URI semantics.
    configuration = ET.Element('configuration')
    sources = ET.SubElement(configuration, 'packageSources')
    ET.SubElement(sources, 'clear')
    ET.SubElement(sources, 'add', key='verified-ilspy', value=str(feed))
    ET.SubElement(sources, 'add', key='nuget.org',
                  value='https://api.nuget.org/v3/index.json', protocolVersion='3')
    config = BUILD / 'NuGet.Config'
    ET.ElementTree(configuration).write(config, encoding='utf-8', xml_declaration=True)
    subprocess.run([dotnet, 'restore', project, '-r', args.rid, '--configfile', config],
                   env=env, check=True)
    restored = BUILD / 'packages/icsharpcode.decompiler' / VERSION / package_name
    if digest(restored) != PACKAGE_SHA256:
        raise RuntimeError('Restored ILSpy package failed SHA-256 verification')
    subprocess.run([dotnet, 'publish', project, '--no-restore', '-c', 'Release',
                    '-r', args.rid, '-m:1'], env=env, check=True)
    if any(digest(ROOT / path) != expected for path, expected in source_inputs.items()):
        raise RuntimeError('Native sources changed during the build; refusing to package')
    library = BUILD / args.rid / LIBRARIES[args.rid]
    references = sorted((dotnet.parent / 'packs/Microsoft.NETCore.App.Ref').glob('10.*/ref/net10.0'),
                        key=lambda path: tuple(int(n) for n in path.parents[1].name.split('.')))
    if not references:
        raise RuntimeError('The .NET 10 SDK reference assembly pack is missing')
    reference_source = references[-1]
    reference_output = library.parent / 'dotnet-refs'
    if reference_output.exists():
        shutil.rmtree(reference_output)
    reference_output.mkdir()
    for reference in reference_source.glob('*.dll'):
        shutil.copy2(reference, reference_output / reference.name)
    metadata = {'reference_pack': reference_source.parents[1].name,
                'rid': args.rid, 'sdk': sdk, 'ilspy_version': VERSION,
                'package_sha256': PACKAGE_SHA256, 'library': library.name,
                'library_bytes': library.stat().st_size, 'library_sha256': digest(library),
                'source_inputs': source_inputs}
    (library.parent / 'build-info.json').write_text(json.dumps(metadata, indent=2) + '\n', encoding='utf-8')
    notices = BUILD / 'licenses'
    notices.mkdir(exist_ok=True)
    for name in ('LICENSE.txt', 'ThirdPartyNotices.txt'):
        shutil.copy2(dotnet.parent / name, notices / name)
    targets = {'osx-arm64': 'aarch64-apple-darwin',
               'linux-x64': 'x86_64-unknown-linux-gnu', 'linux-arm64': 'aarch64-unknown-linux-gnu',
               'win-x64': 'x86_64-pc-windows-msvc'}
    with tempfile.TemporaryDirectory(prefix='stage-', dir=library.parent) as temporary:
        stage = Path(temporary)
        shutil.copy2(library, stage / library.name)
        shutil.copytree(reference_output, stage / 'dotnet-refs')
        shutil.copytree(notices, stage / 'licenses/dotnet')
        shutil.copy2(ROOT / 'crates/rxd-dotnet/native/NOTICE', stage / 'licenses/ILSpy-NOTICE.txt')
        bundled = write_bundle(stage, library.parent,
                               {'target': targets[args.rid], 'version': VERSION,
                                'executable': library.name, 'native_build': metadata})
    print(json.dumps(dict(metadata, compressed_bytes=bundled['bundle_bytes']), indent=2))


if __name__ == '__main__':
    main()
