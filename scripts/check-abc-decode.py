#!/usr/bin/env python3
"""Compare every method/instruction/operand against the pinned xpanda decode oracle."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import struct


def utf16(units):
    return struct.pack('<' + 'H' * len(units), *units).decode('utf-16-le', errors='surrogatepass')


def string_at(data, offset):
    cursor, encoded = offset, 0
    for shift in range(0, 35, 7):
        byte = data[cursor]
        cursor += 1
        encoded |= (byte & 127) << shift
        if byte < 128:
            break
    else:
        raise AssertionError('unterminated string length')
    payload = data[cursor:data.index(0, cursor)].replace(b'\xc0\x80', b'\0')
    units = payload.decode('utf-8', errors='surrogatepass').encode('utf-16-le', errors='surrogatepass')
    assert len(units) // 2 == encoded >> 1
    return units.decode('utf-16-le', errors='surrogatepass')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reference', type=Path, required=True)
    parser.add_argument('--actual', type=Path, required=True)
    parser.add_argument('--abc', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    reference = json.loads((args.reference / 'summary.json').read_text())
    assert reference['stage'] == 'decode' and reference['selectedRecord'] is None
    assert not reference['errors']
    assert reference['reference']['revision'] == '8f27d3ce8b63581c7eb5203bbfdfbb92e28cfccf'
    data = args.abc.read_bytes()
    source_id = hashlib.sha256(data).hexdigest()
    assert source_id == reference['inputSha256'] == 'a2046b75ebd368e0599a50b2b5e5c6dab8070007364af5a09d67d267582c197c'
    records = json.loads((args.reference / 'records.json').read_text())
    errors = []
    corrected_strings = {}
    methods, instructions, operands = 0, 0, 0
    expected_files = {f"{record['offset']:x}.methods.json" for record in records}
    assert {path.name for path in args.actual.glob('*.methods.json')} == expected_files
    for record in records:
        stem = f"{record['offset']:x}"
        expected = json.loads(gzip.decompress((args.reference / 'records' / (stem + '.methods.json.gz')).read_bytes()))
        actual = json.loads((args.actual / (stem + '.methods.json')).read_text())
        assert [method['offset'] for method in expected] == [method['offset'] for method in actual], stem
        for expected, actual in zip(expected, actual):
            methods += 1
            for key in ['name', 'vregs', 'args', 'codeSize', 'codeHex', 'tries']:
                if expected[key] != actual[key]:
                    errors.append({'method': actual['offset'], 'field': key, 'expected': expected[key], 'actual': actual[key]})
            assert len(expected['instructions']) == len(actual['instructions']), actual['offset']
            for left, right in zip(expected['instructions'], actual['instructions']):
                instructions += 1
                for key in ['pc', 'size', 'opcode', 'name']:
                    if left[key] != right[key]:
                        errors.append({'method': actual['offset'], 'pc': right['pc'], 'field': key, 'expected': left[key], 'actual': right[key]})
                assert len(left['params']) == len(right['params'])
                for index, (old, new) in enumerate(zip(left['params'], right['params'])):
                    operands += 1
                    if 'units' in new:
                        new = dict(new)
                        new['value'] = utf16(new.pop('units'))
                    if old == new:
                        continue
                    old_meta = {key:value for key,value in old.items() if key != 'value'}
                    new_meta = {key:value for key,value in new.items() if key != 'value'}
                    if old_meta == new_meta and new.get('type') == 'STRING' and new['value'] == string_at(data, new['offset']):
                        corrected = corrected_strings.setdefault(new['offset'], {'references':0,'actual':new['value'],'reference':old['value']})
                        corrected['references'] += 1
                    else:
                        errors.append({'method':actual['offset'],'pc':right['pc'],'operand':index,'expected':old,'actual':new})
    assert methods == reference['methods']
    assert instructions == reference['instructions']
    report = {'source_id':source_id,'reference_revision':reference['reference']['revision'],
              'records':len(records),'methods':methods,'instructions':instructions,'operands':operands,
              'unexplained_differences':errors,'reference_mutf8_differences':corrected_strings,
              'scope':'Complete static decode comparison, not source reconstruction equivalence'}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps({key:value for key,value in report.items() if key != 'reference_mutf8_differences'},indent=2))
    print('Source-validated reference MUTF8 differences:',len(corrected_strings))
    if errors:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
