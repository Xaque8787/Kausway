# Kausway release files

Use BUILD-INFO.json to identify the component, release track, version, architecture
and exact source commit. A release-candidate download is not a published release.
Verify the adjacent SHA-256 checksum before extracting an archive.

Suite archives contain manager (`kausway`), host and admin. Individual component
archives are also available. Android suite assets contain only host; Android does
not include manager/admin. CLI client downloads belong to the independent client
release. Download that component separately when testing a suite.

Run a Linux suite with `./kausway run --admin-ui`. On first launch open the expiring
setup link from the terminal, create credentials, configure the origin, save, and
start the host. Supply --config and --data-root for application-owned persistent
paths, or intentionally use platform defaults. Never replace live identity/state
when updating. See the included operations/integration documentation for details.

The manager component requires a separately supplied host and optional admin.
Standalone admin cannot control an externally supervised host's process lifecycle.
The frontend is embedded; no Node runtime is required.

For the CLI client, use `--data-dir <persistent-directory> pair '<fresh-invitation>'`,
then `--data-dir <same-directory> proxy --listen 127.0.0.1:8097`. Treat invitations
as temporary passwords. Retain the same data directory across client restarts.

Project and third-party license notices are included under licenses/.
