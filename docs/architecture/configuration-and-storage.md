# Shared configuration and storage (Phase 1)

The `kausway-config` crate supplies the schema, platform paths, private filesystem
helpers, configuration revisions and diagnostic metadata for the host and future
manager/admin. The manager is now implemented in Phase 2; see [manager operations](../operations/manager.md). The admin executable remains planned.

## Host use

```sh
kausway-host serve
kausway-host serve --config /app/config/config.toml --data-dir /app/data/host
```

All host subcommands accept `--config` (`KAUSWAY_CONFIG`) and `--data-dir`
(`KAUSWAY_DATA_DIR`). Config selection never relocates identity/state. CLI wins
over environment, which wins over saved configuration, then built-in defaults.
Runtime overrides do not rewrite the file. `--control-allow-remote=false` can
explicitly override a saved true value; the flag without a value means true.

| Platform | Configuration file | Suite data root |
| --- | --- | --- |
| Linux | `$XDG_CONFIG_HOME/kausway/config.toml` or `~/.config/kausway/config.toml` | `$XDG_DATA_HOME/kausway` or `~/.local/share/kausway` |
| Android native CLI | Absolute XDG/HOME paths as on Linux; otherwise `<cwd>/kausway/config.toml` | Absolute XDG/HOME paths as on Linux; otherwise `<cwd>/kausway/data` |
| macOS | `~/Library/Application Support/Kausway/config.toml` | `~/Library/Application Support/Kausway` |
| Windows | `%LOCALAPPDATA%\Kausway\config.toml` | `%LOCALAPPDATA%\Kausway` |

Paths belong to the account running the process. Relative XDG roots are ignored.
Default component data directories are `host/`, `manager/`, `admin/`, `client/`.
The host uses `host/`; its explicit `--data-dir` selects the component directory,
not the suite root. The manager passes these same resolved paths to
children. Independent applications should choose distinct explicit paths.
The Android CLI client now uses the shared Android default data root plus `client/`.
Its explicit `--data-dir` / `KAUSWAY_CLIENT_DATA_DIR` wins. Other CLI client platforms
retain `./kausway-client-data`; client configuration-file/TUI integration is separate work.
The Android SDK already uses app-private storage and is unchanged.
See [Android storage](../android/storage.md) for terminal and app integration examples.

Windows/macOS path rules are implemented assumptions, not platform validation.
Native build and runtime checks will be added with their release workflows.

Missing configuration is initialized automatically. Only the selected component's
state is created. Inaccessible or corrupt storage fails rather than selecting a
fallback. The host locks storage before initializing identity on serve startup.

## Schema

```toml
schema_version = 1

[host]
origin = "http://127.0.0.1:8096"
policy = "auto"
relay_url = ""
websockets = true
websocket_origin = "rewrite"

[host.control]
listen = "127.0.0.1:47950"
allow_remote = false

[manager]
host_enabled = false
shutdown_timeout_secs = 15
restart_on_failure = true

[admin]
enabled = false
listen = "127.0.0.1:47951"

[admin.auth]
# Both username and encoded Argon2id password_hash must be supplied together.
# No plaintext password or default account is created.
```

Host values are optional. For policy and WebSocket settings, omission preserves
existing state.json behavior: the live control API's persisted values remain in
force. Explicit file/environment/CLI values override that state at startup.
The shared metadata describes startup resolution; it is not a live host snapshot.
An admin API must combine it with authoritative host state in a later phase.
Unknown fields and unsupported schema versions fail. Control tokens remain in
protected host state or explicit runtime overrides, never saved settings.
The manager reads its settings when explicitly launched. The admin section
provides configuration for the optional launch hook; the web server is a later phase.
Configuration alone never starts a process.

## Persistence and security

`Store::load_or_create` and `Store::save` coordinate using a sibling lock file.
Each snapshot has a SHA-256 revision of its exact file contents. Save requires
that revision and rejects stale writers. Writes sync a private temporary file,
atomically rename it, and sync the parent directory on Unix. Lock files remain
on disk; OS locks are released when their file handles close.

On Unix, configuration/component directories are restricted to 0700 and secret
files to 0600. Windows currently inherits directory ACLs; the embedding
application must use a private per-user/service location. Windows/macOS runtime
validation and explicit ACL hardening remain release-platform work; they are not
claimed tested by Linux checks.

Configuration Debug output and parse errors omit contents. Credential metadata
is redacted before serialization. The configuration object itself serializes
secrets for protected persistence and must not be returned directly by an API.

## Setting inventory

| Setting/operation | Classification | Authority |
| --- | --- | --- |
| Origin | Restart | Startup configuration |
| Relay selection | Restart | Startup configuration |
| Control listener/remote opt-in/token | Restart | Startup configuration/protected state |
| Transport policy | Existing live API; startup override supported | Host |
| WebSocket enable/origin-header policy | Existing live API; startup override supported | Host |
| Data/config location | Launch only; read-only in future admin | Launcher |
| Identity and paired devices | Managed state | Host |
| Invitations, cancellation, revocation | Existing immediate operations | Host |
| Session/path diagnostics | Read-only live information | Host |
| Manager desired state/restart policy | Managed by the optional manager | Manager |
| Admin listener | Restart | Admin |
| Admin credentials | Future credential workflow; invalidate sessions | Admin |

No new live configuration endpoints or lifecycle controls are added in Phase 1.
