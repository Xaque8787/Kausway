# Release versions and component registration

`releases.toml` is the source of truth for release versions. Edit it, synchronize,
review the diff, and commit all affected files together:

```sh
python3 scripts/releases/sync-versions.py
python3 scripts/releases/sync-versions.py --check
python3 -m unittest discover -s scripts/releases -p 'test_*.py'
```

Requires Python 3.11+. The dedicated SemVer parser is vendored and pinned at
`scripts/releases/_vendor/`; no pip installation or network access is needed.
Synchronization neither builds nor publishes and never changes Git tags.
`--check` reports drift with a nonzero exit status and writes no version files.
All proposed edits are validated before any files are written.

## Release tracks

| Track | Members |
| --- | --- |
| Suite | Manager, host, admin, suite archives, Jellyfin plugin |
| Client | Shared client engine, CLI client, Android JNI adapter and Android SDK |
| Container | Image packaging; explicitly selects a suite release |

The prepared suite and client versions are `0.5.0`. Container `0.1.0` selects
suite `0.5.0` for its first release. These declarations do not establish that a
release has been published; inspect GitHub Releases/GHCR for availability.
The tracks remain independent: changing one must not automatically change the others.

The container's `suite_version` is an explicit dependency selection. It can refer
to an older published suite; the sync command does not silently advance it when
the suite version changes. The publisher verifies the selected release exists.

Rust workspace members use explicit generated package versions. The workspace's
default version remains synchronized to the suite for compatibility with existing
consumers. Private config and transport crates also follow the suite version;
their versions do not define the wire protocol or require matching client releases.
Local path dependencies can reference these crates across release-track versions.

The CLI resides in the host Cargo package, but its `--version`
uses the client engine's `VERSION` constant. Android Gradle reads its generated
`release-version.properties`; the SDK POM and sample consume that client version.

## Component registry and future SDKs

`scripts/releases/components.toml` records Rust package ownership, executable
ownership, SDK metadata mappings and Jellyfin metadata locations. Every current
workspace package must be registered. Invalid/missing registrations fail checks.

To add another SDK to the shared client release:

1. Give it generated version metadata and register a unique `sdk_versions` entry.
   A properties-file example is:

   ```toml
   [[sdk_versions]]
   name = "ios"
   track = "client"
   format = "properties"
   path = "sdks/ios/release-version.properties"
   key = "version"
   ```

2. Create that metadata file with a `version=...` line and make the SDK build read
   it. Run synchronization instead of choosing an independent SDK version.
3. If the native build requires a different format, add a narrow format handler
   in `versions.py` and tests; registering an unsupported format must fail.
4. Register any new workspace Rust crate and assign its track.
5. Add platform builds/tests, development artifacts, release asset inventory,
   distribution metadata and any signing configuration in the relevant workflows.
   For new required CI jobs, update the parent graph, its summary dependencies and
   `scripts/ci/validation-policy.json` together; see [CI details](ci.md).
6. Document supported platforms and test that a client-only bump updates the new
   SDK while leaving suite/plugin/container versions unchanged.

Registration is version wiring, not a declaration that iOS or another platform
is already implemented. New SDKs share the client release and do not need separate
release checkboxes under this model.

## Stable and prerelease versions

Use strict SemVer, such as `0.5.0` or `0.5.0-rc.1`. Numeric prerelease identifiers
compare numerically (`rc.2 < rc.10`); a stable version is newer than its prereleases.
Leading zeros, incomplete versions and PEP 440 post-release notation are rejected.
Build metadata (`+...`) is deliberately excluded; commit/build identity belongs
in artifact manifests instead.

Jellyfin assembly/file/plugin versions use the suite's numeric core plus `.0`.
The project's `Version` retains the full suite SemVer for informational metadata.
Synchronization rejects suite components exceeding the assembly range `0..65534`.
Both `0.5.0-rc.1` and `0.5.0` therefore build plugin `0.5.0.0`. Under the current
release system plugin builds/tests still run for prereleases, but the plugin is
distributed only with stable suite releases. The version helper exposes this
distinction; candidate and publication inventory checks enforce it.

## Checks and build callers

`bash scripts/ci/check-version.sh` validates all registrations and prints the suite
version for existing callers. `--track client` or `--track container` prints that
track after validation. Legacy `v...` suite tag syntax remains accepted by the helper
for compatibility; final publication creates only the track-prefixed tags.

Scoped checks exist only for partial source Docker stages:

```sh
python3 scripts/releases/sync-versions.py --check --scope suite --print-version suite
```

Normal development and CI must use the default all-component check. Development
archives record per-binary versions; suite and client downloads are separate.
Source-built image manifests identify suite and container versions separately;
final container publication uses the selected published suite's exact binaries.

See [branch-independent CI](ci.md) and [manual publication](publishing.md) for
validation, version-advancement checks, immutable releases and retry behavior.

## Acceptance checks

The Python tests cover independent bumps, synchronization repeatability, readonly
checks, third-party lock preservation, registration errors, SemVer and plugin mapping.
With Rust dependencies available locally, run the compiled-version check too:

```sh
python3 scripts/releases/check-version-builds.py
```

It compiles a disposable source fixture with different suite/client versions and
verifies all four executable version outputs. Outputs use a `version-acceptance/`
subdirectory of the Cargo target directory; real manifests and normal binaries are
not modified. It also verifies Cargo accepts the synchronized lockfile offline.
