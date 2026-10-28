#!/usr/bin/env python3
"""Generate the dynamic ABC opcode table from the hash-pinned official ISA."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'crates/rxd-ark/resources/isa.yaml'
OUTPUT = ROOT / 'crates/rxd-ark/src/opcodes.rs'
SHA256 = '690d960793e07d18c94d81a644e916592bb29962c48c7e2f5f48376989f530f9'


def parse(text):
    prefixes = {}
    groups = []
    current = None
    group = None
    prefix = None
    section = None
    properties = False
    for line in text.splitlines():
        if line == 'prefixes:':
            section = 'prefixes'
        elif line == 'groups:':
            section = 'groups'
        elif section == 'prefixes':
            if line.startswith('  - name: '):
                prefix = line.split(': ', 1)[1]
            elif line.startswith('    opcode_idx: '):
                prefixes[prefix] = int(line.split(': ', 1)[1], 0)
        elif section == 'groups':
            if line.startswith('  - title: '):
                group = {'properties': [], 'instructions': []}
                groups.append(group)
                current = None
                properties = False
            elif line == '    properties:':
                properties = True
            elif line.startswith('    ') and not line.startswith('      '):
                properties = False
            elif properties and line.startswith('      - '):
                group['properties'].append(line.strip()[2:])
            elif line.startswith('      - sig: '):
                current = {'sig': line.split(': ', 1)[1]}
                group['instructions'].append(current)
            elif current is not None:
                match = re.fullmatch(r'        (opcode_idx|format|properties): \[(.*)\]', line)
                if match:
                    key, value = match.groups()
                    current[key] = [part.strip() for part in value.split(',') if part.strip()]
                elif line.startswith('        prefix: ') or line.startswith('        acc: '):
                    key, value = line.strip().split(': ', 1)
                    current[key] = value
    assert prefixes and groups
    return prefixes, groups


def generate(source):
    raw = source.read_bytes()
    assert hashlib.sha256(raw).hexdigest() == SHA256, 'ISA pin changed; review the source and update the generator explicitly'
    text = raw.decode()
    assert re.search(r'^version: 24\.0\.0\.0$', text, re.M)
    prefixes, groups = parse(text)
    entries = []
    seen = set()
    for group in groups:
        for instruction in group['instructions']:
            signature = instruction['sig'].split(None, 1)
            name = signature[0]
            arguments = signature[1].split(',') if len(signature) == 2 else []
            properties = set(group['properties']) | set(instruction.get('properties', []))
            flags = int('jump' in properties) | (int('conditional' in properties) << 1) | (int('return' in properties) << 2)
            acc = instruction.get('acc', '').split(':', 1)[0]
            assert acc in ['', 'none', 'in', 'out', 'inout'], (name, acc)
            flags |= (int(acc in ['in', 'inout']) << 3) | (int(acc in ['out', 'inout']) << 4)
            assert len(instruction['opcode_idx']) == len(instruction['format'])
            for encoded, form in zip(instruction['opcode_idx'], instruction['format']):
                code = int(encoded, 0)
                size = 1
                if 'prefix' in instruction:
                    code = prefixes[instruction['prefix']] | (code << 8)
                    size = 2
                    assert form.startswith('pref_')
                    form_args = form[len('pref_'):]
                else:
                    form_args = form
                assert form_args.startswith('op_'), (name, form)
                parts = form_args[len('op_'):].split('_')
                if parts == ['none']:
                    parts = []
                assert len(parts) == len(arguments) * 2, (name, form, arguments)
                params = []
                bits = 0
                for index, argument in enumerate(arguments):
                    argument = argument.strip().split(':')
                    field, width = parts[index * 2:index * 2 + 2]
                    width = int(width)
                    assert width in [4, 8, 16, 32, 64]
                    if argument[0].startswith('v'):
                        assert field.startswith('v')
                        kind = 'OperandType::Register'
                    elif argument[0].startswith('imm'):
                        assert field.startswith('imm')
                        encoding = argument[1]
                        assert re.fullmatch(r'[iuf](8|16|32|64)', encoding), (name, argument)
                        kind = {'i': 'Signed', 'u': 'Unsigned', 'f': 'Float'}[encoding[0]]
                        kind = 'OperandType::' + kind
                        assert encoding[0] != 'f' or width == 64
                    else:
                        assert field.startswith('id')
                        entity = {'string_id':'String', 'method_id':'Method', 'literalarray_id':'Literal',
                                  'type_id':'Type', 'field_id':'Field'}[argument[0]]
                        kind = f'OperandType::Id(EntityKind::{entity})'
                    params.append(f'Parameter {{ kind: {kind}, bits: {width} }}')
                    bits += width
                assert bits % 8 == 0
                assert code not in seen, f'duplicate opcode {code:#x}'
                seen.add(code)
                entries.append((code, f'Opcode {{ code: {code}, name: {json.dumps(name)}, format: {json.dumps(form)}, '
                    f'size: {size + bits // 8}, flags: {flags}, parameters: &[{", ".join(params)}], '
                    f'properties: &[{", ".join(json.dumps(prop) for prop in sorted(properties))}] }}'))
    result = '// Generated by scripts/gen-abc-opcodes.py from the pinned Huawei ISA (Apache-2.0).\n'
    result += '// Copyright (c) 2021-2022 Huawei Device Co., Ltd.; see REFERENCE.md.\n'
    result += 'use super::{EntityKind, Opcode, OperandType, Parameter};\n'
    values = sorted(prefixes.values())
    pattern = f'{values[0]}..={values[-1]}' if values == list(range(values[0], values[-1] + 1)) else ' | '.join(str(value) for value in values)
    result += 'pub(super) fn is_prefix(byte: u8) -> bool { matches!(byte, ' + pattern + ') }\n'
    result += 'pub(super) const OPCODES: &[Opcode] = &[\n'
    result += ',\n'.join(item for _, item in sorted(entries)) + ',\n];\n'
    result = subprocess.check_output(['rustfmt', '--edition', '2024'], input=result, text=True)
    return result, len(entries)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    result, count = generate(SOURCE)
    if args.check:
        assert OUTPUT.read_text() == result, 'generated opcode table differs; rerun the generator'
    else:
        OUTPUT.write_text(result)
    print(f'{count} dynamic ABC opcode encodings: {OUTPUT.relative_to(ROOT)}')


if __name__ == '__main__':
    main()
