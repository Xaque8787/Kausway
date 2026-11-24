# Local admin account setup

Development builds support optional [short-code invitations](short-code-pairing.md), with publication status and the full URL retained as fallback.

`kausway-admin` contains an embedded web frontend and local authentication server.
It provides first-run setup, sign-in/out, administrator credential changes and
[host administration](host-administration.md), including
settings, invitations, devices, sessions and manager-backed lifecycle controls.
No Node runtime or separate frontend server is needed.

## Install and start

Download the suite or standalone admin archive from the
[latest suite release](https://github.com/Xaque8787/Kausway/releases/latest).
For source builds, see [Contributing](https://github.com/Xaque8787/Kausway/blob/main/CONTRIBUTING.md#build-from-source).

```sh
# Standalone; does not require a host or manager.
kausway-admin

# Application-selected configuration and component storage.
kausway-admin --config /app/config/config.toml --data-dir /app/data/admin

# Or launch through the manager (sibling binaries by default).
kausway run --config /app/config/config.toml --data-root /app/data --admin-ui
```

The commands assume the executables are on PATH; otherwise use their full paths
or `./kausway-admin` / `./kausway` from the extracted archive directory. A separately
downloaded manager also needs the host and optional admin companions.

Admin defaults to `127.0.0.1:47951`. Paths follow the shared
[platform rules](../architecture/configuration-and-storage.md). Its `--data-dir`
selects the admin component directory, whereas manager `--data-root` selects the
suite root. Supplying both explicit paths works without HOME, including services
and containers. An explicit standalone launch runs regardless of `admin.enabled`;
that setting controls manager launch behavior.

| Admin argument | Environment | Saved fallback |
| --- | --- | --- |
| `--config` | `KAUSWAY_CONFIG` | Platform default file |
| `--data-dir` | `KAUSWAY_ADMIN_DATA_DIR` | Platform data root + `admin/` |
| `--listen` | `KAUSWAY_ADMIN_LISTEN` | `admin.listen` |
| `--browser-base-url` | `KAUSWAY_ADMIN_BROWSER_BASE_URL` | `admin.browser_base_url` |
| `--manager-socket` | — | Supplied by the manager for verified lifecycle controls |

A single admin process owns its component directory using `admin.lock`.

## First-run browser setup

Without configured credentials, startup generates a random, single-use setup link
and prints it to stderr. When launched by the manager, admin sends a structured
startup event and the manager prints the link to its startup logs. It is not
included in retained lifecycle diagnostics or status responses.

Open that link, choose a username and password, and then sign in normally.
There is no default account or universal password.

The link:

- Expires after 15 minutes using an in-memory monotonic deadline.
- Uses a URL fragment so the token is not automatically sent in HTTP requests.
- Is removed from the browser address/history entry by the frontend before API calls.
- Is invalidated when replaced, when the process exits, or after successful setup.
- Authorizes account creation only; it cannot log in or reset an existing account.

Startup logs intentionally contain this temporary secret. Restrict access to those
logs. Passwords, session tokens and password hashes are never logged. Admin has no
HTTP access log containing request bodies or URLs.

If the link expires, run locally:

```sh
kausway-admin setup-link --config /app/config/config.toml --data-dir /app/data/admin
```

This replaces the previous link. It requires running admin and refuses once an
account exists. Visiting `/setup` without a link requires manually entering a
valid token; the first arbitrary visitor cannot claim the installation.

Both username and password hash must be present together. Malformed credentials
fail closed rather than reopening setup. Account creation succeeds only after an
atomic revision-checked save. Concurrent requests cannot replace the winning
account. A persistence failure leaves setup incomplete and reports an error.

## Terminal provisioning and recovery

Interactive password prompts are hidden and ask for confirmation:

```sh
kausway-admin credentials set \
  --config /app/config/config.toml --data-dir /app/data/admin

kausway-admin credentials reset \
  --config /app/config/config.toml --data-dir /app/data/admin
```

`set` initializes an account and refuses to replace one. `reset` explicitly
replaces it. Commands work while admin is stopped; on Unix they contact its
protected local socket when it is running, invalidating sessions immediately.
They do not require a manager or host.

For automation, use a username argument and password stdin:

```sh
kausway-admin credentials set \
  --config /app/config/config.toml --data-dir /app/data/admin \
  --username alice --password-stdin < /run/secrets/kausway_admin_password
```

One trailing LF or CRLF is removed from stdin input; other password content is
preserved. The password is not accepted through argv or an environment variable.
Username must contain 1–64 UTF-8 bytes with no control characters or surrounding
whitespace. Password must contain 12–1024 UTF-8 bytes.

An explicit reset can replace a malformed Argon2 hash in an otherwise valid schema.
An invalid TOML file or incomplete credential fields must first be corrected
locally; the server does not reinterpret corrupt configuration as a new install.

The same commands can run inside a container that contains the executable:

```sh
docker exec -it my-container kausway-admin credentials set \
  --config /etc/kausway/config.toml --data-dir /var/lib/kausway/admin

docker exec my-container sh -c 'kausway-admin credentials reset \
  --config /etc/kausway/config.toml --data-dir /var/lib/kausway/admin \
  --username alice --password-stdin < /run/secrets/kausway_admin_password'
```

Those paths are examples for an application-owned layout. The official Docker
suite selects `/config/config.toml` and `/data/admin`; see
[Docker setup, terminal commands and persistence](../integrations/docker-suite.md).
Config/data paths must match those supplied to the running component and be
mounted persistently by the container deployment.

## Storage and sessions

Only username and an encoded, salted Argon2id hash are saved under `[admin.auth]`
in the shared config. Other host/manager configuration is preserved. Authentication
never changes host identities, pairings or control credentials.

Setup/session tokens are random 256-bit values; only their digests are retained
in memory. Sessions expire after one hour and are not renewed automatically.
Restarting admin invalidates all sessions. Username/password changes and local
resets also invalidate every session; browser changes require the current password.
Externally changed credentials are detected on the next operation and invalidate
old sessions before authorization.

Session cookies are HttpOnly, SameSite=Strict, scoped to `/`, and named by browser
port so independent localhost installations do not overwrite each other's cookie.
They use local HTTP, so Secure cookies/TLS termination are not claimed here.
Modifying HTTP requests require an allowed Host, matching Origin, JSON content,
and session CSRF authorization where applicable. Setup uses its one-time token;
login uses strict same-origin checks rather than a pre-login session.

There is a global limit of ten setup/login/password-verification attempts per
minute. Argon2 work is serialized off the async executor with bounded admission.
Accepted stored hashes have bounded Argon2id version/work parameters; an edited
hash cannot request unbounded verifier memory. At most 64 live sessions exist.
HTTP/local connections, frames, bodies, read times and retained state are bounded.

## Container browser addresses

For a container interface listener, use `--listen 0.0.0.0:47951` and publish it
only on the Docker host's loopback interface. If the published browser port differs,
set, for example:

```text
--browser-base-url http://127.0.0.1:48000
```

The base URL controls generated links and Host validation, not the actual listener.
It must be a plain HTTP loopback origin, with no user info, extra path or query.
Remote-machine management is outside this design. Host control and manager IPC
remain internal and must not be published as browser ports.

## Protected local interface

On Unix, `<admin-data-dir>/control.sock` is inside a 0700 directory, has mode 0600,
and additionally checks the peer's effective UID. This channel is privileged: a
same-user caller can create or explicitly reset credentials. No unauthenticated
HTTP endpoint issues setup links or performs local resets.

Each connection carries one bounded newline-delimited JSON request, including the
canonical selected config path. Supported operations are `setup-link`,
`credentials.set`, and `credentials.reset`; the latter two include username and
password. Replies contain `ok` and `result`, or a generic error. Secrets are not
logged. Configuration mismatch is rejected. `status` and `entry-link` support
application handoff and manager forwarding.
See [application integration](../integrations/application-suite.md) for the implemented
private protocol and reference helper; no separately published SDK is required.

## Validation scope

Linux tests cover token expiry/replacement, hash-only persistence, malformed hash
rejection/recovery, CSRF, sessions, resets, rate limiting and storage failures.
`scripts/ci/admin-smoke.py <binary-directory>` exercises real HTTP/CLI flows,
concurrent setup, offline provisioning, persistence, and manager log handoff.
The smoke test is included in Build Validation.

Windows/macOS native validation remains deferred to their build workflows. Live
CLI IPC is implemented on Unix; on Windows, stop admin for offline credential
commands until named-pipe support is implemented. No Android admin release or
runtime support is claimed by these Linux checks.
