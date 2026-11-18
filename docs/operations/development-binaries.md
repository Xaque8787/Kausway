# Development binary distributions (Phase 6)

The **Work development binaries** workflow compiles the components and produces
checksummed development `.tar.gz` archives. It runs on pushes to `work`. Every job
also checks `github.ref == refs/heads/work`, so a main/tag/manual-main invocation
does not build or publish. Existing release, Build Validation and Android SDK
workflow files are unchanged. No release permissions, tags or GitHub Releases are
used by the development workflow. Pushes are the primary trigger: GitHub may not
expose manual dispatch until the workflow file exists on the default branch; its
job guards still restrict execution to `work` after a later merge.

The base version remains `0.4.0`. Archive/artifact names contain
`0.4.0-dev.g<12-character-commit>`, while `BUILD-INFO.json` records the full commit.
`--version` still reports the base version: use the manifest to identify a work
build. Published 0.4.0 assets are never overwritten. A coordinated 0.5.0 bump is
reserved for the final work-branch release-preparation commit before merging.

## Get the binaries

Open a successful **Work development binaries** Actions run for the desired work
commit and download the matching `development-kausway-...-<target>` artifact.
GitHub transports Actions artifacts inside ZIP files; the deliverables inside
are `.tar.gz` files, preserving executable modes.

The workflow currently builds:

| Target | Payload | Runtime validation |
| --- | --- | --- |
| `x86_64-unknown-linux-musl` | Linux x86-64 manager, host, admin, CLI client | Native runner, extracted suite and individual binaries |
| `aarch64-unknown-linux-musl` | Linux ARM64 manager, host, admin, CLI client | Native ARM64 runner, extracted suite and individual binaries |
| `aarch64-linux-android` | Android arm64-v8a host and CLI client | Cross-build/package verification only; API 24+ |
| `armv7-linux-androideabi` | Android armeabi-v7a host and CLI client | Cross-build/package verification only; API 24+ |

Linux musl executables must have no dynamic interpreter or shared-library dependency;
the package script checks this. Architecture is checked from ELF headers. Android
packages must instead have an Android linker and the matching ELF architecture.
A 64-bit CPU with a 32-bit Android userland needs the 32-bit Android build.
Linux and Android executables are not interchangeable.

The separate Android SDK workflow passed on a subsequent work push after an earlier
emulator pairing timeout. These host/client package checks do not execute Android
binaries or replace SDK runtime validation. The existing SDK workflow is unchanged
and is not a prerequisite for development binary artifacts. No Android
manager/admin, Windows, macOS, 32-bit Linux or other
architecture support is introduced by this workflow. Windows/macOS native build
and runtime validation remain deferred.

## Payload layout

A Linux target produces one suite and four component archives:

```text
kausway-suite-<development-id>-<target>.tar.gz
kausway-manager-<development-id>-<target>.tar.gz
kausway-host-<development-id>-<target>.tar.gz
kausway-admin-<development-id>-<target>.tar.gz
kausway-test-client-<development-id>-<target>.tar.gz
<each archive>.sha256
SHA256SUMS
```

Android produces `kausway-android-cli-...tar.gz` containing host/client plus their
two individual component archives. Each archive has a single matching top-level
folder. Manager's actual executable name is `kausway`; the other executables are
`kausway-host`, `kausway-admin` and `kausway-test-client`.

Inside each archive:

```text
<archive-name>/
  <included executables>
  README.md
  BUILD-INFO.json
  config.example.toml
  docs/
  protocol/
  examples/application-integration/
  licenses/
```

The same independently runnable compiled binaries appear in the suite and component
archives. A manager component still requires the separate host binary and optionally
admin (sibling executables or explicit binary overrides). Admin can run without a
manager, with lifecycle controls omitted. Its frontend is embedded; no Node runtime
or separate asset server is needed. Android archives contain documentation for the
whole project but ship only host/client executables.

No live config, password, identity, paired-device record, control token or session
is included. The example config contains no administrator account or shared secret.
Client configuration-file/TUI support remains separate work; use existing CLI flags.

## Checksums and provenance

With the artifact downloaded/extracted:

```sh
sha256sum -c SHA256SUMS
# Or verify one component using its adjacent .sha256 file.
tar -xzf kausway-suite-<development-id>-<target>.tar.gz
```

`BUILD-INFO.json` records the base version, exact source commit, development ID,
target, runtime requirements, supplied binary names and checksums for every payload
file. Local packaging records `source_dirty: true` when uncommitted changes exist;
workflow checkouts are clean. Internal build-machine paths are not distributed.

Tar entries have fixed commit timestamps, normalized owners and explicit 0755/0644
modes. Gzip headers are deterministic. The same inputs produce the same archive;
this is packaging reproducibility, not a claim of bit-identical compiler builds.
Checksums detect corruption and identify payloads; there is no release signing step.

Licenses include Kausway's declared MIT/Apache-2.0 texts and target-filtered Cargo
third-party notices. The inventory conservatively includes normal/build dependencies
and excludes dev-only dependencies. For published crate archives lacking a license
file, Apache-2.0 is selected only if explicitly permitted by their declared license;
these cases are identified in the inventory rather than silently omitted.
Version-specific supplemental notices under `scripts/packaging/notices/` retain
their exact upstream source revision when a published crate omits its license.

## First launch and persistence

From a freshly extracted Linux suite:

```sh
./kausway run --config "$PWD/local/config.toml" --data-root "$PWD/local/data" --admin-ui
```

These explicit paths isolate the development installation. Missing directories and
config are created automatically. Open the expiring setup link printed to the
terminal, create an administrator, configure the host and choose Start. Saved data
lives outside the packaged payload; do not ship it in another archive.

Omitting path arguments uses the [platform defaults](../architecture/configuration-and-storage.md).
Extracting an archive never changes those defaults. On Android, see
[Android storage](../android/storage.md) for HOME/XDG and working-directory fallback.

For independently supervised components:

```sh
./kausway-host serve --config /app/config/kausway.toml --data-dir /app/data/host
./kausway-admin --config /app/config/kausway.toml --data-dir /app/data/admin --host-data-dir /app/data/host
```

The examples above run in separate terminals. See [admin setup/recovery](admin.md),
[host administration](host-administration.md), and [application integration](../integrations/application-suite.md)
for credentials, overrides, readiness, restart policy and supervision.

For an update, stop/reap the old processes before replacing binaries. Retain and
reuse the selected external config and data directories. Do not delete identities
or pairings, copy live state into distribution files, or silently select another
path when storage is inaccessible. See [Docker suite integration](../integrations/docker-suite.md)
for image builds, publication and persisted volumes.

## Local packaging and checks

Requires Python 3.12+, git and readelf. Build the desired target first, then:

```sh
cargo metadata --manifest-path rust/Cargo.toml --locked --format-version 1 \
  --filter-platform x86_64-unknown-linux-musl > /tmp/kausway-metadata.json
python3 scripts/packaging/package-development.py \
  --binary-dir rust/target/x86_64-unknown-linux-musl/release \
  --metadata /tmp/kausway-metadata.json --target x86_64-unknown-linux-musl \
  --commit "$(git rev-parse HEAD)" --output /tmp/kausway-development-artifacts
python3 scripts/ci/development-package-smoke.py /tmp/kausway-development-artifacts --run
```

The output directory must be empty. Unsupported/wrong-architecture binaries,
dynamically linked musl inputs, unrelated workspace metadata or a non-HEAD commit
are rejected rather than producing misleading artifacts. The existing release
packaging script is not used or modified.

For local GNU/Linux packaging validation only, the script also accepts
`x86_64-unknown-linux-gnu`; this is not a published workflow target or a static musl
claim. Android archives are verified without trying to execute them on the Linux
runner. Successful remote matrix runs establish actual musl/ARM64 build results;
local GNU tests alone do not.
