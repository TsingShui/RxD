#!/usr/bin/env python3
"""Trusted preparation helper. Compile source and inspect code; never execute it.

Run with each pinned CPython, -I. Writes only the requested preparation directory.
No dependency on rxd, pycdc, or their generated metadata.
"""
import argparse
import dis
import hashlib
import importlib.util
import json
import marshal
import opcode
import struct
import sys
import sysconfig
import types
from pathlib import Path

SOURCE = r'''"""Pinned synthetic diagnosis fixture; never imported or executed."""
marker = "closure-marker"
raw = b"\xff\x00raw"
surrogate = "\ud800"
unicode_text = "雪☃"
transform = lambda value: ("lambda-marker", value)

def same(x):
    return len(x) + 1

def outer(arg, /, named=1, *, kw=2):
    cell = "shared-marker"
    def same(value):
        nonlocal cell
        cell = value
        return arg, cell, kw
    return same

class Base:
    def method(self):
        return "base-marker"

class Derived(Base):
    def method(self):
        return super().method()

def names(obj, arg):
    global marker
    marker = "changed-marker"
    obj.attr = arg
    return obj.method(arg), marker

def branch(a, b):
    c = a
    d = b
    if c < d:
        return c + d
    return c - d

def protected(value):
    try:
        return value[0]
    except (IndexError, KeyError) as error:
        return str(error)
    finally:
        value.clear()

def loops(seq):
    total = 0
    for value in seq:
        if value:
            total += value
    return ["item-marker" for value in seq], total

def gen(seq):
    yield from seq

async def coro(value):
    return await value
'''


def sha(data):
    return hashlib.sha256(data).hexdigest()


def encode(value, code_index=None):
    if isinstance(value, types.CodeType):
        return {'type': 'code', 'index': code_index[id(value)] if code_index is not None else None, 'name': value.co_name}
    if isinstance(value, str):
        raw = value.encode('utf-8', 'surrogatepass')
        try:
            raw.decode('utf-8')
            text = value
        except UnicodeDecodeError:
            text = None
        return {'type': 'str', 'value': text, 'utf8_surrogatepass_hex': raw.hex()}
    if isinstance(value, bytes):
        return {'type': 'bytes', 'hex': value.hex()}
    if isinstance(value, (tuple, list, frozenset, set)):
        children = [encode(child, code_index) for child in value]
        if isinstance(value, (frozenset, set)):
            children.sort(key=lambda item: json.dumps(item, sort_keys=True))
        return {'type': type(value).__name__, 'items': children}
    if value is None or isinstance(value, (bool, int)):
        return {'type': type(value).__name__, 'value': value}
    if value is Ellipsis:
        return {'type': 'ellipsis'}
    if isinstance(value, (float, complex)):
        return {'type': type(value).__name__, 'repr': repr(value)}
    return {'type': type(value).__name__, 'repr': repr(value)}


def graph(root):
    codes = []
    indexes = {}
    edges = []
    def visit(code, parent, path):
        shared = id(code) in indexes
        if not shared:
            indexes[id(code)] = len(codes)
            codes.append(code)
        index = indexes[id(code)]
        edges.append({'parent': parent, 'const_path': path, 'code': index, 'shared': shared})
        if not shared:
            def walk(value, child_path):
                if isinstance(value, types.CodeType):
                    visit(value, index, child_path)
                elif isinstance(value, (tuple, list, frozenset, set)):
                    for i, item in enumerate(value):
                        walk(item, child_path+[i])
            for i, value in enumerate(code.co_consts):
                walk(value, [i])
    visit(root, None, [])
    return codes, indexes, edges


def inspect_code(code, index, indexes):
    row = {'index':index, 'name':code.co_name, 'qualname':getattr(code,'co_qualname',None),
           'filename':code.co_filename, 'argcount':code.co_argcount,
           'posonlyargcount':code.co_posonlyargcount, 'kwonlyargcount':code.co_kwonlyargcount,
           'nlocals':code.co_nlocals, 'stacksize':code.co_stacksize, 'flags':code.co_flags,
           'firstlineno':code.co_firstlineno, 'code_hex':code.co_code.hex(),
           'consts':[encode(c,indexes) for c in code.co_consts],
           'names':list(code.co_names), 'varnames':list(code.co_varnames),
           'freevars':list(code.co_freevars), 'cellvars':list(code.co_cellvars),
           'line_starts':list(dis.findlinestarts(code)),
           'lnotab_hex':code.co_lnotab.hex(),
           'linetable_hex':getattr(code,'co_linetable',b'').hex(),
           'exceptiontable_hex':getattr(code,'co_exceptiontable',b'').hex()}
    if hasattr(code,'co_lines'):
        row['line_ranges'] = list(code.co_lines())
    else:
        row['line_ranges'] = None
    if hasattr(code,'co_positions'):
        row['positions'] = list(code.co_positions())
    else:
        row['positions'] = None
    kwargs = {'show_caches':True,'adaptive':False} if sys.version_info >= (3,11) else {}
    instructions = []
    for item in dis.get_instructions(code, **kwargs):
        entry = {'offset':item.offset,'opcode':item.opcode,'opname':item.opname,
                 'raw_arg_byte':code.co_code[item.offset+1], 'arg':item.arg,
                 'argval':encode(item.argval,indexes), 'argrepr':('code-index:'+str(indexes[id(item.argval)])) if isinstance(item.argval,types.CodeType) else item.argrepr,
                 'starts_line':item.starts_line, 'is_jump_target':item.is_jump_target,
                 'positions':list(item.positions) if hasattr(item,'positions') else None}
        if hasattr(item,'start_offset'):
            entry['start_offset'] = item.start_offset
            entry['line_number'] = item.line_number
            entry['cache_offset'] = item.cache_offset
            entry['end_offset'] = item.end_offset
        instructions.append(entry)
    row['dis_instructions'] = instructions
    # Preserve CACHE code units separately; CPython 3.13 dis never yields CACHE rows.
    cache_units = []
    entries = getattr(opcode,'_inline_cache_entries',{})
    for item in instructions:
        if item['opname'] == 'CACHE':
            continue
        count = entries.get(item['opname'],0) if isinstance(entries,dict) else entries[item['opcode']]
        for slot in range(count):
            offset = item['offset'] + 2 + slot*2
            cache_units.append({'owner_offset':item['offset'], 'slot':slot, 'offset':offset,
                                'raw_hex':code.co_code[offset:offset+2].hex()})
    row['cache_units'] = cache_units
    if sys.version_info >= (3,11):
        row['exceptions'] = [entry._asdict() for entry in dis.Bytecode(code).exception_entries]
        slots = 0
        try:
            while True:
                code._varname_from_oparg(slots)
                slots += 1
        except IndexError:
            pass
        row['localsplus_names_by_operand'] = [code._varname_from_oparg(i) for i in range(slots)]
    else:
        row['exceptions'] = []
        row['localsplus_names_by_operand'] = None
    return row


def marshal_ranges(payload):
    """Independent range scanner from pinned CPython Python/marshal.c layouts.

    This diagnostic helper only consumes our trusted compiler-produced data;
    it is not rxd code and is not a general untrusted-input parser.
    """
    position = 0
    references = []
    codes = []
    aliases = []
    def take(n):
        nonlocal position
        assert n >= 0 and position+n <= len(payload)
        start = position
        position += n
        return payload[start:position]
    def integer():
        return int.from_bytes(take(4),'little',signed=True)
    def read():
        nonlocal position
        start = position
        tag = take(1)[0]
        kind = chr(tag & 127)
        if kind == 'r':
            index = integer()
            node = references[index]
            aliases.append({'start':start,'end':position,'target_start':node['start'],'target_kind':node['kind']})
            return node
        node = {'kind':kind,'start':start}
        if tag & 128:
            references.append(node)
        if kind in '0NFTS.':
            pass
        elif kind == 'i':
            node['value'] = integer()
        elif kind == 'I':
            take(8)
        elif kind == 'l':
            count=integer()
            take(abs(count)*2)
        elif kind in 'gy':
            take(8 if kind == 'g' else 16)
        elif kind in 'fx':
            for _ in range(1 if kind == 'f' else 2):
                take(take(1)[0])
        elif kind in 'stuaAzZ':
            length = take(1)[0] if kind in 'zZ' else integer()
            node['data_start'] = position
            node['raw'] = take(length)
            node['data_end'] = position
        elif kind in '()[<>':
            count = take(1)[0] if kind == ')' else integer()
            node['items'] = [read() for _ in range(count)]
        elif kind == '{':
            items=[]
            while True:
                key=read()
                if key['kind'] == '0':
                    break
                items.append((key,read()))
            node['items']=items
        elif kind == 'c':
            node['code_index'] = len(codes)
            codes.append(node)
            prefix = ['argcount','posonlyargcount','kwonlyargcount']
            if sys.version_info < (3,11):
                prefix += ['nlocals']
            prefix += ['stacksize','flags']
            fields={field:integer() for field in prefix}
            object_fields=['code','consts','names']
            if sys.version_info >= (3,11):
                object_fields += ['localsplusnames','localspluskinds','filename','name','qualname']
            else:
                object_fields += ['varnames','freevars','cellvars','filename','name']
            fields.update((field,read()) for field in object_fields)
            fields['firstlineno']=integer()
            fields['linetable']=read()
            if sys.version_info >= (3,11):
                fields['exceptiontable']=read()
            node['fields']=fields
        else:
            raise ValueError('unsupported trusted fixture tag '+repr(kind))
        node['end']=position
        return node
    root=read()
    assert root['kind'] == 'c' and position == len(payload)
    rows=[]
    for node in codes:
        fields=node['fields']
        bytecode=fields['code']
        row={'index':node['code_index'], 'marshal_start':node['start'], 'marshal_end':node['end'],
             'bytecode_start':bytecode['data_start'],'bytecode_end':bytecode['data_end'],
             'pyc_marshal_start':node['start']+16,'pyc_marshal_end':node['end']+16,
             'pyc_bytecode_start':bytecode['data_start']+16,'pyc_bytecode_end':bytecode['data_end']+16,
             'line_table_start':fields['linetable']['data_start'], 'line_table_end':fields['linetable']['data_end']}
        if 'localsplusnames' in fields:
            row['localsplus_names']=[n['raw'].decode('utf-8','surrogatepass') for n in fields['localsplusnames']['items']]
            row['localsplus_kinds_hex']=fields['localspluskinds']['raw'].hex()
            row['exception_table_start']=fields['exceptiontable']['data_start']
            row['exception_table_end']=fields['exceptiontable']['data_end']
        rows.append(row)
    return {'basis':'Pinned CPython Python/marshal.c TYPE_* grammar; no rxd/pycdc output',
            'offset_unit':'bytes', 'marshal_payload_bytes':len(payload), 'code_ranges':rows,
            'reference_occurrences':aliases}


def prepare(source, label, output, add_aliases=False):
    root = compile(source, label+'.py', 'exec', dont_inherit=True, optimize=0)
    if add_aliases:
        child = next(c for c in root.co_consts if isinstance(c,types.CodeType))
        # Controlled aliases/containers beyond compiler-generated instruction indexes.
        # CodeType.replace modifies data only; no compiled code is ever executed.
        alias = 'shared-container-marker'
        box = (alias, b'\xff\x80container', '雪☃', '\ud800', child)
        root = root.replace(co_consts=root.co_consts+(alias,alias,box,box,frozenset([alias])))
    payload = marshal.dumps(root,4)
    codes,indexes,edges = graph(root)
    expectations = {'generator':'CPython compile/marshal/dis; no rxd or pycdc output',
                    'python':sys.version, 'python_version_info':list(sys.version_info),
                    'magic_hex':importlib.util.MAGIC_NUMBER.hex(),
                    'source_filename':label+'.py', 'source_sha256':sha(source),
                    'marshal_sha256':sha(payload), 'graph_edges':edges, 'marshal_ranges':marshal_ranges(payload),
                    'codes':[inspect_code(c,i,indexes) for i,c in enumerate(codes)]}
    rows=[]
    for mode, flags, body in [('timestamp',0,struct.pack('<II',0,len(source))),
                              ('hash-unchecked',1,importlib.util.source_hash(source)),
                              ('hash-checked',3,importlib.util.source_hash(source))]:
        raw=importlib.util.MAGIC_NUMBER+struct.pack('<I',flags)+body+payload
        filename=label+'.'+mode+'.pyc'
        (output/filename).write_bytes(raw)
        rows.append({'file':filename, 'bytes':len(raw),'sha256':sha(raw),'header_mode':mode})
    (output/(label+'.marshal')).write_bytes(payload)
    (output/(label+'.oracle.json')).write_text(json.dumps(expectations,indent=2,sort_keys=True,ensure_ascii=True)+'\n')
    (output/(label+'.py')).write_bytes(source)
    return {'label':label, 'source_bytes':len(source), 'source_sha256':sha(source), 'code_count':len(codes),
            'marshal_bytes':len(payload), 'marshal_sha256':sha(payload), 'files':rows}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--representative',action='store_true')
    args=parser.parse_args()
    assert sys.implementation.name == 'cpython' and sys.version_info.releaselevel == 'final'
    version='.'.join(str(x) for x in sys.version_info[:3])
    output=args.output/version
    output.mkdir(parents=True,exist_ok=True)
    rows=[prepare(SOURCE.encode(), 'graph', output, add_aliases=True)]
    extended=('def many():\n' + ''.join('    local_'+str(i)+' = "literal-'+str(i)+'"\n' for i in range(300)) + '    return local_299\n').encode()
    rows.append(prepare(extended,'extended',output))
    if args.representative:
        stdlib=Path(sysconfig.get_path('stdlib'))
        # Files are only read/compiled. None of these analyzed modules is imported.
        for module in ['ast.py','configparser.py','email/policy.py','email/parser.py','dataclasses.py']:
            module_name=module[:-3].replace('/','.')
            assert module_name not in sys.modules, 'analyzed module already imported: '+module_name
            source=(stdlib/module).read_bytes()
            row=prepare(source,module.replace('/','-').removesuffix('.py') if hasattr(str,'removesuffix') else module.replace('/','-')[:-3],output)
            row['source_path']=str(stdlib/module)
            row['upstream_source']='https://github.com/python/cpython/blob/v'+version+'/Lib/'+module
            row['license']='CPython PSF license; verify exact source hash against official tag'
            assert module_name not in sys.modules, 'analyzed module imported during preparation: '+module_name
            rows.append(row)
    manifest={'interpreter':sys.executable,'python':sys.version,'version_info':list(sys.version_info),
              'binary_sha256':sha(Path(sys.executable).read_bytes()), 'magic_hex':importlib.util.MAGIC_NUMBER.hex(),
              'compile_only':True,'analyzed_modules_imported_or_executed':False,'inputs':rows,
              'generator_sha256':sha(Path(__file__).read_bytes()),
              'artifacts':{p.name: {'bytes':p.stat().st_size,'sha256':sha(p.read_bytes())} for p in output.iterdir() if p.name != 'manifest.json'}}
    (output/'manifest.json').write_text(json.dumps(manifest,indent=2,sort_keys=True)+'\n')
    print(json.dumps({'version':version,'output':str(output),'inputs':[{k:v for k,v in r.items() if k in ['label','marshal_bytes','code_count']} for r in rows]}))

if __name__=='__main__':
    main()
