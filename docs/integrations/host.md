# Integrate a server

Run `kausway-host` as a supervised process or adjacent service, pointing its origin
at the server's HTTP endpoint. Wait for readiness, capture logs, handle shutdown
and use bounded restart backoff. The client SDK embeds into the client application;
it does not need to launch the development CLI.

Keep the host control API private. A server-side adapter holds the administrative
token and exposes only authenticated administration operations to its settings UI.
Never return that token to browser JavaScript. Device pairing and revocation are
separate from the origin application's user authentication.

- [Host setup and configuration](../getting-started/host.md)
- [Control API v1](../../protocol/control-v1.md)
- [Transport protocol v1](../../protocol/transport-v1.md)
- [Jellyfin setup and deployment](jellyfin.md)
- [Jellyfin reference plugin](../../integrations/jellyfin/plugin/)
- [Combined Jellyfin deployment](../../deploy/jellyfin/compose.yaml)

The [Docker suite](docker-suite.md) supplies a manager/admin/host sidecar, while
[application integration](application-suite.md) covers supervising the suite or
independent host/admin binaries. Admin can provide settings and pairing UI without
the developer implementing another settings page. Its lifecycle controls require
the matching owning manager and host restart policy.
