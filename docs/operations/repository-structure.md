# Repository structure

The repository groups reusable engines, platform SDKs, integration examples,
release tooling and deployment definitions separately. See the root
[contributor guide](../../CONTRIBUTING.md) for development and publication steps.

## Build boundaries

- `rust/` remains the existing Cargo workspace with the same crate boundaries.
- `sdks/android/` owns the Gradle build and wrapper. `:sample` maps explicitly to
  `examples/android-basic/`; it consumes the root `dist/` AAR, not a project dependency.
- `integrations/jellyfin/plugin/` and `tests/` retain their assembly names and
  namespaces. `global.json` at their common parent pins the .NET SDK.
- `scripts/packaging/` owns binary archive and plugin layout generation.
- `scripts/releases/` owns release versions, candidate verification and manual
  publication. `releases.toml` declares the independent release tracks.
- `deploy/jellyfin/` owns the combined image, Compose and container entrypoint.
- `deploy/standalone/` owns the Linux manager/host/admin suite image and sidecar
  Compose deployment. `examples/docker-integration/` demonstrates binary-copy
  integration; `examples/application-integration/` demonstrates backend supervision.

## Docker paths

Run from the repository root:

```sh
docker compose -f deploy/jellyfin/compose.yaml up -d --build
# Equivalent direct image build:
docker build -f deploy/jellyfin/Dockerfile -t kausway:local .
```

Compose uses the repository root as its build context. Bind mounts resolve beside
the Compose file: `deploy/jellyfin/config`, `cache`, `media` and `kausway-data`.
These directories are ignored by Git and excluded from the image build context.
No existing data is moved automatically. If reusing a local installation with data
at the former repository-root paths, move it deliberately or adjust mount sources.

## Validation

The workflow paths, triggers, caches, artifact paths, packaging scripts and guides
use this layout. Local verification covers version consistency, actionlint,
Rust tests/formatting, plugin tests/packaging, Android release AAR inspection and
independent sample/instrumentation APK compilation, Compose path resolution,
Docker COPY source paths and local documentation links.

**Build Validation** runs on branch pushes, pull requests and main. It calls the
binary, SDK, container and candidate-package workflows and requires all results in
its summary. Final publication is a separate **Publish release** manual dispatch
on a validated main commit. Historical results and environment limitations in
`docs/history/` describe earlier revisions rather than current validation results.
