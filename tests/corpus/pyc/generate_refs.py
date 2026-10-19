#!/usr/bin/env python3
"""Compile-only explicit-name opcode diagnosis using pinned CPython 3.12.14.

The saved oracle comes from CPython dis/CodeType and the independent marshal
scanner in generate.py. Never import, instantiate or execute the analyzed code.
"""
import argparse
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

SOURCE = '''module_name = 1
copy = module_name
del module_name
雪 = 1
copied_unicode = 雪
del 雪
import os.path as path
from os import path as imported
global_name = 1

def globals_and_attributes(obj):
    global global_name
    global_name = obj.attribute
    obj.attribute = global_name
    del obj.attribute
    del global_name

def locals_and_check(flag):
    if flag:
        local_name = 1
    saved = local_name
    del local_name
    return saved

def enclosing(arg):
    closed_name = arg
    def nested():
        nonlocal closed_name
        closed_name = arg
        saved = closed_name
        del closed_name
        return saved
    return nested

def comprehension(seq):
    return [local_name for local_name in seq]

class Base:
    def method(self):
        return 1

class Child(Base):
    def method(self):
        return super().method()

def generic_outer():
    captured = 1
    class Generic[T]:
        type Alias = tuple[T, captured, global_name]
    return Generic
'''

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    assert tuple(sys.version_info[:3]) == (3, 12, 14)
    assert sys.implementation.name == 'cpython' and sys.version_info.releaselevel == 'final'
    helper = Path(__file__).with_name('generate.py')
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location('trusted_corpus_preparation', helper)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    args.output.mkdir(parents=True, exist_ok=True)
    row = module.prepare(SOURCE.encode(), 'refs', args.output)
    sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
    manifest = {'interpreter': sys.executable, 'binary_sha256': sha(Path(sys.executable)),
                'version_info': list(sys.version_info), 'compile_only': True,
                'analyzed_modules_imported_or_executed': False,
                'generator_sha256': sha(Path(__file__)), 'helper_sha256': sha(helper),
                'input': row,
                'artifacts': {p.name: {'bytes': p.stat().st_size, 'sha256': sha(p)}
                              for p in args.output.glob('refs.*') if p.name != 'refs.manifest.json'}}
    (args.output/'refs.manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True)+'\n')

if __name__ == '__main__':
    main()
