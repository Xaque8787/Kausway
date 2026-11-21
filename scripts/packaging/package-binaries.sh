#!/usr/bin/env bash
# Packs kausway-host and kausway-test-client into a tarball with a SHA-256 checksum file.
# Usage: scripts/packaging/package-binaries.sh <release dir> <platform label> <output dir>
set -euo pipefail

src=$(cd "$1" && pwd)
label=$2
out=$3
root=$(cd "$(dirname "$0")/../.." && pwd)
version=$(bash "$root/scripts/ci/check-version.sh" --track suite)
name="kausway-$version-$label"

stage=$(mktemp -d)
mkdir -p "$stage/$name" "$out"
install -m 0755 "$src/kausway-host" "$src/kausway-test-client" "$stage/$name/"
python3 - "$root" "$stage/$name/BUILD-INFO.json" <<'PYINFO'
import json, sys
from pathlib import Path
sys.path.insert(0, str(Path(sys.argv[1]) / 'scripts/releases'))
from versions import binary_versions
versions = binary_versions(Path(sys.argv[1]))
Path(sys.argv[2]).write_text(json.dumps({'binary_versions': {name: versions[name] for name in ['kausway-host', 'kausway-test-client']}}, indent=2) + '\n')
PYINFO
tar -C "$stage" -czf "$out/$name.tar.gz" "$name"
(cd "$out" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
ls -l "$out"
