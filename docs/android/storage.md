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
both paths are explicit. A future manager integration can use the same config
and `--data-root <root>/data`.

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
