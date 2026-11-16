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
  # Host logs never contain invite tokens, but generated links are deliberately not printed.
  for log in host origin; do
    [ ! -f "$work/$log.log" ] || cp "$work/$log.log" "$diagnostics/$log.log"
  done
  tail -n 100 "$work/host.log" || true
  rm -rf "$work"
}
trap cleanup EXIT
mkdir -p "$work/www"
head -c 1048576 /dev/urandom > "$work/www/movie.bin"
python3 "$root/scripts/ci/range-origin.py" 18096 "$work/www" > "$work/origin.log" 2>&1 &
pids+=($!)
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
# Translate address hints in the invitation; do not race the host's address-file writer.
invite=$("$bin" invite --data-dir "$work/host" --label 'Android SDK integration' | python3 "$root/scripts/android/rewrite-emulator-invite.py")
cd "$root/sdks/android"
./gradlew :kausway-client:connectedDebugAndroidTest "-Pandroid.testInstrumentationRunnerArguments.kauswayInvite=$invite"
