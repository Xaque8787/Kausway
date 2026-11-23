#!/usr/bin/env bash
# Smoke test of the built image: Jellyfin starts with the bundled plugin installed and loaded,
# kausway-host runs beside it with its control API, the supervisor restarts kausway-host if it dies,
# and the container stops cleanly.
# Usage: scripts/ci/docker-smoke.sh <image>
set -euo pipefail

image=$1
root=$(cd "$(dirname "$0")/../.." && pwd)
host_version=$(bash "$root/scripts/ci/check-version.sh")
expected_plugin=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$root/integrations/jellyfin/meta.json")
actual_host=$(docker run --rm --entrypoint /usr/local/bin/kausway-host "$image" --version)
test "$actual_host" = "kausway-host $host_version"
echo "bundled $actual_host; expected plugin $expected_plugin"
work=$(mktemp -d)
name=kausway-smoke
mkdir -p "$work/config/plugins/JellyfinKausway_0.0.0.0" "$work/cache" "$work/media" "$work/kausway-data"
# Simulate a persistent installation from an older image.
printf 'stale plugin\n' > "$work/config/plugins/JellyfinKausway_0.0.0.0/stale"
cleanup() {
  echo "----- container log -----"
  docker logs "$name" 2>&1 | tail -n 200 || true
  docker rm -f "$name" > /dev/null 2>&1 || true
}
trap cleanup EXIT

docker run -d --name "$name" --init -p 127.0.0.1:18096:8096 \
  -e KAUSWAY_RELAY_URL=none \
  -v "$work/config:/config" -v "$work/cache:/cache" -v "$work/media:/media:ro" -v "$work/kausway-data:/kausway-data" \
  "$image"

in_container() { docker exec "$name" bash -c "$1"; }

# /health answers 200 "Degraded" while Jellyfin is still migrating; only "Healthy" means ready.
health() { curl -fs --max-time 5 http://127.0.0.1:18096/health || true; }
for _ in $(seq 1 90); do
  [ "$(health)" = Healthy ] && break
  sleep 2
done
test "$(health)" = Healthy
echo "jellyfin healthy"

# The plugin was copied into the persistent plugin directory and Jellyfin loaded it.
ls "$work/config/plugins/"
plugin_version=$expected_plugin
test -f "$work/config/plugins/JellyfinKausway_$plugin_version/Jellyfin.Plugin.Kausway.dll"
test ! -e "$work/config/plugins/JellyfinKausway_0.0.0.0"
in_container "sha256sum /opt/kausway/plugins/JellyfinKausway_$plugin_version/Jellyfin.Plugin.Kausway.dll | sed 's|/opt/kausway/plugins/|/config/plugins/|' | sha256sum --check"
python3 -c 'import json,sys; meta=json.load(open(sys.argv[1])); assert meta["version"]==sys.argv[2], meta' \
  "$work/config/plugins/JellyfinKausway_$plugin_version/meta.json" "$expected_plugin"
for _ in $(seq 1 30); do
  docker logs "$name" 2>&1 | grep -E 'Loaded plugin.*Kausway Remote Access' > /dev/null && break
  sleep 1
done
docker logs "$name" 2>&1 | grep -E 'Loaded plugin.*Kausway Remote Access'
# The plugin's admin routes exist and require a signed-in administrator (404 would mean not loaded).
route_status() { curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:18096/JellyfinKausway/Status; }
for _ in $(seq 1 15); do
  [ "$(route_status)" = 401 ] && break
  sleep 2
done
code=$(route_status)
echo "unauthenticated /JellyfinKausway/Status -> $code"
test "$code" = 401

# kausway-host runs with its control API on loopback inside the container.
for _ in $(seq 1 30); do
  in_container 'curl -fs http://127.0.0.1:47950/v1/health' && break
  sleep 1
done
echo
# The image-selected shared config must land in the persisted host-data bind mount.
in_container 'test -s /kausway-data/config.toml'
# The data directory is private to the container's user, so check the token from inside.
in_container 'test "$(stat -c %a /kausway-data/control.token)" = 600'
in_container 'curl -fs -H "Authorization: Bearer $(cat /kausway-data/control.token)" http://127.0.0.1:47950/v1/status' > "$work/status.json"
python3 -c "import json,sys; s=json.load(open(sys.argv[1])); assert s['control_api']==1 and s['relay']['mode']=='disabled', s; print('status ok', s['endpoint_id'])" "$work/status.json"
# The control API is not published outside the container.
if curl -fs --max-time 2 http://127.0.0.1:47950/v1/health; then
  echo "control API is reachable from outside the container" && exit 1
fi

# The supervisor restarts kausway-host independently of Jellyfin, keeping the same identity.
in_container 'for p in /proc/[0-9]*; do [ "$(cat "$p/comm" 2>/dev/null)" = kausway-host ] && kill "${p#/proc/}"; done; true'
sleep 1
for _ in $(seq 1 30); do
  in_container 'curl -fs http://127.0.0.1:47950/v1/health' > /dev/null 2>&1 && break
  sleep 1
done
in_container 'curl -fs -H "Authorization: Bearer $(cat /kausway-data/control.token)" http://127.0.0.1:47950/v1/status' > "$work/status2.json"
python3 -c "import json,sys; a,b=(json.load(open(p)) for p in sys.argv[1:]); assert a['endpoint_id']==b['endpoint_id'] and b['uptime_secs']<=a['uptime_secs']+5, (a,b); print('restarted ok')" "$work/status.json" "$work/status2.json"
docker logs "$name" 2>&1 | grep "kausway-host exited" > /dev/null
test "$(health)" = Healthy

started=$(date +%s)
docker stop -t 30 "$name"
elapsed=$(( $(date +%s) - started ))
echo "container stopped in ${elapsed}s"
test "$elapsed" -lt 25
test "$(docker inspect -f '{{.State.ExitCode}}' "$name")" != "137"
echo "docker smoke test passed"
