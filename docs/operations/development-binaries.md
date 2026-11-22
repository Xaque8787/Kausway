# Development binary distributions (Phase 6)

**Build Validation** runs on every branch push and pull request, calling
**Development binaries** to produce checksummed `.tar.gz` archives. Its stable
**Validation summary** also requires SDK, plugin and container checks. Component
workflows can be manually dispatched once available on the default branch.
See [CI and development artifacts](../development/ci.md) for the exact graph.

Suite/client versions come from `releases.toml` and are currently both `0.4.0`.
Archive names include the component's version plus `-dev.g<12-character-commit>`;
BUILD-INFO records the full source and run identity. `--version` reports each
binary's suite/client track, not the development suffix. Published assets are
not overwritten. Release version bumps are a separate preparation step.

## Get the binaries

Open a successful **Build Validation** run for your branch/commit, then download
`development-kausway-...-<target>-run<id>-attempt<number>` under Artifacts. Standalone
manual Development binaries runs provide the same payloads. GitHub wraps artifacts
in ZIPs; the actual deliverables inside are `.tar.gz`, preserving executable modes.

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

The parent also requires Android SDK build/consumer/emulator validation; these
cross-built host/client packages alone do not execute Android binaries. Artifacts
may be available before other required checks finish, so confirm the summary.
No Android manager/admin, Windows, macOS or 32-bit Linux workflow target is added.

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

Android produces separate host and test-client archives, with no combined bundle.
The Linux suite contains manager/host/admin; the test-client archive is separate.
Each archive has a single matching top-level
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

The same independently runnable suite binaries appear in suite and component
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
are rejected rather than producing misleading artifacts. This development packager
is separate from [candidate assembly and manual publication](../development/publishing.md).

For local GNU/Linux packaging validation only, the script also accepts
`x86_64-unknown-linux-gnu`; this is not a published workflow target or a static musl
claim. Android archives are verified without trying to execute them on the Linux
runner. Successful remote matrix runs establish actual musl/ARM64 build results;
local GNU tests alone do not.
