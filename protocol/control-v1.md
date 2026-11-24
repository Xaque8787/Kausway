# Control API v1 (media server plugin to host)

Status: implemented in `rust/crates/kausway-host/src/control.rs`. Used by the Jellyfin plugin. Designed so any media server (for example KMS) can manage the same `kausway-host` executable.

The control API is separate from the device transport (`transport-v1.md`) and is versioned independently.

## Transport

- HTTP/1.1 with JSON bodies on a TCP listener. Default `127.0.0.1:47950`, set with `--control-listen` / `KAUSWAY_CONTROL_LISTEN`, or `off` to disable it.
- By default the host only binds loopback addresses. A non-loopback address needs `--control-allow-remote`, because the API can create invitations.
- Requests are limited to 16 KiB bodies. Responses are `application/json; charset=utf-8`.
- Every path is prefixed with the major version, `/v1/`. Breaking changes get `/v2/`, and a host may serve both during a transition. Fields may be added to v1 responses, and clients must ignore unknown fields.

## Authentication

Every endpoint except `GET /v1/health` requires:

```
Authorization: Bearer <control token>
```

The token is the trimmed content of `control.token` in the host's data directory. `serve` creates it on first start with 256 random bits and owner-only permissions; an existing file shorter than 32 characters stops `serve` with an error rather than being replaced. Setting `KAUSWAY_CONTROL_TOKEN` (at least 32 characters) uses that value instead, and nothing is written. The host compares SHA-256 digests of the presented and expected tokens, so timing does not reveal how much of a guess matched. A missing or wrong token gets `401`.

The plugin, or any embedding server, reads the same file (or is given the same environment variable). Anyone who can read the data directory can manage the host. That is intended: the data directory already holds the host's private key.

## Errors

Non-2xx responses carry:

```json
{"error":{"code":"not_found","message":"no active device with that endpoint id"}}
```

| Status | `code` |
|---|---|
| 400 | `bad_request` |
| 401 | `unauthorized` |
| 404 | `not_found` |
| 405 | `method_not_allowed` |
| 409 | `conflict` |
| 413 | `payload_too_large` |
| 500 | `internal` |

## Endpoints

### `GET /v1/health` (no authentication)

```json
{"ok":true,"control_api":1}
```

### `GET /v1/status`

```json
{
  "control_api": 1,
  "transport_protocol": 1,
  "host_version": "0.3.0",
  "endpoint_id": "2118dc05...",
  "origin": "http://127.0.0.1:8096/",
  "relay": {"mode": "default", "url": null},
  "relay_connected": true,
  "addr": { "...": "Iroh EndpointAddr as JSON" },
  "policy": {
    "configured": "p2p_only",
    "enforcement": "best_effort_path_gate",
    "relay_transport_only": false,
    "restart_required": false
  },
  "websocket": {"enabled": true, "origin_header": "rewrite"},
  "devices": 3,
  "pending_invitations": 1,
  "sessions": 1,
  "uptime_secs": 5321
}
```

- `relay.mode` is `default`, `custom`, or `disabled`. `url` is set for `custom`.
- `relay_connected` is true when the endpoint's current address includes a relay.
- `policy.relay_transport_only` reports whether this running endpoint was started without IP transports, which is what `relay_only` does at startup.
- `restart_required` is true when the configured policy needs a restart to take full effect, meaning when switching to or from `relay_only`.

### `GET /v1/sessions`

Live sessions, with the path Iroh has selected right now:

```json
{"sessions":[{
  "endpoint_id":"9436...","name":"Living room TV","connected_at":1793740000,
  "path":{"kind":"direct","rtt_ms":1.3,"remote_addr":"Ip(192.168.1.20:40944)","open_paths":1},
  "requests":42,"active_requests":1,"active_websockets":1,"bytes_sent":73400320
}]}
```

`path.kind` is `direct`, `relay`, or `unknown`. `rtt_ms` and `remote_addr` come from Iroh's path diagnostics.

### `GET /v1/devices`

```json
{"devices":[{"endpoint_id":"9436...","name":"Living room TV","paired_at":1793700000,
  "invitation_id":"inv-3feafff70d54","revoked_at":null,"connected":true}]}
```

### `DELETE /v1/devices/{endpoint_id}`

Revokes an active device: `204`, or `404` if there is no active device with that id. Its open sessions close within about 2 seconds. A revoked device can pair again only with a new invitation.

### `GET /v1/invitations`

Unredeemed, unexpired invitations. Tokens and short codes are never listed. Ordinary invitations remain hash-only; active short-code invitations keep private encrypted recovery material.

```json
{"invitations":[{"id":"inv-3feafff70d54","label":"Phone","created_at":1793740000,"expires_at":1793743600}]}
```

### `POST /v1/invitations`

Request: `{"label":"Phone","ttl_secs":3600}`. `ttl_secs` is optional, between 60 and 86400, default 900.

Response `201`:

```json
{"id":"inv-3feafff70d54","label":"Phone","created_at":1793740000,"expires_at":1793743600,
 "link":"kausway://invite/eyJ2Ijox..."}
```

The link holds the plaintext token. It is returned only once.

Optional `short_code: true` in the creation request returns an additional
`short_code` string and `rendezvous` metadata. The `link` field always contains
the existing full URL. The two inputs share the same token, expiry, cancellation
and one-time redemption. Publication is asynchronous; creation does not wait for
DHT/gateway reachability. Missing or `false` preserves URL-only creation.

`rendezvous` metadata contains `state` (`pending`, `published`, `retrying`),
`published_at` (nullable Unix seconds), `next_attempt_at` (Unix seconds) and
`error` (nullable safe error code). Listings include metadata for code invitations
but never keys, codes, URLs or tokens. A publication acknowledgment is not a
guarantee of global discoverability. `too_large` means the encoded invitation
cannot fit discovery; the original full URL remains valid.

### `GET /v1/invitations/{id}`

Returns one pending invitation's metadata, including optional `rendezvous`.
Returns `404` after cancellation, expiry or redemption. Requires the control
bearer token, as do listings. The host reloads persisted invitations before
answering so CLI-created invitations can be polled immediately.

`GET /v1/status` advertises `rendezvous: {"supported": true, "code_version": 1}`.
Older hosts omit this capability; UIs should offer their existing URL-only flow.

### `DELETE /v1/invitations/{id}`

Cancels an unredeemed invitation: `204`, or `404`.

### `PUT /v1/policy`

Request: `{"policy":"auto|relay_only|p2p_only"}`. Response `200` has the same shape as `status.policy`. It returns `409` when `relay_only` is requested but relays are disabled.

The policy is saved in `state.json`. Request admission follows it immediately. Starting or stopping relay-only transports needs a host restart, which `restart_required` reports. A `--policy` / `KAUSWAY_TRANSPORT_POLICY` setting overrides the saved value at every start.

### `PUT /v1/websocket`

Request: `{"enabled":true|false,"origin_header":"rewrite|pass_through"}`. Both fields are optional; a missing field keeps its current value, and unknown fields are rejected with `400`. Response `200` has the same shape as `status.websocket`:

```json
{"enabled":true,"origin_header":"rewrite"}
```

The settings are saved in `state.json` and apply without a restart. A new WebSocket checks them when it opens, and open WebSockets re-check them every 2 seconds, so turning WebSockets off closes the open ones. Plain HTTP requests are never affected. `--websockets` / `KAUSWAY_WEBSOCKETS` and `--websocket-origin` / `KAUSWAY_WEBSOCKET_ORIGIN` override the saved values at every start.

`status` includes the current `websocket` object, and each entry in `sessions` includes `active_websockets`.

## Shared configuration extensions (Phase 4)

The existing routes and wire behavior remain supported. Current standalone host
launches additionally provide these authenticated routes:

| Method | Route | Behavior |
| --- | --- | --- |
| GET | `/v1/config` | Safe saved/effective configuration, sources, opaque revision, pending overrides and storage paths |
| PUT | `/v1/config` | `{revision, settings}`; validate and atomically save ordinary settings without applying |
| POST | `/v1/config/apply` | `{revision}`; apply saved live policy/WebSocket settings with revision protection |

`settings` contains host, manager and non-auth admin settings. It excludes
credentials, tokens and state files. Changing manager desired host-enabled state
through this route is rejected; lifecycle control belongs to the owning manager.
Runtime overrides do not rewrite saved values. Config conflicts/validation failures
return a configuration error rather than overwrite current settings. A host created
without a shared configuration context reports that context as unavailable.

Apply reports `saved`, `applied`, `applied_revision`, and `restart_required`.
The configuration view also reports `transport_restart_required` when the live
policy differs from endpoint construction. A changed saved live value overridden
at launch is not applied. Relay-only without an enabled runtime relay is refused.

The host's data directory now contains a private `control-address.json` publishing
its current control listener. It contains no control token and is not a PID-based
process adoption mechanism. The authenticated API remains the authority for state.
