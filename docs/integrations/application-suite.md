# Embedding the Kausway suite (Phase 5)

An application can ship `kausway`, `kausway-host` and `kausway-admin` beside each
other, supervise only the manager, and delegate settings/account setup to admin.
The manager owns its children. A developer does not need to implement a settings
page, password hashing or browser session handling.

This integration is implemented and validated on Linux. Windows manager/named-pipe
support and native macOS validation remain deferred. Development binary packaging
is available; [Docker suite integration](docker-suite.md) covers the same process
contract in containers, sidecar deployment and multi-architecture image publishing.

## Application responsibilities

1. Choose writable configuration/data paths (or intentionally accept OS defaults).
2. Spawn the bundled manager with private stdin/stdout pipes and a drained stderr.
3. Wait for the manager protocol handshake and required component readiness.
4. Wire a Configure Kausway action to `admin.open` and open its returned URL.
5. On application exit, request shutdown, close stdin and wait for the manager.

The manager locates sibling executables; explicit binary path overrides remain
available. Do not log protocol frames: replies can contain setup links and requests
can contain credentials. Stderr contains bounded lifecycle diagnostics. With
`--integration-stdio`, startup setup links are suppressed from manager/application
logs and delivered only on an explicit private request.

## Minimal integration example

The repository provides a dependency-free Python reference helper:
[`kausway.py`](../../examples/application-integration/kausway.py). It implements
framing, request correlation, readiness, credential provisioning, browser handoff
and shutdown. It is example source to vendor/adapt, not a separately published SDK.

```python
from pathlib import Path
from kausway import SuiteManager

with SuiteManager(
    Path(application_install_dir) / "kausway/kausway",
    config=Path(application_config_dir) / "kausway/config.toml",
    data_root=Path(application_data_dir) / "kausway",
    application_name="MyApplication",
) as suite:
    # Wire this call to a user action such as Configure Kausway.
    suite.admin_entry(open_browser=True)
    run_application()
```

A small runnable application is also included:

```sh
python3 examples/application-integration/application.py \
  --manager /path/to/binaries/kausway \
  --config /path/to/application/config/kausway/config.toml \
  --data-root /path/to/application/data/kausway
```

It owns one manager, exposes configure/status commands, and shuts down on exit.
No Node or Python dependency is required by the Kausway binaries themselves.

The helper defaults to an enabled host, admin enabled, and application-restart
policy. An application using that policy must select desired host state itself;
admin does not offer Start/Stop controls. Set `host_enabled=None` to respect saved
initial desired state or false to start disabled. `admin=False` supports a host-only
suite. `restart_policy="host"` permits verified manager-backed UI lifecycle actions.

The helper's `wait_ready()` waits for manager control and requested admin readiness.
Use `wait_ready(require_host=True)` when the application also needs an authenticated
running host; this can fail even though admin is available to fix settings. Failed
or disabled required components produce errors instead of a false readiness claim.
The helper does not automatically respawn a failed manager.

Omitting config/data arguments uses Kausway's platform defaults. For independent
embedded installations, choose application-specific paths for both. Files are
created automatically. Never run a replacement suite over still-live children.

## Browser setup handoff

`admin.open` returns one of:

```json
{"state":"setup_required","url":"http://127.0.0.1:47951/setup#token=<temporary-token>","expires_in_secs":900}
```

```json
{"state":"configured","url":"http://127.0.0.1:47951/login"}
```

It obtains the actual browser base URL from the running admin process, so the
application does not assume a port or inspect config to guess account state.
Requesting another setup link replaces the previous token. Account creation
remains in the browser and stores only an Argon2id hash.

Once configured, opening the entry link does not bypass login. The application
never receives browser session cookies. For a container/server application, use
`suite.admin_entry(open_browser=False)` and deliver that sensitive URL through a
private local application action rather than trying to launch a desktop browser
inside the container or publishing the URL on a public page.

The setup token has the same 15-minute expiry and single-use rules as CLI setup.
Normal terminal launches without `--integration-stdio` continue printing a setup
link to startup logs. Local `kausway-admin setup-link` and credential commands
remain supported.

## Application-provisioned credentials

An application which already collects initial credentials can provision them
through the same private channel:

```python
suite.initialize_credentials(username, password)
```

This refuses to overwrite an existing administrator. An explicit reset is separate:

```python
suite.reset_credentials(new_username, new_password)
```

Reset invalidates all existing browser sessions. Neither operation changes host
identity, pairings or host control tokens. Passwords are not placed in process
arguments, environment variables, config plaintext or logs. No public HTTP reset
or setup-token issuance endpoint is introduced.

## Private manager protocol v1

Launch with:

```text
kausway run --integration-stdio --admin-ui --host-enabled \
  --restart-policy application --application-name MyApplication \
  --config <config-file> --data-root <suite-root>
```

Stdin/stdout must be private inherited pipes or sockets. Stderr is separate.
The first stdout frame is:

```json
{"event":"ready","protocol":1,"status":{"...":"manager status"}}
```

It establishes manager control readiness, not a guarantee that the origin is
reachable or every child is ready. `admin.status` separately confirms the running
admin local interface. Normal host status includes starting/running/failed state.

Each subsequent request is one JSON object followed by LF, capped at 16 KiB:

```json
{"id":1,"operation":"admin.open"}
```

Replies echo the integer `id` and `ok`. Admin operations return `result`; lifecycle
operations return `status`. Failures return a generic error and may include safe
manager status. Request IDs are correlation values, not authentication tokens.
Calls on this stream are processed sequentially; the helper serializes threads.

| Operation | Additional fields | Behavior |
| --- | --- | --- |
| `status` | — | Read manager/owned-child state and safe diagnostics |
| `admin.status` | — | Confirm local admin readiness and configured state |
| `admin.open` | — | Obtain setup or login link |
| `admin.credentials.initialize` | `username`, `password` | Create account; refuse replacement |
| `admin.credentials.reset` | `username`, `password` | Explicitly replace account; invalidate sessions |
| `host.start` / `host.stop` / `host.restart` | — | Application-owned lifecycle control |
| `shutdown` | — | Shut down manager and owned children |

The restricted same-user manager socket also accepts the admin operations. They
are not available over unauthenticated HTTP. The manager forwards requests only
to its owned admin's protected local socket using the selected canonical config.

Replies and ready events are bounded. Malformed JSON returns a generic error
without echoing contents. Oversized/truncated input, closed output or prolonged
output backpressure fails the channel and shuts down owned children. EOF on stdin
also requests shutdown, covering a parent exit which closes its private pipe.
Nonblocking pipe I/O allows signals and shutdown even while the parent keeps an
idle input pipe open.

After a request timeout or correlation failure, treat the connection as unusable;
close/shut down and wait rather than sending more requests over uncertain state.
Explicit shutdown flushes its acknowledgement before exit, and the application
must still wait for process termination/reaping. The reference helper uses a
bounded configurable wait and reports escalation rather than silently starting a
replacement. Forced process-tree recovery is the embedding supervisor's decision.

## Independently supervised host and admin

Applications may retain their own host supervisor and launch admin separately.
The manager is optional:

```text
application
  ├── kausway-host serve --config <file> --data-dir <root>/host
  └── kausway-admin --config <file> --data-dir <root>/admin --host-data-dir <root>/host
```

On Unix, use the helper's `admin_request` against admin's private local socket:

```python
from kausway import admin_request

ready = admin_request(admin_directory, config_file, "status")
entry = admin_request(admin_directory, config_file, "entry-link")
# Open entry["url"] through the application's normal browser action.
```

`credentials.set` and `credentials.reset` accept username/password as private
fields. Startup status requires polling until the local socket is ready, with a
deadline and child-exit checks owned by the application. For an application launch,
set `KAUSWAY_ADMIN_STARTUP_EVENTS=1` and consume/discard structured stdout privately
so the initial setup link is not copied to general stderr logs; use `entry-link`
for the link actually presented to the user.

Without a configured owning manager, admin offers live host operations and saved
configuration editing but no process lifecycle buttons. The application must
restart externally supervised components when requested by the UI.

## Validation

`scripts/ci/application-integration-smoke.py <binary-directory>` uses the real
reference helper and binaries. It covers initial setup/login handoff, provisioning
and overwrite refusal, explicit reset/session invalidation, persisted accounts,
host-only mode, correlation/error redaction, signal/shutdown with open pipes, parent
disconnect with owned-child cleanup, and independent admin handoff. Build Validation
runs it alongside the existing host/admin/manager scenario matrix.
