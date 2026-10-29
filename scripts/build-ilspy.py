#!/usr/bin/env python3
"""Build pinned ILSpy engines: native desktop, Linux musl/Zig, or Android Bionic/NDK."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import xml.etree.ElementTree as ET

from runtime_bundle import write_bundle
from android_toolchain import VERSION as NDK_VERSION, prepare as prepare_android
from zig_toolchain import VERSION as ZIG_VERSION, prepare

ROOT = Path(__file__).resolve().parents[1]
BUILD = ROOT / 'target/dotnet-native'
SDK_VERSION = '10.0.401'
RUNTIME_VERSION = '10.0.12'
VERSION = '11.2.0.10050'
PACKAGE_SHA256 = 'e29000428db534f834bb074b795a178ef21dece652505f1d05d4310ca5f48b3b'
TARGETS = {
    'osx-arm64': ('aarch64-apple-darwin', 'librxd_dotnet_native.dylib', None),
    'linux-musl-x64': ('x86_64-unknown-linux-musl', 'librxd_dotnet_native.so', 'x86_64-linux-musl'),
    'linux-musl-arm64': ('aarch64-unknown-linux-musl', 'librxd_dotnet_native.so', 'aarch64-linux-musl'),
    'linux-bionic-arm64': ('aarch64-linux-android', 'librxd_dotnet_native.so', None),
    'win-x64': ('x86_64-pc-windows-msvc', 'rxd_dotnet_native.dll', None),
}


def digest(path):
    result = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            result.update(block)
    return result.hexdigest()


def capture(args, env):
    return subprocess.check_output(list(map(str, args)), env=env, text=True).strip()


def run(args, env):
    print('+ ' + shlex.join(map(str, args)), flush=True)
    subprocess.run(list(map(str, args)), env=env, check=True)


def executable(value, default):
    path = (shutil.which(value) or value) if value else shutil.which(default)
    if not path:
        raise RuntimeError('Install ' + default + ' or pass --' + default)
    return Path(path).resolve()


def verified_package(feed):
    name = 'icsharpcode.decompiler.' + VERSION + '.nupkg'
    package = feed / name
    if not package.exists():
        url = 'https://api.nuget.org/v3-flatcontainer/icsharpcode.decompiler/' + VERSION + '/' + name
        with tempfile.TemporaryDirectory(prefix='download-', dir=feed) as temporary:
            pending = Path(temporary) / name
            with urllib.request.urlopen(url, timeout=60) as response, pending.open('wb') as output:
                shutil.copyfileobj(response, output)
            if digest(pending) != PACKAGE_SHA256:
                raise RuntimeError('Downloaded ILSpy package failed SHA-256 verification')
            pending.replace(package)
    if digest(package) != PACKAGE_SHA256:
        raise RuntimeError('Cached ILSpy package failed SHA-256 verification')
    return name


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('rid', choices=TARGETS)
    parser.add_argument('--dotnet', help='Path to the pinned .NET SDK ' + SDK_VERSION)
    parser.add_argument('--zig', help='Path to Zig ' + ZIG_VERSION + ' (Linux musl targets)')
    parser.add_argument('--ndk', help='Android NDK ' + NDK_VERSION + ' directory (Bionic target)')
    args = parser.parse_args()
    target, filename, triple = TARGETS[args.rid]
    android = args.rid == 'linux-bionic-arm64'
    if args.zig and not triple:
        parser.error('--zig applies only to Linux musl targets')
    if args.ndk and not android:
        parser.error('--ndk applies only to Android Bionic')
    host = {'darwin': 'osx', 'linux': 'linux', 'win32': 'win'}.get(sys.platform)
    if triple or android:
        if host not in ('osx', 'linux'):
            parser.error('Linux/Android cross-builds require macOS or Linux')
    elif args.rid.split('-')[0] != host:
        parser.error('macOS and Windows NativeAOT engines require their native host OS')
    name = 'dotnet.exe' if os.name == 'nt' else 'dotnet'
    dotnet = args.dotnet
    if not dotnet and os.environ.get('DOTNET_ROOT'):
        dotnet = str(Path(os.environ['DOTNET_ROOT']) / name)
    if not dotnet:
        isolated = Path.home() / '.dotnet10' / name
        dotnet = str(isolated) if isolated.is_file() else shutil.which(name)
    dotnet = executable(dotnet, 'dotnet')
    output = BUILD / args.rid
    env = os.environ.copy()
    env.update({'DOTNET_ROOT': str(dotnet.parent), 'DOTNET_CLI_TELEMETRY_OPTOUT': '1',
                'DOTNET_CLI_HOME': str(BUILD / 'sdk-home'),
                'DOTNET_GENERATE_ASPNET_CERTIFICATE': 'false',
                'DOTNET_PROCESSOR_COUNT': '2', 'NUGET_PACKAGES': str(BUILD / 'packages'),
                'ZIG_GLOBAL_CACHE_DIR': str(BUILD / 'zig-cache')})
    sdk = capture([dotnet, '--version'], env)
    if sdk != SDK_VERSION:
        parser.error('The producer requires .NET SDK ' + SDK_VERSION + '; found ' + sdk)
    android_tools = prepare_android(args.ndk) if android else None
    zig = None
    if triple:
        tools = prepare(target, args.zig)
        zig = tools['zig']
        version = tools['version']
    output.mkdir(parents=True, exist_ok=True)
    options = ['-p:RuntimeIdentifier=' + args.rid,
               '-p:PublishAotUsingRuntimePack=true', '-p:SelfContained=true',
               '-p:_IsPublishing=true', '-p:IlcSingleThreaded=true',
               '-p:UseSharedCompilation=false']
    linker = {'kind': 'native', 'host': host}
    if zig:
        wrapper = tools['cc']
        options += ['-p:DisableUnsupportedError=true', '-p:StripSymbols=false',
                    '-p:NativeDebugSymbols=false', '-p:LinkerFlavor=lld',
                    '-p:CppCompilerAndLinker=' + str(wrapper)]
        linker = {'kind': 'zig', 'host': host, 'version': version, 'target': triple,
                  'wrapper_sha256': digest(wrapper), 'strip': 'linker --strip-debug'}

    if android:
        options += ['-p:DisableUnsupportedError=true', '-p:StripSymbols=false',
                    '-p:NativeDebugSymbols=false', '-p:LinkerFlavor=lld',
                    '-p:CppCompilerAndLinker=' + str(android_tools['cc'])]
        linker = {'kind': 'ndk', 'host': host, 'version': android_tools['version'],
                  'api': android_tools['api'], 'target': target,
                  'compiler_sha256': digest(android_tools['cc']), 'strip': 'llvm-strip --strip-unneeded'}

    feed = BUILD / 'feed'
    feed.mkdir(parents=True, exist_ok=True)
    package_name = verified_package(feed)
    project = ROOT / 'crates/rxd-dotnet/native/rxd-dotnet-native.csproj'
    source_files = [project, project.with_name('NOTICE'), Path(__file__).resolve(),
                    ROOT / 'scripts/runtime_bundle.py', ROOT / 'scripts/zig_toolchain.py',
                    ROOT / 'scripts/android_toolchain.py',
                    *sorted(project.parent.rglob('*.cs'))]
    source_inputs = {path.relative_to(ROOT).as_posix(): digest(path) for path in source_files}
    configuration = ET.Element('configuration')
    sources = ET.SubElement(configuration, 'packageSources')
    ET.SubElement(sources, 'clear')
    ET.SubElement(sources, 'add', key='verified-ilspy', value=str(feed))
    ET.SubElement(sources, 'add', key='nuget.org',
                  value='https://api.nuget.org/v3/index.json', protocolVersion='3')
    config = output / 'NuGet.Config'
    ET.ElementTree(configuration).write(config, encoding='utf-8', xml_declaration=True)
    run([dotnet, 'restore', project, '-r', args.rid, '--configfile', config, *options], env)
    restored = BUILD / 'packages/icsharpcode.decompiler' / VERSION / package_name
    if digest(restored) != PACKAGE_SHA256:
        raise RuntimeError('Restored ILSpy package failed SHA-256 verification')
    run([dotnet, 'publish', project, '--no-restore', '-c', 'Release',
         '-r', args.rid, '-m:1', *options], env)
    library = output / filename
    if android:
        run([android_tools['strip'], '--strip-unneeded', library], env)
    if any(digest(ROOT / path) != expected for path, expected in source_inputs.items()):
        raise RuntimeError('Native sources changed during the build; refusing to package')
    reference_source = dotnet.parent / 'packs/Microsoft.NETCore.App.Ref' / RUNTIME_VERSION / 'ref/net10.0'
    if not reference_source.is_dir():
        raise RuntimeError('The SDK reference assembly pack ' + RUNTIME_VERSION + ' is missing')
    reference_output = output / 'dotnet-refs'
    if reference_output.exists():
        shutil.rmtree(reference_output)
    shutil.copytree(reference_source, reference_output, ignore=shutil.ignore_patterns('*.xml'))
    metadata = {'reference_pack': RUNTIME_VERSION, 'runtime': RUNTIME_VERSION,
                'rid': args.rid, 'target': target, 'sdk': sdk, 'linker': linker,
                'ilspy_version': VERSION, 'package_sha256': PACKAGE_SHA256,
                'library': library.name, 'library_bytes': library.stat().st_size,
                'library_sha256': digest(library), 'source_inputs': source_inputs}
    (output / 'build-info.json').write_text(json.dumps(metadata, indent=2) + '\n', encoding='utf-8')
    notices = BUILD / 'licenses'
    notices.mkdir(exist_ok=True)
    for name in ('LICENSE.txt', 'ThirdPartyNotices.txt'):
        shutil.copy2(dotnet.parent / name, notices / name)
    with tempfile.TemporaryDirectory(prefix='stage-', dir=output) as temporary:
        stage = Path(temporary)
        shutil.copy2(library, stage / library.name)
        shutil.copytree(reference_output, stage / 'dotnet-refs')
        shutil.copytree(notices, stage / 'licenses/dotnet')
        shutil.copy2(ROOT / 'crates/rxd-dotnet/native/NOTICE', stage / 'licenses/ILSpy-NOTICE.txt')
        bundled = write_bundle(stage, output,
                               {'target': target, 'version': VERSION,
                                'executable': library.name, 'native_build': metadata})
    print(json.dumps(dict(metadata, compressed_bytes=bundled['bundle_bytes']), indent=2))


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError, OSError) as error:
        sys.exit(str(error))
