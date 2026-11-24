import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('emulator_network', Path(__file__).with_name('wait-emulator-network.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class EmulatorNetworkTests(unittest.TestCase):
    def test_accepts_wifi_and_ethernet_addresses_without_requiring_public_internet(self):
        for addresses in ['2: wlan0: <UP>\n    inet 10.0.2.16/24 brd 10.0.2.255 scope global wlan0',
                          '3: eth0: <UP>\n    inet 10.0.2.15/24 scope global eth0']:
            self.assertTrue(module.usable_ipv4_address(addresses))

    def test_rejects_missing_unusable_or_loopback_only_addresses(self):
        for addresses in ['', 'request send failed: Permission denied',
                          '1: lo: <UP>\n    inet 127.0.0.1/8 scope host lo',
                          'inet 0.0.0.0/32', 'inet 169.254.1.2/16',
                          'inet6 fe80::1/64', 'inet bad-ip']:
            self.assertFalse(module.usable_ipv4_address(addresses))
