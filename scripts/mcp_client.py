"""Standard-library MCP stdio driver; read complete responses including compatibility text."""
import json
from pathlib import Path
import queue
import subprocess
import threading
import time


class Client:
    def __init__(self, binary, roots, stderr, *, env=None, cwd=None, options=(), timeout=35):
        self.timeout = timeout
        self.serial = 0
        self.pending = {}
        self.responses = queue.Queue()
        argv = [str(binary), 'mcp', 'serve']
        for root in roots:
            argv += ['--root', str(root)]
        argv += list(options)
        self.process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=stderr, env=env, cwd=cwd)
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.received_bytes = 0
        self.sent_bytes = 0

    def _read(self):
        try:
            while True:
                raw = self.process.stdout.readline(64 * 1024 * 1024 + 1)
                if not raw:
                    raise EOFError('MCP stdout ended before the expected response')
                if len(raw) > 64 * 1024 * 1024 or not raw.endswith(b'\n'):
                    raise RuntimeError('MCP response exceeded benchmark capture limit')
                packet = json.loads(raw)
                self.responses.put((time.perf_counter_ns(), raw, packet))
        except Exception as error:
            self.responses.put(error)

    def send(self, method, params):
        self.serial += 1
        packet = json.dumps({'jsonrpc': '2.0', 'id': self.serial, 'method': method,
                             'params': params}, ensure_ascii=True).encode() + b'\n'
        start = time.perf_counter_ns()
        self.process.stdin.write(packet)
        self.process.stdin.flush()
        self.sent_bytes += len(packet)
        return self.serial, start, len(packet)

    def receive(self, request):
        serial, start, sent = request
        deadline = time.monotonic() + self.timeout
        while serial not in self.pending:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError('MCP response deadline exceeded')
            try:
                item = self.responses.get(timeout=remaining)
            except queue.Empty as error:
                raise TimeoutError('MCP response deadline exceeded') from error
            if isinstance(item, Exception):
                raise item
            end, raw, packet = item
            self.received_bytes += len(raw)
            if 'id' in packet:
                self.pending[packet['id']] = (end, raw, packet)
        end, raw, packet = self.pending.pop(serial)
        return packet, {'duration_ns': end - start, 'request_bytes': sent,
                        'response_bytes': len(raw)}, raw

    def request(self, method, params):
        return self.receive(self.send(method, params))

    def notify(self, method, params=None):
        packet = {'jsonrpc': '2.0', 'method': method}
        if params is not None:
            packet['params'] = params
        raw = json.dumps(packet).encode() + b'\n'
        self.process.stdin.write(raw)
        self.process.stdin.flush()
        self.sent_bytes += len(raw)

    def initialize(self):
        packet, metrics, raw = self.request('initialize', {
            'protocolVersion': '2025-11-25', 'capabilities': {},
            'clientInfo': {'name': 'rxd-release-verifier', 'version': '1'}})
        if 'error' in packet:
            raise RuntimeError(packet['error'])
        self.notify('notifications/initialized')
        return packet, metrics, raw

    def tool(self, name, arguments):
        return self.request('tools/call', {'name': name, 'arguments': arguments})

    def close(self):
        if not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            status = self.process.wait(timeout=5)
            if status:
                raise RuntimeError(f'MCP server exited with {status}')
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait()
            self.process.stdout.close()
            self.reader.join(timeout=1)


def structured(packet):
    if 'error' in packet:
        raise RuntimeError(packet['error'])
    result = packet['result']
    if result.get('isError'):
        raise RuntimeError(result.get('structuredContent', result))
    value = result['structuredContent']
    texts = [part['text'] for part in result['content'] if part['type'] == 'text']
    if len(texts) != 1 or json.loads(texts[0]) != value:
        raise RuntimeError('structuredContent/compatibility text disagree')
    return value
