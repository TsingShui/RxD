"""Pinned Bionic/NDK tools shared by the Android NativeAOT and Rust producers."""
import os
from pathlib import Path
import shlex
import sys

VERSION = '28.2.13676358'
API = 24
TARGET = 'aarch64-linux-android'
ROOT = Path(__file__).resolve().parents[1]


def prepare(ndk=None):
    host = {'darwin': 'darwin-x86_64', 'linux': 'linux-x86_64'}.get(sys.platform)
    if host is None:
        raise RuntimeError('Android cross-builds require macOS or Linux')
    ndk = ndk or os.environ.get('ANDROID_NDK_HOME') or os.environ.get('ANDROID_NDK_ROOT')
    if not ndk:
        sdk = os.environ.get('ANDROID_HOME') or os.environ.get('ANDROID_SDK_ROOT')
        sdk = Path(sdk) if sdk else Path.home() / (
            'Library/Android/sdk' if sys.platform == 'darwin' else 'Android/Sdk')
        ndk = sdk / 'ndk' / VERSION
    ndk = Path(ndk).expanduser().resolve()
    properties = ndk / 'source.properties'
    if not properties.is_file():
        raise RuntimeError('Install Android NDK ' + VERSION + ' or pass --ndk')
    revision = dict(line.split('=', 1) for line in properties.read_text().splitlines() if '=' in line)
    revision = {key.strip(): value.strip() for key, value in revision.items()}
    if revision.get('Pkg.Revision') != VERSION:
        raise RuntimeError('The Android producer requires NDK ' + VERSION)
    directory = ndk / 'toolchains/llvm/prebuilt' / host / 'bin'
    compiler = TARGET + str(API)
    names = {'cc': compiler + '-clang', 'cxx': compiler + '-clang++',
             'ar': 'llvm-ar', 'ranlib': 'llvm-ranlib', 'strip': 'llvm-strip'}
    tools = {name: directory / filename for name, filename in names.items()}
    for path in tools.values():
        if not path.is_file() or not os.access(path, os.X_OK):
            raise RuntimeError('Missing Android NDK tool: ' + str(path))
    return dict(tools, ndk=ndk, version=VERSION, api=API)


def cargo_environment(tools, env):
    key = TARGET.replace('-', '_')
    result = {'CARGO_BUILD_TARGET': TARGET, 'LIBZ_SYS_STATIC': '1',
              'CARGO_TARGET_' + key.upper() + '_LINKER': str(tools['cc'])}
    for name in ('cc', 'cxx', 'ar', 'ranlib'):
        variable = 'CXX' if name == 'cxx' else name.upper()
        result[variable + '_' + key] = str(tools[name])
    flags = ['-C', 'link-arg=-Wl,-z,max-page-size=16384',
             '-C', 'link-arg=-Wl,-z,common-page-size=16384']
    if env.get('CARGO_ENCODED_RUSTFLAGS'):
        result['CARGO_ENCODED_RUSTFLAGS'] = env['CARGO_ENCODED_RUSTFLAGS'] + '\x1f' + '\x1f'.join(flags)
    else:
        result['RUSTFLAGS'] = shlex.join([*shlex.split(env.get('RUSTFLAGS', '')), *flags])
    return result
