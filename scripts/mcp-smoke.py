#!/usr/bin/env python3
"""Verify a relocated executable through real MCP stdio, without input execution."""
import argparse
import hashlib
import os
from pathlib import Path
import tempfile
from input_identity import input_id
from mcp_client import Client, structured


def verify(binary, directory, env, expect_native=False):
    source = directory / 'mcp-probe.js'
    original = b'function probe() { return "MCP \xce\xbb"; }\r\n'
    source.write_bytes(original)
    with (directory / 'mcp-smoke.stderr').open('wb') as stderr:
        client = Client(binary, [directory], stderr, env=env, cwd=directory)
        try:
            client.initialize()
            packet, _, _ = client.request('tools/list', {})
            names = {tool['name'] for tool in packet['result']['tools']}
            assert {'rxd_open', 'rxd_close', 'js_find', 'js_get', 'evm_analyze'} <= names
            assert ('kuna_functions' in names) == expect_native, names
            opened = structured(client.tool('rxd_open', {'path': str(source)})[0])
            assert opened['source_id'] == input_id(original)
            source.write_bytes(b'changed source')
            selected = {'input': {'target_id': opened['target_id']}, 'string': 'MCP'}
            found = structured(client.tool('js_find', selected)[0])
            assert found['summary']['complete'] and len(found['records']) == 1, found
            record = found['records'][0]
            assert record['detail']['value'] == 'MCP \u03bb', record
            fetched = structured(client.tool('js_get', {
                'input': selected['input'], 'at': record['context_at']})[0])
            assert fetched['detail']['source'].encode() == original[fetched['start']:fetched['end']], fetched
            outlined = structured(client.tool('js_entities', {'input': selected['input']})[0])
            root = outlined['header']['detail']['root_id']
            fetched = structured(client.tool('js_get', {'input': selected['input'], 'at': root})[0])
            assert fetched['detail']['source'].encode() == original, fetched
            closed = structured(client.tool('rxd_close', {'target_id': opened['target_id']})[0])
            assert closed['target_id'] == opened['target_id'], closed
            failed = client.tool('js_find', selected)[0]['result']
            assert failed['isError'] and failed['structuredContent']['error']['code'] == 'TARGET_INVALID', failed
        finally:
            client.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--expect-native', action='store_true')
    args = parser.parse_args()
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(('RXD_', 'KUNA_', 'DOTNET_')) and key != 'SLEIGHHOME'}
    with tempfile.TemporaryDirectory(prefix='rxd-mcp-smoke-') as temporary:
        directory = Path(temporary)
        env['RXD_CACHE_DIR'] = str(directory / 'cache')
        verify(args.binary.resolve(), directory, env, args.expect_native)
        assert not (directory / 'cache/rxd-ilspy').exists()
        assert not (directory / 'cache/rxd-kuna').exists()
    print('Relocated MCP stdio, snapshot/FETCH, closed target and lazy engines verified.')


if __name__ == '__main__':
    main()
