#!/usr/bin/env python3
"""Verify a relocated executable through real MCP stdio, without input execution."""
import argparse
import hashlib
import os
from pathlib import Path
import tempfile
from mcp_client import Client, structured


def verify(binary, directory, env, expect_native=False):
    source = directory / 'mcp-probe.zip'
    original = b'MCP \xce\xbb original bytes'
    import zipfile
    with zipfile.ZipFile(source, 'w') as archive:
        archive.writestr('probe.txt', original)
    with (directory / 'mcp-smoke.stderr').open('wb') as stderr:
        client = Client(binary, [directory], stderr, env=env, cwd=directory)
        try:
            client.initialize()
            packet, _, _ = client.request('tools/list', {})
            names = {tool['name'] for tool in packet['result']['tools']}
            assert {'rxd_open', 'rxd_close', 'zip_entries', 'zip_extract', 'evm_analyze'} <= names
            assert ('kuna_functions' in names) == expect_native, names
            opened = structured(client.tool('rxd_open', {
                'path': str(source), 'backend': 'zip'})[0])
            assert opened['source_id']
            source.write_bytes(b'changed container')
            selected = {'input': {'target_id': opened['target_id']}}
            listed = structured(client.tool('zip_entries', selected)[0])
            record = next(row for row in listed['records']
                          if row['detail'].get('path') == 'probe.txt')
            fetched = structured(client.tool('zip_extract', {
                'input': selected['input'], 'at': record['at'],
                'include_bytes': True})[0])
            from base64 import b64decode
            assert b64decode(fetched['bytes_base64']) == original
            closed = structured(client.tool('rxd_close', {
                'target_id': opened['target_id']})[0])
            assert closed['target_id'] == opened['target_id'], closed
            failed = client.tool('zip_entries', selected)[0]['result']
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
    print('Relocated MCP stdio, immutable snapshot extraction, closed target and lazy engines verified.')


if __name__ == '__main__':
    main()
