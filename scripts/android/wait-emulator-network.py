"""Wait for a usable emulator IPv4 address before initializing the SDK endpoint."""
import argparse
import ipaddress
import json
from pathlib import Path
import re
import subprocess
import time


def usable_ipv4_address(text):
    # Android may restrict RTM_GETLINK/route queries even for the adb shell. Address
    # enumeration is sufficient for this gate; the app's UDP preflight checks routing.
    for raw in re.findall(r'\binet\s+(\S+)', text):
        try:
            address = ipaddress.ip_interface(raw).ip
        except ValueError:
            continue
        if address.version == 4 and not (address.is_loopback or address.is_unspecified
                                         or address.is_multicast or address.is_link_local):
            return True
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    deadline = time.monotonic() + 30
    last = ''
    while time.monotonic() < deadline:
        try:
            result = subprocess.run(['adb', '-e', 'shell', 'ip', '-4', 'address', 'show'],
                                    text=True, capture_output=True, timeout=min(3, max(0.1, deadline - time.monotonic())))
            last = result.stdout + result.stderr
            ready = result.returncode == 0 and usable_ipv4_address(result.stdout)
        except subprocess.TimeoutExpired:
            last, ready = 'ADB address query timed out', False
        args.output.write_text(json.dumps({'ready': ready, 'addresses': last}, indent=2))
        if ready:
            print('Emulator IPv4 address is ready.')
            return
        time.sleep(0.5)
    raise SystemExit('Emulator has no usable IPv4 address; see network-readiness.json')


if __name__ == '__main__':
    main()
