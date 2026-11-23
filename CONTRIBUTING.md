# Contributing

Use any development branch name. Pushes to every branch and pull requests run
**Build Validation**, including tests, cross-platform binary builds, Android SDK
checks, containers and release-candidate packaging. They publish no final releases.

## Normal development

1. Create a branch from the intended base, make the change, and update its user or
   integration documentation. Keep historical records under `docs/history/` separate
   from current instructions.
2. Run checks appropriate to the change. Common repository-root commands are:

   ```sh
   python3 scripts/releases/sync-versions.py --check
   python3 -m unittest discover -s scripts/releases -p 'test_*.py'
   python3 -m unittest discover -s scripts/ci -p 'test_*.py'
   (cd rust && cargo fmt --all -- --check)
   (cd rust && cargo test --workspace --locked)
   ```

   CI also runs lint/MSRV checks, the real-process suite matrix, plugin builds/tests,
   Android SDK/consumer/emulator checks and native Linux container/binary tests.
   See [CI details](docs/development/ci.md) and [pinned tools](docs/operations/versions.md).
3. Push the branch and inspect **Build Validation → Validation summary**. A
   downloadable artifact can exist before another required job fails; wait for the
   full run to pass. Fix failures and push again as needed.
4. Download artifacts for the exact commit/run when practical testing is useful.
   See [binary downloads](docs/operations/development-binaries.md),
   [suite setup](docs/getting-started/suite.md), and
   [local image loading](docs/development/ci.md#test-an-image-locally).
5. Merge after review/validation. The resulting main commit runs the same required
   graph; PR validation of a synthetic merge SHA does not replace that run.

Version bumps are release preparation, not a requirement for each development
commit. Publishing is an explicit maintainer operation after main validation.

## Build from source

Use the [pinned tools](docs/operations/versions.md). The [repository map](docs/operations/repository-structure.md)
explains the Rust, SDK, integration and deployment boundaries. From the repository
root, build the manager, host and admin executables with:

```sh
(cd rust && cargo build --release --locked -p kausway-manager -p kausway-host -p kausway-admin --bins)
```

Executables are under `rust/target/release/` unless `CARGO_TARGET_DIR` is set.
The host package also builds the CLI client. Keep source-built binaries separate
from persistent application data. For other builds, see the
[Android SDK](docs/android/README.md#build-test-and-distribute),
[Jellyfin deployment](docs/integrations/jellyfin.md), and
[suite Dockerfile](docs/integrations/docker-suite.md#source-builds-and-development-images).
Common integration checks, from the repository root:

```sh
dotnet test integrations/jellyfin/tests/Jellyfin.Plugin.Kausway.Tests.csproj -c Release
# Requires Android SDK/NDK and Java 17; see the SDK guide for setup:
bash scripts/android/build-aar.sh
(cd sdks/android && ./gradlew :sample:assembleDebug :kausway-client:assembleDebugAndroidTest)
```

Maven Central publication and iOS bindings are planned separately. To create
downloadable archives, follow [development packaging](docs/operations/development-binaries.md).

## Prepare and publish a release

1. On the development branch, choose the suite, client and/or container versions
   in `releases.toml`. Keep the three tracks independent. Explicitly select
   `container.suite_version` when preparing a container release.
2. Run `python3 scripts/releases/sync-versions.py`, inspect its changes, then run
   `python3 scripts/releases/sync-versions.py --check`. Commit the manifest and
   generated files together. Do not manually bump generated Cargo/plugin/SDK
   metadata or run an unrelated dependency update just to synchronize versions.
3. Push and wait for branch validation. Record practical acceptance results with
   their exact artifact SHA/run/attempt. Merge and wait for main's exact commit to
   pass. Test that main candidate as needed; do not silently substitute an earlier
   branch build in the acceptance record.
4. Manually dispatch **Publish release** on `main`, selecting one or more tracks.
   No final publication follows automatically from a push, version bump or Git tag.
5. Verify the actual released downloads/images, then audit user-facing examples
   and links against those published identities. Keep contributor instructions for
   development artifacts: they remain the supported way to test unreleased changes.

The [release guide](docs/development/publishing.md) defines source pinning,
checks, track dependencies, prerequisites, prereleases, partial-publication recovery,
`latest` promotion and rollback. Real-device acceptance is documented guidance,
not a new workflow approval gate. Existing `v0.4.0` release assets are preserved.

## Add a component, SDK or required check

- Register new Rust package ownership and generated SDK metadata in
  `scripts/releases/components.toml`; SDKs belong to the shared client track.
  Follow [component registration](docs/development/versioning.md#component-registry-and-future-sdks).
- Add platform builds, validation and asset inventory separately. Registration
  alone does not make an SDK distributable or enable a platform.
- When adding a required validation job, update the parent workflow dependencies,
  always-running summary and `scripts/ci/validation-policy.json` together.
- When changing CLI behavior, refresh [the CLI reference](docs/operations/cli-reference.md)
  from the binaries' actual `--help`; keep runtime defaults distinct from parser
  defaults and include new environment bindings.
