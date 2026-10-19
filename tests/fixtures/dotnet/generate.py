#!/usr/bin/env python3
"""Generate original, non-executed ECMA-335 fixtures using only Python stdlib."""
from pathlib import Path
import hashlib
import json
import struct
import sys

ROOT = Path(__file__).resolve().parent

def u16(n):
    return struct.pack('<H', n)

def u32(n):
    return struct.pack('<I', n)

def align(data, n):
    return data + bytes((-len(data)) % n)

strings = bytearray(b'\0')
blobs = bytearray(b'\0')

def string(text):
    pos = len(strings)
    strings.extend(text.encode('utf-8') + b'\0')
    return pos

def blob(data):
    assert len(data) < 128
    pos = len(blobs)
    blobs.extend(bytes([len(data)]) + data)
    return pos

module_name = string('Fixture.dll')
assembly_name = string('RxdFixtures')
type_name = string('Probe')
namespace = string('Rxd.测试')
module_type = string('<Module>')
field_name = string('Counter')
object_name = string('Object')
system_name = string('System')
mscorlib_name = string('mscorlib')
value_type_name = string('ValueType')
exception_name = string('Exception')
struct_name = string('Value')
value_field_name = string('Value')
generic_name = string('GenericBox`1')
generic_field_name = string('Item')
generic_field_sig = blob(b'\x06\x13\x00')
fake_enum_name = string('Enum')
enum_like_name = string('EnumLikeClass')
field_sig = blob(bytes([0x06, 0x08]))
local_sig = blob(bytes([0x07, 0x01, 0x08]))
user_text = '世界\nλ'
encoded = user_text.encode('utf-16-le') + b'\x01'
us_heap = b'\0' + bytes([len(encoded)]) + encoded
unpaired_offset = len(us_heap)
us_heap += b'\x03\x00\xd8\x01'  # one UTF-16 high surrogate, retained as units

# Signatures use CLI element types: I4=8, U4=9, BOOLEAN=2, STRING=14,
# BYREF=16. Calls/fields identify explicit rows in the tables below.
field_token = u32(0x04000001)
methods = [
    ('Add', [b'\x08', b'\x08'], b'\x08', b'\x02\x03\x58\x2a', None),
    ('Checked', [b'\x08', b'\x08'], b'\x08', b'\x02\x03\xd6\x2a', None),
    ('UnsignedGreater', [b'\x09', b'\x09'], b'\x02', b'\x02\x03\xfe\x03\x2a', None),
    ('RefRead', [b'\x10\x08'], b'\x08', b'\x02\x4a\x2a', None),
    ('VolatileRef', [b'\x10\x08'], b'\x08', b'\x02\xfe\x13\x4a\x2a', None),
    ('Effect', [], b'\x08', b'\x7e' + field_token + b'\x17\x58\x80' + field_token + b'\x1d\x2a', None),
    ('Discard', [], b'\x08', b'\x28' + u32(0x06000006) + b'\x26\x16\x2a', None),
    ('Duplicate', [], b'\x08', b'\x28' + u32(0x06000006) + b'\x25\x58\x2a', None),
    ('Merge', [b'\x02'], b'\x08', b'\x02\x2c\x03\x17\x2b\x01\x18\x2a', None),
    ('Finally', [], b'\x08', b'\x17\x0a\xde\x03\x18\x0a\xdc\x06\x2a', [(2, 0, 4, 4, 3, 0)]),
    ('Unicode', [], b'\x0e', b'\x72' + u32(0x70000001) + b'\x2a', None),
    ('UnpairedUnicode', [], b'\x0e', b'\x72' + u32(0x70000000 | unpaired_offset) + b'\x2a', None),
]

# Extra probes retain the original first twelve MethodDef identities. Their
# expectations are specified directly as ECMA-335 bytecode, never decompiler output.
value_sig = b'\x11\x0c'  # valuetype TypeDef row 3
value_token = u32(0x02000003)
catch_token = 0x01000003
filter_code = b'\x26\x7e' + field_token + b'\x17\x58\x80' + field_token + b'\x17\xfe\x11'
filter_handler = 2 + len(filter_code)
methods += [
    ('NestedFinally', [], b'\x08', b'\x17\x0a\xde\x06\x18\x0a\xdc\x19\x0a\xdc\x06\x2a',
        [(2, 0, 4, 4, 3, 0), (2, 0, 7, 7, 3, 0)]),
    ('SharedFinally', [b'\x02'], b'\x08', b'\x02\x2c\x04\x17\x0a\xde\x07\x18\x0a\xde\x05\x19\x0a\xdc\x06\x2a\x06\x17\x58\x2a', [(2, 0, 11, 11, 3, 0)]),
    ('AliasedFinally', [], b'\x08', b'\x17\x0a\xde\x05\x12\x00\x18\x54\xdc\x06\x2a', [(2, 0, 4, 4, 5, 0)]),
    ('Fault', [], b'\x08', b'\x17\x0a\xde\x03\x18\x0a\xdc\x06\x2a', [(4, 0, 4, 4, 3, 0)]),
    ('Catch', [], b'\x08', b'\x14\x7a\x26\x18\x0a\xde\x00\x06\x2a', [(0, 0, 2, 2, 5, catch_token)]),
    ('Filter', [], b'\x08', b'\x14\x7a' + filter_code + b'\x26\x18\x0a\xde\x00\x06\x2a',
        [(1, 0, 2, filter_handler, 5, 2)]),
    ('Rethrow', [], b'\x08', b'\x14\x7a\x26\xfe\x1a\x26\x18\x0a\xde\x00\x06\x2a',
        [(0, 0, 2, 2, 3, catch_token), (0, 0, 5, 5, 5, catch_token)]),
    ('FinallySkip', [], b'\x08', b'\x17\x0a\x2b\x03\x18\x0a\xdc\x06\x2a', [(2, 0, 4, 4, 3, 0)]),
    ('ValueCopy', [value_sig], value_sig, b'\x02\x2a', None),
    ('ValueField', [value_sig], b'\x08', b'\x02\x7b' + u32(0x04000002) + b'\x2a', None),
    ('ValueInit', [b'\x10' + value_sig], b'\x01', b'\x02\xfe\x15' + value_token + b'\x2a', None),
    ('ValueLoad', [b'\x10' + value_sig], value_sig, b'\x02\x71' + value_token + b'\x2a', None),
    ('ValueStore', [b'\x10' + value_sig, value_sig], b'\x01', b'\x02\x03\x81' + value_token + b'\x2a', None),
    ('ValueCopyAddress', [b'\x10' + value_sig, b'\x10' + value_sig], b'\x01', b'\x02\x03\x70' + value_token + b'\x2a', None),
    ('ValueUnbox', [b'\x1c'], value_sig, b'\x02\xa5' + value_token + b'\x2a', None),
    ('NewArray', [b'\x08'], b'\x1d\x1c', b'\x02\x8d' + u32(0x01000001) + b'\x2a', None),
    ('ValueArrayRead', [b'\x1d' + value_sig, b'\x08'], value_sig, b'\x02\x03\xa3' + value_token + b'\x2a', None),
    ('ReadonlyAddress', [b'\x1d' + value_sig, b'\x08'], b'\x10' + value_sig, b'\x02\x03\xfe\x1e\x8f' + value_token + b'\x2a', None),
    ('UnalignedRef', [b'\x10\x08'], b'\x08', b'\x02\xfe\x12\x01\xfe\x13\x4a\x2a', None),
    ('VolatileAdd', [b'\x08', b'\x08'], b'\x08', b'\x02\x03\xfe\x13\x58\x2a', None),
    ('GenericIdentity', [b'\x1e\x00'], b'\x1e\x00', b'\x02\x2a', None),
    ('GenericCaller', [b'\x1e\x01'], b'\x1e\x01', b'\x02\x28' + u32(0x2b000001) + b'\x2a', None),
    ('GenericOwnerField', [b'\x15\x12\x10\x01\x1e\x00'], b'\x1e\x00', b'\x02\x7b' + u32(0x0a000002) + b'\x2a', None),
    ('GenericMixedCaller', [b'\x1e\x01'], b'\x1e\x00', b'\x02\x28' + u32(0x2b000002) + b'\x2a', None),
    ('GenericExternalCall', [b'\x1e\x00'], b'\x1e\x00', b'\x02\x28' + u32(0x0a000001) + b'\x2a', None),
    ('ValueCall', [b'\x10' + value_sig], b'\x08', b'\x02\x28' + bytes(4) + b'\x2a', None),
    ('ValueNew', [b'\x08'], value_sig, b'\x02\x73' + bytes(4) + b'\x2a', None),
]
probe_method_count = len(methods)
methods = [(name, params, ret,
    (b'\x02\x28' + u32(0x06000000 + probe_method_count + 1) + b'\x2a') if name == 'ValueCall' else
    (b'\x02\x73' + u32(0x06000000 + probe_method_count + 2) + b'\x2a') if name == 'ValueNew' else code, eh)
    for name, params, ret, code, eh in methods]
methods += [
    ('GetValue', [], b'\x08', b'\x02\x7b' + u32(0x04000002) + b'\x2a', None),
    ('.ctor', [b'\x08'], b'\x01', b'\x02\x03\x7d' + u32(0x04000002) + b'\x2a', None),
    ('Select', [b'\x1e\x00'], b'\x13\x00', b'\x14\xa5' + u32(0x1b000002) + b'\x2a', None),
    ('AliasClassValue', [], b'\x08', b'\x02\x7b' + u32(0x04000004) + b'\x2a', None),
]

text = bytearray(bytes(72))  # CLI header is filled after methods and metadata.
method_rows = []
param_rows = []
expected = {}
for name, params, ret, code, eh in methods:
    text[:] = align(bytes(text), 4)
    rva = 0x2000 + len(text)
    if eh is None:
        assert len(code) < 64
        body = bytes([(len(code) << 2) | 2]) + code
    else:
        # Fat method header, initlocals+moresections, one int local, finally EH.
        body = u16(0x301B) + u16(8) + u32(len(code)) + u32(0x11000001) + code
        body = align(body, 4)
        body += bytes([1, 4 + 12 * len(eh), 0, 0])
        for flags, start, length, handler, hlength, extra in eh:
            body += u16(flags) + u16(start) + bytes([length]) + u16(handler) + bytes([hlength]) + u32(extra)
    text.extend(body)
    name_index = string(name)
    instance = len(method_rows) >= probe_method_count and name != 'Select'
    generic_arity = 2 if name in ('GenericCaller', 'GenericMixedCaller') else 1 if name in ('GenericIdentity', 'GenericOwnerField', 'GenericExternalCall', 'Select') else 0
    signature_prefix = b'\x20' if instance else b'\x10' + bytes([generic_arity]) if generic_arity else b'\0'
    sig_index = blob(signature_prefix + bytes([len(params)]) + ret + b''.join(params))
    first_param = len(param_rows) + 1
    for i, _ in enumerate(params):
        param_rows.append(u16(0) + u16(i + 1) + u16(string(f'arg{i}')))
    method_rows.append(u32(rva) + u16(0) + u16(0x0006 if instance else 0x0016) + u16(name_index) + u16(sig_index) + u16(first_param))
    expected[name] = {'token': f'0x{0x06000000 + len(method_rows):08x}', 'il_hex': code.hex(), 'rva': rva, 'declaring_type': 'Rxd.测试.EnumLikeClass' if name == 'AliasClassValue' else 'Rxd.测试.Value' if instance else 'Rxd.测试.GenericBox`1' if name == 'Select' else 'Rxd.测试.Probe'}

rows = {
    0: [u16(0) + u16(module_name) + u16(1) + u16(0) + u16(0)],
    1: [u16(6) + u16(object_name) + u16(system_name),
        u16(6) + u16(value_type_name) + u16(system_name),
        u16(6) + u16(exception_name) + u16(system_name)],
    2: [u32(0) + u16(module_type) + u16(0) + u16(0) + u16(1) + u16(1),
        u32(0x00100181) + u16(type_name) + u16(namespace) + u16(5) + u16(1) + u16(1),
        u32(0x00100109) + u16(struct_name) + u16(namespace) + u16(9) + u16(2) + u16(probe_method_count + 1),
        u32(0x00100101) + u16(generic_name) + u16(namespace) + u16(5) + u16(3) + u16(probe_method_count + 3),
        u32(0x00100101) + u16(fake_enum_name) + u16(system_name) + u16(5) + u16(4) + u16(probe_method_count + 4),
        u32(0x00100101) + u16(enum_like_name) + u16(namespace) + u16(5 << 2) + u16(4) + u16(probe_method_count + 4)],
    4: [u16(0x0016) + u16(field_name) + u16(field_sig), u16(0x0006) + u16(value_field_name) + u16(field_sig), u16(0x0006) + u16(generic_field_name) + u16(generic_field_sig), u16(0x0006) + u16(value_field_name) + u16(field_sig)],
    6: method_rows,
    8: param_rows,
    10: [u16(9) + u16(string('Identity')) + u16(blob(b'\x10\x01\x01\x1e\x00\x1e\x00')),
         u16(12) + u16(generic_field_name) + u16(generic_field_sig),
         u16(12) + u16(string('Select')) + u16(blob(b'\x10\x01\x01\x13\x00\x1e\x00'))],
    17: [u16(local_sig)],
    27: [u16(blob(b'\x15\x12\x10\x01\x1e\x00')), u16(blob(b'\x13\x00'))],
    43: [u16((next(i + 1 for i, m in enumerate(methods) if m[0] == 'GenericIdentity') << 1)) + u16(blob(b'\x0a\x01\x1e\x01')),
         u16(7) + u16(blob(b'\x0a\x01\x1e\x01'))],
    42: [u16(n) + u16(0) + u16(((i + 1) << 1) | 1) + u16(string(f'T{n}'))
         for i, m in enumerate(methods)
         for n in range(2 if m[0] in ('GenericCaller', 'GenericMixedCaller') else 1 if m[0] in ('GenericIdentity', 'GenericOwnerField', 'GenericExternalCall', 'Select') else 0)]
        + [u16(0) + u16(0) + u16(4 << 1) + u16(string('T'))],
    35: [u16(4) + u16(0) + u16(0) + u16(0) + u32(0) + u16(0) + u16(mscorlib_name) + u16(0) + u16(0)],
    32: [u32(0x8004) + u16(1) + u16(0) + u16(0) + u16(0) + u32(0) + u16(0) + u16(assembly_name) + u16(0)],
}
rows[42].sort(key=lambda row: (struct.unpack('<H', row[4:6])[0], struct.unpack('<H', row[:2])[0]))
invalid_reference = '--invalid-reference' in sys.argv
if invalid_reference:
    # Two attributes share one constructor. The second parent references a
    # nonexistent MethodDef, so caching constructor validation cannot hide it.
    ctor_name = string('.ctor')
    ctor_sig = blob(b'\x20\x00\x01')
    attr_value = blob(b'\x01\x00\x00\x00')
    rows[10].append(u16(9) + u16(ctor_name) + u16(ctor_sig))  # TypeRef row 1
    ctor_coded_index = (len(rows[10]) << 3) | 3
    rows[12] = [u16(32) + u16(ctor_coded_index) + u16(attr_value),
                u16(1023 << 5) + u16(ctor_coded_index) + u16(attr_value)]
valid = sum(1 << table for table in rows)
tables = u32(0) + bytes([2, 0, 0, 1]) + struct.pack('<Q', valid) + struct.pack('<Q', 0)
tables += b''.join(u32(len(rows[t])) for t in sorted(rows))
tables += b''.join(b''.join(rows[t]) for t in sorted(rows))
streams = [('#~', tables), ('#Strings', bytes(strings)), ('#Blob', bytes(blobs)), ('#GUID', bytes.fromhex('00112233445566778899aabbccddeeff')), ('#US', us_heap)]
version = align(b'v4.0.30319\0', 4)
assert len(version) % 4 == 0
prefix = u32(0x424A5342) + u16(1) + u16(1) + u32(0) + u32(len(version)) + version + u16(0) + u16(len(streams))
headers_size = sum(8 + len(align(name.encode() + b'\0', 4)) for name, _ in streams)
offset = len(prefix) + headers_size
headers = bytearray()
payload = bytearray()
for name, data in streams:
    data = align(data, 4)
    headers.extend(u32(offset + len(payload)) + u32(len(data)) + align(name.encode() + b'\0', 4))
    payload.extend(data)
metadata = prefix + headers + payload
text[:] = align(bytes(text), 4)
metadata_rva = 0x2000 + len(text)
text.extend(metadata)
cli = u32(72) + u16(2) + u16(5) + u32(metadata_rva) + u32(len(metadata)) + u32(1) + u32(0) + bytes(48)
assert len(cli) == 72
text[:72] = cli
raw_size = len(align(bytes(text), 512))
virtual_size = len(text)
# Minimal PE32: no imports/entry point. This file is an analysis fixture.
dos = bytearray(128)
dos[:2] = b'MZ'
dos[0x3c:0x40] = u32(128)
coff = u16(0x14c) + u16(1) + u32(0) + u32(0) + u32(0) + u16(224) + u16(0x2102)
opt = bytearray(224)
opt[:2] = u16(0x10b)
opt[4:8] = u32(raw_size)
opt[20:24] = u32(0x2000)
opt[28:32] = u32(0x400000)
opt[32:40] = u32(0x2000) + u32(512)
opt[40:44] = u16(4) + u16(0)
opt[48:52] = u16(4) + u16(0)
opt[56:64] = u32(((0x2000 + virtual_size + 0x1fff) // 0x2000) * 0x2000) + u32(512)
opt[68:72] = u16(3) + u16(0x8540)
opt[72:88] = u32(0x100000) + u32(0x1000) + u32(0x100000) + u32(0x1000)
opt[92:96] = u32(16)
opt[96 + 14 * 8:96 + 15 * 8] = u32(0x2000) + u32(72)
section = b'.text\0\0\0' + u32(virtual_size) + u32(0x2000) + u32(raw_size) + u32(512) + bytes(12) + u32(0x60000020)
image = align(bytes(dos) + b'PE\0\0' + coff + opt + section, 512) + align(bytes(text), 512)
(ROOT / ('InvalidReference.dll' if invalid_reference else 'Fixture.dll')).write_bytes(image)
(ROOT / ('invalid-reference.json' if invalid_reference else 'expected.json')).write_text(json.dumps({'sha256': hashlib.sha256(image).hexdigest(), 'bytes': len(image), 'type': 'Rxd.测试.Probe', 'string': user_text, 'utf16_units': [0xD800], 'methods': expected}, ensure_ascii=False, indent=2) + '\n')
print(hashlib.sha256(image).hexdigest(), len(image))
