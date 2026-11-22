# Install and run the suite

The Linux suite contains three independently runnable executables:

| Download component | Executable | Purpose |
| --- | --- | --- |
| Manager | `kausway` | Own and supervise host and optional admin children |
| Host | `kausway-host` | Forward paired clients' requests to your HTTP server |
| Admin | `kausway-admin` | Local browser setup, settings and host administration |

The combined suite archive contains all three. The manager needs a separately
supplied host when downloaded individually; keep companions beside it or select
them with `--host-binary` and `--admin-binary`. Admin's frontend is embedded.
The CLI client and Android SDK are separate client-track downloads.

## 1. Choose and verify a download

Open [Kausway Releases](https://github.com/Xaque8787/Kausway/releases) and choose a
`suite-v<version>` release. One page contains all its architectures and components.
For a full suite, select:

| Linux computer | Archive |
| --- | --- |
| x86-64 / AMD64, including Ubuntu x86-64 | `kausway-suite-<version>-x86_64-unknown-linux-musl.tar.gz` |
| ARM64 / AArch64 | `kausway-suite-<version>-aarch64-unknown-linux-musl.tar.gz` |

`uname -m` reports the machine architecture. These static musl builds work on
ordinary Alpine and glibc-based Linux systems. Android has separate host/client
archives; manager/admin, Windows and macOS downloads are not currently built.

Until a new-layout suite release exists, use a successful **Build Validation**
run's binary artifact for the same CPU. Extract its outer Actions ZIP first.
Development archive versions include `-dev.g<commit>`; use the complete version
from the downloaded filename. Existing `v0.4.0` releases keep their older layout.
See [development downloads](../operations/development-binaries.md).

Download the adjacent `.sha256` file, then run in the download directory:

```sh
SUITE_VERSION='<downloaded-suite-version>'
TARGET=x86_64-unknown-linux-musl  # Use aarch64-unknown-linux-musl for ARM64.
ARCHIVE="kausway-suite-$SUITE_VERSION-$TARGET"
sha256sum -c "$ARCHIVE.tar.gz.sha256"
tar -xzf "$ARCHIVE.tar.gz"
cd "$ARCHIVE"
./kausway --version
./kausway-host --version
./kausway-admin --version
```

Tar preserves executable permissions. Keep binaries apart from persistent state
so extracting an update does not replace settings, credentials or pairings.

## 2. Start manager, admin and host supervision

For an isolated test installation:

```sh
./kausway run \
  --config "$HOME/kausway-test/config/config.toml" \
  --data-root "$HOME/kausway-test/data" \
  --admin-ui
```

The manager creates missing configuration/storage and starts admin. A fresh host
starts disabled. Open the expiring setup link printed in this terminal, create
your administrator account, then sign in. There is no default username/password.
The default browser address is `http://127.0.0.1:47951`.

To intentionally test platform storage defaults, use `./kausway run --admin-ui`
instead. On Linux this normally selects `~/.config/kausway/config.toml` and
`~/.local/share/kausway`, subject to XDG overrides. Defaults follow the account
running the process, rather than the executable's directory. Independent apps
should supply their own directories; see [storage rules](../architecture/configuration-and-storage.md).

Run only one instance against those directories. Stop any earlier test instance
using the same browser/control ports, or choose distinct listeners.

If the setup link expires, request another from a second terminal using the same
installation's executable and paths:

```sh
./kausway-admin setup-link \
  --config "$HOME/kausway-test/config/config.toml" \
  --data-dir "$HOME/kausway-test/data/admin"
```

For terminal provisioning, password reset or application-provided credentials,
see [admin setup and recovery](../operations/admin.md) and
[application integration](../integrations/application-suite.md). Store only the
generated password hash; supply automated passwords through stdin/private pipes.

## 3. Configure your server and start the host

In admin Settings, set **`host.origin`** to the HTTP service address reachable
from the machine running the host, for example `http://127.0.0.1:9014` for a
locally published media-server port. Use a plain HTTP authority without a path or
query. This is the upstream server address, not a manually supplied HTTP header.
`host.websocket_origin` is the separate WebSocket `Origin`-header policy.

Save, then choose **Start**. When changing restart-required settings on a running
host, follow the displayed restart action. An application using application-restart
policy instead asks you to restart that application. Launch overrides take
precedence over saved settings; avoid an origin flag/environment override when
users should edit the origin in admin.

Confirm the host is running before opening Devices/Invitations or creating a
pairing link. Those live views depend on the running host. Persisted records can
still exist on disk while it is stopped.

## 4. Pair a client

Create an invitation in admin, copy its complete `kausway://invite/...` link, and
use it once in your client. Kausway pairing is separate from signing into the media
server. Opening the proxied server still requires its own account where applicable.

For CLI testing, download the matching target from `client-v<version>`, verify and
extract it, then use the same persistent client directory for both commands:

```sh
./kausway-test-client --data-dir "$HOME/kausway-test/client" pair '<complete-invitation-link>'
./kausway-test-client --data-dir "$HOME/kausway-test/client" proxy --listen 127.0.0.1:8097
```

Leave the proxy running and open `http://127.0.0.1:8097` on the client computer.
For Android/Termux, including an isolated pairing with a new host, follow
[the terminal instructions](../android/storage.md#termux-client-test).
Applications embedding the AAR use [the Android SDK guide](../android/README.md).
Suite/client release versions can differ; protocol compatibility determines pairing.

## 5. Stop or update

Ctrl-C in the manager terminal shuts down its owned children. To stop only the
host while retaining admin, run `./kausway stop --data-root "$HOME/kausway-test/data"`.
A subsequent manager launch uses the saved desired host state unless overridden.

Stop and reap the old processes before replacing binaries. Retain the same config
and data directories, including the host identity and pairing state. Restart the
new suite with those paths. Use [the Docker guide](../integrations/docker-suite.md)
for container persistence, sidecars and embedding the binaries in another image.
