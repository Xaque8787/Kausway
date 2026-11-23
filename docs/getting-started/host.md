# Run the standalone host

`kausway-host` forwards paired clients' HTTP requests to one existing application.
It can be supervised by your application or run in a terminal. It does not require
Jellyfin, the manager, or admin. For the managed browser setup, use
[the suite walkthrough](suite.md); for containers, use [the Docker guide](../integrations/docker-suite.md).

## Download and start

Open the [latest suite release](https://github.com/Xaque8787/Kausway/releases/latest)
and download the standalone host archive for the machine that will run it. Verify
its adjacent `.sha256` file before extracting. Linux builds are static musl for
x86-64/ARM64; Android host builds require API 24+ and the matching ARM ABI.
[Archive selection and extraction](suite.md#1-choose-and-verify-a-download) explains
filenames; native Windows/macOS downloads are not currently supplied.

From the extracted host directory, check the origin and start the host:

```sh
./kausway-host doctor --origin http://127.0.0.1:8096
./kausway-host serve --origin http://127.0.0.1:8096
```

Replace the example origin with your application's address. It must be plain
HTTP with no base path or query and reachable from the host process. Leave
`serve` running. Defaults create shared configuration and private host storage
for the executing account; explicit paths can be selected independently:

```sh
./kausway-host serve --config /app/config/config.toml \
  --data-dir /app/data/host --origin http://127.0.0.1:8096
```

See [platform storage and precedence](../architecture/configuration-and-storage.md).
Only one host may use a component directory/configuration at a time.

## Pair and administer clients

In a second terminal, use the same host executable and storage selection:

```sh
./kausway-host invite --label 'My phone'
./kausway-host devices
```

If you supplied custom paths, repeat them on these commands. Copy the entire
one-time `kausway://invite/...` link to a client downloaded from a published
**Client** release. Follow [the CLI walkthrough](local-testing.md) or
[Android/Termux instructions](../android/storage.md#termux-client-test) to pair and
start its proxy. Application login remains separate from Kausway pairing.

To revoke a device:

```sh
./kausway-host revoke '<endpoint-id-from-devices>'
```

These commands work while `serve` is running. They share persisted host state,
and the running host picks up changes. Invitations embed the address published
by the running host in `endpoint-addr.json`.

## Optional standalone admin

Download the admin binary separately or use a suite archive, then run it in
another terminal under the same OS account:

```sh
./kausway-admin --config /app/config/config.toml \
  --data-dir /app/data/admin --host-data-dir /app/data/host
```

Use the same config/host directory as the host. Admin can edit settings and
perform supported live operations, but cannot start/stop/restart this externally
supervised host. Restart it through its owning terminal/application when required.
See [admin setup](../operations/admin.md) and
[host administration](../operations/host-administration.md).

## Configuration reference

| Variable | Default | Meaning |
|---|---|---|
| `KAUSWAY_CONFIG` | Platform config file | Shared saved settings; does not relocate host state |
| `KAUSWAY_DATA_DIR` | Platform data root + `host/` | Identity, device registry and lock file |
| `KAUSWAY_ORIGIN` | `http://127.0.0.1:8096` | Local HTTP service to expose |
| `KAUSWAY_TRANSPORT_POLICY` | stored value, else `auto` | `auto`, `relay_only`, `p2p_only` |
| `KAUSWAY_RELAY_URL` | Iroh default relays | Custom relay URL, or `none` to disable relays (LAN or testing only) |
| `KAUSWAY_WEBSOCKETS` | stored value, else `on` | `on` forwards WebSocket connections; `off` refuses them (the pre-WebSocket behavior) |
| `KAUSWAY_WEBSOCKET_ORIGIN` | stored value, else `rewrite` | `rewrite` sends the origin's own address as the WebSocket `Origin` header; `pass_through` forwards the client's value |
| `RUST_LOG` | `info,iroh=warn,noq=warn,kausway_host=info,kausway_transport=info` | Log filter for `serve` |
| `KAUSWAY_CLIENT_DATA_DIR` | `./kausway-client-data` on non-Android; Android platform root + `client/` | Test client identity and paired host |
| `KAUSWAY_CONTROL_LISTEN` | `127.0.0.1:47950` | Control API address, or `off` |
| `KAUSWAY_CONTROL_ALLOW_REMOTE` | unset | Required to listen on a non-loopback address |
| `KAUSWAY_CONTROL_TOKEN` | generated `control.token` | Fixed control token (32+ characters) instead of the file |

CLI flags take precedence over environment, saved shared settings and defaults.
See [platform storage and precedence](../architecture/configuration-and-storage.md).

## Security and persistence

- The data directory holds the host's private key (`identity.key`). It is created with owner-only permissions on Unix. Back it up: if it is lost, every device must pair again.
- Invitation tokens are stored only as SHA-256 digests. They are single use and expire. Failed attempts are rate limited, and the limit is saved across restarts.
- `relay_only` and `p2p_only` are **best-effort**, not guaranteed. See [transport policies](../architecture/transport-policies.md).
- The control API token grants full control of pairing. It lives in `control.token` (owner-only). Admin and server integrations must keep it out of browser responses.

Keep configuration and identity/pairing state across updates. Stop the old host
before replacing its executable. Ctrl-C stops a terminal host cleanly; restart
with the same storage selection to retain its identity and paired devices.

## Other integrations and development

See [Jellyfin integration](../integrations/jellyfin.md) for the plugin and combined
Jellyfin/host image, or [Android SDK integration](../android/README.md) for an app
that embeds the client instead of launching a CLI. Source builds, tests and release
maintenance are in [Contributing](https://github.com/Xaque8787/Kausway/blob/main/CONTRIBUTING.md).
