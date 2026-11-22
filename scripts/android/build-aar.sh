#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
version=$(bash "$root/scripts/ci/check-version.sh" --track client)
name="kausway-client-android-$version"
python3 "$root/scripts/android/prepare-verifier.py"
bash "$root/scripts/android/build-native.sh"
cd "$root/sdks/android"
./gradlew :kausway-client:assembleRelease :kausway-client:generatePomFileForReleasePublication :kausway-client:kauswayReleaseSourcesJar
mkdir -p "$root/dist"
cp kausway-client/build/outputs/aar/kausway-client-release.aar "$root/dist/$name.aar"
cp kausway-client/build/publications/release/pom-default.xml "$root/dist/$name.pom"
python3 "$root/scripts/packaging/package-sdk-sources.py" \
  --input "kausway-client/build/kausway-sdk-sources/kausway-client-$version-sources.jar" --output "$root/dist/$name-sources.jar"
cp "$root/scripts/releases/README-sdk.md" "$root/dist/$name-README.md"
python3 "$root/scripts/android/verify-aar.py" "$root/dist/$name.aar"
python3 "$root/scripts/packaging/package-sdk-notices.py" --output "$root/dist/$name-notices.tar.gz"
for suffix in .aar .pom -sources.jar -notices.tar.gz -README.md; do
  (cd "$root/dist" && sha256sum "$name$suffix" > "$name$suffix.sha256")
done
