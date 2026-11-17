#!/usr/bin/env python3
"""Exercise admin HTTP, local CLI and manager startup logs without printing secrets."""
import concurrent.futures
from http.client import HTTPConnection
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import time

binaries = Path(sys.argv[1]).resolve()
PASSWORD = 'correct horse battery staple'
NEW_PASSWORD = 'replacement horse battery staple'
environment = {k: v for k, v in os.environ.items() if not k.startswith('KAUSWAY_')}
environment.pop('HOME', None)

def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]

with tempfile.TemporaryDirectory(prefix='kw-admin-') as temporary:
    root = Path(temporary)
    config = root / 'config.toml'
    data = root / 'data/admin'
    port = free_port()
    base = f'http://127.0.0.1:{port}'
    logs = root / 'startup.log'
    processes = []

    def launch(command, env=environment):
        with logs.open('a') as output:
            child = subprocess.Popen(command, env=env, stdout=output, stderr=output)
        processes.append(child)
        return child

    def cli(*args, password=None, check=True):
        result = subprocess.run([str(binaries / 'kausway-admin'), '--config', str(config),
                                 '--data-dir', str(data), *args], env=environment,
                                input=password, capture_output=True, text=True, timeout=15)
        if check:
            assert result.returncode == 0, result.stderr
        return result

    def http(path, body=None, cookie=None, origin=None, host=None):
        connection = HTTPConnection('127.0.0.1', port, timeout=10)
        headers = {'Host': host or f'127.0.0.1:{port}'}
        if cookie:
            headers['Cookie'] = cookie
        if body is not None:
            headers.update({'Content-Type': 'application/json', 'Origin': origin or base})
        connection.request('GET' if body is None else 'POST', path,
                           None if body is None else json.dumps(body), headers)
        response = connection.getresponse()
        result = response.status, dict(response.getheaders()), response.read()
        connection.close()
        return result

    def wait_ready():
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            try:
                if http('/api/status')[0] == 200:
                    return
            except OSError:
                pass
            time.sleep(0.05)
        raise AssertionError('admin readiness timed out')

    def login(username='alice', password=PASSWORD):
        status, headers, body = http('/api/login', {'username': username, 'password': password})
        assert status == 200, body
        cookie_header = headers['set-cookie']
        assert 'HttpOnly' in cookie_header and 'SameSite=Strict' in cookie_header
        return cookie_header.split(';')[0], json.loads(body)['csrf']

    server_command = [str(binaries / 'kausway-admin'), '--config', str(config),
                      '--data-dir', str(data), '--listen', f'127.0.0.1:{port}']
    try:
        server = launch(server_command)
        wait_ready()
        status = json.loads(http('/api/status')[2])
        assert not status['configured'] and not status['authenticated']
        assert http('/admin')[0] == 303
        assert http('/api/status', host='attacker.example')[0] == 403
        startup = logs.read_text()
        initial = re.search(r'#token=([0-9a-f]{64})', startup).group(1)
        token = cli('setup-link').stdout.strip().split('#token=')[1]
        setup = {'token': token, 'username': 'alice', 'password': PASSWORD}
        assert http('/api/setup', setup, origin='http://attacker.example')[0] == 403
        assert http('/api/setup', {**setup, 'token': initial})[0] == 403
        assert http('/api/setup', setup)[0] == 200
        assert http('/api/setup', setup)[0] == 409
        saved = config.read_text()
        assert '$argon2id$' in saved and PASSWORD not in saved and token not in saved
        assert cli('setup-link', check=False).returncode != 0
        assert http('/api/login', {'username': 'alice', 'password': 'incorrect'})[0] == 401
        cookie, csrf = login()
        assert http('/admin', cookie=cookie)[0] == 200
        assert json.loads(http('/api/status', cookie=cookie)[2])['username'] == 'alice'
        assert http('/api/logout', {}, cookie=cookie)[0] == 403
        assert http('/api/password', {'csrf': csrf, 'current_password': PASSWORD,
                                      'username': 'alice', 'password': NEW_PASSWORD}, cookie=cookie)[0] == 200
        assert not json.loads(http('/api/status', cookie=cookie)[2])['authenticated']
        cookie, csrf = login(password=NEW_PASSWORD)
        assert cli('credentials', 'set', '--username', 'intruder', '--password-stdin',
                   password=PASSWORD, check=False).returncode != 0
        cli('credentials', 'reset', '--username', 'bob', '--password-stdin', password=PASSWORD)
        assert not json.loads(http('/api/status', cookie=cookie)[2])['authenticated']
        cookie, csrf = login('bob')
        assert http('/api/logout', {'csrf': csrf}, cookie=cookie)[0] == 200
        assert not json.loads(http('/api/status', cookie=cookie)[2])['authenticated']
        server.terminate()
        assert server.wait(timeout=10) == 0
        assert not (data / 'control.sock').exists()

        # Offline reset and a restart retain credentials without reopening setup.
        cli('credentials', 'reset', '--username', 'carol', '--password-stdin', password=NEW_PASSWORD + '\n')
        before_restart = logs.stat().st_size
        server = launch(server_command)
        wait_ready()
        assert json.loads(http('/api/status')[2])['configured']
        assert '#token=' not in logs.read_text()[before_restart:]
        login('carol', NEW_PASSWORD)
        server.terminate()
        assert server.wait(timeout=10) == 0

        # Concurrent browser setup: exactly one account wins.
        second_config = root / 'second.toml'
        server = launch([str(binaries / 'kausway-admin'), '--config', str(second_config),
                         '--data-dir', str(data), '--listen', f'127.0.0.1:{port}'])
        wait_ready()
        result = subprocess.run([str(binaries / 'kausway-admin'), '--config', str(second_config),
                                 '--data-dir', str(data), 'setup-link'], env=environment,
                                capture_output=True, text=True, check=True)
        token = result.stdout.strip().split('#token=')[1]
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(lambda user: http('/api/setup', {'token': token, 'username': user, 'password': PASSWORD})[0], ['alice', 'bob']))
        assert sorted(results) == [200, 409], results
        server.terminate()
        assert server.wait(timeout=10) == 0

        # The manager forwards only the structured setup link to startup logs.
        manager_root = root / 'manager-suite'
        port = free_port()
        base = f'http://127.0.0.1:{port}'
        manager_env = {**environment, 'KAUSWAY_ADMIN_LISTEN': f'127.0.0.1:{port}'}
        manager = launch([str(binaries / 'kausway'), 'run', '--config', str(root / 'manager.toml'),
                          '--data-root', str(manager_root), '--admin-ui'], manager_env)
        wait_ready()
        deadline = time.monotonic() + 5
        while 'Kausway administrator setup (expires in 15 minutes)' not in logs.read_text():
            assert time.monotonic() < deadline
            time.sleep(0.03)
        result = subprocess.run([str(binaries / 'kausway'), 'status', '--data-root', str(manager_root)],
                                env=environment, capture_output=True, text=True, check=True)
        assert '#token=' not in result.stdout
        assert json.loads(result.stdout)['status']['admin']['observed'] == 'running'
        manager.terminate()
        assert manager.wait(timeout=10) == 0
        print('admin HTTP/CLI/manager smoke test passed')
    finally:
        for child in reversed(processes):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
