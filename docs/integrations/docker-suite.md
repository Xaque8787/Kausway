# Docker suite and application integration (Phase 7)

The suite image contains `kausway` (manager), `kausway-host`, `kausway-admin`, and
the development `kausway-test-client`. Its default command starts the manager
with admin enabled. The manager owns its children; it does not adopt a host
which another process already started.

Development images are produced by **Build Validation** on any branch or PR and
are downloadable Actions artifacts, not GHCR publications. The parent calls the
read-only **Development containers** workflow on native AMD64/ARM64 runners, reusing
verified binary archives from that same run. Both architectures run the suite and
Alpine/Ubuntu embedding tests. See [CI details](../development/ci.md).

Published-image examples below are for eventual stable distribution. Until a
release is available, use a locally loaded artifact or source build. The separate
**Legacy container publication** workflow still handles main/legacy tags during
this intermediate phase; manual-only stable publication replaces it in 8.4.
Do not interpret development artifact availability as a registry release.

## Buildx, publication and architectures

Automatic development builds neither authenticate to GHCR nor move `latest`.
The final release system will publish one multi-architecture index after both CPU
variants pass. Integrations should pin a version or **image-index digest** rather
than assume a development image is available under `latest`.

```sh
docker pull ghcr.io/xaque8787/kausway:latest
docker buildx imagetools inspect ghcr.io/xaque8787/kausway:latest
```

The index contains `linux/amd64` and `linux/arm64`. Docker selects the current
target automatically. Unsupported targets fail rather than using the wrong CPU.
No Android, Windows, macOS or 32-bit Linux container is published.

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
contract is the same. Neither path downloads executable release archives.
For a build behind a TLS-inspecting proxy, the source Dockerfile accepts
`--secret id=proxy_ca,src=/path/to/combined-ca-bundle.pem`. This temporary build
mount is not included in the runtime image. Never disable certificate verification.

Development Actions artifacts are single-platform Docker archives, not registry indexes:

```sh
gzip -dc kausway-suite-amd64.docker.tar.gz | docker load
# Loaded name: kausway-suite:test. Select the matching CPU artifact.
```

## Run the suite or use it as a sidecar

```sh
docker run -d --name kausway --init --restart unless-stopped \
  --stop-timeout 45 \
  -p 127.0.0.1:47951:47951 \
  -v kausway-config:/config -v kausway-data:/data \
  ghcr.io/xaque8787/kausway:latest
docker logs kausway
```

No command, origin or credentials must be supplied. Open the expiring setup link
from the logs, create an account, configure the origin in admin, save, and Start
host. A fresh suite starts with the host disabled. Do not set a permanent
`KAUSWAY_HOST_ENABLED=false` override if users should subsequently start it.

`deploy/standalone/compose.yaml` supplies the same defaults and named volumes:

```sh
docker compose -f deploy/standalone/compose.yaml up -d
docker compose -f deploy/standalone/compose.yaml logs kausway
```

For a sidecar, put this service and the application's service on the same Compose
network. In admin use `http://application:8080` (the actual service name/internal
port) as host origin. `127.0.0.1` inside the suite is the suite container, not a
different application container. A host application's address also needs an
appropriate Docker gateway mapping; it is not automatically container localhost.

Only admin's browser port is mapped, bound to the Docker host's loopback. Host
control remains `127.0.0.1:47950` **inside the suite container**; it has no published
port. Manager/admin private sockets are files under `/data`, not network services.
`EXPOSE` is metadata, not a port publication. Docker networking/port publication
does not configure Kausway's origin or otherwise turn a command into an environment
variable: Compose `command:` replaces CMD; `environment:` sets environment values.

Peer transport needs working outbound networking, DNS and relay access. No fixed
inbound peer port is published by this example; direct connectivity depends on the
network, with relay fallback under the selected policy. Host control is unrelated
to peer transport.

Admin listens on `0.0.0.0:47951` inside the image so Docker can forward it. Its
browser origin remains localhost. This is local management, not a remotely exposed
admin deployment. On a remote Docker server use SSH local port forwarding. A
public hostname/reverse-proxy deployment is outside the current admin URL policy.

To use a different local browser port, update **both** the mapping and browser URL:

```yaml
ports:
  - "127.0.0.1:48000:47951"
environment:
  KAUSWAY_ADMIN_BROWSER_BASE_URL: http://127.0.0.1:48000
```

## Persistent configuration and data

| Container path | Contents |
| --- | --- |
| `/config/kausway.toml` | Shared saved settings and administrator password hash |
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

Image environment selects `/config/kausway.toml`, `/data`, and the container admin
listener; these launch overrides take precedence over saved settings. Other saved
settings remain editable. To change those overridden values, change the container
environment and recreate it. See [configuration precedence](../architecture/configuration-and-storage.md).

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
`kausway run --admin-ui=false`. Standalone admin uses
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
    config=Path('/config/kausway.toml'), data_root=Path('/data'),
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

## Validation

`python3 scripts/ci/container-suite-smoke.py <local-image>` tests the suite image,
then copies its binaries into Alpine and Ubuntu consumers. It checks first-run
link issuance, credential persistence/login, host identity persistence, manager
start/stop, non-root/read-only operation, signal shutdown, all four entrypoint
modes and application exit status. Development CI runs it natively for each CPU
before exporting test image artifacts. A process-ready result is not an origin
health check, and a successful image build alone is not a runtime test.
