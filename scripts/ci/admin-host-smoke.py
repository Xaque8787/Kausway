#!/usr/bin/env python3
"""Real admin/host/manager scenario matrix. Usage: admin-host-smoke.py <binary-dir>."""
from contextlib import contextmanager
from http.client import HTTPConnection
from pathlib import Path
import json
import os
import socket
import subprocess
import sys
import tempfile
import time

BIN = Path(sys.argv[1]).resolve()
ENV = {k: v for k, v in os.environ.items() if not k.startswith('KAUSWAY_')}
ENV['KAUSWAY_RELAY_URL'] = 'none'
PASSWORD = 'correct horse battery staple'

def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]

class Suite:
    def __init__(self, root):
        self.root = root
        self.config = root / 'config.toml'
        self.data = root / 'data'
        self.admin_data = self.data / 'admin'
        self.host_data = self.data / 'host'
        self.port = free_port()
        self.control = free_port()
        self.cookie = None
        self.csrf = None
        self.children = []
        root.mkdir(parents=True)
        self.config.write_text(f'schema_version = 1\n[host.control]\nlisten = "127.0.0.1:{self.control}"\n')
        subprocess.run([str(BIN / 'kausway-admin'), '--config', str(self.config), '--data-dir', str(self.admin_data),
                        'credentials', 'set', '--username', 'alice', '--password-stdin'], input=PASSWORD,
                       text=True, capture_output=True, env=ENV, check=True)

    def launch(self, command, env=ENV):
        log = self.root / f'process-{len(self.children)}.log'
        with log.open('w') as output:
            child = subprocess.Popen(command, stdout=output, stderr=output, env=env)
        self.children.append(child)
        return child

    def admin(self, *extra):
        child = self.launch([str(BIN / 'kausway-admin'), '--config', str(self.config), '--data-dir', str(self.admin_data),
                             '--host-data-dir', str(self.host_data), '--listen', f'127.0.0.1:{self.port}', *extra])
        self.login()
        return child

    def manager(self, *extra):
        child = self.launch([str(BIN / 'kausway'), 'run', '--config', str(self.config), '--data-root', str(self.data),
                             '--admin-ui', *extra], {**ENV, 'KAUSWAY_ADMIN_LISTEN': f'127.0.0.1:{self.port}'})
        self.login()
        return child

    def host(self, *extra):
        child = self.launch([str(BIN / 'kausway-host'), 'serve', '--config', str(self.config), '--data-dir', str(self.host_data), *extra])
        self.until(lambda: self.get('/api/overview')['host']['available'])
        return child

    def http(self, path, body=None, auth=True):
        connection = HTTPConnection('127.0.0.1', self.port, timeout=20)
        headers = {}
        if auth and self.cookie:
            headers['Cookie'] = self.cookie
        if body is not None:
            headers.update({'Content-Type': 'application/json', 'Origin': f'http://127.0.0.1:{self.port}'})
            if auth and self.csrf:
                body = {**body, 'csrf': self.csrf}
        connection.request('GET' if body is None else 'POST', path, None if body is None else json.dumps(body), headers)
        response = connection.getresponse()
        status, headers, raw = response.status, dict(response.getheaders()), response.read()
        connection.close()
        return status, headers, json.loads(raw)

    def get(self, path):
        status, _, value = self.http(path)
        assert status == 200, (path, status, value)
        return value

    def post(self, path, body, expected=200):
        status, _, value = self.http(path, body)
        assert status == expected, (path, status, value)
        return value

    def until(self, predicate):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            try:
                value = predicate()
                if value:
                    return value
            except OSError:
                pass
            time.sleep(0.1)
        raise AssertionError(f'scenario condition timed out: {self.root.name}')

    def login(self):
        self.until(lambda: self.http('/api/status', auth=False)[0] == 200)
        status, headers, value = self.http('/api/login', {'username': 'alice', 'password': PASSWORD}, auth=False)
        assert status == 200, value
        self.cookie = headers['set-cookie'].split(';')[0]
        self.csrf = value['csrf']

    def settings(self):
        value = self.get('/api/config')
        # Ordinary config never returns admin credentials or host control tokens.
        assert 'password_hash' not in json.dumps(value)
        assert self.cookie.split('=')[1] not in json.dumps(value)
        return value

    def save(self, view, settings, restart=False, expected=200):
        return self.post('/api/config/save-restart' if restart else '/api/config/save',
                         {'revision': view['revision'], 'settings': settings}, expected)

    def close(self):
        for child in reversed(self.children):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()

@contextmanager
def scenario(root):
    suite = Suite(root)
    try:
        yield suite
    finally:
        suite.close()

with tempfile.TemporaryDirectory(prefix='kw-phase4-') as temp:
    root = Path(temp)
    with scenario(root / 'independent') as s:
        s.admin()
        assert s.http('/api/overview', auth=False)[0] == 401
        assert not s.get('/api/overview')['manager']['configured']
        assert not s.settings()['effective_available']
        s.post('/api/lifecycle', {'action': 'start'}, 503)
        view = s.settings()
        changed = json.loads(json.dumps(view['saved']))
        changed['host']['origin'] = 'https://invalid.example'
        s.save(view, changed, expected=409)
        changed['host']['origin'] = 'http://127.0.0.1:9'
        assert s.save(view, changed)['host_stopped']
        s.save(view, changed, expected=409)
        host = s.host('--policy', 'auto')
        s.until(lambda: (s.host_data / 'endpoint-addr.json').exists())
        live = s.settings()
        assert live['fields']['host.origin']['source'] == 'file'
        assert live['fields']['host.policy']['source'] == 'cli'
        assert not s.get('/api/overview')['lifecycle']['allowed']
        invitation = s.post('/api/invitations/create', {'label': 'smoke device', 'ttl_secs': 900})
        paired = subprocess.run([str(BIN / 'kausway-test-client'), '--data-dir', str(s.root / 'client'),
                                 'pair', invitation['link']], env=ENV, text=True, capture_output=True, timeout=20)
        assert paired.returncode == 0, paired.stderr
        devices = s.get('/api/overview')['devices']
        assert len(devices) == 1
        s.post('/api/devices/revoke', {'id': devices[0]['endpoint_id']})
        pending = s.post('/api/invitations/create', {'label': 'cancel me', 'ttl_secs': 900})
        s.post('/api/invitations/cancel', {'id': pending['id']})
        assert not s.get('/api/overview')['invitations']
        s.post('/api/invitations/cancel', {'id': '../status'}, 400)
        view = s.settings()
        changed = json.loads(json.dumps(view['saved']))
        changed['host']['websockets'] = False
        s.save(view, changed)
        view = s.settings()
        assert view['fields']['host.websockets']['effective'] is True
        s.post('/api/config/apply', {'revision': view['revision']})
        assert s.settings()['fields']['host.websockets']['effective'] is False
        view = s.settings()
        changed = json.loads(json.dumps(view['saved']))
        changed['host']['policy'] = 'p2p_only'
        s.save(view, changed)
        view = s.settings()
        assert 'host.policy' in view['overridden']
        s.post('/api/config/apply', {'revision': view['revision']}, 409)
        # Saving a new listener must not lose the current independently supervised host.
        view = s.settings()
        changed = json.loads(json.dumps(view['saved']))
        changed['host']['control']['listen'] = f'127.0.0.1:{free_port()}'
        s.save(view, changed)
        assert s.get('/api/overview')['host']['available']
        host.terminate()
        assert host.wait(timeout=10) == 0
        assert not s.settings()['effective_available']
        s.host()
        assert s.settings()['fields']['host.policy']['effective'] == 'p2p_only'
        # A wrong component path must not bypass an active host's config authority.
        s.port = free_port()
        s.launch([str(BIN / 'kausway-admin'), '--config', str(s.config),
                  '--data-dir', str(s.root / 'other-admin'), '--host-data-dir', str(s.root / 'wrong-host'),
                  '--listen', f'127.0.0.1:{s.port}'])
        s.cookie = None
        s.csrf = None
        s.login()
        view = s.settings()
        changed = view['saved']
        changed['host']['origin'] = 'http://localhost:9999'
        s.save(view, changed, expected=503)
        assert s.settings()['saved']['host']['origin'] == 'http://127.0.0.1:9'

    with scenario(root / 'managed') as s:
        manager = s.manager('--origin', 'http://127.0.0.1:9')
        overview = s.get('/api/overview')
        assert overview['manager']['available'] and overview['lifecycle']['allowed']
        assert not overview['host']['available']
        s.post('/api/lifecycle', {'action': 'start'})
        s.until(lambda: s.get('/api/overview')['host']['available'])
        view = s.settings()
        assert view['fields']['host.control.listen']['source'] == 'file'
        assert view['fields']['host.origin']['source'] == 'cli'
        settings = view['saved']
        settings['host']['origin'] = 'http://127.0.0.1:8099'
        s.save(view, settings, restart=True)
        s.until(lambda: s.get('/api/overview')['host']['available'])
        view = s.settings()
        assert 'host.origin' in view['overridden']
        assert s.get('/api/overview')['host']['status']['origin'] == 'http://127.0.0.1:9/'
        s.post('/api/lifecycle', {'action': 'stop'})
        assert not s.get('/api/overview')['host']['available']
        assert s.http('/api/status')[0] == 200 and manager.poll() is None
        s.post('/api/lifecycle', {'action': 'start'})
        s.until(lambda: s.get('/api/overview')['host']['available'])

    with scenario(root / 'application') as s:
        s.manager('--restart-policy', 'application', '--application-name', 'ExampleApp')
        overview = s.get('/api/overview')
        assert not overview['lifecycle']['allowed']
        assert overview['lifecycle']['application_name'] == 'ExampleApp'
        s.post('/api/lifecycle', {'action': 'start'}, 409)
        view = s.settings()
        s.save(view, view['saved'], restart=True, expected=409)
        assert s.settings()['revision'] == view['revision']

    with scenario(root / 'unavailable') as s:
        s.admin('--manager-socket', str(s.root / 'missing.sock'))
        overview = s.get('/api/overview')
        assert overview['manager']['configured'] and not overview['manager']['available']
        assert not overview['lifecycle']['allowed']
        s.post('/api/lifecycle', {'action': 'start'}, 503)
        view = s.settings()
        changed = view['saved']
        changed['host']['origin'] = 'http://localhost:8096'
        assert s.save(view, changed)['saved']

    with scenario(root / 'failed-restart') as s:
        s.manager('--host-binary', str(s.root / 'missing-host'), '--restart-on-failure=false')
        view = s.settings()
        changed = view['saved']
        changed['host']['origin'] = 'http://localhost:8098'
        result = s.save(view, changed, restart=True)
        assert result['saved'] and result['restart_error']
        assert s.settings()['saved']['host']['origin'] == 'http://localhost:8098'
        assert s.get('/api/overview')['manager']['status']['host']['observed'] == 'failed'

    with scenario(root / 'owner') as owner, scenario(root / 'wrong-owner') as other:
        owner.manager()
        other.admin('--manager-socket', str(owner.data / 'manager/control.sock'))
        overview = other.get('/api/overview')
        assert not overview['manager']['available']
        assert 'different configuration' in overview['manager']['error']
        other.post('/api/lifecycle', {'action': 'start'}, 503)
        assert owner.get('/api/overview')['manager']['status']['desired'] == 'disabled'

print('admin/host/manager scenario matrix passed')
