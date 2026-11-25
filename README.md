# Kausway

Kausway gives paired devices private access to an existing HTTP application over
[Iroh](https://www.iroh.computer/). Run a host beside your application, then connect
from a client on your phone or computer. A local client proxy lets your browser
or player use the application without understanding Kausway. Direct connections
are used when available, with relay support when needed; no port-forwarding setup
is required for the usual relay-backed deployment.

The optional manager supervises the host and admin UI. The admin UI handles local
setup, settings and invitations. Your media server or other application keeps
running normally and retains its own login. Jellyfin is one supported integration;
the standard suite and container do not include a media server.

## Choose a server setup

Choose one option on the server computer:

| Setup | What to use |
| --- | --- |
| [Binary suite](#binary-suite) | Manager, host and browser admin together; recommended for direct binary use |
| [Standalone host](#standalone-host) | Just the host, managed through terminal commands or your own supervisor |
| [Docker sidecar](#docker-sidecar) | The suite in a container beside your existing application |

Then [connect a client](#connect-a-client) on the device that will access it.

## Download published binaries

- **Suite or individual manager/host/admin:** open the [latest suite release](https://github.com/Xaque8787/Kausway/releases/latest).
- **CLI client or Android SDK:** open [Releases](https://github.com/Xaque8787/Kausway/releases) and choose the newest stable **Client** release. These downloads are separate from the suite.
- **Container:** use `ghcr.io/xaque8787/kausway:latest`; Docker chooses AMD64 or ARM64 automatically.

Choose archives for the computer running each executable and verify their adjacent
`.sha256` files before extracting. Linux uses `x86_64-unknown-linux-musl` or
`aarch64-unknown-linux-musl`. Android has separate host/client builds; manager/admin
are currently Linux components. Suite and client release versions can differ.
See [download and extraction instructions](docs/getting-started/suite.md).

## Binary suite

Extract the published Linux suite archive and run from its directory:

```sh
./kausway run --admin-ui
```

Open the expiring setup link printed in the terminal, create an administrator,
and sign in. The manager creates configuration and storage automatically. On Linux,
normal defaults are `~/.config/kausway/config.toml` and `~/.local/share/kausway`,
subject to XDG overrides. Keep these directories when updating.

In admin **Settings**, set `host.origin` to your application's reachable HTTP
address, such as `http://127.0.0.1:8096`. Save, then choose **Start**. A fresh
installation leaves the host disabled until you start it. Leave the manager
running; Ctrl-C stops its children.

See [suite setup](docs/getting-started/suite.md) for explicit storage paths,
credential recovery and separate component downloads.

## Standalone host

Extract the published host archive and point it at the application:

```sh
./kausway-host serve --origin http://127.0.0.1:8096
```

Leave it running. From a **second terminal**, using the same host executable and
storage selection, create an invitation:

```sh
./kausway-host invite --label 'My phone'
```

Copy the complete invitation link to your client. The standalone host does not
start the manager or admin UI. See [host operations](docs/getting-started/host.md)
for storage, configuration and an optional independently running admin.

## Docker sidecar

Save the following as `compose.yaml` in a new directory. It is also available as
[the standalone Compose file](deploy/standalone/compose.yaml):

```yaml
services:
  kausway:
    image: ghcr.io/xaque8787/kausway:latest
    restart: unless-stopped
    init: true
    stop_grace_period: 45s
    ports:
      - "127.0.0.1:47951:47951"
    volumes:
      - kausway-config:/config
      - kausway-data:/data
    # Image defaults: KAUSWAY_CONFIG=/config/config.toml, KAUSWAY_DATA_ROOT=/data.
    # No command or host origin override: complete first-run setup in admin.
    # Set origin to http://<application-service-name>:<port> on a shared network.

volumes:
  kausway-config:
  kausway-data:
```

Start it and read the setup link:

```sh
docker compose pull
docker compose up -d
docker compose logs kausway
```

Open the setup link on the Docker host, create the admin account, then configure
`host.origin`, save and **Start** the host. The two volumes retain configuration,
credentials, host identity and pairings across container recreation.

The origin must be reachable **from inside Kausway's container**:

- For another container on the same Docker network, use its service name and internal port, such as `http://media-server:8096`.
- For a service reachable on your LAN, use its reachable LAN address and port.
- Container `127.0.0.1` refers to Kausway itself; another Compose project's service name will not resolve unless the containers share a network.

Only the local admin port is published. For host-machine applications, shared
networks, bind mounts, updates or embedding the binaries in your own image, see
[the Docker guide](docs/integrations/docker-suite.md). `latest` follows published
stable container releases, independently of GitHub's latest suite release.

## Connect a client

Start the host, then create an invitation in admin or with `kausway-host invite`.
On the client device, extract the CLI archive from a published **Client** release:

```sh
./kausway-test-client --data-dir "$HOME/kausway-client" pair '<complete-invitation-link>'
./kausway-test-client --data-dir "$HOME/kausway-client" proxy --listen 127.0.0.1:8097
```

For a shorter input, select **Also generate a short code** in admin and wait for
publication acknowledgment. Pass the generated `K1-XXXXXX-XXXXXX-XXXXXX` code to
`pair` in place of the full URL. Both inputs redeem the same invitation once.
See [short-code pairing](docs/operations/short-code-pairing.md) for compatible
versions and discovery requirements.

Leave the proxy running and open `http://127.0.0.1:8097` in a browser on that device.
Use the same client data directory on later starts to keep pairing. Invitations
expire and are single use. Kausway pairing does not replace your application's
own username/password.

- [Android/Termux walkthrough](docs/android/storage.md#termux-client-test): select the phone's binary and keep its data in private storage.
- [End-to-end CLI walkthrough](docs/getting-started/local-testing.md): test with a simple HTTP server, fetch files and use the proxy.
- [Android SDK integration](docs/android/README.md): embed the client engine directly in an app instead of running the CLI.

## More documentation

| Topic | Guide |
| --- | --- |
| Storage defaults and configuration precedence | [Configuration and storage](docs/architecture/configuration-and-storage.md) |
| Account setup and recovery | [Admin operations](docs/operations/admin.md) |
| Settings, devices, invitations and restarts | [Host administration](docs/operations/host-administration.md) |
| Every command and option | [CLI reference](docs/operations/cli-reference.md) |
| Embed/supervise the suite in an application | [Application integration](docs/integrations/application-suite.md) |
| Jellyfin plugin and combined Jellyfin deployment | [Jellyfin integration](docs/integrations/jellyfin.md) |
| Direct/relay policy behavior | [Transport policies](docs/architecture/transport-policies.md) |

## Development and contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for source builds, tests and contribution
steps. [Developer documentation](docs/development/ci.md) covers development
artifacts, packaging, version synchronization and manual release publication.
