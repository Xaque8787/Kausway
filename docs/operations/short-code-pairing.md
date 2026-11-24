# Short-code pairing

Short-code support is currently a development feature. Use matching development
host/admin/client binaries, the rebuilt Android SDK, and the rebuilt Jellyfin
plugin. Published 0.5.0 assets predate this feature. The Android TV fork's input
screen still needs a separate update; rebuilding its SDK alone does not remove
its existing URL-only input validation.

An invitation can have a full `kausway://invite/...` URL and an optional
`K1-XXXXXX-XXXXXX-XXXXXX` code. Either input redeems the same single-use token,
with the same expiry. The URL works without discovery. Already-paired devices
reconnect without looking up a code. Treat both inputs as passwords.

## Host CLI and client CLI

Start the host as usual, either directly or through the manager. A running host
owns publication; the manager and admin UI are optional.

```sh
kausway-host serve --origin http://127.0.0.1:8096
# In another terminal, using the same configuration and host data directory:
kausway-host invite --label 'Phone' --short-code
```

`--short-code` implies JSON output containing `id`, `expires_at`, `link`,
`short_code` and `rendezvous` metadata. Creation initially reports `pending`;
publication runs in the host background. Use the URL immediately or wait for a
publication acknowledgment before trying the code. Acknowledgment does not
prove that every network can discover the record.

Without `--short-code`, the command still prints only the URL. Add `--json` to
obtain ordinary invitation metadata and the URL without creating a code.
Offline URL creation retains its existing behavior; short-code creation requires
a running host. Codes and URLs are delivered only on creation, not returned by
later listings. Restarting preserves a pending code's validity, but cannot
redisplay it. Create a new invitation if you did not retain the original input.
A host permits at most 20 active code invitations. Cancel an unused invitation
or create a URL-only invitation when that limit is reached.

```sh
kausway-test-client --data-dir ./phone-test pair 'K1-XXXXXX-XXXXXX-XXXXXX'
# Or use the original URL; no DHT or gateway lookup occurs:
kausway-test-client --data-dir ./phone-test pair 'kausway://invite/...'
kausway-test-client --data-dir ./phone-test proxy --listen 127.0.0.1:8097
```

The sample code and URL above are placeholders. The client reports resolving
and connecting stages. Ctrl-C cancels pairing. Discovery errors leave saved
pairing and any running proxy intact. A successful new pairing replaces the
saved host as before. A canceled request may already have reached the host;
cancellation cannot undo token redemption that the host has committed.

## Admin and Jellyfin dashboards

Select **Also generate a short code** when creating an invitation. The creation
result shows both inputs, separate copy buttons, expiry and publication status.
The option is disabled when the connected host does not advertise support.

- **Pending:** publication has not been acknowledged. The URL is usable now.
- **Published:** a discovery backend acknowledged publication; try the code.
- **Retrying:** the host retries in the background. Use the full URL as fallback.

The pending-invitation list refreshes status without retrieving secrets. Both
inputs stop authorizing a new device after either is redeemed, canceled or
expired. DHT/gateway caches may retain encrypted records; the host still rejects
an invalid token. The dashboards clear displayed inputs when the invitation is
no longer pending or the page is left. Copying a secret places it in the system
clipboard, which the dashboard cannot reliably clear afterward.

The Jellyfin plugin remains a dashboard adapter to a separately running host.
It does not start a host, manager or DHT helper process. Short-code support does
not change manager ownership or host lifecycle permissions.

## Discovery backends

Mainline DHT is enabled by default and needs working outbound IPv4 UDP. No
rendezvous socket or gateway client starts for URL-only pairing. Discovery
settings are separate from `KAUSWAY_RELAY_URL`, which controls Iroh traffic.

Host `serve` and the CLI client accept these launch options:

| Flag | Environment | Default |
| --- | --- | --- |
| `--rendezvous-dht true\|false` | `KAUSWAY_RENDEZVOUS_DHT` | `true` |
| `--rendezvous-gateway URL` (repeatable) | `KAUSWAY_RENDEZVOUS_GATEWAYS` (comma-separated) | none |
| `--rendezvous-timeout-secs N` | `KAUSWAY_RENDEZVOUS_TIMEOUT_SECS` | `10`, allowed `1`–`30` |

These are launch settings, not saved admin settings. Put CLI client discovery
options before its subcommand. For the manager or Docker suite, provide the
environment variables to the manager/container so its host child inherits them.
The host worker also applies an outer 12-second publication budget per attempt,
including initialization. The client discovery deadline includes bootstrap
initialization and lookup.

When UDP is blocked, configure an interoperable Pkarr HTTPS gateway on publisher
and resolver. No public gateway is selected automatically. For example, replacing
the example URL with a gateway you operate or have verified:

```sh
export KAUSWAY_RENDEZVOUS_GATEWAYS=https://pkarr.example.com
export KAUSWAY_RENDEZVOUS_DHT=false
```

A gateway is a discovery service, not an Iroh media relay. It must implement
Pkarr's binary GET/PUT protocol. Its URL must have no credentials, query or
fragment; redirects are disabled. HTTPS uses bundled public certificate roots.
HTTP is accepted only for explicitly configured `localhost`, `127.0.0.1` or
`[::1]` fixtures. Configure at most eight gateways. Disabling both backends
leaves normal URLs and reconnection usable, but code discovery fails.

## Rust and Android integrations

The Rust `Client::pair(input)` method accepts either input. Existing
`ClientConfig` construction remains valid. Use
`Client::new_with_rendezvous(config, network_config)` to override discovery.
`pair_with_progress` reports `Resolving`/`Connecting`; `pairing_stage()` can be
polled, and `cancel_pairing()` interrupts the current operation. Shutdown
interrupts discovery before closing the client. Failed lookups hold no main
client lifecycle lock and do not replace saved pairing.

Android retains `client.pair(invitationLink)`; pass a code in that same argument.
New `KauswayConfig` options are `rendezvousDht`, `rendezvousGateways` and
`rendezvousTimeoutSeconds`. Use `getPairingProgress()` and `cancelPairing()` from
another coroutine while pairing runs. Errors distinguish discovery not found,
timeout, expiry, invalid record, unavailable backend and cancellation. Pairing
still runs outside the UI thread; coroutine cancellation alone does not interrupt
the SDK's existing noncancellable native call, so use `cancelPairing()`.

Only the paired endpoint address and device identity are retained by the client.
The host keeps private recoverable publication material for active code
invitations; ordinary invitations remain hash-only. See the
[protocol and threat model](../../protocol/rendezvous-v1.md).

## Local verification

After building host/client binaries, run:

```sh
python3 scripts/ci/short-code-e2e.py rust/target/debug
```

This test uses local gateway and HTTP origin fixtures with real binaries, checks
pairing, Range forwarding, cached-record cancellation/single use, and URL pairing
with discovery unavailable. Rust tests additionally exercise an isolated local
DHT. Public DHT availability, real HTTPS gateway interoperability and phone/TV
network acceptance remain separate manual checks.
