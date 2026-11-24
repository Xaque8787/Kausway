#!/usr/bin/env python3
"""Exercise real CLI binaries with a local Pkarr gateway; no public discovery services."""
import argparse
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
from urllib.error import HTTPError
from urllib.request import ProxyHandler, Request, build_opener


class Gateway(BaseHTTPRequestHandler):
    records = {}
    guard = threading.Lock()

    def log_message(self, *args):
        pass  # paths identify rendezvous records

    def do_PUT(self):
        size = int(self.headers.get('Content-Length', 0))
        if not 72 <= size <= 1072:
            self.send_error(413)
            return
        payload = self.rfile.read(size)
        with self.guard:
            self.records[self.path] = payload
        self.send_response(204)
        self.end_headers()

    def do_GET(self):
        with self.guard:
            payload = self.records.get(self.path)
        if payload is None:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


class Origin(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        payload = bytes(range(100))
        if self.headers.get('Range') == 'bytes=10-19':
            self.send_response(206)
            self.send_header('Content-Range', 'bytes 10-19/100')
            payload = payload[10:20]
        else:
            self.send_response(200)
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


@contextmanager
def server(handler):
    instance = ThreadingHTTPServer(('127.0.0.1', 0), handler)
    worker = threading.Thread(target=instance.serve_forever, daemon=True)
    worker.start()
    try:
        yield instance, f'http://127.0.0.1:{instance.server_port}'
    finally:
        instance.shutdown()
        instance.server_close()
        worker.join()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary_directory', type=Path)
    args = parser.parse_args()
    binary = args.binary_directory.resolve()
    with tempfile.TemporaryDirectory(prefix='kausway-short-code-') as directory, server(Gateway) as (gateway, gateway_url), server(Origin) as (_, origin):
        directory = Path(directory)
        config = directory / 'kausway.toml'
        data = directory / 'host'
        with socket.socket() as reserved:
            reserved.bind(('127.0.0.1', 0))
            port = reserved.getsockname()[1]
        common = ['--config', str(config), '--data-dir', str(data)]
        discovery = ['--rendezvous-dht', 'false', '--rendezvous-gateway', gateway_url]
        environment = {key: value for key, value in os.environ.items() if not key.startswith('KAUSWAY_')}
        environment['RUST_LOG'] = 'warn'
        local_http = build_opener(ProxyHandler({}))
        log = (directory / 'host.log').open('w')
        host = subprocess.Popen([str(binary / 'kausway-host'), 'serve', *common, '--origin', origin,
                                 '--relay-url', 'none', '--control-listen', f'127.0.0.1:{port}', *discovery],
                                env=environment, stdout=log, stderr=log)

        def run(command, succeeds=True):
            result = subprocess.run(command, env=environment, capture_output=True, text=True, timeout=40)
            if (result.returncode == 0) != succeeds:
                # Never echo a secret-bearing command or unfiltered process output.
                raise AssertionError('CLI command returned an unexpected exit status')
            return result

        def wait(predicate):
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if host.poll() is not None:
                    raise AssertionError('host exited before readiness')
                if predicate():
                    return
                time.sleep(0.05)
            raise AssertionError('timed out waiting for readiness/publication')

        try:
            wait(lambda: (data / 'endpoint-addr.json').exists() and (data / 'control.token').exists())
            token = (data / 'control.token').read_text().strip()

            def api(path, method='GET', expected=200):
                request = Request(f'http://127.0.0.1:{port}/v1/{path}', method=method,
                                  headers={'Authorization': f'Bearer {token}'})
                try:
                    response = local_http.open(request, timeout=3)
                except HTTPError as error:
                    assert error.code == expected
                    return None
                with response:
                    assert response.status == expected
                    return json.load(response) if response.status != 204 else None

            def create(short):
                flags = ['--short-code'] if short else []
                return json.loads(run([str(binary / 'kausway-host'), 'invite', *common, '--label', 'CLI test', '--json', *flags]).stdout)

            def client(name, *operation, succeeds=True):
                return run([str(binary / 'kausway-test-client'), '--data-dir', str(directory / name),
                            '--relay-url', 'none', *discovery, *operation], succeeds)

            def published(invite):
                status = api('invitations/' + invite['id'])
                assert 'short_code' not in status and 'link' not in status
                assert invite['short_code'] not in json.dumps(status)
                return status['rendezvous']['state'] == 'published'

            invite = create(True)
            assert invite['link'].startswith('kausway://invite/') and invite['short_code'].startswith('K1-')
            wait(lambda: published(invite))
            client('phone', 'pair', invite['short_code'])
            api('invitations/' + invite['id'], expected=404)
            output = directory / 'range.bin'
            client('phone', 'fetch', '/file', '--range', 'bytes=10-19', '--output', str(output))
            assert output.read_bytes() == bytes(range(10, 20))
            denied = client('other-phone', 'pair', invite['short_code'], succeeds=False)
            assert 'authorization' in denied.stderr and invite['short_code'] not in denied.stderr
            cancelled = create(True)
            wait(lambda: published(cancelled))
            api('invitations/' + cancelled['id'], method='DELETE', expected=204)
            denied = client('cancelled-phone', 'pair', cancelled['short_code'], succeeds=False)
            assert 'authorization' in denied.stderr  # cached ciphertext cannot authorize a cancelled token
            ordinary = create(False)
            assert 'short_code' not in ordinary and 'rendezvous' not in ordinary
            gateway.shutdown()  # URL pairing and reconnects must survive unavailable discovery.
            client('url-phone', 'pair', ordinary['link'])
            client('phone', 'fetch', '/file', '--output', str(output))
            assert output.read_bytes() == bytes(range(100))
        finally:
            host.terminate()
            host.wait(timeout=10)
            log.close()
        run([str(binary / 'kausway-host'), 'invite', *common, '--short-code'], succeeds=False)
    print('Short-code CLI integration passed: generation, publication, pairing, Range, cancellation, single use, URL fallback, reconnect and shutdown.')


if __name__ == '__main__':
    main()
