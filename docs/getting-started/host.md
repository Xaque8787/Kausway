# Run the Kausway host

For a short walkthrough, start with [Quick start: local testing](local-testing.md).
See the [complete CLI reference](../operations/cli-reference.md) for all arguments.

Private remote access to a home media server over [Iroh](https://www.iroh.computer/) peer-to-peer connections, without port forwarding or a hosted backend.

The core is a standalone Rust host, `kausway-host`. It knows nothing about Jellyfin. It forwards HTTP requests from paired devices to one configurable local origin. Jellyfin is the first integration, through an admin plugin. Another media server could bundle the same executable.

See [Kausway naming and validation](../history/migration/kausway-naming.md) for the clean-break identities and current verification status.

## Status

| Milestone | Scope | State |
|---|---|---|
| 1. Foundation | Transport protocol v1 messages and framing, policies, pairing primitives, HTTP forwarding primitives, host identity and state storage, `identity` and `doctor` commands, build validation CI | Compiled and tested by CI |
| 2. Rust host | Iroh endpoint, pairing over invite links, sessions, HTTP-over-Iroh streaming with Range and cancellation, revocation, policy gate, test client, end-to-end CI test | Compiled and tested by CI |
| 3. Control API, plugin, Docker | Control API v1 (`protocol/control-v1.md`), live session diagnostics, Jellyfin admin plugin, Docker image with process supervision and automatic plugin install, Compose, CI for plugin and image | Passed baseline Kausway CI; renamed build validation pending |
| Later | Cross-platform workflow, diagnostics, docs, Android TV fork spec | Not started |

## Running with Docker (Jellyfin + kausway-host)

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

To update an existing installation after pulling repository changes:

```sh
docker compose -f deploy/jellyfin/compose.yaml build --pull
docker compose -f deploy/jellyfin/compose.yaml up -d --force-recreate
# Confirm the bundled host version (currently 0.4.0):
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

To pair a test device from another machine, use `kausway-test-client pair '<link>'` and `kausway-test-client proxy`, as described below.

## Prebuilt binaries

Download published assets from [Kausway Releases](https://github.com/Xaque8787/Kausway/releases).
New releases separate suite and client tracks. Each track has one release page
containing all supported architectures, adjacent checksums and `SHA256SUMS`.
Existing `v0.4.0` downloads retain their original combined layout.

| File | Runs on |
|---|---|
| `kausway-host-<suite-version>-x86_64-unknown-linux-musl.tar.gz` | 64-bit x86 Linux (static, no glibc requirement) |
| `kausway-host-<suite-version>-aarch64-unknown-linux-musl.tar.gz` | 64-bit ARM Linux |
| `kausway-host-<suite-version>-aarch64-linux-android.tar.gz` | 64-bit Android 7+ (`adb shell` or Termux) |
| `kausway-host-<suite-version>-armv7-linux-androideabi.tar.gz` | 32-bit ARM Android 7+ |
| `JellyfinKausway_<suite-version>.0.zip` | Stable suite releases only; install into Jellyfin's plugins directory |

Linux also has standalone manager/admin archives and a combined
`kausway-suite-<suite-version>-<target>.tar.gz` containing all three suite binaries.
Download `kausway-test-client-<client-version>-<target>.tar.gz` from the client
release for the separate CLI. See [complete asset policy](../development/release-packaging.md).

```sh
sha256sum -c --ignore-missing SHA256SUMS
```

Linux binaries pass native runtime tests. Android CLI binaries are cross-compiled
and inspected; the SDK additionally runs real-host emulator tests. The CLI is for
development/testing. For unreleased builds, download **Build Validation** artifacts
from a successful run on any branch; see [artifact instructions](../development/ci.md).

### Making a release

Edit the independent track versions in `releases.toml`, run
`python3 scripts/releases/sync-versions.py`, and commit the synchronized files.
After branch validation, merge to main and wait for that exact commit's **Build
Validation** to pass. Manually dispatch **Publish release** on main and select the
desired suite/client/container tracks. Main pushes and Git tags do not publish.
See the [release guide](../development/publishing.md) for pinning, checks, retries
and prerelease/plugin rules. Successful tracks remain published if another fails.

## Building the host without Docker

Requires Rust (the pinned toolchain installs automatically through `rustup`).

```bash
cd rust
cargo test --workspace --locked
cargo build --release --locked -p kausway-host --bins
./target/release/kausway-host doctor --data-dir ./kausway-data --origin http://127.0.0.1:8096
./target/release/kausway-host serve --data-dir ./kausway-data --origin http://127.0.0.1:8096
```

With the host running, in a second terminal:

```bash
./target/release/kausway-host invite --data-dir ./kausway-data --label "Laptop"   # prints a kausway://invite/... link
./target/release/kausway-host devices --data-dir ./kausway-data
./target/release/kausway-host revoke --data-dir ./kausway-data <endpoint id>
```

On the device (any machine, using its own data directory):

```bash
./target/release/kausway-test-client pair 'kausway://invite/...'
./target/release/kausway-test-client status                       # policy, path (direct/relay), RTT
./target/release/kausway-test-client fetch /System/Info/Public
./target/release/kausway-test-client fetch /Videos/<id>/stream?static=true --range bytes=0-1048575 -o part.bin
./target/release/kausway-test-client proxy --listen 127.0.0.1:8097  # open http://127.0.0.1:8097 in a browser
```

The proxy makes the remote Jellyfin usable from a local browser, so you can test real playback, seeking, and cancellation.

`invite`, `devices`, and `revoke` work while `serve` is running. They share `state.json` through `state.lock`, and the running host picks up changes within 2 seconds. The running host writes its current address to `endpoint-addr.json`, and `invite` embeds it in the link.

Configuration (flags or environment):

| Variable | Default | Meaning |
|---|---|---|
| `KAUSWAY_DATA_DIR` | `./kausway-data` | Identity, device registry, lock file |
| `KAUSWAY_ORIGIN` | `http://127.0.0.1:8096` | Local HTTP service to expose |
| `KAUSWAY_TRANSPORT_POLICY` | stored value, else `auto` | `auto`, `relay_only`, `p2p_only` |
| `KAUSWAY_RELAY_URL` | Iroh default relays | Custom relay URL, or `none` to disable relays (LAN or testing only) |
| `KAUSWAY_WEBSOCKETS` | stored value, else `on` | `on` forwards WebSocket connections; `off` refuses them (the pre-WebSocket behavior) |
| `KAUSWAY_WEBSOCKET_ORIGIN` | stored value, else `rewrite` | `rewrite` sends the origin's own address as the WebSocket `Origin` header; `pass_through` forwards the client's value |
| `RUST_LOG` | `info,iroh=warn,noq=warn,kausway_host=info,kausway_transport=info` | Log filter for `serve` |
| `KAUSWAY_CLIENT_DATA_DIR` | `./kausway-client-data` | Test client identity and paired host |
| `KAUSWAY_CONTROL_LISTEN` | `127.0.0.1:47950` | Control API address, or `off` |
| `KAUSWAY_CONTROL_ALLOW_REMOTE` | unset | Required to listen on a non-loopback address |
| `KAUSWAY_CONTROL_TOKEN` | generated `control.token` | Fixed control token (32+ characters) instead of the file |

The plugin reads `JELLYFIN_KAUSWAY_CONTROL_URL`, `JELLYFIN_KAUSWAY_CONTROL_TOKEN_FILE` and `JELLYFIN_KAUSWAY_CONTROL_TOKEN`; these override the plugin's saved settings. The Docker image sets the first two.

## Security notes

- The data directory holds the host's private key (`identity.key`). It is created with owner-only permissions on Unix. Back it up: if it is lost, every device must pair again.
- Invitation tokens are stored only as SHA-256 digests. They are single use and expire. Failed attempts are rate limited, and the limit is saved across restarts.
- `relay_only` and `p2p_only` are **best-effort**, not guaranteed. See `docs/architecture/transport-policies.md`.
- The control API token grants full control of pairing. It lives in `control.token` (owner-only) and never reaches the browser: the dashboard page calls the plugin, which requires a Jellyfin administrator and forwards a fixed set of operations.

## License

MIT OR Apache-2.0

## Android client SDK

The embeddable client engine in `rust/crates/kausway-client` is shared by the existing
CLI and the Android SDK. See [Android SDK integration](../android/README.md) for
AAR installation, typed Kotlin APIs, security, lifecycle and media compatibility,
and [implementation plan](../history/android-implementation-plan.md) for architecture.
The separate **Android SDK** workflow builds the AAR and independent sample.

### Coordinated releases

The client engine, CLI and Android SDK share the **client** release track; suite
and container versions can differ. A client release includes the versioned AAR,
POM, sources, notices, README and checksums on GitHub; Maven Central is not required.
Build Validation requires the reusable SDK workflow's emulator tests before
assembling the client candidate. See [versioning](../development/versioning.md)
and [manual publication](../development/publishing.md).
