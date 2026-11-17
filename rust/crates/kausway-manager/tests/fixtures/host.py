#!/usr/bin/env python3
"""Process fixture: real authenticated HTTP, signals and selectable failures."""
import argparse
import http.server
import json
import os
from pathlib import Path
import signal
import sys

parser = argparse.ArgumentParser()
parser.add_argument('command')
parser.add_argument('--data-dir', required=True)
parser.add_argument('--control-listen', required=True)
args, _ = parser.parse_known_args()
data = Path(args.data_dir)
mode_file = data.parent / 'mode'
mode = mode_file.read_text() if mode_file.exists() else 'normal'
with (data.parent / 'launches').open('a') as launches:
    launches.write(f'{os.getpid()}\n')

def stop(*_):
    (data.parent / 'graceful').write_text('yes')
    sys.exit(0)

signal.signal(signal.SIGTERM, signal.SIG_IGN if mode == 'ignore_term' else stop)
if mode == 'crash':
    sys.exit(7)

# The supervisor must drain but never expose arbitrary secret-bearing output.
print('SENSITIVE_INVITATION_TOKEN' * 4096, flush=True)
print('SENSITIVE_PASSWORD', file=sys.stderr, flush=True)

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        authorized = self.headers.get('Authorization') == 'Bearer ' + os.environ['KAUSWAY_CONTROL_TOKEN']
        status = 200 if authorized and mode != 'unready' else 401
        body = json.dumps({'control_api': 1, 'endpoint_id': 'fixture'}).encode()
        self.send_response(status)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

host, port = args.control_listen.rsplit(':', 1)
http.server.HTTPServer((host, int(port)), Handler).serve_forever()
