#!/usr/bin/env python3
"""Example fail-together supervisor: application + manager OR standalone host/admin.

No shell command evaluation. Any required child exit ends the container; let the
container restart policy restart it. The manager retains its own child policy.
"""
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def main():
    if len(sys.argv) < 2:
        raise SystemExit('Supply the application command through CMD or docker run arguments')
    mode = os.environ.get('KAUSWAY_MODE', 'manager')
    admin = os.environ.get('KAUSWAY_WITH_ADMIN', 'true')
    if mode not in ['manager', 'host'] or admin not in ['true', 'false']:
        raise SystemExit('KAUSWAY_MODE must be manager/host; KAUSWAY_WITH_ADMIN must be true/false')
    config = os.environ.get('KAUSWAY_CONFIG', '/config/config.toml')
    data = Path(os.environ.get('KAUSWAY_DATA_ROOT', '/data'))
    if mode == 'manager':
        commands = [['kausway', 'run', '--config', config, '--data-root', str(data), '--admin-ui=' + admin]]
    else:
        commands = [['kausway-host', 'serve', '--config', config, '--data-dir', str(data / 'host')]]
        if admin == 'true':
            commands += [['kausway-admin', '--config', config, '--data-dir', str(data / 'admin'),
                          '--host-data-dir', str(data / 'host')]]
    commands.append(sys.argv[1:])
    children = []
    stopping = False

    def stop(_signum, _frame):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    result = 0
    try:
        for command in commands:
            if stopping:
                break
            children.append(subprocess.Popen(command, start_new_session=True))
        while not stopping:
            for index, child in enumerate(children):
                code = child.poll()
                if code is not None:
                    # A clean Kausway exit is still a lost required service.
                    result = (code if code >= 0 else 128 - code) if index == len(commands) - 1 else 1
                    stopping = True
                    break
            time.sleep(0.1)
    finally:
        # Each child owns its process group. Signal even if its group leader exited.
        for child in reversed(children):
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + 30
        for child in children:
            try:
                child.wait(timeout=max(0.01, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                pass
        for child in children:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait()
    return result


if __name__ == '__main__':
    sys.exit(main())
