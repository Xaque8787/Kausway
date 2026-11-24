# Rendezvous protocol v1

Status: protocol v1 with host CLI/API, client/Android SDK, admin and Jellyfin
plugin integration. Public-network acceptance remains separate from local
verification; this document does not claim public reachability.

Rendezvous optionally discovers an existing invitation. Pairing and device
transport remain as specified in [transport-v1.md](transport-v1.md). The normal
`kausway://invite/<base64url(JSON)>` URL remains the complete fallback.

## Trust model and fallback

A generated code is a bearer secret with 90 bits of entropy. Its holder can derive
both the record signing key and encryption key, recover the invitation token,
and publish replacement records at the same lookup identity. A known-code holder
can poison the record, including by advancing its sequence or replacing its host
address. A signature proves possession of the code-derived key, not exclusive
ownership by the intended host. Exchange codes through an appropriate trusted
channel. The rendezvous identity is separate from the host's Iroh identity.

DHT nodes and gateways are untrusted storage and transport. They can observe
lookup identities, traffic timing, packet sizes and network addresses, withhold
records, or replay records within freshness limits. The public lookup key permits
offline checking of guesses; security depends on uniform random codes. Storing
an encryption key beside ciphertext in private host state does not protect
against an attacker who can read that state.

The full URL requires no rendezvous lookup and must remain usable when discovery
fails. It still requires working Iroh routes and a valid pairing token. When all
backends are unavailable, a code alone cannot reconstruct it. The host remains
authoritative for token expiry, one-time redemption, cancellation and device
authorization. Neither cached records nor a successful publication override those
checks. DHT records cannot be deleted globally.

## Code generation and normalization

Generate 18 uniform random Crockford Base32 symbols with a CSPRNG, for exactly
90 random bits. The alphabet, in numeric order, is:

```text
0123456789ABCDEFGHJKMNPQRSTVWXYZ
```

The standard display has three groups of six payload symbols:

```text
K1-XXXXXX-XXXXXX-XXXXXX
```

Normalize as follows:

1. Reject input longer than 64 bytes before normalization, and reject non-ASCII
   input. Remove only ASCII spaces (`0x20`) and hyphens (`0x2d`), including leading
   and trailing separators. Tabs, newlines and other whitespace are invalid.
2. Uppercase ASCII letters. Require the prefix `K1`; aliases do not apply to it.
3. In the payload only, map `O` to `0` and `I`/`L` to `1`.
4. Require exactly 18 payload symbols from the alphabet. Reject other characters,
   unsupported prefixes, and incorrect lengths.

The canonical HKDF input is exactly 20 ASCII bytes: `K1` followed by the 18
normalized uppercase symbols, with no separators. It is not a decoded Base32
integer. For example, `k1-oooooo-iiiiii-llllll` becomes
`K1000000111111111111`, displayed as `K1-000000-111111-111111`.
There is no checksum. Code and derived-secret debug output must be redacted.

## Key derivation

Use HKDF-SHA256 (RFC 5869), with literal ASCII salt and info strings:

```text
IKM = ASCII(canonical_code)
PRK = HKDF-Extract(ASCII("kausway-rendezvous-v1"), IKM)
signing_seed = HKDF-Expand(PRK, ASCII("pkarr-ed25519-seed"), 32)
encryption_key = HKDF-Expand(PRK, ASCII("invite-encryption-key"), 32)
lookup_public_key = Ed25519PublicKeyFromSeed(signing_seed)
```

The signing seed is 32 bytes, not an expanded serialized secret key. The shared
keys type derives the public lookup identity from that seed. Do not derive the
host identity or normal pairing token from the code. Do not log either key.

## Encrypted invitation wire format

The plaintext is compact UTF-8 JSON encoding of the three-element tuple:

```text
[1, <EndpointAddr JSON>, "<original pairing token>"]
```

The first element is the existing invitation version, followed by the complete
Iroh address and token. It is not a JSON object or an encoded invitation URL.
The network copy may have routes pruned as described below. The host identity,
token and invitation expiry remain unchanged.

Use XChaCha20Poly1305 with the derived 32-byte encryption key, a fresh random
24-byte nonce for every encryption, and a 16-byte tag appended to the ciphertext.
The sealed envelope is:

```text
version[1] || expires_at_u64_be[8] || nonce[24] || ciphertext || tag[16]
```

The envelope version byte is `0x01`. Expiry is the underlying invitation's Unix
expiry time in seconds. Associated data is exactly:

```text
ASCII("kausway-rendezvous-v1") || envelope[0..9]
```

Thus version and expiry are authenticated cleartext; the address and token are
encrypted. The lookup public key is not part of the associated data. The envelope
is at least 49 bytes and at most 8192 bytes, allowing at most 8143 plaintext
bytes. Unknown versions, invalid sizes, authentication failures and malformed
JSON tuples are rejected. Reconstruct an existing `InviteLink` and validate it
through the existing invitation decoder, including version, token and URL limits.
Reject expiry when `now_seconds >= expires_at`, with no expiry grace period.

The private recovery copy uses this same envelope and tuple plaintext with a
separate fresh nonce. Its address retains all original routes. It is not an
encrypted URL string. Never reuse a nonce with the same key for a new encryption.

## Pkarr representation

Use the Pkarr/BEP 44 signed DNS codec in `iroh-dns` 1.4.1. Store the sealed
envelope as URL-safe Base64 without padding, split into chunks of up to 240 ASCII
characters. Each chunk is a TXT value under `_kausway`, relative to the lookup
public key's z-base-32 DNS origin:

```text
00:<first chunk>
01:<next chunk>
```

The prefix is exactly two decimal digits followed by `:`. There is no `k1`
prefix and no total-count field. The encoder emits consecutive indices from
`00`, with 240-character chunks except for the final chunk. The decoder sorts
by index, requires a nonempty consecutive sequence from zero with no duplicates
or gaps, and accepts only nonempty chunks of at most 240 characters. Concatenate
the chunks, decode Base64url, then open the sealed envelope. DNS answer order is
not significant; signatures cover the exact encoded DNS packet.

| Limit or field | Value |
| --- | --- |
| TXT owner | `_kausway.<z-base-32 lookup public key>.` |
| DNS TTL | 300 seconds; a cache hint |
| DNS wire packet | At most 1000 bytes, including DNS overhead |
| Full signed packet | At most 1104 bytes |
| Gateway relay payload | At most 1072 bytes |
| Mutable-item salt | Absent |

The full signed packet is:

```text
public_key[32] || signature[64] || sequence_u64_be[8] || DNS_wire_packet
```

The Ed25519 signature covers the unsalted BEP 44 bytes:

```text
ASCII("3:seqi" + decimal(sequence) + "e1:v" + decimal(DNS_wire_packet.len) + ":")
    || DNS_wire_packet
```

Mainline stores the DNS bytes as mutable `v`, with the same sequence, key and
signature, and no salt. A Pkarr gateway path appends the z-base-32 lookup key to
its configured base URL. Its binary relay payload omits the first 32 public-key
bytes: `signature || sequence_u64_be || DNS_wire_packet`. GET retrieves it and
PUT publishes it. It is neither JSON nor Base64.

## Sequence and resolution limits

For each newly signed publication, reserve and atomically persist:

```text
next_sequence = max(unix_microseconds_now, last_reserved_sequence + 1)
```

Persist before publication, under the existing host state lock. Failed sends and
crashes do not roll back the reservation. An exact retry can reuse identical
signed bytes; a newly signed refresh uses a new reservation. The sequence must
fit a nonnegative signed 64-bit BEP 44 integer and remain within the allowed
future skew. Process-local codec timestamps alone do not provide persistence.

The record age limit is 3600 seconds and allowed future skew is 300 seconds.
At those boundaries, reject a sequence more than 3600 seconds old or more than
300 seconds ahead. Invitation expiry is independent and may reject an otherwise
fresh packet. Refresh records every 240 seconds while the invitation is active.

Verify the signature and that the packet key matches the queried lookup key
before decrypting. Reconstruct DHT results into the verifying codec path;
unchecked packet constructors are not verification. Then check sequence bounds,
freshness, chunks, authenticated envelope and the existing invitation validator
before returning an invitation to an internal caller.

A backend may select the freshest verified candidate it observes. Equal-sequence
disagreements may use a deterministic tie-break or return an error. There is no
persistent resolver context or antirollback guarantee, and no promise of finding
the globally latest record. Valid signatures and bounded freshness do not prevent
known-code poisoning, selective withholding or replay within the age window.

## Route fitting and fallback URL

Start from the full original invitation for each publication. Preserve every
relay address, the host identity and token. Prefer direct addresses in this
order: public IPv4, public IPv6, private/local, loopback. Keep at least one usable
IP route when there is no relay. A private or loopback address does not promise
internet reachability.

The record code filters invalid IP routes during pruning. Unspecified, multicast
or port-zero IP addresses cannot be the only route in an accepted invitation.

Measure the final encoded DNS packet, including encryption, Base64 and DNS/TXT
overhead. If it exceeds 1000 bytes, prune direct routes while preserving the
requirements above. If the required routes and fields still do not fit, return
an explicit size/route error. Never truncate a relay URL, token or encoded URL,
or silently return an endpoint-id-only invitation. Keep the original fallback
URL unchanged, including routes omitted from the published copy.

## Private state and worker integration

Use an optional map keyed by invitation id in the same private, atomically
written `state.json` as the normal invitation records. Existing files without
the map remain valid. Ordinary invitations remain hash-only.

Ordinary state files retain storage version 1. Opting into rendezvous advances
the file to storage version 2 so an older host rejects it instead of silently
discarding private recovery data. This does not change the invitation or transport
wire version. Public state snapshots exclude the private map.

An opted-in entry contains the signing seed, encryption key, sealed full-original
invitation, last reserved sequence and publication status/scheduling metadata.
Store neither the code nor a plaintext token or URL. The sealed original uses
the tuple envelope above and keeps all original routes. Derive the lookup public
identity from the shared keys type. Redact secrets and sealed-invite debug output.

Create the invitation and its optional private entry in one state mutation.
Reserve sequences and update status with the existing lock/reload/atomic-write
mechanism, preserving unrelated state. Keep network operations outside the state
lock. Redemption, cancellation and expiry remove the corresponding private entry
and stop refresh/retry work; stale completions must not recreate it. Shutdown
stops the worker and releases resources, preserving active entries for restart.
Cleanup cannot delete remote DHT records globally.

The backend uses Mainline 6.2 and the `iroh-dns` 1.4.1 codec. It is currently
compiled in; operation is optional at runtime. Build-time feature gating may be
added later. Initialize the host worker's network backend lazily only when active
pending publications exist. Normal URL invitations and an empty map do not
require rendezvous network initialization. Initialization failure is retryable.

Gateway URLs are explicitly configured. Require HTTPS for nonloopback URLs;
explicit local configuration may use HTTP for `localhost`, `127.0.0.1` or `::1`.
This exception is available for local operation, not only test builds. Validate
TLS normally and do not follow redirects to bypass scheme validation. There is
no default HTTP gateway or reliance on an unverified public gateway. Configured
gateways provide an alternative when DHT operations fail; attempts may be raced.

The shared network-operation timeout defaults to 10 seconds and is configurable
up to 30 seconds. The host worker applies an outer 12-second budget per
publication, including backend initialization. The worker chooses a finite concurrency bound, bounds queued
work and retries, and refreshes every 240 seconds without extending invitation
expiry. Retry/backoff details and shutdown timing are implementation choices,
not additional wire requirements. Cancelling a caller does not promise to cancel
UDP queries already issued by Mainline. A backend acknowledgment does not prove
global reachability.

At most 20 DHT query owners can remain active across clones of one network client.
Owners retain their admission slots until traversal completes, even when a caller
times out or a gateway wins; a busy DHT does not enqueue another query. Mainline
has no whole-traversal cancellation deadline, so these owners can outlive the
network client while draining. HTTP fallback remains available independently.

Application integrations use `Network::new_async` for bounded initialization.
Mainline resolves default bootstrap hostnames through blocking system DNS during
construction. A deadline and a process-wide cap of 20 detached constructor
threads prevent canceled callers from queuing unlimited constructors or holding
up Tokio runtime shutdown. An OS DNS call already in progress can finish after
its caller times out; it retains its admission slot until completion. The client
applies one discovery deadline across initialization and lookup.

## Reproducible local test plan

This plan describes checks to execute as implementation lands; it is not a passing
test report. Record the commit, toolchain, Cargo.lock, exact commands and results.
Use temporary state directories, injected clocks and local backends. Fixed
seeds/nonces belong only in test vectors, never production generation.

1. Check normalization variants, the 64-byte input bound, prefix-only restrictions,
   invalid ASCII/Unicode and lengths. Compare independent HKDF/public-key vectors
   and verify redacted debug output.
2. Round-trip the tuple plaintext, version/expiry header, nonce and authenticated
   ciphertext through TXT chunks, Pkarr and BEP 44 conversion. Check shuffled
   chunks, gaps, duplicates, bad sizes, modified header/tag/signature and a valid
   signature for the wrong queried key. Check exact expiry and the 3600-second
   age/300-second future-skew boundaries.
3. Fit route-rich IPv4/IPv6 and relay invitations. Check preservation of all
   relays, a usable direct-only route, explicit overflow and unchanged original
   fallback URL. Reopen the sealed private original and confirm all routes remain.
4. Load legacy state and normal hash-only invitations. Inspect serialized opted-in
   state for absence of code/plaintext token/URL. Exercise atomic-write failures,
   concurrent state writers and restart around sequence reservation. Check worker
   lazy initialization, bounded operations, retries, 240-second refresh, cleanup
   and shutdown without requiring public-interface changes.
5. Use an isolated Mainline local testnet with explicit bootstrap nodes and no
   public bootstrap, plus a local Pkarr gateway. Check binary GET/PUT framing,
   failed DHT operations, gateway alternatives, body limits, invalid signatures
   and total deadlines. Use explicit loopback HTTP or a locally trusted HTTPS
   certificate; verify nonloopback HTTP configuration is rejected.

The checkpoint includes automated codec, isolated DHT/gateway and host tests.
Run them from `rust/`:

```sh
cargo test -p kausway-rendezvous --locked
cargo test -p kausway-host --lib --test rendezvous_pairing --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The pairing integration test uses a recovered invitation with the existing Iroh
pairing and HTTP implementations, then reconnects without rendezvous. Transport
fixtures use explicit local DHT bootstrap nodes and loopback gateways. Record
actual commands and outcomes; zero matching tests are not acceptance evidence.
Normal URL-only host operation works with rendezvous unused. See the
[short-code guide](../docs/operations/short-code-pairing.md) for CLI, API and SDK
interfaces and the real-binary local gateway test.

## Deferred external-network acceptance

Public DHT bootstrap and independent retrieval, NAT/UDP-blocked conditions,
interoperability with explicitly selected HTTPS gateways, remote retention and
limits, and real-time refresh/restart behavior remain deferred. Record exact
artifacts, configured endpoints, network conditions and observations when those
checks are run. Local codec, mock and testnet success do not verify any public
endpoint or establish production availability. Retain the full URL fallback and
keep URL pairing independent of discovery.
