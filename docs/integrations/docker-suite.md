# Docker suite and application integration

The published suite image contains `kausway` (manager), `kausway-host` and
`kausway-admin`. Its default command starts the manager with admin enabled.
It runs beside your existing HTTP application; it does not contain a media server.
The CLI client is a separate download from a published **Client** release.
For the source-built Jellyfin/host/plugin image, see [Jellyfin integration](jellyfin.md).

## Start with Compose

Save this complete file as `compose.yaml` in a new directory. No repository clone,
build command, origin override or initial credentials are required. The same file
is maintained at [deploy/standalone/compose.yaml](../../deploy/standalone/compose.yaml).

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

```sh
docker compose pull
docker compose up -d
docker compose logs kausway
```

Open the expiring setup link on the Docker host, create an administrator and sign
in. In **Settings**, set `host.origin` to your application's reachable plain HTTP
address without a path or query, save, then choose **Start**. A fresh suite leaves
the host disabled. Devices and invitations become available after it starts.
The manager owns its host/admin children; it does not adopt an already running host.
If the initial link expires before account creation, issue a new one with
`docker compose exec kausway kausway-admin setup-link --data-dir /data/admin`.
After account creation, sign in with that account; see [credential recovery](../operations/admin.md#terminal-provisioning-and-recovery).

Leave `KAUSWAY_HOST_ENABLED` unset when the saved desired state should survive
container recreation. That variable selects initial state on each launch;
Start/Stop can still change it during the current run.

The example uses `latest`, which follows the most recent published stable
**container** release, not development commits or GitHub's latest suite release.
Docker selects `linux/amd64` or `linux/arm64` from the published image index.
No Android, Windows, macOS or 32-bit Linux container is published. For a deployment
that must stay on one image, select a published version tag or image-index digest.

## Reach your application

The origin must be reachable from the Kausway container:

- On a shared Compose network, use the application's service name and **internal**
  port, for example `http://media-server:8096`.
- Separate Compose projects normally have separate networks. Attach both services
  to an explicitly shared Docker network before using service-name addresses.
- A LAN-reachable service can use its LAN address and listening port.
- `127.0.0.1` inside this container refers to Kausway itself, not another container
  or the Docker host.

For an application already running in a different Compose project, find its
network with `docker network ls`. Merge this fragment into the Kausway Compose
file, replacing `your-media-project_default` with that existing network's name:

```yaml
services:
  kausway:
    networks:
      - media
networks:
  media:
    external: true
    name: your-media-project_default
```

Recreate the Kausway service with `docker compose up -d`. It can then use the
application's service name/internal port as origin, provided the application is
attached to that network. Keep the existing image, ports and volumes in the file.

For a host-machine service, Docker Desktop supplies `host.docker.internal`.
On Linux Docker Engine, add this to the `kausway` service:

```yaml
extra_hosts:
  - "host.docker.internal:host-gateway"
```

Then use `http://host.docker.internal:<application-port>` as origin. The application
must listen on an interface reachable from Docker, and its firewall must permit
that traffic. A service bound only to host `127.0.0.1` is not reachable through the
Linux gateway mapping. Use a reachable interface or a deliberately configured
network arrangement.

Only admin's browser port is mapped, bound to the Docker host's loopback. Host
control remains `127.0.0.1:47950` **inside the suite container**; it has no published
port. Manager/admin private sockets are files under `/data`, not network services.
`EXPOSE` is metadata, not a port publication. Compose `command:` replaces CMD;
`environment:` sets environment values. Neither port publication nor Docker
networking configures the host origin.

Peer transport needs working outbound networking, DNS and relay access. No fixed
inbound peer port is published by this example; direct connectivity depends on the
network, with relay fallback under the selected policy. Host control is unrelated
to peer transport.

Admin listens on `0.0.0.0:47951` inside the image so Docker can forward it. Its
browser origin remains localhost. On a remote Docker server, use SSH forwarding
from your local computer:

```sh
ssh -N -L 47951:127.0.0.1:47951 user@docker-server
```

Open the logged setup link locally while that tunnel is running. This is local
management; public hostname/reverse-proxy admin deployment is outside the current
admin URL policy. To change the local browser port, update **both** values:

```yaml
ports:
  - "127.0.0.1:48000:47951"
environment:
  KAUSWAY_ADMIN_BROWSER_BASE_URL: http://127.0.0.1:48000
```

## Run without Compose

```sh
docker run -d --name kausway --init --restart unless-stopped \
  --stop-timeout 45 \
  -p 127.0.0.1:47951:47951 \
  -v kausway-config:/config -v kausway-data:/data \
  ghcr.io/xaque8787/kausway:latest
docker logs kausway
```

Use the same setup and origin steps above. To update a Compose deployment, run
`docker compose pull` followed by `docker compose up -d`; keep both volumes.

## Persistent configuration and data

| Container path | Contents |
| --- | --- |
| `/config/config.toml` | Shared saved settings and administrator password hash |
| `/data/host/` | Host identity, pairings and host state |
| `/data/admin/` | Admin component state and local IPC |
| `/data/manager/` | Manager component state and local IPC |

Persist **both** `/config` and `/data` across replacement containers. Named volumes
are initialized with image directory ownership; the default runtime UID/GID is
`10001:10001`. For bind mounts, create writable directories with that ownership:

```sh
mkdir -p ./kausway/config ./kausway/data
sudo chown -R 10001:10001 ./kausway/config ./kausway/data
# Replace the two -v options with:
# --mount type=bind,src="$PWD/kausway/config",dst=/config
# --mount type=bind,src="$PWD/kausway/data",dst=/data
```

Mount directories rather than a missing config file. Config saving uses atomic
file replacement and requires a writable parent directory. Do not share live
component directories between separate suites or use `compose down -v` when
retaining state. Stop the old suite before upgrading binaries/containers.

The image supports a read-only root with writable config/data mounts and a `/tmp`
tmpfs. An arbitrary `--user` is possible if the caller provisions writable mounts;
it does not automatically chown existing data. No root-only startup chown runs.

Image environment selects `/config/config.toml`, `/data`, and the container admin
listener; these launch overrides take precedence over saved settings. Other saved
settings remain editable. To change those overridden values, change the container
environment and recreate it. See [configuration precedence](../architecture/configuration-and-storage.md).

Earlier development images selected `/config/kausway.toml`. When reusing a test
volume containing that file, deliberately select it with
`KAUSWAY_CONFIG=/config/kausway.toml`, or stop the suite and rename it to
`config.toml` if no different `config.toml` needs preserving. Files are not renamed
or merged automatically. Inspect the selected file with `docker exec kausway
kausway status` before changing anything. The host identity/pairings live under
`/data/host`, so reusing just the config mount does not restore those records.

To reuse a stopped standalone Linux installation as bind mounts, for example:

```yaml
user: "1000:1000"  # Replace with the actual owner's UID:GID.
volumes:
  - /home/your-user/.config/kausway:/config
  - /home/your-user/.local/share/kausway:/data
```

Use that installation's actual paths, including XDG overrides. Confirm the selected
config filename and that its component directories are directly beneath the data
mount. Both mounts must be writable by the chosen UID. Do not run the standalone
suite concurrently against the same files. Origin addresses may need changing:
container localhost and Docker service names resolve in the container's network.

## Copy into an application's image

```dockerfile
ARG KAUSWAY_IMAGE=ghcr.io/xaque8787/kausway:latest
FROM ${KAUSWAY_IMAGE} AS kausway

FROM ubuntu:24.04
# Supply the application's runtime, CA certificates and init/supervision as needed.
COPY --from=kausway /usr/local/bin/kausway /usr/local/bin/kausway-host /usr/local/bin/kausway-admin /usr/local/bin/
COPY --from=kausway /usr/share/kausway/ /usr/share/kausway/
# Add application, writable directories, environment and startup below.
```

The identical COPY statements work with Alpine. Matching static musl binaries do
not depend on the final image's glibc/musl loader. Keep the copied license notices.
The final image should supply a maintained CA trust bundle for TLS/relay access
(for example `ca-certificates`), DNS configuration and writable runtime paths.

`FROM` defaults to `$TARGETPLATFORM` for **each** platform build. Explicit
`FROM --platform=$TARGETPLATFORM ...` is optional. Do not use `$BUILDPLATFORM` for
this binary-copy stage: that can copy builder-CPU binaries into another target.
Build the developer image with `--platform linux/amd64,linux/arm64` to produce both;
both its final base and the Kausway image must support the requested targets.

COPY does not run the suite, inherit CMD/ENTRYPOINT/ENV/USER/VOLUME, or initialize
state. The developer must arrange those. Copy just host when no other component
is wanted; manager still requires host, and admin is optional. No special
architecture-specific URLs or extraction commands are needed.

## Entrypoint supervises the application and Kausway

The runnable [example Dockerfile](../../examples/docker-integration/Dockerfile)
copies the suite, then uses [entrypoint.sh](../../examples/docker-integration/entrypoint.sh)
and [supervise.py](../../examples/docker-integration/supervise.py). It runs a small
Python origin as a placeholder for the developer's real application:

```sh
docker build -t my-application -f examples/docker-integration/Dockerfile examples/docker-integration
docker run --init --stop-timeout 45 \
  -p 127.0.0.1:47951:47951 \
  -v application-config:/config -v application-data:/data my-application
```

For an unpublished development image, first load the matching Actions image
archive, then build this example with `--build-arg KAUSWAY_IMAGE=kausway-suite:test`.
Use the same local Docker engine and native target. A remote Buildx worker cannot
see an image that exists only in another engine's local store. No GHCR development
publication is required; [CI instructions](../development/ci.md) cover artifact loading.

| Environment | Processes |
| --- | --- |
| `KAUSWAY_MODE=manager`, `KAUSWAY_WITH_ADMIN=true` (defaults) | Application + manager; manager owns host and admin |
| `KAUSWAY_MODE=manager`, `KAUSWAY_WITH_ADMIN=false` | Application + manager; manager owns host |
| `KAUSWAY_MODE=host`, `KAUSWAY_WITH_ADMIN=true` | Application + independently supervised host and admin |
| `KAUSWAY_MODE=host`, `KAUSWAY_WITH_ADMIN=false` | Application + standalone host |

The wrapper's `"$@"` preserves CMD arguments or a `docker run`/Compose command
replacement. It does not recover a base image's old ENTRYPOINT. Set the actual
application command explicitly. The supervisor forwards termination to process
groups, waits up to 30 seconds, escalates when necessary, and reaps direct children.
If the application exits, its status ends the container. If a required Kausway
process exits, the container fails; container restart policy can restart the whole
application. It does not duplicate manager child-restart logic. Use Docker `--init`
or Compose `init: true` to reap orphan descendants, and allow at least 45 seconds
for container shutdown. Readiness is separate from process creation: an application
which requires Kausway before serving should use the integration helper below.

For standalone host/admin, admin cannot start/stop/restart the external host.
The application/container supervisor applies restart-required changes. Manager
mode permits admin lifecycle actions under the default host restart policy;
application restart policy instead asks the user to restart the application.

To run just a host using the suite image itself:

```sh
docker run --init --stop-timeout 45 \
  -v kausway-config:/config -v kausway-data:/data \
  ghcr.io/xaque8787/kausway:latest \
  kausway-host serve --data-dir /data/host
```

To run the manager without admin, replace the default command with
`kausway run --admin-ui=false`. Add `--host-enabled` when it should immediately
launch the host; otherwise use `kausway start` or the saved desired state.
Standalone admin uses
`kausway-admin --data-dir /data/admin --host-data-dir /data/host`; it needs the
same local host namespace/storage for host administration. The image's
`KAUSWAY_DATA_ROOT` is manager-specific, so standalone components use explicit
component paths, not that variable alone.

## Application/backend owns the processes instead

Do not also use the entrypoint supervisor when the application already owns the
same processes. Vendor the existing [integration helper](../../examples/application-integration/kausway.py)
and create one manager per application instance:

```python
from pathlib import Path
from kausway import SuiteManager

suite = SuiteManager(
    Path('/usr/local/bin/kausway'),
    config=Path('/config/config.toml'), data_root=Path('/data'),
    host_enabled=None, admin=True, restart_policy='host',
)
suite.wait_ready()  # Manager and admin; require_host=True when host is required.

# In an existing authenticated backend action, after its normal authorization/CSRF checks:
entry = suite.admin_entry(open_browser=False)
# Return entry['url'] privately; the web UI opens it in the user's browser.

# Other predefined, authorized backend actions:
suite.request('host.start')
suite.request('host.stop')
suite.request('host.restart')

# In the application's shutdown hook:
suite.shutdown()
```

Start once in the owning server process, not independently in every request or
pre-fork web worker. This backend can be native or containerized. It uses private
stdio to obtain the setup/login link without parsing general container logs.
Do not log private frames, return links on public endpoints, or accept arbitrary
shell commands. The web UI calls the developer's backend; the backend calls this
fixed integration interface. The browser must be able to reach the configured
localhost admin address, which Docker alone does not expose to remote users.

Initial credentials can be supplied with
`suite.initialize_credentials(username, password)`; an intentional reset uses
`suite.reset_credentials(...)`. Passwords travel over private pipes, not process
arguments. For independently owned host/admin use `subprocess.Popen` with argument
arrays and explicit component paths, plus the existing `admin_request` local
socket helper for readiness/link issuance. See [application integration](application-suite.md)
for those complete examples and failure/shutdown handling.

## Commands inside a running container

```sh
docker exec kausway kausway status
docker exec kausway kausway-admin setup-link --data-dir /data/admin
docker exec -it kausway kausway-admin credentials set --data-dir /data/admin
docker exec -it kausway kausway-admin credentials reset --data-dir /data/admin
docker exec kausway kausway start
docker exec kausway kausway stop
```

Compose equivalents use `docker compose exec kausway ...`. Credential prompts
hide passwords; noninteractive integrations can use `--username` and
`--password-stdin` with `docker exec -i`. Do not place passwords on the command line.
Run under the service's OS user so local IPC permissions match.

These commands act on existing processes. Starting another persistent manager
through `docker exec` is not a container restart strategy. A `docker exec ...
kausway shutdown` stops the suite's main process, so container restart policy may
restart the container. Use `kausway stop` to stop only its host while retaining admin.

## Source builds and development images

Development images come from **Build Validation** on branches and pull requests.
They are downloadable Actions artifacts, not GHCR releases, and never move
`latest`. The parent calls **Development containers** on native AMD64/ARM64 runners
and reuses verified binary archives from the same run. See [CI details](../development/ci.md).

To build from repository source (root directory is the build context):

```sh
docker buildx build --platform linux/amd64 --load \
  --build-arg VCS_REF="$(git rev-parse HEAD)" \
  -f deploy/standalone/Dockerfile -t kausway-suite:local .

# Requires builders for both platforms, or configured emulation, and registry access:
docker buildx build --platform linux/amd64,linux/arm64 \
  -f deploy/standalone/Dockerfile -t YOUR_REGISTRY/kausway:development --push .
```

The source Dockerfile compiles static musl binaries. CI compiles on native runners,
verifies ELF architecture/static linkage, stages notices with
`scripts/packaging/stage-container.py`, and uses `runtime.Dockerfile`. The runtime
contract is the same. Development source builds do not download release binaries.
Final container publication instead verifies the explicitly selected suite release
and stages its existing binaries with `stage-container-release.py`.
For a build behind a TLS-inspecting proxy, the source Dockerfile accepts
`--secret id=proxy_ca,src=/path/to/combined-ca-bundle.pem`. This temporary build
mount is not included in the runtime image. Never disable certificate verification.

Development Actions artifacts are single-platform Docker archives, not registry indexes:

```sh
gzip -dc kausway-suite-amd64.docker.tar.gz | docker load
# Loaded name: kausway-suite:test. Select the matching CPU artifact.
```

Inspect the published multi-architecture index with:

```sh
docker buildx imagetools inspect ghcr.io/xaque8787/kausway:latest
```

Final publication is a separate manual, main-only operation. See
[publication and recovery](../development/publishing.md).

## Validation

`python3 scripts/ci/container-suite-smoke.py <local-image>` tests the suite image,
then copies its binaries into Alpine and Ubuntu consumers. It checks the default
config filename and an explicit filename override, first-run
link issuance, credential persistence/login, host identity persistence, manager
start/stop, persistence across container recreation, non-root/read-only operation,
signal shutdown, all four entrypoint modes and application exit status.
Development CI runs it natively for each CPU
before exporting test image artifacts. A process-ready result is not an origin
health check, and a successful image build alone is not a runtime test.
