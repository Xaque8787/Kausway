#!/usr/bin/env python3
"""Exercise the real suite image and binary-copy consumers on a native Docker host."""
import argparse
from http.client import HTTPConnection
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]


def docker(*args, input=None, check=True):
    try:
        result = subprocess.run(['docker', *args], input=input, text=True, capture_output=True,
                                check=check, timeout=600)
    except subprocess.CalledProcessError as error:
        error.add_note(error.stderr[-12000:])
        raise
    return (result.stdout + (result.stderr if args[0] == 'logs' else '')).strip()


def wait_for(callback):
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        try:
            value = callback()
            if value:
                return value
        except (subprocess.CalledProcessError, OSError, ValueError, KeyError):
            pass
        time.sleep(0.2)
    raise AssertionError('Container did not reach expected readiness')


def verify_image(image, name, config_path='/config/config.toml'):
    config, data = name + '-config', name + '-data'
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    base = f'http://127.0.0.1:{port}'
    password = 'container smoke test password'

    def status():
        return json.loads(docker('exec', name, 'kausway', 'status'))['status']

    def start_container():
        overrides = ['-e', 'KAUSWAY_CONFIG=' + config_path] if config_path != '/config/config.toml' else []
        docker('run', '-d', '--init', '--name', name, '--read-only', '--tmpfs', '/tmp',
               '-p', f'127.0.0.1:{port}:47951', '-v', config + ':/config', '-v', data + ':/data',
               '-e', 'KAUSWAY_RELAY_URL=none', '-e', 'KAUSWAY_ADMIN_BROWSER_BASE_URL=' + base, *overrides, image)
        ready = wait_for(status)
        assert ready['config_path'] == config_path
        assert ready['host_data_dir'] == '/data/host'

    def login():
        connection = HTTPConnection('127.0.0.1', port, timeout=5)
        connection.request('POST', '/api/login', json.dumps({'username': 'container-test', 'password': password}),
                           {'Content-Type': 'application/json', 'Origin': base})
        response = connection.getresponse()
        body = response.read()
        connection.close()
        assert response.status == 200, (response.status, body)

    try:
        start_container()
        link = wait_for(lambda: docker('exec', name, 'kausway-admin', '--data-dir', '/data/admin', 'setup-link'))
        assert link.startswith(base + '/setup#token=')
        docker('exec', '-i', name, 'kausway-admin', '--data-dir', '/data/admin', 'credentials', 'set', '--username', 'container-test',
               '--password-stdin', input=password)
        login()
        saved = docker('exec', name, 'cat', config_path)
        assert '$argon2id$' in saved and password not in saved
        docker('exec', name, 'kausway', 'start')
        wait_for(lambda: status()['host']['observed'] == 'running')
        identity = docker('exec', name, 'sha256sum', '/data/host/identity.key')
        docker('stop', '-t', '45', name)
        assert docker('inspect', '-f', '{{.State.ExitCode}}', name) == '0'
        # Recreate the container, retaining only its mounted config/data volumes.
        docker('rm', name)
        start_container()
        wait_for(lambda: status()['host']['observed'] == 'running')
        assert docker('exec', name, 'sha256sum', '/data/host/identity.key') == identity
        login()
        docker('exec', name, 'kausway', 'stop')
        wait_for(lambda: status()['host']['observed'] == 'stopped')
        docker('stop', '-t', '45', name)
        assert docker('inspect', '-f', '{{.State.ExitCode}}', name) == '0'
    except Exception:
        print(docker('logs', name, check=False))
        raise
    finally:
        docker('rm', '-f', name, check=False)
        docker('volume', 'rm', config, data, check=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image')
    args = parser.parse_args()
    prefix = 'kausway-smoke-' + uuid.uuid4().hex[:10]
    images = []
    try:
        # The actual suite defaults, non-root UID, readonly root and persisted volumes.
        assert docker('image', 'inspect', '-f', '{{.Config.User}}', args.image) == '10001:10001'
        docker('run', '--rm', args.image, 'kausway', '--version')
        verify_image(args.image, prefix)
        # Existing test deployments can deliberately select their earlier filename.
        verify_image(args.image, prefix + '-custom-config', '/config/kausway.toml')
        with tempfile.TemporaryDirectory(prefix='kausway-consumer-') as temporary:
            context = Path(temporary)
            for base in ['alpine:3.22', 'ubuntu:24.04']:
                tag = prefix + '-' + base.split(':')[0]
                images.append(tag)
                (context / 'Dockerfile').write_text(f'''FROM {args.image} AS suite
FROM {base}
COPY --from=suite /usr/local/bin/ /usr/local/bin/
COPY --from=suite /usr/share/kausway/ /usr/share/kausway/
RUN mkdir /config /data && chown 10001:10001 /config /data
ENV KAUSWAY_CONFIG=/config/config.toml KAUSWAY_DATA_ROOT=/data KAUSWAY_ADMIN_LISTEN=0.0.0.0:47951
USER 10001:10001
VOLUME ["/config", "/data"]
CMD ["kausway", "run", "--admin-ui"]
''')
                docker('build', '--builder', 'default', '-t', tag, str(context))
                verify_image(tag, tag)
            # Execute the documented entrypoint with CMD replacement, all four modes,
            # application failure, and shutdown while a real application is running.
            tag = prefix + '-entrypoint'
            images.append(tag)
            docker('build', '--builder', 'default', '--build-arg', 'KAUSWAY_IMAGE=' + args.image,
                   '-t', tag, str(ROOT / 'examples/docker-integration'))
            for mode in ['manager', 'host']:
                for admin in ['true', 'false']:
                    name = prefix + '-application'
                    try:
                        docker('run', '-d', '--init', '--name', name, '-e', 'KAUSWAY_MODE=' + mode,
                               '-e', 'KAUSWAY_WITH_ADMIN=' + admin, '-e', 'KAUSWAY_RELAY_URL=none',
                               tag, 'sleep', '300')
                        if mode == 'manager':
                            wait_for(lambda: json.loads(docker('exec', name, 'kausway', 'status'))['status'])
                        else:
                            wait_for(lambda: docker('exec', name, 'test', '-s', '/data/host/identity.key') == '')
                        if admin == 'true':
                            wait_for(lambda: docker('exec', name, 'kausway-admin', '--data-dir', '/data/admin', 'setup-link'))
                        docker('stop', '-t', '45', name)
                        assert docker('inspect', '-f', '{{.State.ExitCode}}', name) == '0'
                    except Exception:
                        print(docker('logs', name, check=False))
                        raise
                    finally:
                        docker('rm', '-f', '-v', name, check=False)
            result = subprocess.run(['docker', 'run', '--rm', '--init', tag, 'sh', '-c', 'exit 7'], capture_output=True, timeout=60)
            assert result.returncode == 7
    finally:
        for image in images:
            docker('image', 'rm', '-f', image, check=False)
    print('Suite and Alpine/Ubuntu consumers passed setup, persistence, lifecycle and entrypoint checks')


if __name__ == '__main__':
    main()
