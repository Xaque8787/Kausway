#!/usr/bin/env python3
"""Exercise the real application helper, private pipes, and standalone admin handoff."""
from http.client import HTTPConnection
import importlib.util
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('kausway_application', ROOT / 'examples/application-integration/kausway.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
SuiteManager, RequestError = module.SuiteManager, module.RequestError
BIN = Path(sys.argv[1]).resolve()
for key in list(os.environ):
    if key.startswith('KAUSWAY_'):
        del os.environ[key]
os.environ['KAUSWAY_RELAY_URL'] = 'none'
PASSWORD = 'integration secret password'

def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]

def http(base, path, body=None, cookie=None):
    address = base.removeprefix('http://')
    connection = HTTPConnection(address, timeout=5)
    headers = {'Cookie': cookie} if cookie else {}
    if body is not None:
        headers.update({'Origin': base, 'Content-Type': 'application/json'})
    connection.request('GET' if body is None else 'POST', path, None if body is None else json.dumps(body), headers)
    response = connection.getresponse()
    result = response.status, dict(response.getheaders()), json.loads(response.read())
    connection.close()
    return result

def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False

with tempfile.TemporaryDirectory(prefix='kw-app-') as tmp:
    root = Path(tmp)
    os.environ['KAUSWAY_ADMIN_LISTEN'] = f'127.0.0.1:{port()}'
    logs = root / 'manager.log'
    with logs.open('wb') as output:
        app = SuiteManager(BIN / 'kausway', config=root / 'config.toml', data_root=root / 'data',
                           extra_args=['--control-listen', f'127.0.0.1:{port()}'], stderr=output)
        try:
            status = app.wait_ready(require_host=True)
            host_pid, admin_pid = status['host']['pid'], status['admin']['pid']
            entry = app.admin_entry()
            assert entry['state'] == 'setup_required' and '#token=' in entry['url']
            app.initialize_credentials('alice', PASSWORD)
            entry = app.admin_entry()
            assert entry['state'] == 'configured' and entry['url'].endswith('/login')
            base = entry['url'].removesuffix('/login')
            status, headers, _ = http(base, '/api/login', {'username': 'alice', 'password': PASSWORD})
            assert status == 200
            cookie = headers['set-cookie'].split(';')[0]
            try:
                app.initialize_credentials('intruder', PASSWORD)
                raise AssertionError('Initial provisioning replaced an account')
            except RequestError:
                pass
            app.reset_credentials('bob', PASSWORD)
            assert not http(base, '/api/status', cookie=cookie)[2]['authenticated']
            assert http(base, '/api/login', {'username': 'bob', 'password': PASSWORD})[0] == 200
            assert app.request('status')['status']['restart_policy'] == 'application'
        finally:
            app.shutdown(timeout=10)
        assert not alive(host_pid) and not alive(admin_pid)
    text = logs.read_text()
    assert PASSWORD not in text and '#token=' not in text

    # A subsequent application launch retains credentials and returns login directly.
    os.environ['KAUSWAY_ADMIN_LISTEN'] = '127.0.0.1:0'
    with SuiteManager(BIN / 'kausway', config=root / 'config.toml', data_root=root / 'data',
                      host_enabled=False, stderr=subprocess.DEVNULL) as app:
        entry = app.admin_entry()
        assert entry['state'] == 'configured' and ':0/' not in entry['url']

    # Optional admin is genuinely optional for host-only integrations.
    with SuiteManager(BIN / 'kausway', config=root / 'host-only.toml', data_root=root / 'host-only',
                      host_enabled=False, admin=False, stderr=subprocess.DEVNULL) as app:
        try:
            app.admin_entry()
            raise AssertionError('Missing admin should fail clearly')
        except RequestError:
            pass

    # Failed host readiness is explicit while admin remains usable for recovery.
    os.environ['KAUSWAY_ADMIN_LISTEN'] = '127.0.0.1:0'
    app = SuiteManager(BIN / 'kausway', config=root / 'failed-host.toml', data_root=root / 'failed-host',
                       extra_args=['--host-binary', str(root / 'missing-host'), '--restart-on-failure=false'],
                       stderr=subprocess.DEVNULL)
    try:
        try:
            app.wait_ready(require_host=True)
            raise AssertionError('Required failed host was reported ready')
        except RequestError:
            pass
        app.wait_ready()
        assert app.admin_entry()['state'] == 'setup_required'
    finally:
        app.shutdown(timeout=10)

    # Failed manager startup never reports a ready handshake.
    app = SuiteManager(BIN / 'kausway', config=root / 'failed-admin.toml', data_root=root / 'failed-admin',
                       extra_args=['--admin-binary', str(root / 'missing-admin')], stderr=subprocess.DEVNULL)
    try:
        try:
            app.wait_ready(timeout=5)
            raise AssertionError('Failed startup was reported ready')
        except module.IntegrationError:
            pass
    finally:
        app.shutdown(timeout=10)

    # Raw protocol: no JSON echo of secret-bearing malformed frames; correlated responses.
    def raw(name, *args):
        process = subprocess.Popen([str(BIN / 'kausway'), 'run', '--integration-stdio',
                                    '--config', str(root / f'{name}.toml'), '--data-root', str(root / name), *args],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        assert select.select([process.stdout], [], [], 10)[0]
        event = json.loads(process.stdout.readline())
        assert event['event'] == 'ready' and event['protocol'] == 1
        return process

    process = raw('protocol')
    process.stdin.write(b'{"password":"must-not-echo"}\n')
    process.stdin.flush()
    response = process.stdout.readline()
    assert b'must-not-echo' not in response and not json.loads(response)['ok']
    process.stdin.write(b'{"id":1,"operation":"status"}\n')
    process.stdin.flush()
    assert json.loads(process.stdout.readline())['id'] == 1
    process.stdin.write(b'{"id":2,"operation":"shutdown"}\n')
    process.stdin.flush()
    assert json.loads(process.stdout.readline())['ok']
    # No stdin close: explicit shutdown must not wait on a blocking stdin worker.
    assert process.wait(timeout=5) == 0
    process.stdin.close()
    process.stdout.close()

    process = raw('oversized')
    try:
        process.stdin.write(b'x' * 17000 + b'\n')
        process.stdin.flush()
    except BrokenPipeError:
        pass
    assert process.wait(timeout=10) != 0
    try:
        process.stdin.close()
    except BrokenPipeError:
        pass
    process.stdout.close()

    process = raw('closed-output')
    process.stdout.close()
    process.stdin.write(b'{"id":1,"operation":"status"}\n')
    process.stdin.flush()
    assert process.wait(timeout=10) != 0
    process.stdin.close()

    process = raw('signal')
    process.send_signal(signal.SIGTERM)
    assert process.wait(timeout=5) == 0  # stdin deliberately still open
    process.stdin.close()
    process.stdout.close()

    # Parent disconnect must shut down actual owned children, not leave them behind.
    os.environ['KAUSWAY_ADMIN_LISTEN'] = f'127.0.0.1:{port()}'
    process = raw('disconnect', '--admin-ui', '--host-enabled', '--control-listen', f'127.0.0.1:{port()}')
    deadline = time.monotonic() + 10
    while True:
        result = subprocess.run([str(BIN / 'kausway'), 'status', '--data-root', str(root / 'disconnect')], capture_output=True, text=True, check=True)
        status = json.loads(result.stdout)['status']
        if status['host']['observed'] == 'running':
            break
        assert time.monotonic() < deadline
        time.sleep(0.05)
    host_pid, admin_pid = status['host']['pid'], status['admin']['pid']
    process.stdin.close()
    assert process.wait(timeout=10) == 0
    process.stdout.close()
    assert not alive(host_pid) and not alive(admin_pid)

    # Independently supervised admin uses the same protected link/provisioning contract.
    independent_config, independent_data = root / 'independent.toml', root / 'independent-admin'
    os.environ['KAUSWAY_ADMIN_STARTUP_EVENTS'] = '1'
    os.environ['KAUSWAY_ADMIN_LISTEN'] = f'127.0.0.1:{port()}'
    process = subprocess.Popen([str(BIN / 'kausway-admin'), '--config', str(independent_config), '--data-dir', str(independent_data)],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        deadline = time.monotonic() + 10
        while True:
            try:
                ready = module.admin_request(independent_data, independent_config, 'status')
                assert ready['ready']
                break
            except (OSError, module.IntegrationError):
                assert time.monotonic() < deadline
                time.sleep(0.05)
        entry = module.admin_request(independent_data, independent_config, 'entry-link')
        assert entry['state'] == 'setup_required'
        module.admin_request(independent_data, independent_config, 'credentials.set', username='alice', password=PASSWORD)
        assert module.admin_request(independent_data, independent_config, 'entry-link')['state'] == 'configured'
    finally:
        process.terminate()
        assert process.wait(timeout=10) == 0

print('application integration private-pipe and independent-admin checks passed')
