# Quick start: local testing

Run any reachable plain HTTP server through a Kausway host, then access it from
the client CLI or a browser. No Jellyfin plugin, Android SDK or Docker is required.
The host and client can run on the same computer or on separate computers.

## 1. Download the CLI binaries

Open the [latest suite release](https://github.com/Xaque8787/Kausway/releases/latest)
for the host. For the CLI client, open [Releases](https://github.com/Xaque8787/Kausway/releases)
and choose the newest stable **Client** release. Select archives for **the computer
that will run each binary** and download each adjacent `.sha256` file.

Download the standalone **host** archive from the suite release and the separate
**test-client** archive from the client release. Extract each and copy its executable
into a working directory such as `kausway-test-tools`; the following commands use
that directory. If they run on different computers, download the matching CPU for
each. Suite/client versions are independent.

| Computer | Archive suffix |
|---|---|
| Linux x86_64, including Linux inside WSL | `x86_64-unknown-linux-musl.tar.gz` |
| Linux ARM64 | `aarch64-unknown-linux-musl.tar.gz` |
| Android ARM64, via adb shell or a suitable terminal environment | `aarch64-linux-android.tar.gz` |
| Android 32-bit ARM | `armv7-linux-androideabi.tar.gz` |

The full filename is `kausway-host-<suite-version>-<suffix>` or
`kausway-test-client-<client-version>-<suffix>`. Native Windows and macOS CLI
binaries are not currently built. The Android CLI archives are different from the
Android SDK AAR and do not contain an Android application.

On Linux, in your download directory, substitute the actual version and filename:

```sh
SUITE_VERSION='<downloaded-suite-version>'
CLIENT_VERSION='<downloaded-client-version>'
TARGET=x86_64-unknown-linux-musl
HOST_ARCHIVE="kausway-host-$SUITE_VERSION-$TARGET"
CLIENT_ARCHIVE="kausway-test-client-$CLIENT_VERSION-$TARGET"
sha256sum -c "$HOST_ARCHIVE.tar.gz.sha256"
sha256sum -c "$CLIENT_ARCHIVE.tar.gz.sha256"
tar -xzf "$HOST_ARCHIVE.tar.gz"
tar -xzf "$CLIENT_ARCHIVE.tar.gz"
mkdir -p kausway-test-tools
cp "$HOST_ARCHIVE/kausway-host" kausway-test-tools/
cp "$CLIENT_ARCHIVE/kausway-test-client" kausway-test-tools/
cd kausway-test-tools
chmod +x kausway-host kausway-test-client
./kausway-host --version
./kausway-test-client --version
```

Run the following commands from the extracted binary directory. Keep it available
in each terminal. Relative data-directory paths are resolved from your working directory.

## 2. Start a server to test

If you already have a server, use its HTTP address and skip this example.
The origin must be `http://hostname:port`, without a path or query string;
HTTPS origins are not supported yet. It must be reachable **from the host computer**.

For a reproducible example, open a separate terminal on the host computer:

```sh
mkdir -p /tmp/kausway-demo
printf 'Hello through Kausway!\n' > /tmp/kausway-demo/hello.txt
python3 -m http.server 8080 --bind 127.0.0.1 --directory /tmp/kausway-demo
```

Leave it running. This Python server is only a test origin, not part of Kausway.
For your own service, replace `http://127.0.0.1:8080` below, for example with
`http://192.168.1.20:3000`. `127.0.0.1` always refers to the host computer itself.

## 3. Check and start the Kausway host

In a second terminal, from the extracted binary directory:

```sh
./kausway-host doctor --data-dir ./demo-host --origin http://127.0.0.1:8080 --control-listen off
./kausway-host serve --data-dir ./demo-host --origin http://127.0.0.1:8080 --control-listen off
```

Wait for `host started` and leave this terminal running. Doctor checks configuration
and origin reachability; it can create local identity/state files. The control API
is disabled here because the walkthrough uses CLI administration.

## 4. Create a pairing invitation

In a third terminal **on the host computer**, use the same host data directory:

```sh
./kausway-host invite --data-dir ./demo-host --label "Demo client" --ttl-secs 900
```

Copy the complete `kausway://invite/...` link. It works once and expires after
15 minutes. Treat it as a temporary password. Keep the host running while pairing;
creating the invitation after startup includes the running host's address hints.

## 5. Pair the client and check the connection

On the client computer (or another terminal on the same computer), from its
extracted binary directory, replace the placeholder with the complete link:

```sh
./kausway-test-client --data-dir ./demo-client --name "Demo client" pair 'kausway://invite/PASTE_COMPLETE_PAYLOAD_HERE'
./kausway-test-client --data-dir ./demo-client status
```

Single-quote the link so the shell passes it unchanged. Status connects to the
paired host and reports authorization, transport policy, path and five round-trip
measurements. Pairing is stored in `./demo-client`; reuse that directory for later commands.

## 6. Fetch a file or API response

For the Python example:

```sh
./kausway-test-client --data-dir ./demo-client fetch /hello.txt
./kausway-test-client --data-dir ./demo-client fetch /hello.txt --output hello-copy.txt
```

The response body goes to stdout or the output file; status, headers and transfer
information go to stderr. Fetch uses HTTP GET. For your own server, supply its path
rather than its full URL, and any headers it requires:

```sh
./kausway-test-client --data-dir ./demo-client fetch '/api/items?limit=10' --header 'Authorization: Bearer YOUR_SERVER_TOKEN' --header 'Accept: application/json'
./kausway-test-client --data-dir ./demo-client fetch /movie.mp4 --range bytes=0-1048575 --output first-megabyte.bin
```

Kausway pairing does not replace your server's login/authentication. Range requests
work only if the origin supports them; Python's basic file server may return a full
response instead of a partial response.

## 7. Open the server through a local proxy

On the client computer:

```sh
./kausway-test-client --data-dir ./demo-client proxy --listen 127.0.0.1:8097
```

Leave it running and open `http://127.0.0.1:8097/hello.txt` in a browser **on the
client computer**, or use that URL in an HTTP client. For your own server, open its
usual path beneath this local address. Each client application points at this
loopback proxy; it does not need to understand Kausway. Absolute redirects, embedded
absolute URLs and domain-bound cookies may need application-specific handling.

## 8. Stop or revoke access

Use Ctrl-C in the proxy, host and test-server terminals to stop them. Restarting
with the same data directories retains identity and pairing.

To list and revoke a client, run on the host computer:

```sh
./kausway-host devices --data-dir ./demo-host
./kausway-host revoke --data-dir ./demo-host CLIENT_ENDPOINT_ID_FROM_DEVICES
```

Revocation ends open sessions within a few seconds. Create a fresh invitation to
pair again. To simulate another device, choose a different client data directory.

## Local-only networking and troubleshooting

The commands above use Iroh's default public relays when needed. To disable relay
and address-lookup services for direct local testing, add `--relay-url none` to
**host serve** and **every client command**. Also add it to doctor/invite when used.
Both computers must have a working direct network path; a local emulator or firewall
can prevent UDP connectivity even when an HTTP server is reachable.

- Doctor fails: check the origin URL and that the server is listening.
- Pairing fails: keep the host running, use an unexpired unused link and check network/firewall access.
- Not paired: use the client data directory used by `pair`.
- Proxy port occupied: choose another `--listen` port.
- HTTP 401/403: provide the origin application's credentials.
- Wrong executable format: download the archive for the executing computer's OS/architecture.

See the [complete CLI reference](../operations/cli-reference.md) for every command,
argument, environment variable and default, or run either binary with `--help`.
