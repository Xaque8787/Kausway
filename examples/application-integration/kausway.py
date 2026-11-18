"""Dependency-free example wrapper for Kausway's private integration protocol v1.

Treat replies as sensitive. This module never logs protocol frames or credentials.
The application owns restart policy; this wrapper does not silently respawn suites.
"""
import json
import queue
import socket
import subprocess
import threading
import time
import webbrowser
from pathlib import Path


class IntegrationError(RuntimeError):
    """Transport/protocol failure. The process must be shut down before relaunch."""


class RequestError(RuntimeError):
    """A valid, correlated Kausway error response; the connection remains usable."""


class SuiteManager:
    def __init__(self, executable, *, config=None, data_root=None, application_name='MyApplication',
                 host_enabled=True, admin=True, restart_policy='application', extra_args=(), stderr=None):
        args = [str(Path(executable).resolve()), 'run', '--integration-stdio',
                '--admin-ui=' + str(admin).lower(), '--restart-policy', restart_policy,
                '--application-name', application_name]
        if config is not None:
            args += ['--config', str(config)]
        if data_root is not None:
            args += ['--data-root', str(data_root)]
        if host_enabled is not None:
            args += ['--host-enabled=' + str(host_enabled).lower()]
        args += list(extra_args)
        self.process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=stderr, bufsize=0)
        self._admin_enabled = admin
        self._frames = queue.Queue(maxsize=32)
        self._lock = threading.RLock()
        self._id = 0
        self._broken = False
        self._ready = False
        self._reader = threading.Thread(target=self._read, daemon=True)
        self._reader.start()

    def _read(self):
        try:
            while True:
                line = self.process.stdout.readline(65537)
                if not line:
                    raise IntegrationError('Manager exited or closed its private output')
                if len(line) > 65536 or not line.endswith(b'\n'):
                    raise IntegrationError('Invalid manager response frame')
                value = json.loads(line)
                if not isinstance(value, dict):
                    raise IntegrationError('Invalid manager response object')
                self._frames.put(value, timeout=1)
        except Exception:
            # Do not include a JSON parser exception: it can quote secret-bearing input.
            try:
                self._frames.put(IntegrationError('Manager integration connection closed or invalid'), timeout=1)
            except queue.Full:
                pass

    def _receive(self, timeout):
        try:
            value = self._frames.get(timeout=timeout)
            if isinstance(value, Exception):
                raise value
            return value
        except (queue.Empty, IntegrationError):
            self._broken = True
            self.process.stdin.close()
            raise IntegrationError('Manager response timed out or its connection failed') from None

    def wait_ready(self, timeout=30, *, require_admin=None, require_host=False):
        if require_admin is None:
            require_admin = self._admin_enabled
        with self._lock:
            deadline = time.monotonic() + timeout
            if not self._ready:
                event = self._receive(timeout)
                if event.get('event') != 'ready' or event.get('protocol') != 1:
                    self._broken = True
                    raise IntegrationError('Unsupported manager integration handshake')
                self._ready = True
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RequestError('Requested components did not become ready')
                status = self.request('status', timeout=remaining)['status']
                if require_host and status['host']['observed'] == 'failed' and status['host']['retry_in_ms'] is None:
                    raise RequestError('Host failed; inspect status diagnostics')
                if require_admin and not status['admin_enabled']:
                    raise RequestError('Admin is disabled')
                if require_host and status['desired'] == 'disabled':
                    raise RequestError('Host is disabled')
                try:
                    if require_admin:
                        self.request('admin.status', timeout=remaining)
                    if not require_host or status['host']['observed'] == 'running':
                        return status
                except RequestError:
                    if status['admin']['observed'] == 'failed':
                        raise RequestError('Admin process failed') from None
                time.sleep(min(0.1, max(0, deadline - time.monotonic())))

    def request(self, operation, *, timeout=140, **fields):
        with self._lock:
            if not self._ready or self._broken:
                raise IntegrationError('Call wait_ready first, or shut down the failed connection')
            self._id += 1
            request_id = self._id
            body = json.dumps({'id': request_id, 'operation': operation, **fields}).encode() + b'\n'
            if len(body) > 16384:
                raise ValueError('Integration request exceeds 16 KiB')
            try:
                remaining = memoryview(body)
                while remaining:
                    written = self.process.stdin.write(remaining)
                    if not written:
                        raise OSError('Closed manager input')
                    remaining = remaining[written:]
                self.process.stdin.flush()
            except (OSError, ValueError):
                self._broken = True
                raise IntegrationError('Manager input is unavailable') from None
            reply = self._receive(timeout)
            if reply.get('id') != request_id:
                self._broken = True
                raise IntegrationError('Manager reply correlation failed')
            if reply.get('ok') is not True:
                raise RequestError(reply.get('error', 'Manager rejected operation'))
            return reply

    def admin_entry(self, *, open_browser=False):
        entry = self.request('admin.open')['result']
        if open_browser and not webbrowser.open(entry['url'], new=2):
            raise RequestError('Could not open browser; retain the entry URL privately for retry')
        return entry

    def initialize_credentials(self, username, password):
        return self.request('admin.credentials.initialize', username=username, password=password)['result']

    def reset_credentials(self, username, password):
        return self.request('admin.credentials.reset', username=username, password=password)['result']

    def shutdown(self, timeout=400):
        with self._lock:
            try:
                if self.process.poll() is None and self._ready and not self._broken:
                    self.request('shutdown')
            finally:
                # Closing the private pipe also requests shutdown, including parent crashes.
                if not self.process.stdin.closed:
                    self.process.stdin.close()
                try:
                    self.process.wait(timeout=timeout)
                except subprocess.TimeoutExpired:
                    # Escalation is explicit; do not launch a replacement over a live suite.
                    self.process.terminate()
                    raise IntegrationError('Manager shutdown exceeded application timeout; wait before relaunch') from None
                finally:
                    if self.process.poll() is not None:
                        self._reader.join(timeout=1)
                        self.process.stdout.close()

    def __enter__(self):
        try:
            self.wait_ready()
            return self
        except Exception:
            self.shutdown()
            raise

    def __exit__(self, *_):
        self.shutdown()


def admin_request(data_directory, config, operation, *, username='', password='', timeout=10):
    """For applications independently supervising host/admin on Unix."""
    if not hasattr(socket, 'AF_UNIX'):
        raise IntegrationError('Live admin IPC requires Unix support in this version')
    request = json.dumps({'operation': operation, 'config': str(Path(config).resolve()),
                          'username': username, 'password': password}).encode() + b'\n'
    if len(request) > 16384:
        raise ValueError('Admin request exceeds 16 KiB')
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(timeout)
        connection.connect(str(Path(data_directory) / 'control.sock'))
        connection.sendall(request)
        with connection.makefile('rb') as stream:
            line = stream.readline(16385)
        if len(line) > 16384 or not line.endswith(b'\n'):
            raise IntegrationError('Invalid admin reply frame')
        try:
            reply = json.loads(line)
        except (ValueError, UnicodeError):
            raise IntegrationError('Invalid admin reply') from None
        if reply.get('ok') is not True:
            raise RequestError(reply.get('error', 'Admin rejected operation'))
        return reply['result']
