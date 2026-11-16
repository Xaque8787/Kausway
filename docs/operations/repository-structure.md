# Repository structure

The repository groups reusable engines, platform SDKs, integration examples and
deployment definitions separately. Public APIs, artifact names, package versions
and protocol behavior are unchanged by this directory refactor.

## Build boundaries

- `rust/` remains the existing Cargo workspace with the same crate boundaries.
- `sdks/android/` owns the Gradle build and wrapper. `:sample` maps explicitly to
  `examples/android-basic/`; it consumes the root `dist/` AAR, not a project dependency.
- `integrations/jellyfin/plugin/` and `tests/` retain their assembly names and
  namespaces. `global.json` at their common parent pins the .NET SDK.
- `scripts/packaging/` owns binary archive and plugin layout generation.
- `deploy/jellyfin/` owns the combined image, Compose and container entrypoint.
  A future standalone host image belongs in `deploy/standalone/`; none is added here.

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

GitHub Actions must rerun Build Validation, Android SDK and manual Binaries after
this change is pushed. Emulator execution is unavailable locally; the complete
Docker build is subject to the existing Microsoft registry network restriction.
Historical results in `docs/history/` describe earlier revisions.
