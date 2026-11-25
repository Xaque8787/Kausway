# CLI reference

For short-code generation, client input and discovery launch options, see [short-code pairing](short-code-pairing.md).

This reference captures the implemented manager, host, admin and CLI client help.
Run the same `--help` commands against your downloaded version to check for changes.
Paths are relative to the current working directory unless absolute. Explicit flags
override their environment variables. Host options belong after the subcommand;
client data/relay/name options are global and may appear before or after it.

## Storage defaults

Host subcommands accept `--config` / `KAUSWAY_CONFIG` for the shared suite file.
Host data defaults are platform-specific; see [configuration and storage](../architecture/configuration-and-storage.md).
On Android, native host/CLI defaults honor absolute XDG/HOME paths, otherwise use
`<cwd>/kausway/config.toml` and `<cwd>/kausway/data/host` or `client/`.
Explicit host/client data-directory overrides always take precedence. The client
help snapshots below describe non-Android builds (`./kausway-client-data`).
Embedded Android apps should select app-private directories as described in
[Android storage](../android/storage.md).

## Additional parameter rules

- Host `--origin`: plain HTTP authority only; no base path, query
  or HTTPS. Default: `http://127.0.0.1:8096`.
- Host `--policy`: `auto`, `relay_only`, `p2p_only`. Omission uses saved state
  (new state defaults to `auto`). Restrictive policies have the documented
  [best-effort semantics](../architecture/transport-policies.md).
- Host `--websockets`: `on` or `off`; omission uses saved state (new state: on).
- Host `--websocket-origin`: `rewrite` or `pass_through`; omission uses saved state
  (new state: rewrite).
- `--relay-url`: omitted/empty uses Iroh defaults; `none` disables relays and lookup;
  otherwise a custom relay URL. Client and host configure this independently.
- Host `invite --ttl-secs`: 60–86400 seconds, default 900; `--label` defaults to
  `New device`. Invitations are single use. CLI `--name` identifies the client;
  the invitation label names its device entry on the host.
- Host `--control-token`: at least 32 characters; omission uses or creates
  `<data-dir>/control.token`. `--control-listen off` disables the API. Remote bind
  addresses require `--control-allow-remote`; keep privileged access private.
- Client `fetch PATH`: origin-form path starting with `/`, optionally with a query;
  HTTP GET only. `--header 'name:value'` is repeatable. `--range` passes a Range
  header; origin support determines its response. `--output`/`-o` writes the body
  to a file instead of stdout; response metadata goes to stderr.
- Client `proxy --listen`: numeric IP:port socket address, default `127.0.0.1:8097`.
  Keep the proxy on loopback for local use; Ctrl-C stops it. There is no separate
  CLI `unpair` command. Host `revoke` removes device authorization.
- Host `RUST_LOG` controls serve logging; default
  `info,iroh=warn,noq=warn,kausway_host=info,kausway_transport=info`.
- `--help`/`-h` is available for every command; top-level `--version`/`-V` prints
  the binary version. No positional arguments are optional unless help brackets them.

## Manager and admin operations

`kausway run --admin-ui` starts manager-owned host/admin supervision. Fresh saved
state leaves the host disabled until Start. Local `kausway start/stop/restart`
commands require the running manager; they do not adopt an independently owned host.
See [manager operations](manager.md) and [host administration](host-administration.md).

`kausway-admin` without a subcommand starts its local web server. `setup-link`
requires that server to be running and unconfigured. `credentials set` initializes
an account; `credentials reset` explicitly replaces it. Both support a hidden
terminal prompt or `--username NAME --password-stdin`; plaintext password arguments
are not supported. See [setup and recovery](admin.md).

## Complete command help

Snapshots below come from Linux binaries built from this source. Help describes
parser bindings; effective file/state/platform defaults follow the rules above.
Use the corresponding platform build when checking Android-specific defaults.

### kausway

```text
Optional local Kausway process manager

Usage: kausway <COMMAND>

Commands:
  run       Supervise owned host and optional admin children in the foreground
  status    Inspect this manager's owned children
  start     Persist enabled state and start the owned host
  stop      Persist disabled state and stop the owned host
  restart   Persist enabled state, stop and start the owned host
  shutdown  Shut down the manager and its children without changing desired state
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

### kausway run

```text
Supervise owned host and optional admin children in the foreground

Usage: kausway run [OPTIONS]

Options:
      --integration-stdio
          Private newline-delimited JSON control over inherited stdin/stdout; EOF shuts down owned children
      --restart-policy <RESTART_POLICY>
          [env: KAUSWAY_RESTART_POLICY=] [possible values: host, application]
      --application-name <APPLICATION_NAME>
          [env: KAUSWAY_APPLICATION_NAME=]
      --config <CONFIG>
          [env: KAUSWAY_CONFIG=]
      --data-root <DATA_ROOT>
          Suite root: contains host/, manager/ and admin/ (not a host component directory) [env: KAUSWAY_DATA_ROOT=]
      --host-binary <HOST_BINARY>
          [env: KAUSWAY_HOST_BINARY=]
      --admin-binary <ADMIN_BINARY>
          [env: KAUSWAY_ADMIN_BINARY=]
      --host-enabled [<HOST_ENABLED>]
          [env: KAUSWAY_HOST_ENABLED=] [possible values: true, false]
      --admin-ui [<ADMIN_UI>]
          [env: KAUSWAY_ADMIN_ENABLED=] [possible values: true, false]
      --origin <ORIGIN>
          Explicit host origin override (saved values remain unchanged)
      --relay-url <RELAY_URL>

      --policy <POLICY>

      --websockets <WEBSOCKETS>

      --websocket-origin <WEBSOCKET_ORIGIN>

      --shutdown-timeout-secs <SHUTDOWN_TIMEOUT_SECS>
          [env: KAUSWAY_SHUTDOWN_TIMEOUT_SECS=]
      --restart-on-failure [<RESTART_ON_FAILURE>]
          [env: KAUSWAY_RESTART_ON_FAILURE=] [possible values: true, false]
      --restart-backoff-initial-secs <RESTART_BACKOFF_INITIAL_SECS>
          [env: KAUSWAY_RESTART_BACKOFF_INITIAL_SECS=]
      --restart-backoff-max-secs <RESTART_BACKOFF_MAX_SECS>
          [env: KAUSWAY_RESTART_BACKOFF_MAX_SECS=]
      --max-failures <MAX_FAILURES>
          [env: KAUSWAY_MAX_FAILURES=]
      --readiness-timeout-secs <READINESS_TIMEOUT_SECS>
          [env: KAUSWAY_READINESS_TIMEOUT_SECS=]
      --control-listen <CONTROL_LISTEN>
          Override the host control listener. Must be a concrete loopback IP:port [env: KAUSWAY_CONTROL_LISTEN=]
  -h, --help
          Print help
```

### kausway status

```text
Inspect this manager's owned children

Usage: kausway status [OPTIONS]

Options:
      --data-root <DATA_ROOT>  [env: KAUSWAY_DATA_ROOT=]
  -h, --help                   Print help
```

### kausway start

```text
Persist enabled state and start the owned host

Usage: kausway start [OPTIONS]

Options:
      --data-root <DATA_ROOT>  [env: KAUSWAY_DATA_ROOT=]
  -h, --help                   Print help
```

### kausway stop

```text
Persist disabled state and stop the owned host

Usage: kausway stop [OPTIONS]

Options:
      --data-root <DATA_ROOT>  [env: KAUSWAY_DATA_ROOT=]
  -h, --help                   Print help
```

### kausway restart

```text
Persist enabled state, stop and start the owned host

Usage: kausway restart [OPTIONS]

Options:
      --data-root <DATA_ROOT>  [env: KAUSWAY_DATA_ROOT=]
  -h, --help                   Print help
```

### kausway shutdown

```text
Shut down the manager and its children without changing desired state

Usage: kausway shutdown [OPTIONS]

Options:
      --data-root <DATA_ROOT>  [env: KAUSWAY_DATA_ROOT=]
  -h, --help                   Print help
```

### kausway-host

```text
Standalone Iroh host for a local HTTP media server

Usage: kausway-host <COMMAND>

Commands:
  identity  Print this host's endpoint id, creating the identity on first run
  doctor    Check configuration, data directory, identity, state, and origin reachability
  serve     Run the host: accept paired devices and forward their requests to the origin
  invite    Create a one-time invitation link for a new device
  devices   List paired devices
  revoke    Revoke a paired device. Its open sessions end within a few seconds
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

### kausway-host identity

```text
Print this host's endpoint id, creating the identity on first run

Usage: kausway-host identity [OPTIONS]

Options:
      --config <CONFIG>      Shared suite configuration file (does not relocate host state) [env: KAUSWAY_CONFIG=]
      --data-dir <DATA_DIR>  Directory holding identity.key, state.json, and host.lock [env: KAUSWAY_DATA_DIR=]
  -h, --help                 Print help
```

### kausway-host doctor

```text
Check configuration, data directory, identity, state, and origin reachability

Usage: kausway-host doctor [OPTIONS]

Options:
      --config <CONFIG>
          Shared suite configuration file (does not relocate host state) [env: KAUSWAY_CONFIG=]
      --data-dir <DATA_DIR>
          Directory holding identity.key, state.json, and host.lock [env: KAUSWAY_DATA_DIR=]
      --origin <ORIGIN>
          Local HTTP service to forward requests to. Plain http:// only [env: KAUSWAY_ORIGIN=] [default: http://127.0.0.1:8096]
      --policy <POLICY>
          auto, relay_only, or p2p_only. When omitted, the policy stored in state.json is used [env: KAUSWAY_TRANSPORT_POLICY=]
      --relay-url <RELAY_URL>
          Iroh relay URL. Empty uses Iroh's default public relays; `none` disables relays entirely [env: KAUSWAY_RELAY_URL=]
      --control-listen <CONTROL_LISTEN>
          Control API listen address (see protocol/control-v1.md), or `off` [env: KAUSWAY_CONTROL_LISTEN=] [default: 127.0.0.1:47950]
      --control-allow-remote [<CONTROL_ALLOW_REMOTE>]
          Allow the control API to listen on a non-loopback address [env: KAUSWAY_CONTROL_ALLOW_REMOTE=] [default: false] [possible values: true, false]
      --control-token <CONTROL_TOKEN>
          Control API token. When unset, the token in <data-dir>/control.token is used or created [env: KAUSWAY_CONTROL_TOKEN]
      --websockets <WEBSOCKETS>
          Forward WebSocket connections: on or off. When omitted, the setting in state.json is used [env: KAUSWAY_WEBSOCKETS=]
      --websocket-origin <WEBSOCKET_ORIGIN>
          Origin header sent to the origin on WebSocket upgrades: rewrite or pass_through. When omitted, the setting in state.json is used [env: KAUSWAY_WEBSOCKET_ORIGIN=]
  -h, --help
          Print help
```

### kausway-host serve

```text
Run the host: accept paired devices and forward their requests to the origin

Usage: kausway-host serve [OPTIONS]

Options:
      --config <CONFIG>
          Shared suite configuration file (does not relocate host state) [env: KAUSWAY_CONFIG=]
      --data-dir <DATA_DIR>
          Directory holding identity.key, state.json, and host.lock [env: KAUSWAY_DATA_DIR=]
      --origin <ORIGIN>
          Local HTTP service to forward requests to. Plain http:// only [env: KAUSWAY_ORIGIN=] [default: http://127.0.0.1:8096]
      --policy <POLICY>
          auto, relay_only, or p2p_only. When omitted, the policy stored in state.json is used [env: KAUSWAY_TRANSPORT_POLICY=]
      --relay-url <RELAY_URL>
          Iroh relay URL. Empty uses Iroh's default public relays; `none` disables relays entirely [env: KAUSWAY_RELAY_URL=]
      --control-listen <CONTROL_LISTEN>
          Control API listen address (see protocol/control-v1.md), or `off` [env: KAUSWAY_CONTROL_LISTEN=] [default: 127.0.0.1:47950]
      --control-allow-remote [<CONTROL_ALLOW_REMOTE>]
          Allow the control API to listen on a non-loopback address [env: KAUSWAY_CONTROL_ALLOW_REMOTE=] [default: false] [possible values: true, false]
      --control-token <CONTROL_TOKEN>
          Control API token. When unset, the token in <data-dir>/control.token is used or created [env: KAUSWAY_CONTROL_TOKEN]
      --websockets <WEBSOCKETS>
          Forward WebSocket connections: on or off. When omitted, the setting in state.json is used [env: KAUSWAY_WEBSOCKETS=]
      --websocket-origin <WEBSOCKET_ORIGIN>
          Origin header sent to the origin on WebSocket upgrades: rewrite or pass_through. When omitted, the setting in state.json is used [env: KAUSWAY_WEBSOCKET_ORIGIN=]
  -h, --help
          Print help
```

### kausway-host invite

```text
Create a one-time invitation link for a new device

Usage: kausway-host invite [OPTIONS]

Options:
      --config <CONFIG>        Shared suite configuration file (does not relocate host state) [env: KAUSWAY_CONFIG=]
      --data-dir <DATA_DIR>    Directory holding identity.key, state.json, and host.lock [env: KAUSWAY_DATA_DIR=]
      --label <LABEL>          Name shown in the device list, for example "Living room TV" [default: "New device"]
      --ttl-secs <TTL_SECS>    How long the link stays valid, in seconds (60 to 86400)
      --relay-url <RELAY_URL>  Relay to put in the link when the running host's address is not known. Same values as serve [env: KAUSWAY_RELAY_URL=]
  -h, --help                   Print help
```

### kausway-host devices

```text
List paired devices

Usage: kausway-host devices [OPTIONS]

Options:
      --config <CONFIG>      Shared suite configuration file (does not relocate host state) [env: KAUSWAY_CONFIG=]
      --data-dir <DATA_DIR>  Directory holding identity.key, state.json, and host.lock [env: KAUSWAY_DATA_DIR=]
  -h, --help                 Print help
```

### kausway-host revoke

```text
Revoke a paired device. Its open sessions end within a few seconds

Usage: kausway-host revoke [OPTIONS] <ENDPOINT_ID>

Arguments:
  <ENDPOINT_ID>  Endpoint id of the device, as shown by `devices`

Options:
      --config <CONFIG>      Shared suite configuration file (does not relocate host state) [env: KAUSWAY_CONFIG=]
      --data-dir <DATA_DIR>  Directory holding identity.key, state.json, and host.lock [env: KAUSWAY_DATA_DIR=]
  -h, --help                 Print help
```

### kausway-admin

```text
Local Kausway administrator setup and login

Usage: kausway-admin [OPTIONS] [COMMAND]

Commands:
  setup-link   Replace the current expiring setup link (requires running admin)
  credentials
  help         Print this message or the help of the given subcommand(s)

Options:
      --config <CONFIG>
          [env: KAUSWAY_CONFIG=]
      --data-dir <DATA_DIR>
          [env: KAUSWAY_ADMIN_DATA_DIR=]
      --listen <LISTEN>
          [env: KAUSWAY_ADMIN_LISTEN=]
      --browser-base-url <BROWSER_BASE_URL>
          [env: KAUSWAY_ADMIN_BROWSER_BASE_URL=]
      --manager-socket <MANAGER_SOCKET>
          Local manager endpoint for verified host lifecycle controls
      --host-data-dir <HOST_DATA_DIR>
          [env: KAUSWAY_ADMIN_HOST_DATA_DIR=]
      --host-control-listen <HOST_CONTROL_LISTEN>
          [env: KAUSWAY_ADMIN_HOST_CONTROL_LISTEN=]
      --host-control-token <HOST_CONTROL_TOKEN>
          [env: KAUSWAY_CONTROL_TOKEN]
      --restart-policy <RESTART_POLICY>
          [env: KAUSWAY_ADMIN_RESTART_POLICY=] [default: application] [possible values: host, application]
      --application-name <APPLICATION_NAME>
          [env: KAUSWAY_APPLICATION_NAME=] [default: application]
  -h, --help
          Print help
  -V, --version
          Print version
```

### kausway-admin setup-link

```text
Replace the current expiring setup link (requires running admin)

Usage: kausway-admin setup-link [OPTIONS]

Options:
      --config <CONFIG>      [env: KAUSWAY_CONFIG=]
      --data-dir <DATA_DIR>  [env: KAUSWAY_ADMIN_DATA_DIR=]
  -h, --help                 Print help
```

### kausway-admin credentials

```text
Usage: kausway-admin credentials [OPTIONS] <COMMAND>

Commands:
  set    Create initial credentials; refuses to overwrite an account
  reset  Explicitly replace credentials and invalidate existing sessions
  help   Print this message or the help of the given subcommand(s)

Options:
      --config <CONFIG>      [env: KAUSWAY_CONFIG=]
      --data-dir <DATA_DIR>  [env: KAUSWAY_ADMIN_DATA_DIR=]
  -h, --help                 Print help
```

### kausway-admin credentials set

```text
Create initial credentials; refuses to overwrite an account

Usage: kausway-admin credentials set [OPTIONS]

Options:
      --config <CONFIG>      [env: KAUSWAY_CONFIG=]
      --username <USERNAME>
      --data-dir <DATA_DIR>  [env: KAUSWAY_ADMIN_DATA_DIR=]
      --password-stdin       Read the password from stdin instead of a hidden terminal prompt
  -h, --help                 Print help
```

### kausway-admin credentials reset

```text
Explicitly replace credentials and invalidate existing sessions

Usage: kausway-admin credentials reset [OPTIONS]

Options:
      --config <CONFIG>      [env: KAUSWAY_CONFIG=]
      --username <USERNAME>
      --data-dir <DATA_DIR>  [env: KAUSWAY_ADMIN_DATA_DIR=]
      --password-stdin       Read the password from stdin instead of a hidden terminal prompt
  -h, --help                 Print help
```

### kausway-test-client

```text
Test client for kausway hosts

Usage: kausway-test-client [OPTIONS] <COMMAND>

Commands:
  pair    Redeem an invitation link and remember the host
  status  Connect and report authorization, policy, path, and round-trip times
  fetch   Fetch one path and write the body to stdout or a file
  proxy   Serve the host's origin on a local port, for example to open Jellyfin in a browser
  help    Print this message or the help of the given subcommand(s)

Options:
      --data-dir <DATA_DIR>    Directory holding this client's identity and the paired host's address [env: KAUSWAY_CLIENT_DATA_DIR=] [default: ./kausway-client-data]
      --relay-url <RELAY_URL>  Relay URL. Empty uses Iroh's default relays; `none` disables relays [env: KAUSWAY_RELAY_URL=]
      --name <NAME>            Name the host shows for this device [default: kausway-test-client]
  -h, --help                   Print help
  -V, --version                Print version
```

### kausway-test-client pair

```text
Redeem an invitation link and remember the host

Usage: kausway-test-client pair [OPTIONS] <LINK>

Arguments:
  <LINK>

Options:
      --data-dir <DATA_DIR>    Directory holding this client's identity and the paired host's address [env: KAUSWAY_CLIENT_DATA_DIR=] [default: ./kausway-client-data]
      --relay-url <RELAY_URL>  Relay URL. Empty uses Iroh's default relays; `none` disables relays [env: KAUSWAY_RELAY_URL=]
      --name <NAME>            Name the host shows for this device [default: kausway-test-client]
  -h, --help                   Print help
```

### kausway-test-client status

```text
Connect and report authorization, policy, path, and round-trip times

Usage: kausway-test-client status [OPTIONS]

Options:
      --data-dir <DATA_DIR>    Directory holding this client's identity and the paired host's address [env: KAUSWAY_CLIENT_DATA_DIR=] [default: ./kausway-client-data]
      --relay-url <RELAY_URL>  Relay URL. Empty uses Iroh's default relays; `none` disables relays [env: KAUSWAY_RELAY_URL=]
      --name <NAME>            Name the host shows for this device [default: kausway-test-client]
  -h, --help                   Print help
```

### kausway-test-client fetch

```text
Fetch one path and write the body to stdout or a file

Usage: kausway-test-client fetch [OPTIONS] <PATH>

Arguments:
  <PATH>  Origin-form path, for example /System/Info/Public

Options:
      --data-dir <DATA_DIR>    Directory holding this client's identity and the paired host's address [env: KAUSWAY_CLIENT_DATA_DIR=] [default: ./kausway-client-data]
      --range <RANGE>          Range header value, for example bytes=0-1048575
      --header <HEADERS>       Extra header as name:value. Repeatable
      --relay-url <RELAY_URL>  Relay URL. Empty uses Iroh's default relays; `none` disables relays [env: KAUSWAY_RELAY_URL=]
      --name <NAME>            Name the host shows for this device [default: kausway-test-client]
  -o, --output <OUTPUT>
  -h, --help                   Print help
```

### kausway-test-client proxy

```text
Serve the host's origin on a local port, for example to open Jellyfin in a browser

Usage: kausway-test-client proxy [OPTIONS]

Options:
      --data-dir <DATA_DIR>    Directory holding this client's identity and the paired host's address [env: KAUSWAY_CLIENT_DATA_DIR=] [default: ./kausway-client-data]
      --listen <LISTEN>        [default: 127.0.0.1:8097]
      --relay-url <RELAY_URL>  Relay URL. Empty uses Iroh's default relays; `none` disables relays [env: KAUSWAY_RELAY_URL=]
      --name <NAME>            Name the host shows for this device [default: kausway-test-client]
  -h, --help                   Print help
```
