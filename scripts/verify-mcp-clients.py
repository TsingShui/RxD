#!/usr/bin/env python3
"""Optional actual-client MCP acceptance. Never installs clients or calls a model.

Requires explicit existing client executables. Evidence stays outside source; every
client receives a fresh HOME, no inherited credentials and an unrelated launch cwd.
An unavailable client fails this invocation; it is not a skipped-as-passed check.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
from input_identity import input_identity
import subprocess
import signal
import queue
import threading
import time

from mcp_client import Client, structured


SOURCE = b'function probe() { return "native MCP \\u03bb"; }\r\n'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(argv, env, cwd, log):
    process = subprocess.Popen([str(a) for a in argv], env=env, cwd=cwd,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    timed_out = False
    try:
        stdout, stderr = process.communicate(timeout=90)
    except subprocess.TimeoutExpired:
        timed_out = True
        # Reap only this probe's own process group, including a stuck stdio child.
        os.killpg(process.pid, signal.SIGKILL)
        stdout, stderr = process.communicate()
    log.with_suffix('.stdout').write_bytes(stdout)
    log.with_suffix('.stderr').write_bytes(stderr)
    log.with_suffix('.command.json').write_text(json.dumps({
        'argv': [str(a) for a in argv], 'cwd': str(cwd), 'timed_out': timed_out,
        'environment': env, 'exit': process.returncode}, indent=2) + '\n')
    if process.returncode or timed_out:
        raise RuntimeError(f'{log.name}: exit {process.returncode}, timeout={timed_out}; inspect evidence')
    return stdout.decode()


def check_query(result):
    assert not result.get('isError'), result
    value = result['structuredContent']
    assert value['header']['source_id'] == input_identity(SOURCE), value
    assert value['summary']['complete'], value
    assert len(value['records']) == 1, value
    assert value['records'][0]['detail']['value'] == 'native MCP λ', value


def check_denial(result):
    assert result['isError'], result
    assert result['structuredContent']['error']['code'] == 'ACCESS_DENIED', result


def recorded_query(entry, env, cwd, source, outside, directory):
    # Existing stdio driver prepends exactly mcp/serve; retain every recorded arg.
    assert entry['args'][:2] == ['mcp', 'serve'], entry
    with (directory / 'recorded.stderr').open('wb') as stderr:
        client = Client(entry['command'], [], stderr, env=env, cwd=cwd,
                        options=entry['args'][2:])
        frames = []
        try:
            frames.append(client.initialize()[0])
            catalog = client.request('tools/list', {})[0]
            frames.append(catalog)
            names = {t['name'] for t in catalog['result']['tools']}
            assert {'js_find', 'rxd_open', 'rxd_close'} <= names, names
            assert all(name.startswith('js_') or name in {'rxd_open', 'rxd_close'} for name in names), names
            found = client.tool('js_find', {'input': {'path': source.name},
                                            'string': 'native MCP'})[0]
            frames.append(found)
            structured(found)  # Also verifies the compatibility text copy.
            check_query(found['result'])
            denied = client.tool('js_find', {'input': {'path': str(outside)},
                                             'string': 'native MCP'})[0]
            frames.append(denied)
            check_denial(denied['result'])
        finally:
            (directory / 'recorded.frames.json').write_text(json.dumps(frames, indent=2) + '\n')
            client.close()


def codex_query(executable, env, cwd, source, outside, directory):
    """Pinned app-server no-turn route, never a model/assistant interaction."""
    allowed = {'initialize', 'thread/start', 'mcpServerStatus/list', 'mcpServer/tool/call'}
    frames = []
    responses = queue.Queue()
    with (directory / 'app-server.stderr').open('wb') as stderr:
        argv = [str(executable), 'app-server', '--stdio']
        process = subprocess.Popen(argv, env=env, cwd=cwd, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=stderr, start_new_session=True)
        def read():
            try:
                while True:
                    raw = process.stdout.readline(16 * 1024 * 1024 + 1)
                    if not raw:
                        raise EOFError('Codex app-server stdout ended')
                    if len(raw) > 16 * 1024 * 1024:
                        raise RuntimeError('Codex frame capture bound exceeded')
                    responses.put(json.loads(raw))
            except Exception as error:
                responses.put(error)
        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        serial = 0
        def send(packet):
            frames.append({'sent': packet})
            process.stdin.write(json.dumps(packet).encode() + b'\n')
            process.stdin.flush()
        def request(method, params):
            nonlocal serial
            assert method in allowed, 'inference/approval routes are forbidden'
            serial += 1
            send({'id': serial, 'method': method, 'params': params})
            deadline = time.monotonic() + 45
            while True:
                packet = responses.get(timeout=max(0.01, deadline - time.monotonic()))
                if isinstance(packet, Exception):
                    raise packet
                frames.append({'received': packet})
                if 'method' in packet and 'id' in packet:
                    raise RuntimeError(f'Unanswered native approval/elicitation request: {packet}')
                if packet.get('id') == serial:
                    if 'error' in packet:
                        raise RuntimeError(packet['error'])
                    return packet['result']
                if time.monotonic() >= deadline:
                    raise TimeoutError('Codex app-server response deadline exceeded')
        try:
            request('initialize', {'clientInfo': {'name': 'rxd_native_verifier', 'version': '1'},
                                   'capabilities': {'experimentalApi': True}})
            send({'method': 'initialized'})
            thread = request('thread/start', {'cwd': str(cwd), 'ephemeral': True})['thread']['id']
            status = request('mcpServerStatus/list', {'threadId': thread, 'serverName': 'rxd', 'detail': 'full'})
            servers = status['data']
            assert len(servers) == 1 and servers[0]['name'] == 'rxd', status
            assert servers[0]['runtimeStatus'] == 'connected', status
            assert 'js_find' in servers[0]['tools'], status
            found = request('mcpServer/tool/call', {'threadId': thread, 'server': 'rxd', 'tool': 'js_find',
                            'arguments': {'input': {'path': source.name}, 'string': 'native MCP'}})
            check_query(found)
            denied = request('mcpServer/tool/call', {'threadId': thread, 'server': 'rxd', 'tool': 'js_find',
                             'arguments': {'input': {'path': str(outside)}, 'string': 'native MCP'}})
            check_denial(denied)
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=10)
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()
                process.stdout.close()
                reader.join(timeout=1)
                (directory / 'app-server.frames.json').write_text(json.dumps(frames, indent=2) + '\n')
                (directory / 'app-server.command.json').write_text(json.dumps({
                    'argv': argv, 'cwd': str(cwd), 'environment': env,
                    'exit': process.returncode}, indent=2) + '\n')
            assert process.returncode == 0, f'Codex app-server exit {process.returncode}'


def environment(directory):
    home = directory / 'home'
    home.mkdir()
    # Deliberate allowlist: not a copy of os.environ. No provider keys, proxies,
    # account stores, user extensions, shared daemons or live config directories.
    return {'HOME': str(home), 'USERPROFILE': str(home),
            'PATH': '/usr/bin:/bin', 'LANG': 'en_US.UTF-8', 'TMPDIR': str(directory),
            'XDG_CONFIG_HOME': str(home / '.config'),
            'XDG_CACHE_HOME': str(home / '.cache'),
            'RXD_CACHE_DIR': str(directory / 'engine-cache'),
            'PI_CODING_AGENT_DIR': str(home / 'pi'), 'PI_OFFLINE': '1',
            'PI_SKIP_VERSION_CHECK': '1', 'PI_TELEMETRY': '0',
            'CLAUDE_CONFIG_DIR': str(home / 'cc'),
            'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC': '1',
            'DISABLE_AUTOUPDATER': '1', 'DISABLE_TELEMETRY': '1',
            'DISABLE_ERROR_REPORTING': '1',
            'CODEX_HOME': str(home / 'codex'), 'OTEL_SDK_DISABLED': 'true'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--client', choices=['pi', 'cc', 'codex'], action='append', required=True)
    parser.add_argument('--claude', type=Path)
    parser.add_argument('--pi', type=Path)
    parser.add_argument('--pi-package', type=Path, help='existing @earendil-works/pi-coding-agent directory')
    parser.add_argument('--node', type=Path)
    parser.add_argument('--codex', type=Path)
    parser.add_argument('--output', type=Path, required=True, help='new evidence directory (must not exist)')
    args = parser.parse_args()
    clients = sorted(set(args.client))
    required = {'cc': [args.claude], 'pi': [args.pi, args.pi_package, args.node],
                'codex': [args.codex]}
    for client in clients:
        if any(path is None or not path.exists() for path in required[client]):
            parser.error(f'{client}: explicit existing client paths required; unavailable is not passed')
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    binary = args.output / 'relocated bin λ' / 'rxd'
    binary.parent.mkdir()
    shutil.copy2(args.binary.resolve(), binary)
    report = {'binary_sha256': digest(binary), 'source_sha256': hashlib.sha256(SOURCE).hexdigest(),
              'verifier_sha256': {p.name: digest(p) for p in
                                  [Path(__file__), Path(__file__).with_name('verify-pi-mcp-session.mjs'),
                                   Path(__file__).with_name('mcp_client.py')]},
              'binary_source': str(args.binary.resolve()),
              'platform': os.uname().sysname + ' ' + os.uname().machine,
              'routes': {}, 'status': 'failed'}
    try:
        for client in clients:
            directory = args.output / client
            directory.mkdir()
            env = environment(directory)
            if client == 'pi':
                env['PATH'] = str(args.node.resolve().parent) + os.pathsep + env['PATH']
            analysis = directory / 'analysis λ'
            cwd = directory / 'unrelated-cwd'
            analysis.mkdir()
            cwd.mkdir()
            source = analysis / 'probe λ.js'
            source.write_bytes(SOURCE)
            outside = directory / 'outside.js'
            outside.write_bytes(SOURCE)
            executable = {'cc': args.claude, 'pi': args.pi, 'codex': args.codex}[client].resolve()
            version = run([executable, '--version'], env, cwd, directory / 'version').strip()
            report['routes'][client] = {'version': version, 'executable': str(executable),
                                        'sha256': digest(executable), 'status': 'failed'}
            output = run([binary, 'mcp', 'install', '-a', client, '--base', analysis,
                          '--root', analysis, '--backends', 'js'], env, cwd, directory / 'install')
            target = json.loads(output)['targets'][0]
            assert target['outcome'] == 'installed', target
            config = Path(target['path'])
            # CC/Pi native JSON, independent of the installer's printed registration.
            (directory / 'generated-config.txt').write_bytes(config.read_bytes())
            report['routes'][client]['generated_config_sha256'] = digest(config)
            if client != 'codex':
                entry = json.loads(config.read_text())['mcpServers']['rxd']
                recorded_query(entry, env, cwd, source, outside, directory)
            if client == 'cc':
                got = run([executable, 'mcp', 'get', 'rxd'], env, cwd, directory / 'native-get')
                listed = run([executable, 'mcp', 'list'], env, cwd, directory / 'native-list')
                assert 'Connected' in got and str(binary) in got, got
                assert 'rxd:' in listed and 'Connected' in listed, listed
                report['routes'][client]['route'] = 'native get/list health + separate recorded-argv MCP query/denial; no CC session/model'
            elif client == 'pi':
                package = args.pi_package.resolve()
                metadata = json.loads((package / 'package.json').read_text())
                assert metadata['version'] == version, (metadata['version'], version)
                report['routes'][client]['sdk'] = {
                    'package': str(package), 'version': metadata['version'],
                    'entry_sha256': digest(package / 'dist/index.js'),
                    'cli_sha256': digest(package / 'dist/cli.js'),
                    'node_sha256': digest(args.node.resolve())}
                session_probe = Path(__file__).with_name('verify-pi-mcp-session.mjs')
                scenarios = ['enabled', 'builtin-disabled', 'override', 'server-disabled']
                for scenario in scenarios:
                    agent = Path(env['PI_CODING_AGENT_DIR'])
                    settings = agent / 'settings.json'
                    extension = agent / 'extensions/replacement-mcp.js'
                    if scenario == 'builtin-disabled':
                        settings.write_text('{"extensions":["-builtin:mcp"]}\n')
                    elif scenario == 'override':
                        settings.unlink()
                        extension.parent.mkdir()
                        extension.write_text('export default function(pi) { pi.registerCommand("mcp", '
                                             '{description:"isolated override fixture", handler:async()=>{}}); }\n')
                    elif scenario == 'server-disabled':
                        extension.unlink()
                        value = json.loads(config.read_text())
                        value['mcpServers']['rxd']['enabled'] = False
                        config.write_text(json.dumps(value) + '\n')
                    before = config.read_bytes()
                    repeat = json.loads(run([binary, 'mcp', 'install', '-a', 'pi'], env, cwd,
                                            directory / (scenario + '-install')))
                    assert repeat['targets'][0]['outcome'] == 'unchanged', repeat
                    assert config.read_bytes() == before
                    listed = json.loads(run([executable, 'mcp', 'list', '--json'], env, cwd,
                                            directory / (scenario + '-native-list')))
                    assert listed['errors'] == [], listed
                    assert len(listed['servers']) == 1, listed
                    expected = 'disabled' if scenario == 'server-disabled' else 'connected'
                    assert listed['servers'][0]['state'] == expected, listed
                    if scenario == 'builtin-disabled':
                        assert 'native_mcp_disabled' in repeat['targets'][0]['warnings'], repeat
                    (directory / (scenario + '-list.json')).write_text(json.dumps(listed, indent=2) + '\n')
                    run([args.node.resolve(), session_probe, package, source.name, outside, scenario,
                         directory / (scenario + '-session.json')], env, cwd,
                        directory / (scenario + '-session'))
                    assert config.read_bytes() == before, 'session probe rewrote registration'
                report['routes'][client]['route'] = 'native list + authored-history replay through SDK session-wrapped codemode/nested MCP query/denial, hooks and parent IDs; isolated disabled/override cases; not model-issued use'
            elif client == 'codex':
                got = json.loads(run([executable, 'mcp', 'get', 'rxd', '--json'], env, cwd,
                                     directory / 'native-get'))
                listed = json.loads(run([executable, 'mcp', 'list', '--json'], env, cwd,
                                        directory / 'native-list'))
                entry = got['transport']
                assert entry['command'] == str(binary) and got['enabled'], got
                assert any(item['name'] == 'rxd' for item in listed), listed
                recorded_query(entry, env, cwd, source, outside, directory)
                # A non-authenticated loopback-only provider prevents catalog traffic
                # from reaching a hosted model. No turn/start or provider request.
                with config.open('a') as stream:
                    stream.write('\n[model_providers.rxd_verification]\nname = "No inference"\n'
                                 'base_url = "http://127.0.0.1:9/v1"\nwire_api = "responses"\n'
                                 'requires_openai_auth = false\n')
                config.write_text('model_provider = "rxd_verification"\nmodel = "no-inference"\n' + config.read_text())
                before = config.read_bytes()
                codex_query(executable, env, cwd, source, outside, directory)
                assert config.read_bytes() == before, 'native probe rewrote configuration'
                report['routes'][client]['route'] = 'native get/list + app-server thread/start, MCP status/tool calls; no turn/start or inference'

            assert not Path(env['RXD_CACHE_DIR']).exists(), 'installer/query initialized native engines'
            assert not list(Path(env['HOME']).rglob('trust.json')), 'probe changed project trust'
            report['routes'][client]['config_sha256'] = digest(config)
            report['routes'][client]['status'] = 'passed'
        report['status'] = 'passed'
    finally:
        (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
