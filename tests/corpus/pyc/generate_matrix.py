#!/usr/bin/env python3
"""Compile cross-version probes with exact final CPython runtimes; never call analyzed code."""
import argparse
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

SOURCE = '''global_name = 1
module_name = global_name
del module_name
import os.path as path
from os import path as imported

def enclosing(arg):
    cell = arg
    class Captured:
        saved = cell
        def read(self):
            return cell
    def nested(value):
        nonlocal cell
        cell = value
        saved = cell
        del cell
        return arg, saved
    return Captured, nested

def names(obj, arg):
    global global_name
    global_name = obj.attribute
    obj.attribute = arg
    del obj.attribute
    del global_name
    return obj.method(arg)

def locals_and_check(flag):
    if flag:
        local_name = 1
    saved = local_name
    del local_name
    return saved

def packed(a, b, pair):
    first, second = pair
    saved = a
    return b, saved, first, second

def protected(resource):
    with resource:
        return "with-marker"

def comprehension(seq, cell):
    def read():
        return cell
    result = [cell for cell in seq]
    return result, read

def free_comprehension(cell):
    def nested(seq):
        result = [cell for cell in seq]
        return result, cell
    return nested
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    assert tuple(sys.version_info[:3]) in [(3, 8, 20), (3, 9, 6), (3, 10, 16), (3, 11, 11), (3, 12, 14), (3, 13, 2)]
    assert sys.implementation.name == 'cpython' and sys.version_info.releaselevel == 'final'
    helper = Path(__file__).with_name('generate.py')
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location('trusted_matrix_preparation', helper)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    version = '.'.join(str(v) for v in sys.version_info[:3])
    output = args.output/version
    output.mkdir(parents=True, exist_ok=True)
    row = module.prepare(SOURCE.encode(), 'semantics', output)
    sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
    manifest = {'interpreter': sys.executable, 'binary_sha256': sha(Path(sys.executable)),
                'version_info': list(sys.version_info), 'compile_only': True,
                'analyzed_modules_imported_or_executed': False,
                'generator_sha256': sha(Path(__file__)), 'helper_sha256': sha(helper),
                'input': row,
                'artifacts': {p.name: {'bytes': p.stat().st_size, 'sha256': sha(p)}
                              for p in output.glob('semantics.*') if p.name != 'semantics.manifest.json'}}
    (output/'semantics.manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True)+'\n')

if __name__ == '__main__':
    main()
