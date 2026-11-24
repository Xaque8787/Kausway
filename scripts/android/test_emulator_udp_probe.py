import importlib.util
from pathlib import Path
import socket
import threading
import unittest

spec = importlib.util.spec_from_file_location('udp_probe', Path(__file__).with_name('emulator-udp-probe.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class EmulatorUdpProbeTests(unittest.TestCase):
    def exchange(self, payload):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
            server.bind(('127.0.0.1', 0))
            server.settimeout(1)
            client.settimeout(0.1)
            observed = []
            thread = threading.Thread(target=lambda: observed.append(module.echo_packet(server)))
            thread.start()
            client.sendto(payload, server.getsockname())
            try:
                received = client.recv(4096)
            except socket.timeout:
                received = None
            thread.join(timeout=2)
            self.assertFalse(thread.is_alive())
            return received, observed[0]

    def test_echoes_quic_initial_sized_datagram(self):
        payload = module.MARKER + b'x' * (1200 - len(module.MARKER))
        received, event = self.exchange(payload)
        self.assertEqual(received, payload)
        self.assertEqual(event, {'peer': '127.0.0.1', 'bytes': 1200})

    def test_ignores_unrelated_and_oversized_packets(self):
        for payload in [b'not a fixture probe', module.MARKER + b'x' * 3000]:
            self.assertEqual(self.exchange(payload), (None, None))
