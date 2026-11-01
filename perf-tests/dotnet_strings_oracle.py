#!/usr/bin/env python3
"""Prepare string expectations from raw heaps and trusted PEReader IL sites; never use rxd output."""
import argparse
import collections
import hashlib
import json
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[1]


def canonical_hash(value):
    return hashlib.sha256(json.dumps(value, ensure_ascii=True, separators=(',', ':')).encode()).hexdigest()


def heap_entries(image):
    read16 = lambda at: struct.unpack_from('<H', image, at)[0]
    read32 = lambda at: struct.unpack_from('<I', image, at)[0]
    pe = read32(0x3c)
    optional = pe + 24
    sections = optional + read16(pe + 20)

    def file_offset(rva):
        for index in range(read16(pe + 6)):
            row = sections + index * 40
            start, size, raw = read32(row + 12), read32(row + 16), read32(row + 20)
            if start <= rva < start + size:
                return raw + rva - start
        raise ValueError('RVA is outside mapped raw bytes')

    directory = optional + (96 if read16(optional) == 0x10b else 112)
    cli = file_offset(read32(directory + 14 * 8))
    metadata = file_offset(read32(cli + 8))
    assert read32(metadata) == 0x424a5342
    pos = metadata + 16 + read32(metadata + 12)
    count = read16(pos + 2)
    pos += 4
    heap = None
    for _ in range(count):
        offset, size = read32(pos), read32(pos + 4)
        end = image.index(0, pos + 8)
        name = image[pos + 8:end].decode('ascii')
        if name == '#US':
            heap = image[metadata + offset:metadata + offset + size]
        pos = (end + 4) // 4 * 4
    if heap is None:
        return []
    assert heap[0] == 0
    pos = 1
    entries = []
    while pos < len(heap):
        offset, first = pos, heap[pos]
        pos += 1
        if first < 128:
            length = first
        elif first < 192:
            length = ((first & 63) << 8) | heap[pos]
            pos += 1
        else:
            assert first < 224
            length = ((first & 31) << 24) | int.from_bytes(heap[pos:pos + 3], 'big')
            pos += 3
        if length == 0:
            continue
        assert length % 2 == 1 and pos + length <= len(heap) and heap[pos + length - 1] <= 1
        raw = heap[pos:pos + length - 1]
        try:
            value, units = raw.decode('utf-16-le'), None
        except UnicodeDecodeError:
            value, units = None, list(struct.unpack('<' + 'H' * (len(raw) // 2), raw))
        entries.append((offset, 0x70000000 | offset, value, units))
        pos += length
    return entries


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--metadata-oracle', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    image = args.input.read_bytes()
    digest = hashlib.sha256(image).hexdigest()
    oracle = json.loads(args.metadata_oracle.read_text())
    assert oracle['sha256'] == digest
    entries = heap_entries(image)
    owners = {method: ty['token'] for ty in oracle['types'] for method in ty['methods']}
    sites = [site for site in oracle['sites'] if site['opcode'] == 'ldstr']
    grouped = collections.defaultdict(lambda: collections.defaultdict(list))
    for site in sites:
        grouped[site['target_token']][site['method_token']].append(site['il_offset'])
    groups = []
    for offset, token, value, units in entries:
        if grouped[token]:
            for method, offsets in sorted(grouped[token].items()):
                groups.append([offset, token, value, units, method, owners[method], sorted(offsets)])
        else:
            groups.append([offset, token, value, units, None, None, []])
    # Cross-check the two independent sources before publishing expectations.
    values = {token: value for _, token, value, _ in entries}
    assert all(values[site['target_token']] == site['string_value'] for site in sites)
    workloads = []
    for name, contains, exact in [('unfiltered', None, None), ('unexpected', 'Unexpected', None),
                                  ('url', 'http://james.newtonking.com/projects/json', None),
                                  ('direct-state', None, 'Unexpected state: {0}.')]:
        if exact is not None:
            selected = [[site['method_token'], site['il_offset'], site['target_token'], owners[site['method_token']]]
                        for site in sites if site['string_value'] == exact]
            selected.sort()
        else:
            selected = [group for group in groups if contains is None or (group[2] is not None and contains in group[2])]
        workloads.append({'name': name, 'contains': contains, 'string': exact,
                          'records': len(selected), 'canonical_sha256': canonical_hash(selected),
                          'uses': len(selected) if exact is not None else sum(len(group[-1]) for group in selected)})
    provenance = json.loads((ROOT / 'tests/corpus/dotnet/class-expectations.json').read_text())['provenance']
    assert provenance['dll_sha256'] == digest
    report = {'provenance': provenance, 'oracle': oracle['oracle'] + '; #US independently decoded from original PE bytes',
              'metadata_oracle_sha256': hashlib.sha256(args.metadata_oracle.read_bytes()).hexdigest(),
              'heap_entries': len(entries), 'load_sites': len(sites), 'workloads': workloads}
    args.output.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
