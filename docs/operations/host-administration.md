# Host administration (Phase 4)

The authenticated admin UI now provides Overview, Invitations, Devices, Sessions,
Settings, Diagnostics and Account screens. Existing standalone host/client commands
and the host control API remain supported. Admin does not contain another transport
engine and never adopts or signals a host process itself.

## Full suite

Build the three executables beside each other, then run:

```sh
cargo build --manifest-path rust/Cargo.toml \
  -p kausway-host -p kausway-manager -p kausway-admin --bins --locked

kausway run --config /app/config/config.toml --data-root /app/data --admin-ui
```

On a fresh installation, obtain the startup setup link as described in
[admin account setup](admin.md). The host starts disabled unless explicitly enabled.
Configure the origin and transport settings, save, and choose Start host.

Admin verifies that the configured manager owns the same canonical config file and
host component directory before enabling lifecycle controls. A running unrelated
manager is insufficient. Stop/start/restart requests are serialized by the manager;
admin remains available while its host is stopped or failed.

The manager's authenticated host readiness check is distinct from the admin web
server's availability. A restart request can be accepted before the host is ready;
Overview shows the subsequent starting/running/failed state and retry diagnostics.
Manager status remains responsive during graceful child shutdown.

## Independent host and admin

An application or supervisor can run the processes separately:

```sh
kausway-host serve --config /app/config/config.toml --data-dir /app/data/host

kausway-admin --config /app/config/config.toml --data-dir /app/data/admin \
  --host-data-dir /app/data/host
```

Without `--host-data-dir` / `KAUSWAY_ADMIN_HOST_DATA_DIR`, admin selects `host/`
beside its selected admin component directory. Supply it explicitly for custom
layouts. Storage paths are displayed read-only in Settings.

The host publishes its actual local control address to a private
`control-address.json` in host storage after binding. Admin reads it so a saved
listener change does not disconnect administration from the still-running old
listener. After external restart, it discovers the newly published address.

For externally selected listeners/tokens, admin also supports:

| Argument | Environment |
| --- | --- |
| `--host-control-listen IP:PORT` | `KAUSWAY_ADMIN_HOST_CONTROL_LISTEN` |
| `--host-control-token TOKEN` | `KAUSWAY_CONTROL_TOKEN` |
| `--host-data-dir PATH` | `KAUSWAY_ADMIN_HOST_DATA_DIR` |

Prefer the protected token file over a token argument. Host control tokens never
reach the browser. Manager-backed admin uses the manager's actual owned listener
and current token file rather than an inherited token override.

Admin only connects to concrete loopback control IPs with nonzero ports. There is
no remote host URL setting, generic reverse proxy or arbitrary-request console.
Configuration and mutation operations verify host configuration/storage identity.
Concurrent independent hosts need distinct config files as well as distinct host
data directories; a running host holds an exclusive config ownership lease.

No manager means **no process lifecycle controls**, even when admin has access to
the host control token. Invitation creation/cancellation, device revocation,
sessions and supported live settings remain available while the host runs.

## Availability matrix

| Condition | UI behavior |
| --- | --- |
| Host running, no manager configured | Live operations and settings; explain external supervision; omit lifecycle buttons |
| Host and owning manager running | Live operations, settings, lifecycle actions and manager diagnostics |
| Host stopped, owning manager available | Saved settings and Start; admin remains available |
| Host stopped, no manager | Saved configuration editing; application/external restart instructions |
| Configured manager unavailable | Explicit connection error; lifecycle disabled; effective host values unavailable |
| Configured manager owns another installation | Ownership error; no lifecycle actions are sent |
| Manager reports host failure | Show failure and diagnostics; keep admin available |
| Application-restart policy | Omit lifecycle buttons; require application restart |

If the host holds its storage lock but the control API is unavailable or cannot
validate the configuration context, admin refuses to write around the running
host. It reports that configuration was not saved. When storage is unlocked,
offline saves take that lock and a config-specific host lease, then use shared
validation/revision checks. The config lease also prevents bypassing a running host
by accidentally selecting the wrong component directory. Offline
editing does not create a replacement identity or pairings.

## Settings and persistence

Every configuration view includes an opaque revision, safe saved settings, field
metadata, effective availability, pending/overridden fields, and read-only paths.
Effective values come from the running host, manager and admin, not from the admin
process's interpretation of host startup arguments.

The host supplies its actual startup CLI/environment/file/default sources. Live
policy/WebSocket values come from authoritative host state; `state` indicates that
source. Stopped-host effective values use `unavailable`. Known manager/admin values
can still be shown while the host is stopped. Missing values are not presented as
known effective defaults for an independently supervised stopped host.

Ordinary settings exclude admin auth, host control tokens, identities and pairings.
`manager.host_enabled` is read-only in this API; use lifecycle controls to change
desired state. Other manager policy settings and admin startup settings can be
saved. Manager policy is reloaded before its next host launch; admin listener and
launch configuration require restarting the appropriate process/suite.

Actions are explicit:

- **Save:** validate and persist; leave running host settings unchanged.
- **Apply saved live settings:** apply saved policy and WebSocket settings together
  through the host, with a revision check and one state-file update.
- **Save and restart host:** save, then request the owning manager's restart. It is
  available only under host-restart policy.

A saved value overridden by CLI/environment is labeled accordingly. Restarting with
that override does not apply the saved value. Apply refuses a differing saved live
value with a launch override; an unchanged overridden value does not block applying
other live fields. Omitted policy/WebSocket settings preserve host state.

Policy changes crossing the relay-only endpoint-construction boundary can still
require restart after the configured policy is applied. Both the apply response
and configuration/overview report this requirement. Existing transport enforcement
limitations remain documented in [transport policies](../architecture/transport-policies.md).

Changing the origin prompts a warning about the service available to authorized
clients. Stop/restart and live connection-affecting changes require confirmation.
A failed restart retains saved settings and reports the failure; there is no silent
rollback or false claim that saved changes became effective.

Configuration writes reject stale revisions instead of overwriting other edits.
Entering Settings refreshes an unedited form. Background Overview polling never
replaces an edited form; an explicit reload asks before discarding unsaved edits.
If another writer changes configuration during editing, reload and reconcile the
reported conflict before saving.

## Application restart policy

For a suite embedded in an application:

```sh
kausway run --config /app/config/config.toml --data-root /app/data \
  --admin-ui --host-enabled --restart-policy application --application-name MyApplication
```

The embedding application selects the initial desired host state, shown here with
`--host-enabled`. Under this policy admin has no Start/Stop controls; the
application's own startup choice determines whether its host runs after restart.

Environment equivalents are `KAUSWAY_RESTART_POLICY` and
`KAUSWAY_APPLICATION_NAME`. Saved equivalents live under `[manager]`:

```toml
[manager]
restart_policy = "application"
application_name = "MyApplication"
```

Admin uses the connected manager's effective policy. It hides lifecycle controls,
rejects lifecycle/save-and-restart HTTP requests, and explains that MyApplication
must restart. Read-only status and supported live host operations remain available.
The application/OS can still use the manager CLI/local protocol to supervise its
own process tree.

For independently supervised admin, `--restart-policy application` /
`KAUSWAY_ADMIN_RESTART_POLICY` and `--application-name` supply the explanatory
context. Application policy is the standalone default. Neither flag grants process
control without an owning manager.

## Security and diagnostics

All operational APIs require a valid admin session. Mutations additionally require
same-origin JSON requests and the session CSRF token. Existing setup/login/reset
rules are unchanged. The browser receives no host control credentials or account
password hash.

Host operations are explicitly mapped to supported endpoints; user-supplied device
and invitation IDs cannot select arbitrary URLs or paths. Manager calls use local
IPC. Connection counts, requests, replies, operation times and diagnostics remain
bounded. Long lifecycle operations allow the configured graceful shutdown timeout;
header/body deadlines remain short.

Invitation links are shown only on creation, kept in browser memory, and cleared
by Refresh host or navigation away from the document. They are not in the pending
list, persisted browser storage or manager diagnostics. Device names, errors and
diagnostics are rendered as text, not HTML.

## Validation

```sh
cargo test --manifest-path rust/Cargo.toml --workspace --locked
python3 scripts/ci/admin-host-smoke.py rust/target/debug
```

Build Validation runs the real-process scenario matrix alongside existing admin,
manager and transport checks. It covers independent/full-suite configurations,
missing/wrong manager, stopped/unreachable host, application policy, failed restart,
revision conflicts, credential exclusion, invitation/pairing/revocation/cancellation,
listener changes, launch overrides and live apply.

Linux is validated in this phase. Windows/macOS native validation remains deferred
to their build workflows, and Windows manager IPC is still unimplemented.
