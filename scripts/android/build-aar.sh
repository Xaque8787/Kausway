#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
version=$(bash "$root/scripts/ci/check-version.sh")
name="kausway-client-android-$version"
python3 "$root/scripts/android/prepare-verifier.py"
bash "$root/scripts/android/build-native.sh"
cd "$root/android"
./gradlew :kausway-client:assembleRelease :kausway-client:generatePomFileForReleasePublication
mkdir -p "$root/dist"
cp kausway-client/build/outputs/aar/kausway-client-release.aar "$root/dist/$name.aar"
cp kausway-client/build/publications/release/pom-default.xml "$root/dist/$name.pom"
python3 "$root/scripts/android/verify-aar.py" "$root/dist/$name.aar"
(cd "$root/dist" && sha256sum "$name.aar" > "$name.aar.sha256" && sha256sum "$name.pom" > "$name.pom.sha256")
