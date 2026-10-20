#!/usr/bin/env python3
"""PYC oracle helpers for other runners: name-hit classification and FETCH checks.

The PYC measurement runner that used to own these was removed (see
`benches/README.md`); the helpers stay because the MCP runner still validates PYC
navigation against the same pinned oracle. They read saved expectations and never
execute analyzed input.
"""
OPERATIONS = {
    'LOAD_NAME': 'name_read', 'STORE_NAME': 'name_write', 'DELETE_NAME': 'name_delete',
    'LOAD_GLOBAL': 'global_read', 'STORE_GLOBAL': 'global_write', 'DELETE_GLOBAL': 'global_delete',
    'LOAD_FAST': 'local_read', 'LOAD_FAST_CHECK': 'local_read', 'STORE_FAST': 'local_write',
    'DELETE_FAST': 'local_delete', 'LOAD_FAST_AND_CLEAR': 'local_read_clear',
    'MAKE_CELL': 'cell_create', 'LOAD_CLOSURE': 'closure_capture',
    'LOAD_DEREF': 'closure_read', 'STORE_DEREF': 'closure_write', 'DELETE_DEREF': 'closure_delete',
    'LOAD_CLASSDEREF': 'class_closure_read', 'LOAD_ATTR': 'attribute_read',
    'STORE_ATTR': 'attribute_write', 'DELETE_ATTR': 'attribute_delete',
    'LOAD_FROM_DICT_OR_GLOBALS': 'dict_or_global_read', 'LOAD_FROM_DICT_OR_DEREF': 'dict_or_closure_read',
    'LOAD_FAST_LOAD_FAST': 'local_read', 'STORE_FAST_STORE_FAST': 'local_write',
}


def names(ins):
    value = ins['argval']
    return [item['value'] for item in value['items']] if value['type'] == 'tuple' else [value['value']]


def name_hits(oracle, name):
    hits = []
    for owner, code in enumerate(oracle['codes']):
        for ins in code['dis_instructions']:
            operation = OPERATIONS.get(ins['opname'])
            if operation is None and ins['opname'] != 'STORE_FAST_LOAD_FAST':
                continue
            for position, value in enumerate(names(ins)):
                if value == name:
                    category = operation if operation else ['local_write', 'local_read'][position]
                    hits.append((owner, ins['offset'], ins['opcode'], ins['arg'], category, position))
    return hits


def validate_view(view, code, ranges):
    detail = view['detail']
    assert detail['complete'] is True
    assert detail['bytecode_hex'] == code['code_hex']
    assert view['start'] == ranges['pyc_marshal_start'] and view['end'] == ranges['pyc_marshal_end']
    units = {unit['offset']: unit for unit in detail['instructions']}
    assert len(units) == len(code['code_hex'])//4
    for ins in code['dis_instructions']:
        unit = units[ins['offset']]
        assert (unit['opcode'], unit['name'], unit['raw_operand']) == (ins['opcode'], ins['opname'], ins['raw_arg_byte'])
        if unit['role'] == 'instruction':
            assert unit['operand'] == ins['arg'], (code['name'], ins)
            if unit['jump_target'] is not None:
                assert unit['jump_target'] == ins['argval']['value']
            if unit['targets'] and unit['targets'][0]['table'] != 'consts':
                values = {value['id']: value['value'] for value in detail['values']}
                assert [values[target['value_id']] for target in unit['targets']] == names(ins)
    for cache in code['cache_units']:
        unit = units[cache['offset']]
        assert (unit['role'], unit['owner_offset'], unit['raw_hex']) == ('cache', cache['owner_offset'], cache['raw_hex'])
    for table in ['consts', 'names', 'varnames', 'freevars', 'cellvars']:
        assert len(detail['tables'][table]['entries']) == len(code[table])
    exceptions = [{key: entry[key] for key in ['start', 'end', 'target', 'depth', 'lasti']} for entry in detail['exceptions']]
    assert exceptions == code['exceptions']
    for offset, unit in units.items():
        if code['positions'] is not None:
            location = next(entry for entry in detail['line_mapping'] if entry['start'] <= offset < entry['end'])
            assert [location[key] for key in ['line', 'end_line', 'column', 'end_column']] == code['positions'][offset//2]
        elif code['line_ranges'] is not None:
            line = next(line for start, end, line in code['line_ranges'] if start <= offset < end)
            assert unit['line'] == line
        else:
            line = next(line for start, line in reversed(code['line_starts']) if start <= offset)
            assert unit['line'] == line
