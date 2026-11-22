#!/usr/bin/env python3
"""Verify development payloads; optionally run a clean extracted native Linux suite."""
import argparse
import hashlib
from http.client import HTTPConnection
import json
import os
from pathlib import Path, PurePosixPath
import re
import socket
import shutil
import sys
import subprocess
import tarfile
import tempfile
import time
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'releases'))
from layout import CLIENT, SUITE, groups

BINS = {'linux': SUITE + CLIENT, 'android': ['kausway-host'] + CLIENT}
FORBIDDEN = {'identity.key', 'client.key', 'control.token', 'state.json', 'config.toml',
             'host.lock', 'admin.lock', 'manager.lock', 'control.sock', '.git', '.env'}
ROOT = Path(__file__).resolve().parents[2]
PASSWORD = 'development package test password'


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def verify_archive(path, expected, destination):
    if digest(path) != expected:
        raise ValueError(f'Checksum mismatch: {path.name}')
    with tarfile.open(path, 'r:gz') as archive:
        members = archive.getmembers()
        names = [member.name for member in members]
        if len(names) != len(set(names)):
            raise ValueError('Duplicate archive members')
        roots = {PurePosixPath(name).parts[0] for name in names}
        if roots != {path.name.removesuffix('.tar.gz')}:
            raise ValueError('Unexpected archive root')
        for member in members:
            parts = PurePosixPath(member.name)
            if parts.is_absolute() or '..' in parts.parts or any(p in FORBIDDEN for p in parts.parts):
                raise ValueError('Unsafe path or live state in distribution')
            if not member.isdir() and not member.isfile():
                raise ValueError('Archive links/devices are not supported')
            expected_mode = 0o755 if member.isdir() or parts.name in BINS['linux'] else 0o644
            if member.mode != expected_mode:
                raise ValueError(f'Incorrect archive permissions: {member.name}')
        archive.extractall(destination, filter='data')
    root = destination / path.name.removesuffix('.tar.gz')
    manifest = json.loads((root / 'BUILD-INFO.json').read_text())
    if manifest['format_version'] != 1 or (manifest['development'] is not True and manifest.get('release_candidate') is not True):
        raise ValueError('Not a recognized binary package manifest')
    if not re.fullmatch('[0-9a-f]{40}', manifest['commit']):
        raise ValueError('Invalid package commit')
    identity = (f"{manifest['base_version']}-dev.g{manifest['commit'][:12]}" if manifest['development'] else manifest['base_version'])
    if manifest['build_id'] != identity or identity not in path.name:
        raise ValueError('Development provenance does not match the archive')
    entries = [entry for entry in groups(manifest['target']) if entry[1] == manifest['component']]
    if len(entries) != 1 or manifest['release_track'] != entries[0][0]:
        raise ValueError('Incorrect release track/component')
    expected_bins = entries[0][2]
    if manifest['binaries'] != expected_bins:
        raise ValueError('Incorrect component binary inventory')
    expected_versions = {name: manifest['versions']['client' if name == 'kausway-test-client' else 'suite'] for name in expected_bins}
    if manifest['binary_versions'] != expected_versions:
        raise ValueError('Incorrect binary release-track versions')
    if manifest['platform'] == 'android' and manifest['minimum_android_api'] != 24:
        raise ValueError('Missing Android runtime requirement')
    actual = {p.relative_to(root).as_posix(): digest(p) for p in root.rglob('*') if p.is_file() and p != root / 'BUILD-INFO.json'}
    if actual != manifest['files']:
        raise ValueError('Payload inventory/hash mismatch')
    actual_bins = [p.name for p in root.iterdir() if p.name in BINS['linux']]
    if sorted(actual_bins) != sorted(expected_bins):
        raise ValueError('Unexpected or missing executable')
    config = tomllib.loads((root / 'config.example.toml').read_text())
    if config.get('admin', {}).get('auth') or config['manager']['host_enabled']:
        raise ValueError('Example config initializes credentials or starts a host')
    if not (root / 'licenses/THIRD-PARTY.json').is_file():
        raise ValueError('Missing third-party inventory')
    return root, manifest


def free_port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def wait_for(check):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (OSError, subprocess.CalledProcessError):
            pass
        time.sleep(0.05)
    raise AssertionError('Extracted suite did not reach the expected state')


def run_suite(root, versions, work):
    work.mkdir()
    env = {key: value for key, value in os.environ.items() if not key.startswith('KAUSWAY_')}
    env.update({'HOME': str(work / 'home'), 'XDG_CONFIG_HOME': str(work / 'config'),
                'XDG_DATA_HOME': str(work / 'data'), 'KAUSWAY_RELAY_URL': 'none',
                'KAUSWAY_ADMIN_LISTEN': '127.0.0.1:0'})
    config = work / 'config/kausway/config.toml'
    data = work / 'data/kausway'
    for name in BINS['linux']:
        result = subprocess.run([str(root / name), '--version'], env=env, capture_output=True, text=True, check=True)
        assert result.stdout.strip() == f'{name} {versions[name]}'
    processes = []

    def manager_status():
        result = subprocess.run([str(root / 'kausway'), 'status'], env=env, capture_output=True, text=True, check=True)
        return json.loads(result.stdout)['status']

    def http(base, path, body=None, cookie=None):
        connection = HTTPConnection(base.removeprefix('http://'), timeout=10)
        headers = {'Cookie': cookie} if cookie else {}
        if body is not None:
            headers.update({'Content-Type': 'application/json', 'Origin': base})
        connection.request('GET' if body is None else 'POST', path, None if body is None else json.dumps(body), headers)
        reply = connection.getresponse()
        status, headers, raw = reply.status, dict(reply.getheaders()), reply.read()
        connection.close()
        assert status == 200, (status, raw)
        return headers, json.loads(raw)

    def launch(name, initial=False):
        with (work / name).open('w') as logs:
            command = [str(root / 'kausway'), 'run', '--admin-ui']
            if initial:
                command += ['--host-enabled=false']
            process = subprocess.Popen(command, cwd=work, env=env, stdout=logs, stderr=logs)
        processes.append(process)
        wait_for(manager_status)
        return process

    def stop(process):
        subprocess.run([str(root / 'kausway'), 'shutdown'], env=env, capture_output=True, check=True)
        assert process.wait(timeout=20) == 0

    try:
        first = launch('first.log', initial=True)
        link = wait_for(lambda: subprocess.run([str(root / 'kausway-admin'), 'setup-link'], env=env,
                                              text=True, capture_output=True, check=True).stdout.strip())
        assert '#token=' in link
        base = link.split('/setup#token=')[0]
        subprocess.run([str(root / 'kausway-admin'), 'credentials', 'set', '--username', 'package-test', '--password-stdin'],
                       env=env, input=PASSWORD, capture_output=True, text=True, check=True)
        saved = config.read_text()
        assert '$argon2id$' in saved and PASSWORD not in saved
        headers, login = http(base, '/api/login', {'username': 'package-test', 'password': PASSWORD})
        cookie, csrf = headers['set-cookie'].split(';')[0], login['csrf']
        for asset, marker in [('/admin', b'<html'), ('/app.js', b'initialize'), ('/style.css', b':root')]:
            connection = HTTPConnection(base.removeprefix('http://'), timeout=10)
            connection.request('GET', asset, headers={'Cookie': cookie})
            response = connection.getresponse()
            assert response.status == 200 and marker in response.read()
            connection.close()
        _, view = http(base, '/api/config', cookie=cookie)
        settings = view['saved']
        settings['host']['origin'] = 'http://127.0.0.1:9'
        settings['host']['control']['listen'] = f'127.0.0.1:{free_port()}'
        http(base, '/api/config/save', {'csrf': csrf, 'revision': view['revision'], 'settings': settings}, cookie)
        http(base, '/api/lifecycle', {'csrf': csrf, 'action': 'start'}, cookie)
        wait_for(lambda: manager_status()['host']['observed'] == 'running')
        identity = digest(data / 'host/identity.key')
        stop(first)
        second = launch('second.log')
        wait_for(lambda: manager_status()['host']['observed'] == 'running')
        assert digest(data / 'host/identity.key') == identity
        assert tomllib.loads(config.read_text())['host']['origin'] == 'http://127.0.0.1:9'
        assert '#token=' not in (work / 'second.log').read_text()
        stop(second)
        # Standalone CLI client still creates/reuses its own component identity.
        client_work = work / 'client-work'
        client_work.mkdir()
        command = [str(root / 'kausway-test-client'), 'status']
        subprocess.run(command, cwd=client_work, env=env, capture_output=True, timeout=10)
        client_key = client_work / 'kausway-client-data/client.key'
        before = digest(client_key)
        subprocess.run(command, cwd=client_work, env=env, capture_output=True, timeout=10)
        assert digest(client_key) == before
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('artifacts', type=Path)
    parser.add_argument('--run', action='store_true', help='Execute native Linux payloads from clean extraction')
    args = parser.parse_args()
    sums = {}
    for line in (args.artifacts / 'SHA256SUMS').read_text().splitlines():
        digest_text, name = line.split('  ', 1)
        if not re.fullmatch('[0-9a-f]{64}', digest_text) or Path(name).name != name or name in sums:
            raise ValueError('Invalid checksum inventory')
        sums[name] = digest_text
    archives = sorted(args.artifacts.glob('*.tar.gz'))
    if set(sums) != {path.name for path in archives} or not archives:
        raise ValueError('Missing or unexpected archive')
    with tempfile.TemporaryDirectory(prefix='kausway-extracted-') as temporary:
        temp = Path(temporary)
        payloads = {}
        identities = set()
        components = set()
        binary_hashes = {}
        for path in archives:
            assert path.with_name(path.name + '.sha256').read_text() == f'{sums[path.name]}  {path.name}\n'
            root, manifest = verify_archive(path, sums[path.name], temp)
            identities.add((manifest['commit'], manifest['target'], json.dumps(manifest['ci'], sort_keys=True)))
            components.add(manifest['component'])
            for binary in manifest['binaries']:
                current = manifest['files'][binary]
                if binary in binary_hashes and binary_hashes[binary] != current:
                    raise ValueError('Suite/component binary mismatch')
                binary_hashes[binary] = current
            if args.run:
                if manifest['platform'] != 'linux':
                    raise ValueError('Do not execute Android packages on a Linux runner')
                for binary in manifest['binaries']:
                    subprocess.run([str(root / binary), '--help'], capture_output=True, check=True)
            payloads[manifest['component']] = (root, manifest)
        if len(identities) != 1:
            raise ValueError('Mixed build provenance or missing suite')
        expected_components = {component for _, component, _ in groups(manifest['target'])}
        if components != expected_components:
            raise ValueError('Incomplete component archive set')
        # Corruption must be rejected before extraction/execution.
        try:
            verify_archive(archives[0], '0' * 64, temp)
            raise AssertionError('Corrupt checksum accepted')
        except ValueError as error:
            assert 'Checksum mismatch' in str(error)
        if args.run:
            suite, manifest = payloads['suite']
            combined = temp / 'runtime-binaries'
            combined.mkdir()
            versions = {}
            for component in ['suite', 'test-client']:
                source, info = payloads[component]
                versions.update(info['binary_versions'])
                for name in info['binaries']:
                    shutil.copyfile(source / name, combined / name)
                    (combined / name).chmod(0o755)
            run_suite(combined, versions, temp / 'runtime')
            env = {key: value for key, value in os.environ.items() if not key.startswith('KAUSWAY_')}
            env['KAUSWAY_CONFIG'] = str(temp / 'e2e-config/config.toml')
            result = subprocess.run(['bash', str(ROOT / 'scripts/ci/e2e.sh'), str(combined)], env=env,
                                    capture_output=True, text=True, timeout=120)
            if result.returncode:
                print(result.stdout[-12000:])
                print(result.stderr[-12000:])
                raise AssertionError('Extracted host/client end-to-end test failed')
            assert {p.relative_to(suite).as_posix(): digest(p) for p in suite.rglob('*') if p.is_file() and p != suite / 'BUILD-INFO.json'} == manifest['files']
    print(f'Verified {len(archives)} development archives' + (' and clean native suite persistence' if args.run else ' (no target execution)'))


if __name__ == '__main__':
    main()
