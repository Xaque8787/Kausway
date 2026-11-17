#!/usr/bin/env python3
"""Real manager/host lifecycle smoke test. Usage: manager-smoke.py <binary-directory>."""
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

binaries = Path(sys.argv[1]).resolve()
environment = {k: v for k, v in os.environ.items() if not k.startswith('KAUSWAY_')}
environment['KAUSWAY_RELAY_URL'] = 'none'
# Explicit paths must work in service accounts without a HOME directory.
environment.pop('HOME', None)

with tempfile.TemporaryDirectory(prefix='kw-smoke-') as temporary:
    root = Path(temporary)
    config = root / 'config.toml'
    config.write_text('schema_version = 1\n[host]\norigin = "http://127.0.0.1:8096"\n')
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        address = f'127.0.0.1:{reservation.getsockname()[1]}'
    processes = []

    def launch(arguments):
        process = subprocess.Popen(arguments, env=environment,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        processes.append(process)
        return process

    def control(operation, check=True):
        result = subprocess.run([str(binaries / 'kausway'), operation, '--data-root', str(root)],
                                env=environment, capture_output=True, text=True, timeout=20)
        if check:
            assert result.returncode == 0, result.stderr
            return json.loads(result.stdout)['status']
        return result

    def wait_for(predicate):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            result = control('status', check=False)
            if result.returncode == 0:
                status = json.loads(result.stdout)['status']
                if predicate(status):
                    return status
            time.sleep(0.05)
        raise AssertionError('manager readiness/state timed out')

    command = [str(binaries / 'kausway'), 'run', '--config', str(config),
               '--data-root', str(root), '--control-listen', address]
    try:
        manager = launch(command + ['--host-enabled', '--origin', 'http://127.0.0.1:9'])
        running = wait_for(lambda v: v['host']['observed'] == 'running')
        first_pid = running['host']['pid']
        token = (root / 'host/control.token').read_text()

        def host_status():
            request = urllib.request.Request(f'http://{address}/v1/status',
                                             headers={'Authorization': f'Bearer {(root / "host/control.token").read_text()}'})
            with urllib.request.urlopen(request, timeout=3) as response:
                return json.load(response)

        original = host_status()
        assert original['origin'] == 'http://127.0.0.1:9/'
        assert 'http://127.0.0.1:8096' in config.read_text()
        control('restart')
        restarted = wait_for(lambda v: v['host']['observed'] == 'running')
        assert restarted['host']['pid'] != first_pid
        assert host_status()['endpoint_id'] == original['endpoint_id']
        assert (root / 'host/control.token').read_text() != token
        control('stop')
        wait_for(lambda v: v['host']['observed'] == 'stopped')
        assert manager.poll() is None
        control('shutdown')
        assert manager.wait(timeout=10) == 0

        # An external host owns the exact same component directory.
        external = launch([str(binaries / 'kausway-host'), 'serve', '--config', str(config),
                           '--data-dir', str(root / 'host'), '--control-listen', address])
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                host_status()
                break
            except (OSError, ValueError):
                time.sleep(0.05)
        else:
            raise AssertionError('external host did not become ready')
        token = (root / 'host/control.token').read_text()
        manager = launch(command + ['--host-enabled=false', '--restart-on-failure=false'])
        wait_for(lambda v: v['host']['observed'] == 'stopped')
        assert control('start', check=False).returncode != 0
        assert (root / 'host/control.token').read_text() == token
        assert external.poll() is None
        control('shutdown')
        assert manager.wait(timeout=10) == 0
        assert external.poll() is None
        external.send_signal(signal.SIGTERM)
        assert external.wait(timeout=10) == 0
        print('manager/host smoke test passed')
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
