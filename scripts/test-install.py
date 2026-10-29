#!/usr/bin/env python3
"""Offline installer E2E. Stub executables are authored fixtures, never analysis inputs."""
import functools
import hashlib
import http.server
import io
import os
from pathlib import Path
import subprocess
import shutil
import signal
import time
import tarfile
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[1]

class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        self.server.requests.append(self.path)
        if self.path == '/latest' and self.server.latest_tag:
            self.send_response(302)
            self.send_header('Location', '/tag/' + self.server.latest_tag)
            self.end_headers()
            # Advancing latest cannot change the chosen archive/checksum tag.
            self.server.latest_tag = 'v9.9.9'
            return
        super().do_GET()

class InstallTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.mirror = self.root / 'mirror'
        self.mirror.mkdir()
        self.tools = self.root / 'tools'
        self.tools.mkdir()
        self.platform('Darwin', 'arm64')
        handler = functools.partial(QuietHandler, directory=str(self.mirror))
        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler)
        self.server.latest_tag = None
        self.server.requests = []
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = f'http://127.0.0.1:{self.server.server_port}'
        self.dest = self.root / '工具 with spaces'
        self.download_tmp = self.root / 'tmp'
        self.download_tmp.mkdir()
        self.env = dict(os.environ, TMPDIR=str(self.download_tmp), PATH=f'{self.tools}:{os.environ["PATH"]}',
                        RXD_INSTALL_TEST_BASE_URL=self.url, HOME=str(self.root / 'home'))
        (self.mirror / 'install.sh').write_bytes((ROOT / 'install.sh').read_bytes())

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
        self.tmp.cleanup()

    def platform(self, system, machine, libc='glibc 2.39'):
        self.tool('uname', f'case "$1" in -s) echo {system};; -m) echo {machine};; esac\n')
        self.tool('getconf', f'echo "{libc}"\n')

    def tool(self, name, body):
        path = self.tools / name
        path.write_text('#!/bin/sh\n' + body)
        path.chmod(0o755)

    def package(self, tag='v0.1.0', platform='macos-arm64', data=None, kind=None):
        directory = self.mirror / 'download' / tag
        directory.mkdir(parents=True, exist_ok=True)
        name = f'rxd-{platform}.tar.gz'
        archive = directory / name
        if data is None:
            data = f'#!/bin/sh\nprintf "rxd {tag[1:]}\\n"\n'.encode()
        with tarfile.open(archive, 'w:gz') as tar:
            entry = tarfile.TarInfo(f'rxd-{platform}/rxd')
            if kind:
                entry.type = kind
                entry.linkname = '/tmp/rxd-installer-must-not-touch'
            else:
                entry.size = len(data)
            entry.mode = 0o755
            tar.addfile(entry, None if kind else io.BytesIO(data))
        (directory / (name + '.sha256')).write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + f'  {name}\n')
        return archive

    def latest(self, tag):
        self.server.latest_tag = tag
        page = self.mirror / 'tag' / tag
        page.parent.mkdir(parents=True, exist_ok=True)
        page.write_text('Release page')

    def install(self, tag='v0.1.0'):
        # The actual public seam: curl downloads the script to sh's stdin.
        if tag is None:
            return subprocess.run(['sh', '-c', 'curl -fsSL "$1/install.sh" | sh -s -- --dir "$2"',
                                   'installer-test', self.url, str(self.dest)],
                                  env=self.env, capture_output=True, text=True)
        return subprocess.run(['sh', '-c', 'curl -fsSL "$1/install.sh" | sh -s -- --version "$2" --dir "$3"',
                               'installer-test', self.url, tag, str(self.dest)],
                              env=self.env, capture_output=True, text=True)

    def test_installs_and_updates_fixed_version(self):
        self.package()
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(subprocess.check_output([self.dest / 'rxd', '--version'], text=True), 'rxd 0.1.0\n')
        self.assertIn(str(self.dest / 'rxd'), result.stdout)
        self.package('v0.2.0')
        result = self.install('v0.2.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(subprocess.check_output([self.dest / 'rxd', '--version'], text=True), 'rxd 0.2.0\n')
        self.assertEqual(sorted(p.name for p in self.dest.iterdir()), ['rxd'])

    def test_latest_stable_install_update_binds_downloads_to_one_tag(self):
        for tag, expected in (('v0.1.0', 'rxd 0.1.0\n'), ('v0.2.0', 'rxd 0.2.0\n')):
            self.package(tag)
            self.latest(tag)
            self.server.requests.clear()
            result = self.install(None)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(subprocess.check_output([self.dest / 'rxd', '--version'], text=True), expected)
            self.assertEqual(self.server.requests, ['/install.sh', '/latest', '/tag/' + tag,
                             f'/download/{tag}/rxd-macos-arm64.tar.gz',
                             f'/download/{tag}/rxd-macos-arm64.tar.gz.sha256'])

    def assert_old_preserved(self, result):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertEqual((self.dest / 'rxd').read_bytes(), b'old working installation')
        self.assertEqual(sorted(p.name for p in self.dest.iterdir()), ['rxd'])
        self.assertEqual(list(self.download_tmp.iterdir()), [])

    def test_failed_latest_resolution_preserves_old_and_exact_tag_skips_resolution(self):
        self.dest.mkdir()
        (self.dest / 'rxd').write_bytes(b'old working installation')
        # Missing latest (including rate-limit/HTTP errors) cannot fall back.
        self.assert_old_preserved(self.install(None))
        for tag in ('v0.2.0-rc1', 'unexpected', 'v9.9.9'):
            self.latest(tag)
            self.assert_old_preserved(self.install(None))
        # A success response that stays at /latest is not a concrete release.
        self.server.latest_tag = None
        (self.mirror / 'latest').write_text('unresolved latest')
        self.assert_old_preserved(self.install(None))
        self.package()
        self.server.requests.clear()
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn('/latest', self.server.requests)

    def test_failed_updates_preserve_old_executable(self):
        self.dest.mkdir()
        (self.dest / 'rxd').write_bytes(b'old working installation')
        for failure in ('http', 'missing-checksum', 'malformed-checksum', 'checksum',
                        'corrupt-tar', 'symlink', 'hardlink', 'wrong-version', 'cannot-run'):
            with self.subTest(failure=failure):
                archive = self.package()
                checksum = archive.with_name(archive.name + '.sha256')
                if failure == 'http':
                    archive.unlink()
                elif failure == 'missing-checksum':
                    checksum.unlink()
                elif failure == 'malformed-checksum':
                    checksum.write_text('not a checksum\n')
                elif failure == 'checksum':
                    archive.write_bytes(b'corrupt download')
                elif failure == 'corrupt-tar':
                    archive.write_bytes(b'not a tar archive')
                    checksum.write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + f'  {archive.name}\n')
                elif failure in ('symlink', 'hardlink'):
                    self.package(kind=tarfile.SYMTYPE if failure == 'symlink' else tarfile.LNKTYPE)
                elif failure == 'wrong-version':
                    self.package(data=b'#!/bin/sh\necho "rxd 9.9.9"\n')
                elif failure == 'cannot-run':
                    self.package(data=b'#!/nonexistent/interpreter\n')
                self.assert_old_preserved(self.install())

    def test_platform_aliases_and_runtime_failures(self):
        for system, machine, platform in (('Linux', 'amd64', 'linux-x86_64'),
                                          ('Linux', 'aarch64', 'linux-arm64'),
                                          ('Darwin', 'aarch64', 'macos-arm64')):
            with self.subTest(system=system, machine=machine):
                self.platform(system, machine)
                self.package(platform=platform)
                result = self.install()
                self.assertEqual(result.returncode, 0, result.stderr)
        (self.dest / 'rxd').write_bytes(b'old working installation')
        for system, machine, libc in (('Darwin', 'x86_64', ''), ('FreeBSD', 'arm64', ''),
                                      ('Linux', 'riscv64', '')):
            with self.subTest(system=system, machine=machine, libc=libc):
                self.platform(system, machine, libc)
                self.assert_old_preserved(self.install())

    def test_invalid_arguments_do_not_touch_destination(self):
        self.package()
        for tag in ('latest', '../v0.1.0', 'v0.1.0;echo unsafe'):
            result = self.install(tag)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(self.dest.exists())

    def test_ignores_unsafe_siblings_and_rejects_duplicate_selected_entries(self):
        archive = self.package()
        original = archive.read_bytes()
        for duplicate in (False, True):
            with self.subTest(duplicate=duplicate):
                with tarfile.open(fileobj=io.BytesIO(original), mode='r:gz') as src:
                    content = src.extractfile(src.getmembers()[0]).read()
                with tarfile.open(archive, 'w:gz') as tar:
                    for name in (['rxd-macos-arm64/rxd'] * 2 if duplicate else
                                 ['rxd-macos-arm64/rxd', '../../must-not-escape']):
                        entry = tarfile.TarInfo(name)
                        entry.size = len(content)
                        tar.addfile(entry, io.BytesIO(content))
                archive.with_name(archive.name + '.sha256').write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + f'  {archive.name}\n')
                result = self.install()
                self.assertEqual(result.returncode == 0, not duplicate, result.stderr)
                self.assertFalse((self.root / 'must-not-escape').exists())

    def test_failed_replacement_and_interrupted_staging_preserve_old(self):
        self.dest.mkdir()
        (self.dest / 'rxd').write_bytes(b'old working installation')
        self.package()
        self.tool('mv', 'exit 1\n')
        self.assert_old_preserved(self.install())
        (self.tools / 'mv').unlink()
        marker = self.root / 'staging-started'
        self.package(data=f'#!/bin/sh\nset -e\ntrap \'exit 143\' TERM\ntouch "{marker}"\nsleep 30\necho "rxd 0.1.0"\n'.encode())
        proc = subprocess.Popen(['sh', '-c', 'curl -fsSL "$1/install.sh" | sh -s -- --version v0.1.0 --dir "$2"',
                                 'interrupt-test', self.url, str(self.dest)], env=self.env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        try:
            deadline = time.monotonic() + 10
            while not marker.exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertTrue(marker.exists(), 'installer did not reach staging')
            os.killpg(proc.pid, signal.SIGTERM)
            proc.communicate(timeout=10)
            self.assertNotEqual(proc.returncode, 0)
            self.assertEqual((self.dest / 'rxd').read_bytes(), b'old working installation')
            self.assertEqual(sorted(p.name for p in self.dest.iterdir()), ['rxd'])
            self.assertEqual(list(self.download_tmp.iterdir()), [])
        finally:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            proc.communicate()

    def test_missing_hash_dependency_and_unusable_destination_fail(self):
        self.package()
        # Only public OS command boundaries are controlled; installer helpers are untouched.
        for name in ('sh', 'curl', 'tar', 'mktemp', 'mkdir', 'chmod', 'mv', 'rm', 'cat', 'cmp', 'tr', 'grep', 'sed'):
            (self.tools / name).symlink_to(shutil.which(name))
        self.env['PATH'] = str(self.tools)
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('SHA-256', result.stderr)
        self.assertFalse(self.dest.exists())
        self.env['PATH'] += ':' + os.environ['PATH']
        self.dest.write_bytes(b'not a directory')
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.dest.read_bytes(), b'not a directory')
        self.assertEqual(list(self.download_tmp.iterdir()), [])

    @unittest.skipUnless(os.environ.get('RXD_INSTALL_TEST_BINARY'), 'set RXD_INSTALL_TEST_BINARY for real native RxD smoke')
    def test_real_native_binary_install_and_repeat_update(self):
        binary = Path(os.environ['RXD_INSTALL_TEST_BINARY'])
        version = subprocess.check_output([binary, '--version'], text=True).strip()
        tag = 'v' + version.removeprefix('rxd ')
        import platform
        system = platform.system()
        machine = platform.machine()
        self.platform(system, machine)
        artifact = {'Darwin': 'macos', 'Linux': 'linux'}[system] + '-' + ('arm64' if machine in ('arm64', 'aarch64') else 'x86_64')
        self.package(tag, artifact, binary.read_bytes())
        for selected in (tag, None, None):
            self.latest(tag)
            result = self.install(selected)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(subprocess.check_output([self.dest / 'rxd', '--version'], text=True).strip(), version)

if __name__ == '__main__':
    unittest.main()
