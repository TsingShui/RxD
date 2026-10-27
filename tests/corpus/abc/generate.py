#!/usr/bin/env python3
"""Generate an inert dynamic ABC 12.0.6.0 fixture from the pinned file/ISA definitions."""
import argparse
import json
from pathlib import Path
import struct
import zlib


def uleb(value):
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def mutf8(text):
    encoded = text.encode('utf-16-le', errors='surrogatepass')
    units = struct.unpack('<' + 'H' * (len(encoded) // 2), encoded)
    result = bytearray(uleb(len(units) * 2 + int(all(0 < unit < 128 for unit in units))))
    for unit in units:
        if 0 < unit < 128:
            result.append(unit)
        elif unit < 2048:
            result.extend((192 | (unit >> 6), 128 | (unit & 63)))
        else:
            result.extend((224 | (unit >> 12), 128 | ((unit >> 6) & 63), 128 | (unit & 63)))
    result.append(0)
    return result


def module_fixture(output):
    data = bytearray(124)
    patches = []

    def append(value):
        offset = len(data)
        data.extend(value)
        return offset

    def patch():
        return append(bytes(4))

    def put(offset, value):
        patches.append((offset, value))

    record = append(mutf8('L&fixture/Module&;'))
    append(struct.pack('<I', 0) + uleb(0) + uleb(1) + uleb(1) + b'\0')
    append(struct.pack('<HH', 2, 1))
    field_name_patch = patch()
    append(uleb(0) + b'\2')
    module_patch = patch()
    append(b'\0')
    method = append(struct.pack('<HH', 2, 65535))
    method_name_patch = patch()
    append(uleb(8) + b'\1')
    code_patch = patch()
    append(b'\0')
    field_name = append(mutf8('moduleRecordIdx'))
    method_name = append(mutf8('readModule'))
    local_import = append(mutf8('consoleLogger'))
    imported = append(mutf8('default'))
    request = append(mutf8('@example/logger'))
    local_export = append(mutf8('counter'))
    module = append(struct.pack('<II', 0, 1) + struct.pack('<I', request)
                    + struct.pack('<I', 1) + struct.pack('<IIH', local_import, imported, 0)
                    + struct.pack('<I', 0) + struct.pack('<I', 1)
                    + struct.pack('<II', local_export, local_export)
                    + struct.pack('<II', 0, 0))
    code_bytes = bytes([0x7e, 0, 0xfe, 9, 0, 0, 0x7d, 0, 0x7c, 0, 0x64])
    code = append(uleb(0) + uleb(3) + uleb(len(code_bytes)) + uleb(0) + code_bytes)
    for address, value in [(field_name_patch, field_name), (module_patch, module),
                           (method_name_patch, method_name), (code_patch, code)]:
        put(address, value)
    for address, value in patches:
        struct.pack_into('<I', data, address, value)
    struct.pack_into('<II', data, 60, record, module)
    struct.pack_into('<IIIIIIIIII', data, 68, record, len(data), 3, 108, 1, 120,
                     0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff)
    struct.pack_into('<III', data, 108, 2, 6, record)
    struct.pack_into('<I', data, 120, local_import)
    data[:8] = b'PANDA\0\0\0'
    data[12:16] = bytes([12, 0, 6, 0])
    struct.pack_into('<IIIIIIIIIII', data, 16, len(data), 0, 0, 1, 60, 0, 0, 1, 64, 1, 68)
    struct.pack_into('<I', data, 8, zlib.adler32(data[12:]))
    (output / 'module.abc').write_bytes(data)
    expected = {'record': f'record:0x{record:x}', 'method': f'method:0x{method:x}',
                'request': '@example/logger', 'import': 'consoleLogger', 'export': 'counter'}
    (output / 'module.json').write_text(json.dumps(expected, ensure_ascii=False, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path(__file__).resolve().parent)
    args = parser.parse_args()
    data = bytearray(136)
    patches = []

    def append(value):
        offset = len(data)
        data.extend(value)
        return offset

    def placeholder():
        return append(bytes(4))

    record_name = 'L&fixture/Main你好&;'
    record = append(mutf8(record_name))
    append(struct.pack('<I', 0) + uleb(0) + uleb(0) + uleb(2) + b'\0')
    methods = []
    for name in ['func_main_0', '加法😀']:
        offset = append(struct.pack('<HH', 2, 65535))
        name_patch = placeholder()
        append(uleb(8) + b'\1')
        code_patch = placeholder()
        append(b'\0')
        methods.append({'at': f'method:0x{offset:x}', 'offset': offset, 'name': name,
                        'name_patch': name_patch, 'code_patch': code_patch})
    other_name = 'L&fixture/Other&;'
    other = append(mutf8(other_name))
    append(struct.pack('<I', 0) + uleb(0) + uleb(0) + uleb(0) + b'\0')
    names = []
    for method in methods:
        name = append(mutf8(method['name']))
        names.append(name)
        patches.append((method.pop('name_patch'), name))
    text = append(mutf8('你好\0😀'))
    for method, instructions in zip(methods, [b'\x65', b'\x62\x07\0\0\0\x64']):
        code = append(uleb(0) + uleb(3) + uleb(len(instructions)) + uleb(0) + instructions)
        patches.append((method.pop('code_patch'), code))
        method.update(code_offset=code, code_size=len(instructions), args=3, vregs=0)
    for offset, value in patches:
        struct.pack_into('<I', data, offset, value)
    struct.pack_into('<II', data, 60, record, other)
    struct.pack_into('<IIIIIIIIII', data, 68, record, len(data), 4, 108, 3, 124,
                     0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff)
    struct.pack_into('<IIII', data, 108, 2, 6, record, other)
    struct.pack_into('<III', data, 124, *names, text)
    data[:8] = b'PANDA\0\0\0'
    data[12:16] = bytes([12, 0, 6, 0])
    struct.pack_into('<IIIIIIIIIII', data, 16, len(data), 0, 0, 2, 60, 0, 0, 0, 0, 1, 68)
    struct.pack_into('<I', data, 8, zlib.adler32(data[12:]))
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / 'minimal.abc').write_bytes(data)
    expected = {'version': '12.0.6.0', 'record': {'at': f'record:0x{record:x}',
                'offset': record, 'name': record_name, 'methods': methods},
                'empty_record': {'at': f'record:0x{other:x}', 'offset': other, 'name': other_name},
                'text_offset': text, 'text': '你好\0😀', 'bytes': len(data)}
    (args.output / 'minimal.json').write_text(json.dumps(expected, ensure_ascii=False, indent=2) + '\n')
    module_fixture(args.output)


if __name__ == '__main__':
    main()
