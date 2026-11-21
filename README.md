# Kausway

Kausway provides private HTTP access over [Iroh](https://www.iroh.computer/)
peer-to-peer connections, without port forwarding. A standalone Rust host forwards
requests to a configured origin; client applications use the shared client engine
through the Android SDK or development CLI. Jellyfin is the reference server integration.

## Start here

- [Development binary downloads](docs/operations/development-binaries.md): branch-independent suite/component archives, targets, checksums and first launch.
- [CI and development artifacts](docs/development/ci.md): required validation summary, SDK/plugin/image downloads and local container builds.

- [Quick start: local testing](docs/getting-started/local-testing.md): download the CLIs, start a host, pair a client and access any HTTP origin.
- [Complete CLI reference](docs/operations/cli-reference.md): every command, argument and environment variable.

- [Run the host](docs/getting-started/host.md): CLI, pairing, Docker and configuration.
- [Use the Android SDK](docs/android/README.md): AAR integration, lifecycle and media routing.
- [Embed the suite in an application](docs/integrations/application-suite.md): private browser handoff, provisioning and process ownership.
- [Docker suite and embedding](docs/integrations/docker-suite.md): multi-architecture image, sidecar, persisted volumes, binary-copy stages and application supervision.
- [Integrate a server](docs/integrations/host.md): host process and authenticated control API.

## Repository map

| Directory | Purpose |
|---|---|
| `rust/` | Rust workspace: transport, host, configuration, manager, shared client engine and Android JNI adapter |
| `sdks/android/` | Kotlin SDK, Gradle wrapper and Android build configuration |
| `examples/android-basic/` | Example consuming the built AAR; included as Gradle project `:sample` |
| `integrations/jellyfin/` | Reference plugin, tests and pinned .NET SDK |
| `deploy/jellyfin/` | Combined Jellyfin/Kausway Dockerfile, Compose and entrypoint |
| `deploy/standalone/` | Multi-architecture manager/host/admin suite image and sidecar Compose |
| `scripts/android/` | Native/AAR builds, artifact inspection and emulator fixtures |
| `scripts/ci/` | Version checks and end-to-end/smoke validation |
| `scripts/packaging/` | CLI archive and Jellyfin plugin packaging |
| `protocol/` | Transport and control API contracts |
| `docs/` | Current guides, architecture, operations and historical records |
| `.github/workflows/` | Parent Build Validation, reusable development builds and legacy publishers pending Phase 8.4 |

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

Build Validation runs on every branch/PR and automatically calls binary, Android SDK
and container development builds. Download its artifacts for testing. The legacy
publishers remain separate until Phase 8.4; development builds do not publish.
See [pinned versions](docs/operations/versions.md),
[release version synchronization](docs/development/versioning.md),
[transport policies](docs/architecture/transport-policies.md) and the
[structure validation record](docs/operations/repository-structure.md).

Maven Central publication and iOS bindings are planned separately. The standalone
suite deployment lives in `deploy/standalone/`; branch builds provide tested image
artifacts. Legacy main/tag publication is being replaced later in Phase 8.

Shared suite configuration and platform storage defaults are described in
[configuration and storage](docs/architecture/configuration-and-storage.md).
The optional manager is implemented: see [manager operations](docs/operations/manager.md).
The [admin server](docs/operations/admin.md) supports first-run account setup,
login and credential recovery. [Host administration](docs/operations/host-administration.md)
adds configuration, devices, invitations, sessions and manager-mediated lifecycle controls.
