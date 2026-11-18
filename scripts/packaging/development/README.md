# Kausway development binaries

These are development artifacts from the `work` branch, not a published release.
`BUILD-INFO.json` records the exact commit, platform, base version, included
executables and payload checksums. The binaries still report base version 0.4.0;
use the manifest/commit to identify the development build. No Node or Rust runtime
is required. Application integration's Python helper is optional example source.

## Verify and extract

Verify the archive's adjacent `.sha256` file, or the supplied `SHA256SUMS`, before
extracting. On Linux/Android terminals with sha256sum:

```sh
sha256sum -c kausway-suite-<development-id>-<target>.tar.gz.sha256
tar -xzf kausway-suite-<development-id>-<target>.tar.gz
cd kausway-suite-<development-id>-<target>
```

GitHub Actions wraps downloaded artifacts in a ZIP. The payload inside that
wrapper is `.tar.gz`; executable permissions are preserved in the tar archive.
Choose an archive matching the OS, CPU architecture and runtime listed in
`docs/operations/development-binaries.md`. Android packages contain only the host
and CLI client and require Android API 24+. Linux binaries are not Android binaries.

## Full Linux suite

Use explicit local paths to keep this development test separate from other installs:

```sh
./kausway run --config "$PWD/local/config.toml" --data-root "$PWD/local/data" --admin-ui
```

Missing directories/configuration are created automatically. Open the expiring
setup link printed to the terminal, create an administrator and sign in. Configure
the host, save, then Start host. Admin's default browser address is
`http://127.0.0.1:47951`. The host initially stays disabled. Restart-required or
launch-overridden values are labeled in Settings. Other installations may already
use these ports; choose different listeners in saved settings or launch overrides.

The manager owns its host/admin children. Ctrl-C gracefully stops the suite.
Admin remains available when just the host is stopped. With no manager configured,
admin cannot start/stop/restart a host owned by another application.

## Standalone host and admin

```sh
./kausway-host serve --config "$PWD/local/config.toml" --data-dir "$PWD/local/data/host"
./kausway-admin --config "$PWD/local/config.toml" --data-dir "$PWD/local/data/admin" --host-data-dir "$PWD/local/data/host"
```

Run these in separate terminals under the same OS account. Standalone admin is
optional and is not shipped for Android. A component archive includes only its
listed executable: download companion components separately, or choose the suite.
The standalone manager still needs `kausway-host`, as a sibling or through
`--host-binary`; optional admin uses the same rule with `--admin-binary`.

## Credentials

```sh
./kausway-admin setup-link --config "$PWD/local/config.toml" --data-dir "$PWD/local/data/admin"
./kausway-admin credentials set --config "$PWD/local/config.toml" --data-dir "$PWD/local/data/admin"
./kausway-admin credentials reset --config "$PWD/local/config.toml" --data-dir "$PWD/local/data/admin"
```

Setup links expire after 15 minutes and are sensitive until consumed. Credential
prompts hide the password. For automation use `--username NAME --password-stdin`.
`set` refuses to replace an existing account; `reset` explicitly replaces it.
Only an Argon2id hash is stored. Credential changes invalidate sessions, not host
identities or pairings. No username/password, key or shared token is pre-created.

## CLI client

```sh
./kausway-host invite --config "$PWD/local/config.toml" --data-dir "$PWD/local/data/host" --label 'My client'
./kausway-test-client --data-dir "$PWD/local/data/client" pair '<invitation-link>'
./kausway-test-client --data-dir "$PWD/local/data/client" proxy --listen 127.0.0.1:8097
```

The host runs separately. Copy its generated one-time link to the client; treat it
as a password until used. Client configuration-file/TUI support is not implemented;
use CLI arguments and its existing environment variables. Run `--help` for commands.

## Storage, applications and updating

The optional `config.example.toml` is a template, never live state. Extracting an
archive does not change OS storage defaults, initialize an account, or move state.
Without explicit paths, each executable selects documented platform defaults.
Embedded applications should select their own persistent configuration/data paths.
See `docs/architecture/configuration-and-storage.md` and `docs/android/storage.md`.

Stop the old suite before replacing executables. Keep its external config/data
paths and use them again after replacement; never delete identity/pairing state
as part of extracting an update. These development archives are not migrations
or replacements for existing published release assets.

For application-owned supervision and a Configure Kausway action, see
`docs/integrations/application-suite.md` and `examples/application-integration/`.
Do not log private integration frames: they can contain setup links or passwords.

Kausway is licensed under MIT OR Apache-2.0. See `licenses/`, including bundled
third-party license texts and the dependency inventory. See
`docs/integrations/docker-suite.md` for the multi-platform Docker suite workflow
and image integration examples; work builds are unpublished test artifacts.
