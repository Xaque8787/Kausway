# Jellyfin integration

The plugin and host support [short-code invitations](../operations/short-code-pairing.md) through the same separately supervised host.

Kausway can expose Jellyfin through a standalone host, the standard suite sidecar,
or the combined source-built deployment below. The standard published Kausway
image contains manager/host/admin only; it does not contain Jellyfin or this plugin.

## Published standalone plugin

Download `JellyfinKausway_<plugin-version>.zip` and its checksum from the
[latest stable suite release](https://github.com/Xaque8787/Kausway/releases/latest).
Extract its versioned directory into Jellyfin's plugins directory and restart
Jellyfin. Use a Jellyfin server compatible with the plugin's `targetAbi` metadata.
Suite prereleases intentionally omit the plugin; stable suite releases include it.

The plugin is a dashboard adapter for a separately running `kausway-host`, not a
bundled host or client. It needs access to that host's authenticated control API.
Its settings can be overridden by `JELLYFIN_KAUSWAY_CONTROL_URL`,
`JELLYFIN_KAUSWAY_CONTROL_TOKEN_FILE` and `JELLYFIN_KAUSWAY_CONTROL_TOKEN`.
Keep the control token private. The standard sidecar's control API binds to its own
loopback interface, so installing this ZIP into a different container does not
make that private API reachable automatically. Use the sidecar's admin UI, or an
integration with a deliberately arranged private shared network namespace.

## Combined Jellyfin + host + plugin image

This is built from the repository's source, not published as the standard GHCR
suite image. Clone the repository to use its Dockerfile/Compose deployment.
The entrypoint supervises Jellyfin and the host directly; it does not run the
Kausway manager or admin UI.

Requires only Docker with Compose. Everything is compiled inside the build from this repository's sources.

```bash
git clone https://github.com/Xaque8787/Kausway.git
cd Kausway
docker compose -f deploy/jellyfin/compose.yaml up -d --build
docker compose -f deploy/jellyfin/compose.yaml logs -f        # wait for Jellyfin and "control API v1 listening"
```

Open http://localhost:8096, finish Jellyfin's setup wizard, then go to Dashboard > Plugins > Kausway Remote Access. From there you can create invitation links, see connected devices with their path (direct or relay) and round-trip time, revoke devices, and change the transport policy.

How the image works:

- At every start, the entrypoint copies the bundled plugin into `./config/plugins/JellyfinKausway_<version>/` (replacing older versions of this plugin only), then launches Jellyfin.
- `kausway-host` runs as a separate process under the entrypoint's supervisor, which restarts it with backoff if it exits. Jellyfin does not start or stop it.
- The plugin talks to `kausway-host` only through control API v1 on `127.0.0.1:47950` inside the container. That port is not published. The token is read from `deploy/jellyfin/kausway-data/control.token`.
- Folders next to `compose.yaml`: `deploy/jellyfin/config`, `deploy/jellyfin/cache`, `deploy/jellyfin/media` (mounted read-only at `/media`; add it as a library), `./kausway-data`.

The image sets `KAUSWAY_CONFIG=/kausway-data/config.toml` and
`KAUSWAY_DATA_DIR=/kausway-data`. The existing `./kausway-data:/kausway-data`
bind mount therefore persists the shared Kausway configuration as well as host
identity, pairings and state. These defaults also apply with `docker run` when
that directory is mounted. Compose can override them for a custom layout.

Earlier combined images used the host's platform default for its shared config,
normally `/root/.config/kausway/config.toml`. An existing file is not automatically
moved. Before replacing an old container, stop the processes and copy that file
to the persisted location if you need its saved settings, preserving private
permissions and checking for an existing destination. Alternatively, explicitly
select and persist a custom config path. Existing `/kausway-data` identity and
pairing files remain at the same locations.

To update an existing installation after pulling repository changes:

```sh
docker compose -f deploy/jellyfin/compose.yaml build --pull
docker compose -f deploy/jellyfin/compose.yaml up -d --force-recreate
# Confirm the bundled host reports the source checkout's suite version:
docker compose -f deploy/jellyfin/compose.yaml exec jellyfin kausway-host --version
```

The image builds the host and plugin from the checked-out source, not GitHub
release downloads. Build-time version checks require the host, plugin assembly
and plugin metadata versions to agree. Recreating the container installs the
bundled plugin over the persistent copy and removes older Kausway plugin versions;
Jellyfin configuration, media and Iroh identity remain in the existing mounts.
The Android AAR is a client dependency and is not needed in the server image.
The pinned Jellyfin server and its plugin SDK/ABI are updated together; `--pull`
does not move the image to a different Jellyfin version.

To pair a device, use the [client walkthrough](../getting-started/local-testing.md) or the [Android terminal guide](../android/storage.md#termux-client-test).

## Client access

Pair a client and start its local proxy, then open the proxy URL in the browser
or point a compatible player at it. Sign into Jellyfin with the normal Jellyfin
account. Pairing only grants access through Kausway; it is not a Jellyfin login.
See [the client walkthrough](../getting-started/local-testing.md).
