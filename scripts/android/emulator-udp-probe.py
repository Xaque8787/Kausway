"""Test-only UDP echo fixture. Never receives invitations or application credentials."""
import argparse
import json
from pathlib import Path
import signal
import socket

MARKER = b'kausway-emulator-udp-v1:'


def echo_packet(server):
    data, peer = server.recvfrom(4096)
    if len(data) <= 2048 and data.startswith(MARKER):
        server.sendto(data, peer)
        return {'peer': peer[0], 'bytes': len(data)}
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ready', type=Path, required=True)
    parser.add_argument('--stats', type=Path, required=True)
    args = parser.parse_args()
    running = True

    def stop(*_):
        nonlocal running
        running = False

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    stats = []
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server:
        server.bind(('0.0.0.0', 0))
        server.settimeout(0.2)
        ready = args.ready.with_suffix('.tmp')
        ready.write_text(json.dumps({'port': server.getsockname()[1]}))
        ready.replace(args.ready)
        args.stats.write_text('[]')
        while running:
            try:
                event = echo_packet(server)
            except socket.timeout:
                continue
            if event:
                if len(stats) < 64:
                    stats.append(event)
                args.stats.write_text(json.dumps(stats, indent=2))


if __name__ == '__main__':
    main()
