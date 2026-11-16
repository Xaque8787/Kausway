#!/usr/bin/env bash
# Checks that every version in the repository agrees, and with the release tag if one is given.
# Prints the version on success.
# Usage: scripts/ci/check-version.sh [tag]
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
tag=${1:-}
fail=0

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/rust/Cargo.toml" | head -n 1)
if ! [[ $version =~ ^([0-9]+\.[0-9]+\.[0-9]+)(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "rust/Cargo.toml: version '$version' is not MAJOR.MINOR.PATCH[-PRERELEASE]" >&2
  exit 1
fi
# Jellyfin plugin versions are four numbers and cannot carry a pre-release suffix.
plugin="${BASH_REMATCH[1]}.0"

expect() {
  local file=$1 found=$2 want=$3
  if [ "$found" != "$want" ]; then
    echo "$file: found version '$found', expected '$want'" >&2
    fail=1
  fi
}

for crate in kausway-host kausway-transport kausway-client kausway-client-android; do
  locked=$(awk -v n="$crate" '$0 == "name = \"" n "\"" { getline; gsub(/version = |"/, ""); print }' "$root/rust/Cargo.lock")
  expect "rust/Cargo.lock ($crate)" "$locked" "$version"
done

expect integrations/jellyfin/meta.json \
  "$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$root/integrations/jellyfin/meta.json")" "$plugin"
csproj="$root/integrations/jellyfin/plugin/Jellyfin.Plugin.Kausway.csproj"
expect "Jellyfin.Plugin.Kausway.csproj (AssemblyVersion)" \
  "$(sed -n 's/.*<AssemblyVersion>\(.*\)<\/AssemblyVersion>.*/\1/p' "$csproj")" "$plugin"
expect "Jellyfin.Plugin.Kausway.csproj (FileVersion)" \
  "$(sed -n 's/.*<FileVersion>\(.*\)<\/FileVersion>.*/\1/p' "$csproj")" "$plugin"

if [ -n "$tag" ]; then
  expect "tag $tag" "$tag" "v$version"
fi

if [ "$fail" -ne 0 ]; then
  echo "Version mismatch. Set the same version everywhere listed above before tagging." >&2
  exit 1
fi
echo "$version"
