# Local suite manager (Phase 2)

`kausway` is an optional foreground supervisor. It starts the existing
`kausway-host` executable as a child; the standalone host remains supported.
Linux is the validated platform for this phase. Unix IPC/signals are implemented;
Windows currently returns an explicit unsupported-platform error. No Windows or
macOS release support is claimed by these Linux checks.

## Build and launch

From the repository root:

```sh
cargo build --manifest-path rust/Cargo.toml -p kausway-manager -p kausway-host --bins --locked
```

The binaries are in `rust/target/debug/` unless `CARGO_TARGET_DIR` overrides it.
The manager finds siblings beside its own executable, not through the working
directory or PATH. `--host-binary PATH` / `KAUSWAY_HOST_BINARY` and
`--admin-binary PATH` / `KAUSWAY_ADMIN_BINARY` select explicit executables.

```sh
# Initialize shared configuration and storage; a fresh host is disabled.
kausway run

# Or select application-owned paths and start a host immediately.
kausway run \
  --config /app/config/kausway/config.toml \
  --data-root /app/data/kausway \
  --host-enabled \
  --origin http://127.0.0.1:8096
```

`--config` / `KAUSWAY_CONFIG` selects the shared file. `--data-root` /
`KAUSWAY_DATA_ROOT` selects the suite root. Omitting them uses the platform
locations in [configuration and storage](../architecture/configuration-and-storage.md).
The children receive the same config file and their own `host/` or `admin/`
component directory. Explicit config/data paths work without HOME.

Do not point two unrelated applications at the same default suite root if they
need independent hosts. There is one manager per suite data root and one running
host per host component directory.

## Local lifecycle commands

Run in a second terminal:

```sh
kausway status --data-root /app/data/kausway
kausway start --data-root /app/data/kausway
kausway stop --data-root /app/data/kausway
kausway restart --data-root /app/data/kausway
kausway shutdown --data-root /app/data/kausway
```

The data-root argument is optional when using defaults. No config argument is
needed by these commands: the running manager owns its selected config path.
Output is JSON and errors return nonzero exit status.

Start and restart save `manager.host_enabled = true`; stop saves false before
stopping. Save conflicts fail the request rather than silently changing desired
state. A start/restart response acknowledges launch, not authenticated readiness;
poll status until `host.observed` is `running` or `failed`.

Manager shutdown and SIGTERM/SIGINT stop and reap owned children without changing
the saved desired state. A subsequent manager launch uses that saved state.
`--host-enabled[=true|false]` / `KAUSWAY_HOST_ENABLED` overrides initial desired
state for that invocation; subsequent explicit lifecycle operations persist their
new desired state normally. It does not continuously override stop/start commands.

## Ownership and readiness

The manager never reads a PID file to adopt or signal an external process. It
signals only tracked child handles. Before launching, it checks the host data
lock. The child independently acquires the lock, so a competing standalone launch
cannot run a second host against the same storage.

Managed hosts require an enabled, concrete loopback control IP and nonzero port.
The manager reads `--control-listen` / `KAUSWAY_CONTROL_LISTEN`, then
`host.control.listen`, then `127.0.0.1:47950`. `off`, wildcard listeners and port
zero are rejected for managed hosts. This does not change standalone host options.

For every host launch, the manager generates a new private control token, writes
`host/control.token`, and supplies it privately to the child through the
environment. It intentionally replaces an inherited `KAUSWAY_CONTROL_TOKEN`.
The token is never printed. Admin/local integrations must read the current token
rather than cache it across host restarts.

Only an authenticated `/v1/status` response with the expected protocol fields
makes the child ready. Merely opening a port, writing a PID, or answering public
`/v1/health` does not. Manager readiness does not imply the origin is reachable or
a relay is connected; host doctor and transport diagnostics cover those concerns.

## Failure and restart policy

Desired state is `enabled` or `disabled`. Observed state is `stopped`, `starting`,
`running`, `stopping`, or `failed`. Failures leave desired state enabled.

```toml
[manager]
host_enabled = false
shutdown_timeout_secs = 15
restart_on_failure = true
restart_backoff_initial_secs = 1
restart_backoff_max_secs = 30
max_failures = 5
readiness_timeout_secs = 30
```

Unexpected exit (including exit code zero), spawn failure, or readiness timeout
counts as a failure. Retries use exponential backoff, capped at the configured
maximum. The failure threshold counts total attempts, including the initial
attempt. At the threshold, automatic retries stop; explicit start/restart resets
the counter. A run lasting at least 60 seconds resets the previous failure streak
when it exits. Explicit stop cancels pending retries.

The manager reloads saved startup configuration before each launch. These policy
settings also have CLI overrides and corresponding environment variables:

| CLI flag | Environment |
| --- | --- |
| `--shutdown-timeout-secs` | `KAUSWAY_SHUTDOWN_TIMEOUT_SECS` |
| `--restart-on-failure[=true\|false]` | `KAUSWAY_RESTART_ON_FAILURE` |
| `--restart-backoff-initial-secs` | `KAUSWAY_RESTART_BACKOFF_INITIAL_SECS` |
| `--restart-backoff-max-secs` | `KAUSWAY_RESTART_BACKOFF_MAX_SECS` |
| `--max-failures` | `KAUSWAY_MAX_FAILURES` |
| `--readiness-timeout-secs` | `KAUSWAY_READINESS_TIMEOUT_SECS` |

Shutdown timeout is 1–120 seconds, readiness timeout 1–300 seconds, backoff bounds
are positive with initial <= maximum <= 3600 seconds, and failure limit is 1–100.
A child that ignores SIGTERM is killed and reaped after the shutdown timeout.

Explicit `--origin`, `--relay-url`, `--policy`, `--websockets` and
`--websocket-origin` are passed to the host unchanged as CLI overrides. Existing
host environment variables are inherited. Saved settings are not expanded into
CLI overrides. Use saved settings when users should be able to edit them later.

## Protected local control protocol

The socket is `<data-root>/manager/control.sock`, inside a 0700 directory, with
0600 socket permissions and a same-effective-UID peer check. It has no TCP port
and is not a browser endpoint. Processes trusted as the same OS user can control
the manager.

One newline-delimited JSON request per connection:

```json
{"operation":"status"}
```

Operations: `status`, `host.start`, `host.stop`, `host.restart`, `shutdown`.
Replies contain `ok` and `status`; failures also contain `error`. The status
includes protocol version 1, desired state, observed host/admin state, tracked
PIDs, failure count, retry delay, and recent lifecycle diagnostics.

Requests are capped at 16 KiB, incomplete frames time out after five seconds,
and concurrent connections/queued requests are bounded. Lifecycle actions are
serialized. The shutdown acknowledgement is flushed before the manager exits;
the launching application should still wait for the manager process to finish.

Only manager-generated lifecycle messages are retained (up to 64). Arbitrary
child stdout/stderr is drained with bounded buffers and omitted because it can
contain invitation links or credentials. To diagnose a host configuration error,
use standalone `kausway-host doctor` with the same selected config and overrides.
Admin emits a bounded structured startup setup-link event. The manager forwards
that intentionally sensitive link to startup stderr, never to retained diagnostics.
Other raw child output remains omitted. See [admin setup](admin.md).

## Optional admin launch hook

`--admin-ui[=true|false]` / `KAUSWAY_ADMIN_ENABLED` overrides `admin.enabled` at
manager startup. Admin is disabled by default. If enabled, the executable must
exist; Phase 3 now provides the admin authentication server.

The agreed child invocation is:

```text
kausway-admin --config <shared-file> --data-dir <root>/admin --manager-socket <socket>
```

Admin remains running when the host is stopped. Manager exit reaps it. An admin
exit is reported as failed without stopping host supervision. At this phase,
admin status reports process existence only; web readiness is not inferred from process existence. Phase 3 supplies
authentication; an exited admin is reported as failed without automatic recovery.

## Validation

```sh
cargo test --manifest-path rust/Cargo.toml -p kausway-manager --locked
python3 scripts/ci/manager-smoke.py rust/target/debug
```

The process tests cover authenticated readiness rejection, bounded crash recovery,
intentional stop, persistence, concurrent starts, malformed requests, forced
shutdown, duplicate-manager protection, external-host refusal, diagnostics
redaction, and admin survival while host is stopped. The smoke test exercises the
actual host, including identity preservation and refusal to stop an external host.

## Phase 4 integration

See [host administration](host-administration.md) for the UI, configuration API,
application-restart policy and ownership checks. Manager status now includes its
selected configuration/host directory, actual owned listener, effective manager
settings and launch sources. Status is served independently of serialized lifecycle
work so it remains responsive during graceful stop.

The manager preserves the control listener's CLI/environment/file/default source
in the child. An internal managed-host guard enforces loopback-only binding rather
than turning saved listener settings into artificial CLI overrides.
