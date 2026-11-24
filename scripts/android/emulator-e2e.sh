#!/usr/bin/env bash
# Run with an already-booted Android emulator. No external relay or media server required.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
pids=()
diagnostics="$root/dist/android-emulator-diagnostics"
mkdir -p "$diagnostics"
cleanup() {
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  for pid in "${pids[@]}"; do wait "$pid" 2>/dev/null || true; done
  # Only network metadata and our explicit fixture log tag; never full logcat/runner arguments.
  python3 - "$diagnostics" "$work/host/endpoint-addr.json" <<'PYDIAG'
import json, subprocess, sys
from pathlib import Path
out = Path(sys.argv[1])
try:
    doc = json.loads(Path(sys.argv[2]).read_text())
    (out / 'host-routes-final.json').write_text(json.dumps({'addrs': doc.get('addrs', [])}, indent=2))
except (OSError, ValueError):
    pass
commands = {
    'runner-addresses.json': ['ip', '-j', 'address', 'show'],
    'runner-routes.json': ['ip', '-j', 'route', 'show'],
    'emulator-addresses.txt': ['adb', '-e', 'shell', 'ip', 'address', 'show'],
    'emulator-routes.txt': ['adb', '-e', 'shell', 'ip', 'route', 'show'],
    'emulator-fixture.log': ['adb', '-e', 'logcat', '-d', '-s', 'KauswayFixture:I', '*:S'],
}
for name, command in commands.items():
    try:
        result = subprocess.run(command, text=True, capture_output=True, timeout=5)
        (out / name).write_text(result.stdout + result.stderr)
    except (OSError, subprocess.TimeoutExpired) as error:
        (out / name).write_text(type(error).__name__)
PYDIAG
  # Host logs never contain invite tokens, but generated links are deliberately not printed.
  for log in host origin udp; do
    [ ! -f "$work/$log.log" ] || cp "$work/$log.log" "$diagnostics/$log.log"
  done
  tail -n 100 "$work/host.log" || true
  rm -rf "$work"
}
trap cleanup EXIT
python3 "$root/scripts/android/wait-emulator-network.py" --output "$diagnostics/network-readiness.json"
mkdir -p "$work/www"
head -c 1048576 /dev/urandom > "$work/www/movie.bin"
python3 "$root/scripts/ci/range-origin.py" 18096 "$work/www" > "$work/origin.log" 2>&1 &
pids+=($!)
# Isolate the fixture from configuration left by earlier validation steps.
export KAUSWAY_CONFIG="$work/config.toml"
export KAUSWAY_RELAY_URL=none
bin="${CARGO_TARGET_DIR:-$root/rust/target}/debug/kausway-host"
"$bin" serve --data-dir "$work/host" --origin http://127.0.0.1:18096 --control-listen off > "$work/host.log" 2>&1 &
pids+=($!)
for _ in $(seq 1 100); do
  [ -s "$work/host/endpoint-addr.json" ] && break
  sleep 0.1
done
test -s "$work/host/endpoint-addr.json"
# Record reachable-address hints without copying the invitation or token.
python3 - "$work/host/endpoint-addr.json" <<'PYADDR' > "$diagnostics/host-routes.json"
import json, sys
with open(sys.argv[1]) as source:
    address = json.load(source)
print(json.dumps({"addrs": address.get("addrs", [])}, indent=2))
PYADDR
# Check bidirectional, QUIC-sized UDP separately from authentication and proxying.
python3 "$root/scripts/android/emulator-udp-probe.py" \
  --ready "$work/udp-ready.json" --stats "$diagnostics/udp-probe.json" > "$work/udp.log" 2>&1 &
pids+=($!)
for _ in $(seq 1 100); do
  [ -s "$work/udp-ready.json" ] && break
  sleep 0.1
done
test -s "$work/udp-ready.json"
probe_routes=$(python3 - "$work/udp-ready.json" "$diagnostics/host-routes.json" <<'PYPROBE'
import ipaddress, json, sys
from pathlib import Path
port = json.loads(Path(sys.argv[1]).read_text())['port']
hosts = ['10.0.2.2']
for route in json.loads(Path(sys.argv[2]).read_text())['addrs']:
    if 'Ip' not in route:
        continue
    address = ipaddress.ip_address(route['Ip'].rsplit(':', 1)[0].strip('[]'))
    if address.version == 4 and not (address.is_loopback or address.is_unspecified or address.is_multicast) and str(address) not in hosts:
        hosts.append(str(address))
print(','.join(f'{host}:{port}' for host in hosts[:8]))
PYPROBE
)
# Translate address hints in the invitation; do not race the host's address-file writer.
invite=$("$bin" invite --data-dir "$work/host" --label 'Android SDK integration' | python3 "$root/scripts/android/rewrite-emulator-invite.py")
cd "$root/sdks/android"
./gradlew :kausway-client:connectedDebugAndroidTest "-Pandroid.testInstrumentationRunnerArguments.kauswayInvite=$invite" \
  "-Pandroid.testInstrumentationRunnerArguments.kauswayUdpProbe=$probe_routes"
