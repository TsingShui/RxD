"""Pinned musl compiler wrappers shared by NativeAOT and Cargo producers."""
import argparse
import os
from pathlib import Path
import shlex
import shutil
import subprocess

VERSION = '0.17.0'
CARGO_ZIGBUILD_VERSION = '0.23.4'
TARGETS = {'aarch64-unknown-linux-musl': 'aarch64-linux-musl',
           'x86_64-unknown-linux-musl': 'x86_64-linux-musl'}
ROOT = Path(__file__).resolve().parents[1]


def prepare(target, zig=None):
    triple = TARGETS[target]
    found = (shutil.which(zig) or zig) if zig else shutil.which('zig')
    if not found:
        raise RuntimeError('Install Zig ' + VERSION + ' or pass --zig')
    zig = Path(found).resolve()
    version = subprocess.check_output([zig, 'version'], text=True).strip()
    if version != VERSION:
        raise RuntimeError('The producer requires Zig ' + VERSION + '; found ' + version)
    output = ROOT / 'target/zig-tools' / target
    output.mkdir(parents=True, exist_ok=True)
    commands = {'cc': ['cc', '-target', triple, '-fuse-ld=lld'],
                'cxx': ['c++', '-target', triple, '-fuse-ld=lld'],
                'ar': ['ar'], 'ranlib': ['ar', 's']}
    tools = {'zig': zig, 'triple': triple, 'version': version}
    for name, arguments in commands.items():
        path = output / ('zig-' + name)
        value = '#!/usr/bin/env python3\nimport os, sys\n'
        value += 'for key in ("SDKROOT", "MACOSX_DEPLOYMENT_TARGET", "CPATH", "LIBRARY_PATH"):\n    os.environ.pop(key, None)\n'
        value += 'args = [arg.replace(' + repr('--target=' + target) + ', ' + repr('--target=' + triple) + ') for arg in sys.argv[1:]]\n'
        value += 'args = [' + repr(triple) + ' if arg == ' + repr(target) + ' else arg for arg in args]\n'
        value += 'os.execv(' + repr(str(zig)) + ', ' + repr([str(zig), *arguments]) + ' + args)\n'
        if not path.exists() or path.read_text() != value:
            path.write_text(value, encoding='utf-8')
        path.chmod(0o755)
        tools[name] = path
    return tools


def cargo_environment(target, tools, env):
    key = target.replace('-', '_')
    cargo_zig = shutil.which('cargo-zigbuild')
    if not cargo_zig:
        raise RuntimeError('Install cargo-zigbuild ' + CARGO_ZIGBUILD_VERSION)
    version = subprocess.check_output([cargo_zig, '--version'], text=True).strip()
    if version != 'cargo-zigbuild ' + CARGO_ZIGBUILD_VERSION:
        raise RuntimeError('The Rust producer requires cargo-zigbuild ' + CARGO_ZIGBUILD_VERSION)
    linker = tools['cc'].parent / 'cargo-zig-cc'
    value = '#!/usr/bin/env python3\nimport os, sys\n'
    value += 'os.environ["CARGO_ZIGBUILD_ZIG_PATH"] = ' + repr(str(tools['zig'])) + '\n'
    value += 'os.execv(' + repr(cargo_zig) + ', ' + repr([cargo_zig, 'zig', 'cc', '--', '-target', tools['triple']]) + ' + sys.argv[1:])\n'
    if not linker.exists() or linker.read_text() != value:
        linker.write_text(value, encoding='utf-8')
    linker.chmod(0o755)
    result = {'CARGO_BUILD_TARGET': target, 'LIBZ_SYS_STATIC': '1',
              'CARGO_TARGET_' + key.upper() + '_LINKER': str(linker),
              'CARGO_ZIGBUILD_CACHE_DIR': str(ROOT / 'target/cargo-zig-cache'),
              'ZIG_GLOBAL_CACHE_DIR': str(ROOT / 'target/zig-cache')}
    for name in ('cc', 'cxx', 'ar', 'ranlib'):
        variable = 'CXX' if name == 'cxx' else name.upper()
        result[variable + '_' + key] = str(tools[name])
    flags = ['-C', 'target-feature=-crt-static', '-C', 'link-arg=-dynamic']
    if env.get('CARGO_ENCODED_RUSTFLAGS'):
        result['CARGO_ENCODED_RUSTFLAGS'] = env['CARGO_ENCODED_RUSTFLAGS'] + '\x1f' + '\x1f'.join(flags)
    else:
        result['RUSTFLAGS'] = shlex.join([*shlex.split(env.get('RUSTFLAGS', '')), *flags])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True, choices=TARGETS)
    parser.add_argument('--zig')
    parser.add_argument('--github-env', type=Path, required=True)
    args = parser.parse_args()
    values = cargo_environment(args.target, prepare(args.target, args.zig), os.environ)
    with args.github_env.open('a', encoding='utf-8') as output:
        for name, value in values.items():
            if '\n' in value or '\r' in value:
                raise RuntimeError('Toolchain environment must be single-line')
            output.write(name + '=' + value + '\n')


if __name__ == '__main__':
    main()
