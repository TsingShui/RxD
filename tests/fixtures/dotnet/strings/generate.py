#!/usr/bin/env python3
"""Original non-executed ECMA-335 string-use fixture; Python stdlib only."""
from pathlib import Path
import hashlib
import json
import struct

ROOT = Path(__file__).resolve().parent
u16 = lambda value: struct.pack('<H', value)
u32 = lambda value: struct.pack('<I', value)

def align(data, amount):
    return data + bytes((-len(data)) % amount)

names = bytearray(b'\0')
def name(value):
    index = len(names)
    names.extend(value.encode() + b'\0')
    return index

heap = bytearray(b'\0')
entries = []
for label, units in [
    ('shared', list('shared 世界\nλ'.encode('utf-16-le'))),
    ('duplicate', list('shared 世界\nλ'.encode('utf-16-le'))),
    ('unused', list('unused inventory'.encode('utf-16-le'))),
    ('empty', []),
    ('unicode', list('🙂世界'.encode('utf-16-le'))),
    ('nul', list('a\0b'.encode('utf-16-le'))),
    ('high', [0, 0xd8]),
    ('low', [0, 0xdc]),
    ('tail', list('prefix'.encode('utf-16-le')) + [0, 0xd8] + list('尾'.encode('utf-16-le'))),
]:
    offset = len(heap)
    raw = bytes(units)
    assert len(raw) + 1 < 128
    code_units = list(struct.unpack('<' + 'H' * (len(raw) // 2), raw))
    terminal = int(any(unit > 255 or 1 <= unit <= 8 or 14 <= unit <= 31 or unit in (39, 45, 127)
                       for unit in code_units))
    heap.extend(bytes([len(raw) + 1]) + raw + bytes([terminal]))
    try:
        value = raw.decode('utf-16-le')
        lossless = None
    except UnicodeDecodeError:
        value = None
        lossless = code_units
    entries.append({'label': label, 'offset': offset, 'token': 0x70000000 | offset,
                    'value': value, 'utf16_units': lossless, 'uses': []})
entry = {value['label']: value for value in entries}

text = bytearray(72)
methods = []
method_rows = []
for method_index, (owner, method_name, labels) in enumerate([
    ('Rxd.Strings.Alpha', 'Repeated', ['shared', 'shared', 'duplicate', 'empty', 'nul', 'high']),
    ('Rxd.Strings.Alpha', 'Unicode', ['unicode', 'low', 'tail']),
    ('Rxd.Strings.Beta', 'Shared', ['shared', 'duplicate', 'tail']),
], 1):
    code = bytearray()
    for label in labels:
        item = entry[label]
        item['uses'].append({'method_token': 0x06000000 | method_index, 'il_offset': len(code)})
        code.extend(b'\x72' + u32(item['token']) + b'\x26')
    code.extend(b'\x2a')
    assert len(code) < 64
    text[:] = align(bytes(text), 4)
    file_offset = 512 + len(text)
    rva = 0x2000 + len(text)
    text.extend(bytes([(len(code) << 2) | 2]) + code)
    method_rows.append(u32(rva) + u16(0) + u16(0x16) + u16(name(method_name)) + u16(1) + u16(1))
    methods.append({'token': 0x06000000 | method_index, 'name': method_name,
                    'declaring_type': owner, 'declaring_type_token': 0x02000002 if method_index < 3 else 0x02000003,
                    'file_offset': file_offset, 'code_offset': file_offset + 1, 'il_hex': code.hex()})

rows = {
    0: [u16(0) + u16(name('Strings.dll')) + u16(1) + u16(0) + u16(0)],
    1: [u16(6) + u16(name('Object')) + u16(name('System'))],
    2: [u32(0) + u16(name('<Module>')) + u16(0) + u16(0) + u16(1) + u16(1),
        u32(0x00100181) + u16(name('Alpha')) + u16(name('Rxd.Strings')) + u16(5) + u16(1) + u16(1),
        u32(0x00100181) + u16(name('Beta')) + u16(name('Rxd.Strings')) + u16(5) + u16(1) + u16(3)],
    6: method_rows,
    32: [u32(0x8004) + u16(1) + u16(0) + u16(0) + u16(0) + u32(0) + u16(0) + u16(name('RxdStrings')) + u16(0)],
    35: [u16(4) + u16(0) + u16(0) + u16(0) + u32(0) + u16(0) + u16(name('mscorlib')) + u16(0) + u16(0)],
}
valid = sum(1 << table for table in rows)
tables = u32(0) + bytes([2, 0, 0, 1]) + struct.pack('<QQ', valid, 0)
tables += b''.join(u32(len(rows[table])) for table in sorted(rows))
tables += b''.join(b''.join(rows[table]) for table in sorted(rows))
streams = [('#~', tables), ('#Strings', bytes(names)), ('#Blob', b'\0\x03\0\0\x01'),
           ('#GUID', bytes.fromhex('00112233445566778899aabbccddeeff')), ('#US', bytes(heap))]
version = align(b'v4.0.30319\0', 4)
prefix = u32(0x424a5342) + u16(1) + u16(1) + u32(0) + u32(len(version)) + version + u16(0) + u16(len(streams))
headers_size = sum(8 + len(align(label.encode() + b'\0', 4)) for label, _ in streams)
offset = len(prefix) + headers_size
headers = bytearray()
payload = bytearray()
text[:] = align(bytes(text), 4)
metadata_offset = 512 + len(text)
for label, data in streams:
    start = offset + len(payload)
    if label == '#US':
        us_file_offset = metadata_offset + start
    data = align(data, 4)
    headers.extend(u32(start) + u32(len(data)) + align(label.encode() + b'\0', 4))
    payload.extend(data)
metadata = prefix + headers + payload
metadata_rva = 0x2000 + len(text)
text.extend(metadata)
text[:72] = u32(72) + u16(2) + u16(5) + u32(metadata_rva) + u32(len(metadata)) + u32(1) + u32(0) + bytes(48)
raw_size = len(align(bytes(text), 512))
virtual_size = len(text)
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
opt[56:64] = u32((0x2000 + virtual_size + 0x1fff) // 0x2000 * 0x2000) + u32(512)
opt[68:72] = u16(3) + u16(0x8540)
opt[72:88] = u32(0x100000) + u32(0x100000 // 16) + u32(0x100000) + u32(0x100000 // 16)
opt[92:96] = u32(16)
opt[208:216] = u32(0x2000) + u32(72)
section = b'.text\0\0\0' + u32(virtual_size) + u32(0x2000) + u32(raw_size) + u32(512) + bytes(12) + u32(0x60000020)
image = align(bytes(dos) + b'PE\0\0' + coff + opt + section, 512) + align(bytes(text), 512)
(ROOT / 'Strings.dll').write_bytes(image)
(ROOT / 'expected.json').write_text(json.dumps({'sha256': hashlib.sha256(image).hexdigest(),
    'bytes': len(image), 'us_file_offset': us_file_offset, 'entries': entries, 'methods': methods},
    ensure_ascii=False, indent=2) + '\n')
print(hashlib.sha256(image).hexdigest(), len(image))
