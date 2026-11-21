#!/usr/bin/env bash
# Validate registered versions; print suite by default for existing callers.
# Usage: check-version.sh [--track suite|client|container] [legacy-or-track-tag]
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
track=suite
if [ "${1:-}" = --track ]; then
  track=${2:?Missing release track}
  shift 2
fi
if [ "$#" -gt 1 ]; then echo 'Unexpected version-check arguments' >&2; exit 1; fi
version=$(python3 "$root/scripts/releases/sync-versions.py" --check --print-version "$track")
if [ -n "${1:-}" ]; then
  tag=$1
  if [ "$tag" != "$track-v$version" ] && ! { [ "$track" = suite ] && [ "$tag" = "v$version" ]; }; then
    echo "Tag $tag does not match $track version $version" >&2
    exit 1
  fi
fi
echo "$version"
