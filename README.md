# Kausway

Kausway provides private HTTP access over [Iroh](https://www.iroh.computer/)
peer-to-peer connections, without port forwarding. A standalone Rust host forwards
requests to a configured origin; client applications use the shared client engine
through the Android SDK or development CLI. Jellyfin is the reference server integration.

## Start here

- [Quick start: local testing](docs/getting-started/local-testing.md): download the CLIs, start a host, pair a client and access any HTTP origin.
- [Complete CLI reference](docs/operations/cli-reference.md): every command, argument and environment variable.

- [Run the host](docs/getting-started/host.md): CLI, pairing, Docker and configuration.
- [Use the Android SDK](docs/android/README.md): AAR integration, lifecycle and media routing.
- [Integrate a server](docs/integrations/host.md): host process and authenticated control API.

## Repository map

| Directory | Purpose |
|---|---|
| `rust/` | Rust workspace: transport, host, shared client engine and Android JNI adapter |
| `sdks/android/` | Kotlin SDK, Gradle wrapper and Android build configuration |
| `examples/android-basic/` | Example consuming the built AAR; included as Gradle project `:sample` |
| `integrations/jellyfin/` | Reference plugin, tests and pinned .NET SDK |
| `deploy/jellyfin/` | Combined Jellyfin/Kausway Dockerfile, Compose and entrypoint |
| `scripts/android/` | Native/AAR builds, artifact inspection and emulator fixtures |
| `scripts/ci/` | Version checks and end-to-end/smoke validation |
| `scripts/packaging/` | CLI archive and Jellyfin plugin packaging |
| `protocol/` | Transport and control API contracts |
| `docs/` | Current guides, architecture, operations and historical records |
| `.github/workflows/` | Build Validation, Android SDK and manual Binaries workflows |

## Build and validate

From the repository root:

```sh
(cd rust && cargo test --workspace --locked)
bash scripts/ci/check-version.sh
dotnet test integrations/jellyfin/tests/Jellyfin.Plugin.Kausway.Tests.csproj -c Release
# Requires the Android SDK/NDK and Java 17:
bash scripts/android/build-aar.sh
(cd sdks/android && ./gradlew :sample:assembleDebug :kausway-client:assembleDebugAndroidTest)
# Builds the combined Jellyfin/Kausway deployment:
docker compose -f deploy/jellyfin/compose.yaml up -d --build
```

Build Validation and Android SDK run automatically. Run Binaries manually with
Publish unchecked to generate downloadable artifacts without publishing a release.
See [pinned versions](docs/operations/versions.md),
[transport policies](docs/architecture/transport-policies.md) and the
[structure validation record](docs/operations/repository-structure.md).

Maven Central publication, iOS bindings and a standalone sidecar Docker image are
planned separately. The future standalone deployment will live in `deploy/standalone/`.
