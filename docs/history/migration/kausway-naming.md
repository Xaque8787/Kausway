# Kausway naming and validation

Kausway is a fresh project identity. No compatibility aliases, old invitation
schemes, old plugin IDs or old environment-variable fallbacks are retained.
The coordinated version remains 0.4.0; this change does not publish a release.

## Current identities

| Area | Identity |
|---|---|
| Repository | `Xaque8787/Kausway` |
| Rust workspace | `rust/` |
| Crates | `kausway-host`, `kausway-transport`, `kausway-client`, `kausway-client-android` |
| Executables | `kausway-host`, `kausway-test-client` |
| Android module | `:kausway-client` |
| Kotlin SDK | `io.github.xaque8787.kausway`, `KauswayClient`, `KauswayConfig`, `KauswayException`, `KauswayErrorCode` |
| Android sample | `io.github.xaque8787.kausway.sample` |
| Maven coordinates (not yet published) | `io.github.xaque8787:kausway-client-android:0.4.0` |
| JNI library | `libkausway_client.so` |
| Invitation scheme / transport ALPN | `kausway://invite/…` / `kausway/1` |
| Host environment | `KAUSWAY_*` |
| Docker Compose project/image | `kausway` / `kausway:local` |
| Plugin namespace / installation prefix | `Jellyfin.Plugin.Kausway` / `JellyfinKausway_` |
| Plugin UUID | `7c7a2311-48af-55e1-bc29-c41da361c57f` |
| Plugin API route | `JellyfinKausway` |
| Plugin environment | `JELLYFIN_KAUSWAY_*` |

The Iroh library, its dependency crates and its technical terminology retain
upstream names. Jellyfin framework references remain unchanged. License notices,
source provenance and actual historical validation links are preserved.

## Local validation

- 96 Rust tests pass; formatting and Clippy with warnings denied pass.
- CLI host/client end-to-end test passes using the new invitation scheme/ALPN.
- All 10 Jellyfin plugin tests pass; plugin DLL packaging and UUID consistency pass.
- Three Android invitation fixture tests pass.
- Three-ABI release AAR builds; ELF architecture, JNI exports, 16 KiB alignment,
  native dependency checks and packaged Kotlin API verification pass.
- Independent sample and Android instrumentation APK compile.
- All three workflows pass actionlint; version consistency is 0.4.0.
- Plugin dashboard JavaScript syntax and Compose configuration checks pass.
- Local CLI archive creation/checksum verification pass (debug binaries, not
  a substitute for the release platform matrix).

## Pending external validation

- No Android emulator is installed/attached here; rerun the Android SDK workflow
  to execute renamed-package instrumentation against the host.
- The full Docker build is blocked locally by network policy on Microsoft's
  redirected .NET registry download. Rerun Build Validation in GitHub Actions.
- GitHub CLI authentication is unavailable and direct API access is blocked;
  remote workflow dispatch/results cannot be verified from this workspace.
- After pushing this change, verify Build Validation and Android SDK, then run
  Binaries manually with Publish unchecked. Check archive names, checksums,
  plugin zip and AAR/POM assets. No release has been published by this change.

Historical passing workflows on the baseline import predate these renames.
