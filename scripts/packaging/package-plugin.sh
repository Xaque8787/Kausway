#!/bin/sh
# Lays out the built plugin as <out>/JellyfinKausway_<version>/ using the version in meta.json.
# Usage: package-plugin.sh <publish dir> <meta.json> <out dir>
set -eu

build=$1
meta=$2
out=$3
version=$(sed -n 's/^ *"version": *"\([0-9.]*\)".*/\1/p' "$meta")
if [ -z "$version" ]; then
  echo "no version found in $meta" >&2
  exit 1
fi
dir="$out/JellyfinKausway_$version"
install -D -m 0644 "$build/Jellyfin.Plugin.Kausway.dll" "$dir/Jellyfin.Plugin.Kausway.dll"
install -m 0644 "$meta" "$dir/meta.json"
echo "packaged plugin into $dir"
