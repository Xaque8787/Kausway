# Android storage

Android host and CLI executables do not have an Android `Context`. Their terminal
storage defaults therefore differ from the client SDK's app-private defaults.

## Standalone host and CLI

Explicit `--config` / `KAUSWAY_CONFIG` and `--data-dir` /
`KAUSWAY_DATA_DIR` (host) or `KAUSWAY_CLIENT_DATA_DIR` (client) take precedence.
Selecting a config file does not relocate component data.

When omitted, resolve each root independently:

| Root | First choice | Second choice | No absolute environment root |
| --- | --- | --- | --- |
| Configuration | `$XDG_CONFIG_HOME/kausway` | `$HOME/.config/kausway` | `<working-directory>/kausway` |
| Data | `$XDG_DATA_HOME/kausway` | `$HOME/.local/share/kausway` | `<working-directory>/kausway/data` |

Only absolute, nonempty environment roots are used. Relative HOME/XDG values are
ignored. A terminal environment such as Termux can therefore use its private HOME.
An adb shell without HOME/XDG uses the directory from which the command is run.
The fallback uses the working directory, not the executable's location.

For example, from a writable working directory with HOME/XDG unset:

```text
working-directory/
  kausway/
    config.toml
    data/
      host/
        identity.key
        state.json
        ...
      client/
        ...
```

Only the component being used is initialized. The host creates its shared
configuration automatically. The CLI client initializes its data; it does not yet
consume or create a client configuration file.

Changing working directory changes this fallback installation's location. Use
explicit absolute paths when identity must be independent of the launch directory:

```sh
kausway-host serve \
  --config /data/local/tmp/my-kausway/config.toml \
  --data-dir /data/local/tmp/my-kausway/data/host

kausway-test-client --data-dir /data/local/tmp/my-kausway/data/client status
```

Choose a location writable and appropriately protected for the executing account;
`/data/local/tmp` is an adb-shell example, not an application storage location.
Unix directories are restricted to 0700 and secret files to 0600. If the selected
location is inaccessible or corrupt, fail there; never silently retry elsewhere.

## Embedded Android applications

Applications should supply dedicated app-private paths rather than relying on
terminal environment variables or process working directories. For host launch
arguments, obtain paths from the application Context:

```kotlin
val root = File(context.noBackupFilesDir, "kausway")
val hostArguments = listOf(
    "serve",
    "--config", File(root, "config.toml").absolutePath,
    "--data-dir", File(root, "data/host").absolutePath,
)
```

Pass these arguments through the application's existing supported native-process
launcher. This is directory selection, not a claim that arbitrary downloaded
executables can be launched from app storage on every Android version.
The host creates the required directories and files; no HOME is required when
both paths are explicit. Android builds supply host/client only; manager/admin
are Linux suite components and are not Android integration targets.

## Termux client test

Download the Android **test-client** archive matching the phone's supported
userland ABI, plus its `.sha256`, from the newest stable **Client** release on
[GitHub Releases](https://github.com/Xaque8787/Kausway/releases). Do not use a Linux musl archive
or the SDK AAR for these terminal commands. If downloaded through the phone's
browser, grant Termux shared-storage access once with `termux-setup-storage`.

Copy both downloads into Termux-private storage before extracting/running:

```sh
pkg install coreutils tar
mkdir -p "$HOME/kausway-cli"
cd "$HOME/kausway-cli"
# Replace these placeholders with the complete downloaded version and target.
CLIENT_VERSION='<downloaded-client-version>'
TARGET=aarch64-linux-android  # Use armv7-linux-androideabi for 32-bit ARM.
ARCHIVE="kausway-test-client-$CLIENT_VERSION-$TARGET"
cp "$HOME/storage/downloads/$ARCHIVE.tar.gz" "$HOME/storage/downloads/$ARCHIVE.tar.gz.sha256" .
sha256sum -c "$ARCHIVE.tar.gz.sha256"
tar -xzf "$ARCHIVE.tar.gz"
cd "$ARCHIVE"
./kausway-test-client --version
```

For [development downloads](../development/ci.md), extract the outer Actions ZIP first and use the full
`-dev.g<commit>` version in its contained filename. Android shared storage is for
transferring downloads; run executables and keep identities in Termux-private HOME.

To pair with a new test host without disturbing an existing client, choose a
previously unused data directory. Create a fresh invitation in the running host's
admin UI and paste its **complete** `kausway://invite/...` link below. With a
short-code-capable host/client, you can instead select **Also generate a short
code**, wait for publication acknowledgment and pass the code to `pair`. See
[short-code pairing](../operations/short-code-pairing.md) for version requirements.

```sh
CLIENT_DATA="$HOME/.local/share/kausway/client-new-host"
./kausway-test-client --data-dir "$CLIENT_DATA" pair '<complete-invitation-link>'
./kausway-test-client --data-dir "$CLIENT_DATA" proxy --listen 127.0.0.1:8097
```

Keep the proxy running and open `http://127.0.0.1:8097` in the phone's browser.
Sign into the media server with its own account. Use the same `CLIENT_DATA` on
subsequent starts to retain pairing. Another clean test should use another unused
directory; deleting the earlier directory is unnecessary. Invitations expire and
are single use, so request another after a successful redemption. Older clients
expecting the former invitation scheme should be replaced with a current client.

## Client SDK storage

The existing client SDK already defaults to:

```kotlin
File(context.noBackupFilesDir, "kausway-client")
```

It also supports an explicitly selected app-private directory through
`KauswayConfig(storageDirectory = ...)`. The SDK rejects paths outside its
application-private directory. Its defaults, backup exclusion and JNI contract
are unchanged by the native CLI fallback.

## Validation scope

Injected-environment tests run the Android path-selection algorithm on Linux,
covering missing/relative variables, HOME/XDG precedence, independent roots and
working-directory errors. Android-target checks compile the actual Android code
branches. These are not a replacement for running binaries on a device/emulator.
Windows/macOS validation remains deferred until their native build workflows are
introduced; Linux passing does not certify those platforms.
