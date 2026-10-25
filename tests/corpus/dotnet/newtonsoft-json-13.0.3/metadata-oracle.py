#!/usr/bin/env python3
"""Inspect raw PE/CLI tables independently to locate malformed-input probes; IL/EH uses PEReader facts."""
import hashlib
import json
import pathlib
import struct

HERE = pathlib.Path(__file__).resolve().parent
provenance = json.loads((HERE.parent / "class-expectations.json").read_text())["provenance"]
data = (HERE / "Newtonsoft.Json.dll").read_bytes()
assert len(data) == provenance["dll_bytes"]
assert hashlib.sha256(data).hexdigest() == provenance["dll_sha256"]


def number(offset, width):
    assert 0 <= offset <= len(data) - width
    return int.from_bytes(data[offset:offset + width], "little")


assert data[:2] == b"MZ"
pe = number(0x3C, 4)
assert data[pe:pe + 4] == b"PE\0\0"
section_count = number(pe + 6, 2)
optional_size = number(pe + 20, 2)
optional = pe + 24
magic = number(optional, 2)
assert magic in (0x10B, 0x20B)
directories = optional + (96 if magic == 0x10B else 112)
sections = optional + optional_size


def file_offset(rva):
    for index in range(section_count):
        header = sections + 40 * index
        virtual_address = number(header + 12, 4)
        raw_size = number(header + 16, 4)
        raw_pointer = number(header + 20, 4)
        if virtual_address <= rva < virtual_address + raw_size:
            return raw_pointer + rva - virtual_address
    raise ValueError("RVA outside the original section bytes")


cli = file_offset(number(directories + 14 * 8, 4))
root = file_offset(number(cli + 8, 4))
assert data[root:root + 4] == b"BSJB"
position = root + 16 + number(root + 12, 4)
stream_count = number(position + 2, 2)
position += 4
streams = {}
for _ in range(stream_count):
    offset, size = struct.unpack_from("<II", data, position)
    name_start = position + 8
    name_end = data.index(0, name_start)
    streams[data[name_start:name_end].decode("ascii")] = (root + offset, size)
    position = name_start + ((name_end - name_start + 1 + 3) & ~3)

tables, _ = streams["#~"]
heap_flags = number(tables + 6, 1)
valid = number(tables + 8, 8)
position = tables + 24
counts = [0] * 64
for table in range(64):
    if valid & (1 << table):
        counts[table] = number(position, 4)
        position += 4


def index_width(table):
    return 2 if counts[table] < 65536 else 4


def coded_width(tags, tables):
    return 2 if max(counts[t] for t in tables) < (1 << (16 - tags)) else 4


string_width = 4 if heap_flags & 1 else 2
guid_width = 4 if heap_flags & 2 else 2
blob_width = 4 if heap_flags & 4 else 2
widths = [
    2 + string_width + 3 * guid_width,  # Module
    coded_width(2, [0, 26, 35, 1]) + 2 * string_width,  # TypeRef
    4 + 2 * string_width + coded_width(2, [2, 1, 27]) + index_width(4) + index_width(6),
    index_width(4),  # FieldPtr
    2 + string_width + blob_width,  # Field
    index_width(6),  # MethodPtr
    8 + string_width + blob_width + index_width(8),  # MethodDef
]
starts = []
for table, width in enumerate(widths):
    starts.append(position)
    position += counts[table] * width
first_field_signature = number(starts[4] + 2 + string_width, blob_width)
last_method_signature_offset = starts[6] + (counts[6] - 1) * widths[6] + 8 + string_width
print(json.dumps({
    "oracle": "Independent PE/CLI table-header decoder (metadata-oracle.py)",
    "provenance": provenance,
    "counts": {"classes": counts[2], "fields": counts[4], "methods": counts[6]},
    "mutation": {
        "last_method_signature_index_offset": last_method_signature_offset,
        "blob_index_width": blob_width,
        "first_field_signature_index": first_field_signature,
    },
}, indent=2))
