//! Untrusted pkarr transports. Invitation decryption and age policy live in `record`.
//!
//! Operations race all configured transports and return the first verified result.
//! They do not promise the globally newest DHT or gateway record.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use futures_util::{Stream, StreamExt};
use iroh::PublicKey;
use iroh_dns::pkarr::SignedPacket;
use mainline::{MutableItem, async_dht::AsyncDht};
use reqwest::{Client, StatusCode, Url};
use tokio::{
    sync::{Semaphore, oneshot},
    task::JoinSet,
    time::Instant,
};

use crate::Error;

const MAX_DNS: usize = 1000;
const MAX_RELAY: usize = 64 + 8 + MAX_DNS;

/// Explicit transport policy for rendezvous discovery.
#[derive(Clone, Debug)]
pub struct NetworkConfig {
    pub dht: bool,
    /// `None` uses mainline's public bootstrap nodes; `Some([])` uses none.
    pub bootstrap: Option<Vec<SocketAddr>>,
    /// Pkarr gateway base URLs; the z32 key is appended as a path segment.
    pub gateways: Vec<String>,
    /// One deadline for the entire publish or resolve, including body reads.
    pub timeout: Duration,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            dht: true,
            bootstrap: None,
            gateways: Vec::new(),
            timeout: Duration::from_secs(10),
        }
    }
}

impl NetworkConfig {
    /// Validate settings without creating sockets, clients or background tasks.
    pub fn validate(&self) -> Result<(), Error> {
        if self.gateways.len() > 8
            || self.timeout.is_zero()
            || self.timeout > Duration::from_secs(30)
            || self
                .bootstrap
                .as_ref()
                .is_some_and(|nodes| nodes.iter().any(|addr| !addr.is_ipv4() || addr.port() == 0))
        {
            return Err(Error::Configuration);
        }
        for gateway in &self.gateways {
            gateway_url(gateway)?;
        }
        Ok(())
    }
}

/// A reusable transport client. No operation logs keys, records, or raw errors.
#[derive(Clone)]
pub struct Network {
    dht: Option<AsyncDht>,
    http: Option<Client>,
    gateways: Vec<Url>,
    timeout: Duration,
    operations: Arc<Semaphore>,
    // Independent of caller lifetimes: owned permits live until actor queries complete.
    dht_queries: Arc<Semaphore>,
}

impl Network {
    /// Bound bootstrap DNS/client construction without tying up the runtime's blocking pool.
    /// Mainline's system DNS cannot be interrupted; at most 20 constructors remain outstanding.
    pub async fn new_async(config: NetworkConfig) -> Result<Self, Error> {
        static CONSTRUCTORS: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();
        config.validate()?;
        let timeout = config.timeout;
        construct(
            timeout,
            CONSTRUCTORS
                .get_or_init(|| Arc::new(Semaphore::new(20)))
                .clone(),
            move || Self::new(config),
        )
        .await
    }
    /// Construct synchronously. Does not wait for the DHT to bootstrap.
    pub fn new(config: NetworkConfig) -> Result<Self, Error> {
        if config.gateways.len() > 8
            || config.timeout > Duration::from_secs(30)
            || config.timeout.is_zero()
            || Instant::now().checked_add(config.timeout).is_none()
            || (!config.dht && config.gateways.is_empty())
        {
            return Err(Error::Configuration);
        }
        let gateways = config
            .gateways
            .iter()
            .map(|s| gateway_url(s))
            .collect::<Result<Vec<_>, _>>()?;
        let http = if gateways.is_empty() {
            None
        } else {
            let roots =
                rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|_| Error::Configuration)?
            .with_root_certificates(roots)
            .with_no_client_auth();
            Some(
                Client::builder()
                    .tls_backend_preconfigured(tls)
                    .redirect(reqwest::redirect::Policy::none())
                    .no_proxy()
                    .build()
                    .map_err(|_| Error::Configuration)?,
            )
        };
        let dht = if config.dht {
            let mut builder = mainline::Dht::builder();
            // Mainline 6 speaks IPv4 UDP only. Do not silently discard bootstrap entries.
            if let Some(bootstrap) = &config.bootstrap {
                if bootstrap
                    .iter()
                    .any(|addr| !addr.is_ipv4() || addr.port() == 0)
                {
                    return Err(Error::Configuration);
                }
                builder.bootstrap(bootstrap);
            }
            builder.request_timeout(config.timeout.min(mainline::DEFAULT_REQUEST_TIMEOUT));
            match builder.build() {
                Ok(dht) => Some(dht.as_async()),
                Err(_) if !gateways.is_empty() => None,
                Err(_) => return Err(Error::Network),
            }
        } else {
            None
        };
        Ok(Self {
            dht,
            http,
            gateways,
            timeout: config.timeout,
            operations: Arc::new(Semaphore::new(20)),
            dht_queries: Arc::new(Semaphore::new(20)),
        })
    }

    /// Succeeds as soon as any backend acknowledges the verified packet.
    pub async fn publish(&self, packet: &SignedPacket) -> Result<(), Error> {
        let deadline = Instant::now() + self.timeout;
        let _permit = tokio::time::timeout_at(deadline, self.operations.acquire())
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|_| Error::Network)?;
        // SignedPacket also has unchecked constructors: never trust the type alone.
        let packet = verify_packet(packet.as_bytes(), None)?;
        let mut pending = JoinSet::new();
        if let Some(dht) = self.dht.clone()
            && let Ok(permit) = self.dht_queries.clone().try_acquire_owned()
        {
            let item = to_mutable(&packet)?;
            let (sender, receiver) = oneshot::channel();
            // This owner is deliberately outside the caller's abort-on-drop JoinSet.
            // Mainline removes its pending query before the put future completes.
            tokio::spawn(async move {
                let _permit = permit;
                let result = dht
                    .put_mutable(item, None)
                    .await
                    .map(|_| ())
                    .map_err(|_| Error::Network);
                let _ = sender.send(result);
            });
            pending.spawn(async move { receiver.await.unwrap_or(Err(Error::Network)) });
        }
        for gateway in &self.gateways {
            let url = packet_url(gateway, &packet.public_key());
            let client = self.http.as_ref().expect("gateway HTTP client").clone();
            let payload = packet.to_relay_payload();
            pending.spawn(async move {
                let response = client
                    .put(url)
                    .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                    .body(payload)
                    .send()
                    .await
                    .map_err(http_error)?;
                if response.status().is_success() {
                    Ok(())
                } else {
                    Err(Error::Network)
                }
            });
        }
        first_success(pending, deadline, Error::Network).await
    }

    /// Return the first signature-verified, unsalted packet for exactly `key`.
    ///
    /// The record layer must still enforce invitation freshness and decrypt it.
    pub async fn resolve(&self, key: &PublicKey) -> Result<SignedPacket, Error> {
        self.resolve_validated(key, |packet| Ok(packet.clone()))
            .await
    }

    /// Validate each transport candidate before selecting a winner. A record-layer
    /// validator can decrypt and enforce freshness, returning the decoded invitation.
    pub async fn resolve_validated<T, F>(&self, key: &PublicKey, validate: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: Fn(&SignedPacket) -> Result<T, Error> + Send + Sync + 'static,
    {
        let deadline = Instant::now() + self.timeout;
        let _permit = tokio::time::timeout_at(deadline, self.operations.acquire())
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|_| Error::Network)?;
        let validate = Arc::new(validate);
        let mut pending = JoinSet::new();
        if let Some(dht) = self.dht.clone()
            && let Ok(permit) = self.dht_queries.clone().try_acquire_owned()
        {
            let key = *key;
            let validate = validate.clone();
            let (sender, receiver) = oneshot::channel();
            tokio::spawn(async move {
                let _permit = permit;
                let items = dht.get_mutable(key.as_bytes(), None, None);
                drain_dht(items, key, move |packet| validate(packet), sender).await;
                // Retain the client as well as the permit throughout traversal.
                drop(dht);
            });
            pending.spawn(async move { receiver.await.unwrap_or(Err(Error::Network)) });
        }
        for gateway in &self.gateways {
            let url = packet_url(gateway, key);
            let key = *key;
            let validate = validate.clone();
            let client = self.http.as_ref().expect("gateway HTTP client").clone();
            pending.spawn(async move {
                let mut response = client.get(url).send().await.map_err(http_error)?;
                if response.status() == StatusCode::NOT_FOUND {
                    return Err(Error::NotFound);
                }
                if !response.status().is_success() {
                    return Err(Error::Network);
                }
                if response
                    .content_length()
                    .is_some_and(|len| len > MAX_RELAY as u64)
                {
                    return Err(Error::TooLarge);
                }
                let mut bytes = Vec::with_capacity(32 + MAX_RELAY);
                bytes.extend_from_slice(key.as_bytes());
                while let Some(chunk) = response.chunk().await.map_err(http_error)? {
                    if chunk.len() > 32 + MAX_RELAY - bytes.len() {
                        return Err(Error::TooLarge);
                    }
                    bytes.extend_from_slice(&chunk);
                }
                validate(&verify_packet(&bytes, Some(&key))?)
            });
        }
        let failure = if pending.is_empty() {
            Error::Network
        } else {
            Error::NotFound
        };
        first_success(pending, deadline, failure).await
    }
}

async fn construct<T: Send + 'static>(
    timeout: Duration,
    slots: Arc<Semaphore>,
    build: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    let deadline = Instant::now() + timeout;
    let permit = tokio::time::timeout_at(deadline, slots.acquire_owned())
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Network)?;
    let (sender, receiver) = oneshot::channel();
    // A canceled waiter does not release admission until the blocking constructor finishes.
    std::thread::Builder::new()
        .name("kausway-discovery-init".into())
        .spawn(move || {
            let _permit = permit;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(build))
                .unwrap_or(Err(Error::Network));
            let _ = sender.send(result);
        })
        .map_err(|_| Error::Network)?;
    tokio::time::timeout_at(deadline, receiver)
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Network)?
}

/// Select the first fully usable item, then drain to EOF so the actor query remains
/// accounted for. Disconnected callers also leave the owner draining without decrypting.
/// At most 20 owners exist across Network clones; busy DHT admission never queues.
/// Owners may outlive the last Network until traversal ends. Mainline has no whole-query
/// deadline, so an indefinitely active traversal retains a slot rather than admitting more.
async fn drain_dht<S, T, F>(
    items: S,
    key: PublicKey,
    validate: F,
    sender: oneshot::Sender<Result<T, Error>>,
) where
    S: Stream<Item = MutableItem> + Unpin,
    F: Fn(&SignedPacket) -> Result<T, Error>,
{
    let mut items = items;
    let mut sender = Some(sender);
    let mut validate = Some(validate);
    let mut failure = Error::NotFound;
    while let Some(item) = items.next().await {
        if sender.as_ref().is_some_and(|sender| sender.is_closed()) {
            sender = None;
            validate = None;
        }
        let Some(validator) = validate.as_ref() else {
            continue;
        };
        let result = from_mutable(&item, &key).and_then(|packet| {
            // A caller-supplied validator must not unwind the owner and prematurely
            // release a slot while mainline's traversal is still active.
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| validator(&packet)))
                .map_err(|_| Error::Network)?
        });
        match result {
            Ok(value) => {
                let _ = sender
                    .take()
                    .expect("active DHT result sender")
                    .send(Ok(value));
                validate = None;
            }
            Err(error) => failure = preferred_error(failure, error),
        }
    }
    if let Some(sender) = sender {
        let _ = sender.send(Err(failure));
    }
}

fn gateway_url(value: &str) -> Result<Url, Error> {
    // Require an explicit scheme and reject even empty userinfo before URL normalization.
    let authority = value.split_once("://").ok_or(Error::Configuration)?;
    if !matches!(authority.0, "https" | "http")
        || authority
            .1
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .contains('@')
        || value.contains('\\')
        || value.trim() != value
    {
        return Err(Error::Configuration);
    }
    let url = Url::parse(value).map_err(|_| Error::Configuration)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
        || (url.scheme() == "http"
            && !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")))
    {
        return Err(Error::Configuration);
    }
    Ok(url)
}

fn packet_url(base: &Url, key: &PublicKey) -> Url {
    let mut url = base.clone();
    // Validated HTTP(S) URLs always support path segments.
    url.path_segments_mut()
        .expect("HTTP base URL")
        .pop_if_empty()
        .push(&key.to_z32());
    url
}

fn verify_packet(bytes: &[u8], key: Option<&PublicKey>) -> Result<SignedPacket, Error> {
    if bytes.len() > 32 + MAX_RELAY {
        return Err(Error::TooLarge);
    }
    let packet = SignedPacket::from_bytes(bytes).map_err(|_| Error::InvalidRecord)?;
    if packet.timestamp().as_micros() > i64::MAX as u64
        || key.is_some_and(|key| packet.public_key() != *key)
    {
        return Err(Error::InvalidRecord);
    }
    Ok(packet)
}

fn to_mutable(packet: &SignedPacket) -> Result<MutableItem, Error> {
    let packet = verify_packet(packet.as_bytes(), None)?;
    let seq = i64::try_from(packet.timestamp().as_micros()).map_err(|_| Error::InvalidRecord)?;
    // Only reached after independent signature verification, despite mainline's constructor name.
    Ok(MutableItem::new_signed_unchecked(
        *packet.public_key().as_bytes(),
        packet.signature().to_bytes(),
        packet.encoded_packet(),
        seq,
        None,
    ))
}

fn from_mutable(item: &MutableItem, key: &PublicKey) -> Result<SignedPacket, Error> {
    if item.seq() < 0
        || item.key() != key.as_bytes()
        || item.salt().is_some()
        || item.target() != &MutableItem::target_from_key(key.as_bytes(), None)
    {
        return Err(Error::InvalidRecord);
    }
    if item.value().len() > MAX_DNS {
        return Err(Error::TooLarge);
    }
    let mut bytes = Vec::with_capacity(104 + item.value().len());
    bytes.extend_from_slice(item.key());
    bytes.extend_from_slice(item.signature());
    bytes.extend_from_slice(&(item.seq() as u64).to_be_bytes());
    bytes.extend_from_slice(item.value());
    verify_packet(&bytes, Some(key))
}

fn http_error(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::Timeout
    } else {
        Error::Network
    }
}

fn preferred_error(previous: Error, next: Error) -> Error {
    let rank = |error| match error {
        Error::TooLarge => 6,
        Error::InvalidRecord => 5,
        Error::Expired => 4,
        Error::Timeout => 3,
        Error::Network => 2,
        _ => 1,
    };
    if rank(next) > rank(previous) {
        next
    } else {
        previous
    }
}

async fn first_success<T: Send + 'static>(
    mut pending: JoinSet<Result<T, Error>>,
    deadline: Instant,
    mut failure: Error,
) -> Result<T, Error> {
    while !pending.is_empty() {
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        match tokio::time::timeout_at(deadline, pending.join_next()).await {
            Err(_) => return Err(Error::Timeout),
            Ok(Some(Ok(Ok(value)))) => {
                return if Instant::now() >= deadline {
                    Err(Error::Timeout)
                } else {
                    Ok(value)
                };
            }
            Ok(Some(Ok(Err(error)))) => {
                // Deterministic precedence: malformed data > network errors > absence.
                failure = preferred_error(failure, error);
            }
            Ok(Some(Err(_))) => failure = preferred_error(failure, Error::Network),
            Ok(None) => break,
        }
    }
    // JoinSet aborts HTTP tasks and DHT result waiters. Separately admitted DHT owners
    // keep their permits and drain mainline until its pending actor queries complete.
    Err(failure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream, UdpSocket},
        sync::{Mutex, mpsc},
    };

    #[derive(Clone)]
    enum Reply {
        Store(Arc<Mutex<Option<Vec<u8>>>>),
        Fixed(u16, Vec<u8>, bool),
        Block(bool), // true sends headers, then stalls the body
    }

    struct Gateway {
        url: String,
        task: tokio::task::JoinHandle<()>,
        seen: mpsc::UnboundedReceiver<(String, String, Vec<u8>)>,
        closed: mpsc::UnboundedReceiver<()>,
    }

    impl Drop for Gateway {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl Gateway {
        async fn new(reply: Reply) -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let url = format!("http://{}/pkarr", listener.local_addr().unwrap());
            let (seen_tx, seen) = mpsc::unbounded_channel();
            let (closed_tx, closed) = mpsc::unbounded_channel();
            let task = tokio::spawn(async move {
                let mut handlers = JoinSet::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let (socket, _) = accepted.unwrap();
                            let (reply, seen, closed) = (reply.clone(), seen_tx.clone(), closed_tx.clone());
                            handlers.spawn(async move { serve(socket, reply, seen, closed).await; });
                        }
                        _ = handlers.join_next(), if !handlers.is_empty() => {}
                    }
                }
            });
            Self {
                url,
                task,
                seen,
                closed,
            }
        }

        fn config(&self) -> NetworkConfig {
            NetworkConfig {
                dht: false,
                gateways: vec![self.url.clone()],
                ..Default::default()
            }
        }

        async fn request(&mut self) -> (String, String, Vec<u8>) {
            tokio::time::timeout(Duration::from_secs(3), self.seen.recv())
                .await
                .unwrap()
                .unwrap()
        }

        async fn disconnected(&mut self) {
            tokio::time::timeout(Duration::from_secs(3), self.closed.recv())
                .await
                .unwrap()
                .unwrap();
        }
    }

    async fn serve(
        mut socket: TcpStream,
        reply: Reply,
        seen: mpsc::UnboundedSender<(String, String, Vec<u8>)>,
        closed: mpsc::UnboundedSender<()>,
    ) {
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            if request.len() > 8192 || socket.read_exact(&mut byte).await.is_err() {
                return;
            }
            request.push(byte[0]);
        }
        let header = String::from_utf8(request).unwrap();
        let mut first = header.lines().next().unwrap().split_whitespace();
        let method = first.next().unwrap().to_owned();
        let path = first.next().unwrap().to_owned();
        let len = header
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
            .unwrap_or(0);
        assert!(len <= MAX_RELAY);
        let mut body = vec![0; len];
        if socket.read_exact(&mut body).await.is_err() {
            return;
        }
        let _ = seen.send((method.clone(), path, body.clone()));
        let (status, body, chunked) = match reply {
            Reply::Store(store) => {
                if method == "PUT" {
                    *store.lock().await = Some(body);
                    (204, Vec::new(), false)
                } else {
                    match store.lock().await.clone() {
                        Some(body) => (200, body, false),
                        None => (404, Vec::new(), false),
                    }
                }
            }
            Reply::Fixed(status, body, chunked) => (status, body, chunked),
            Reply::Block(headers) => {
                if headers
                    && socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n")
                        .await
                        .is_err()
                {
                    return;
                }
                // Detect actual client cancellation, not just timeout of its public future.
                while socket.read(&mut byte).await.is_ok_and(|n| n != 0) {}
                let _ = closed.send(());
                return;
            }
        };
        let wire = if chunked {
            let mut wire = format!("HTTP/1.1 {status} OK\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n", body.len()).into_bytes();
            wire.extend_from_slice(&body);
            wire.extend_from_slice(b"\r\n0\r\n\r\n");
            wire
        } else {
            let mut wire = format!("HTTP/1.1 {status} OK\r\nConnection: close\r\nContent-Length: {}\r\nLocation: /redirected\r\n\r\n", body.len()).into_bytes();
            wire.extend_from_slice(&body);
            wire
        };
        let _ = socket.write_all(&wire).await;
    }

    fn packet() -> SignedPacket {
        SignedPacket::from_txt_strings(
            &iroh::SecretKey::from_bytes(&[7; 32]),
            "_test",
            ["local"],
            300,
        )
        .unwrap()
    }

    fn resign(packet: &SignedPacket, seq: u64, dns: &[u8]) -> Vec<u8> {
        let mut signable = format!("3:seqi{seq}e1:v{}:", dns.len()).into_bytes();
        signable.extend_from_slice(dns);
        let mut bytes = packet.public_key().as_bytes().to_vec();
        bytes.extend_from_slice(
            &iroh::SecretKey::from_bytes(&[7; 32])
                .sign(&signable)
                .to_bytes(),
        );
        bytes.extend_from_slice(&seq.to_be_bytes());
        bytes.extend_from_slice(dns);
        bytes
    }

    #[tokio::test]
    async fn slow_constructor_has_deadline_retains_slot_and_never_uses_runtime_blocking_pool() {
        let slots = Arc::new(Semaphore::new(1));
        let (release, blocked) = std::sync::mpsc::channel();
        let (started, ready) = oneshot::channel();
        let constructor = {
            let slots = slots.clone();
            tokio::spawn(async move {
                construct(Duration::from_millis(100), slots, move || {
                    let _ = started.send(());
                    let _ = blocked.recv();
                    Ok(42)
                })
                .await
            })
        };
        tokio::time::timeout(Duration::from_secs(1), ready)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(constructor.await.unwrap(), Err(Error::Timeout));
        assert_eq!(slots.available_permits(), 0);
        assert_eq!(
            construct(Duration::from_millis(10), slots.clone(), || Ok(1)).await,
            Err(Error::Timeout)
        );
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while slots.available_permits() == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            construct(Duration::from_secs(1), slots, || Ok(7))
                .await
                .unwrap(),
            7
        );
    }

    #[test]
    fn strict_configuration_and_gateway_paths() {
        let config = NetworkConfig::default();
        assert!(config.dht && config.bootstrap.is_none() && config.gateways.is_empty());
        for value in [
            "example.com",
            "ftp://example.com",
            "http://example.com",
            "http://127.0.0.2",
            "https://u:p@example.com",
            "https://@example.com",
            "https://example.com?",
            "https://example.com#",
            " https://example.com",
            "https://example.com\\x",
        ] {
            assert!(
                matches!(gateway_url(value), Err(Error::Configuration)),
                "{value}"
            );
        }
        for value in [
            "https://example.com/pkarr",
            "http://127.0.0.1:8080/pkarr/",
            "http://localhost",
            "http://[::1]:8080",
        ] {
            let base = gateway_url(value).unwrap();
            assert!(
                packet_url(&base, &packet().public_key())
                    .path()
                    .ends_with(&packet().public_key().to_z32())
            );
        }
        assert!(matches!(
            Network::new(NetworkConfig {
                dht: false,
                ..config.clone()
            }),
            Err(Error::Configuration)
        ));
        assert!(matches!(
            Network::new(NetworkConfig {
                timeout: Duration::ZERO,
                ..config.clone()
            }),
            Err(Error::Configuration)
        ));
        assert!(matches!(
            Network::new(NetworkConfig {
                timeout: Duration::from_secs(31),
                ..config.clone()
            }),
            Err(Error::Configuration)
        ));
        assert!(matches!(
            Network::new(NetworkConfig {
                gateways: vec!["https://example.com".into(); 9],
                ..config
            }),
            Err(Error::Configuration)
        ));
    }

    #[test]
    fn reverify_dht_and_unchecked_packets() {
        let packet = packet();
        let key = packet.public_key();
        let item = to_mutable(&packet).unwrap();
        assert_eq!(from_mutable(&item, &key).unwrap(), packet);
        let wrong = iroh::SecretKey::from_bytes(&[8; 32]).public();
        assert!(matches!(
            from_mutable(&item, &wrong),
            Err(Error::InvalidRecord)
        ));
        for (seq, salt, signature, dns) in [
            (-1, None, *item.signature(), item.value().to_vec()),
            (
                item.seq(),
                Some(&b"salt"[..]),
                *item.signature(),
                item.value().to_vec(),
            ),
            (item.seq(), None, [0; 64], item.value().to_vec()),
            (item.seq(), None, *item.signature(), vec![0; 1001]),
        ] {
            let item =
                MutableItem::new_signed_unchecked(*key.as_bytes(), signature, &dns, seq, salt);
            assert!(from_mutable(&item, &key).is_err());
        }
        let mut bad = packet.as_bytes().to_vec();
        bad[32] ^= 1;
        let unchecked = SignedPacket::from_bytes_unchecked(&bad).unwrap();
        assert!(matches!(to_mutable(&unchecked), Err(Error::InvalidRecord)));
        let overflow = resign(&packet, i64::MAX as u64 + 1, packet.encoded_packet());
        assert!(matches!(
            verify_packet(&overflow, Some(&key)),
            Err(Error::InvalidRecord)
        ));
        let malformed = resign(&packet, 1, b"not DNS");
        assert!(matches!(
            verify_packet(&malformed, Some(&key)),
            Err(Error::InvalidRecord)
        ));
    }

    #[tokio::test]
    async fn independent_http_publisher_and_resolver_without_udp() {
        let mut gateway = Gateway::new(Reply::Store(Arc::new(Mutex::new(None)))).await;
        let publisher = Network::new(gateway.config()).unwrap();
        let resolver = Network::new(gateway.config()).unwrap();
        assert!(publisher.dht.is_none() && resolver.dht.is_none());
        let packet = packet();
        publisher.publish(&packet).await.unwrap();
        let (method, path, body) = gateway.request().await;
        assert_eq!(method, "PUT");
        assert_eq!(path, format!("/pkarr/{}", packet.public_key().to_z32()));
        assert_eq!(body, packet.to_relay_payload());
        assert_eq!(
            resolver.resolve(&packet.public_key()).await.unwrap(),
            packet
        );
        assert_eq!(gateway.request().await.0, "GET");
    }

    #[tokio::test]
    async fn independent_dht_publisher_and_resolver_on_loopback_testnet() {
        let testnet = mainline::Testnet::builder(3)
            .bind_address(Ipv4Addr::LOCALHOST)
            .build()
            .unwrap();
        let config = NetworkConfig {
            bootstrap: Some(
                testnet
                    .bootstrap
                    .iter()
                    .map(|s| s.parse().unwrap())
                    .collect(),
            ),
            timeout: Duration::from_secs(10),
            ..Default::default()
        };
        let publisher = Network::new(config.clone()).unwrap();
        let packet = packet();
        publisher.publish(&packet).await.unwrap();
        drop(publisher); // the resolver must retrieve remote storage, not publisher memory
        let resolver = Network::new(config).unwrap();
        assert_eq!(
            resolver.resolve(&packet.public_key()).await.unwrap(),
            packet
        );
    }

    #[tokio::test]
    async fn blocked_udp_does_not_delay_gateway_fallback() {
        let blackhole = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let gateway = Gateway::new(Reply::Store(Arc::new(Mutex::new(None)))).await;
        let mut config = gateway.config();
        config.dht = true;
        config.bootstrap = Some(vec![blackhole.local_addr().unwrap()]);
        config.timeout = Duration::from_secs(2);
        let network = Network::new(config).unwrap();
        let packet = packet();
        tokio::time::timeout(Duration::from_secs(1), async {
            network.publish(&packet).await.unwrap();
            assert_eq!(network.resolve(&packet.public_key()).await.unwrap(), packet);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn http_rejects_malformed_oversized_wrong_key_and_redirects() {
        let packet = packet();
        let mut bad_sig = packet.to_relay_payload();
        bad_sig[0] ^= 1;
        let wrong = SignedPacket::from_txt_strings(
            &iroh::SecretKey::from_bytes(&[8; 32]),
            "_test",
            ["wrong"],
            300,
        )
        .unwrap();
        let cases = [
            (200, vec![], false, Error::InvalidRecord),
            (200, vec![0; 71], true, Error::InvalidRecord),
            (200, bad_sig, false, Error::InvalidRecord),
            (200, wrong.to_relay_payload(), true, Error::InvalidRecord),
            (
                200,
                resign(&packet, 1, b"not DNS")[32..].to_vec(),
                false,
                Error::InvalidRecord,
            ),
            (200, vec![0; MAX_RELAY + 1], false, Error::TooLarge),
            (200, vec![0; MAX_RELAY + 1], true, Error::TooLarge),
            (404, vec![], false, Error::NotFound),
            (302, packet.to_relay_payload(), false, Error::Network),
        ];
        for (status, body, chunked, expected) in cases {
            let mut gateway = Gateway::new(Reply::Fixed(status, body, chunked)).await;
            let network = Network::new(gateway.config()).unwrap();
            assert!(
                matches!(network.resolve(&packet.public_key()).await, Err(error) if error == expected)
            );
            gateway.request().await;
            assert!(gateway.seen.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn shared_deadline_includes_headers_body_and_operation_queue() {
        for body in [false, true] {
            let mut gateway = Gateway::new(Reply::Block(body)).await;
            let mut config = gateway.config();
            config.timeout = Duration::from_millis(200);
            let network = Network::new(config).unwrap();
            let key = packet().public_key();
            let start = Instant::now();
            assert!(matches!(network.resolve(&key).await, Err(Error::Timeout)));
            assert!(start.elapsed() < Duration::from_secs(2));
            gateway.disconnected().await;
            let _all = network.operations.acquire_many(20).await.unwrap();
            assert!(matches!(network.resolve(&key).await, Err(Error::Timeout)));
        }
    }

    #[tokio::test]
    async fn dropping_publish_or_resolve_cancels_gateway_work() {
        for publish in [false, true] {
            let mut gateway = Gateway::new(Reply::Block(false)).await;
            let network = Network::new(gateway.config()).unwrap();
            let packet = packet();
            let key = packet.public_key();
            let mut operation: std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<(), Error>>>,
            > = if publish {
                Box::pin(network.publish(&packet))
            } else {
                Box::pin(async { network.resolve(&key).await.map(|_| ()) })
            };
            tokio::select! {
                _ = &mut operation => panic!("blocked operation completed"),
                _ = gateway.request() => {}
            }
            drop(operation);
            gateway.disconnected().await;
            assert_eq!(network.operations.available_permits(), 20);
        }
    }

    #[tokio::test]
    async fn validators_and_one_failed_gateway_do_not_hide_other_success() {
        let packet = packet();
        let old = SignedPacket::from_bytes(&resign(&packet, 1, packet.encoded_packet())).unwrap();
        let stale = Gateway::new(Reply::Fixed(200, old.to_relay_payload(), false)).await;
        let invalid = Gateway::new(Reply::Fixed(200, vec![0; 72], true)).await;
        let healthy = Gateway::new(Reply::Fixed(200, packet.to_relay_payload(), true)).await;
        let mut blocked = Gateway::new(Reply::Block(false)).await;
        let mut config = stale.config();
        config.gateways.extend([
            invalid.url.clone(),
            healthy.url.clone(),
            blocked.url.clone(),
        ]);
        let network = Network::new(config).unwrap();
        let expected = packet.timestamp();
        let resolved = network
            .resolve_validated(&packet.public_key(), move |candidate| {
                if candidate.timestamp() != expected {
                    return Err(Error::Expired);
                }
                Ok(candidate.clone())
            })
            .await
            .unwrap();
        assert_eq!(resolved, packet);
        // A stalled sibling is aborted if it had started its request.
        if blocked.seen.try_recv().is_ok() {
            blocked.disconnected().await;
        }
        let failing = Gateway::new(Reply::Fixed(500, vec![], false)).await;
        let stored = Gateway::new(Reply::Store(Arc::new(Mutex::new(None)))).await;
        let mut config = failing.config();
        config.gateways.push(stored.url.clone());
        Network::new(config)
            .unwrap()
            .publish(&packet)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn dht_stream_filters_foreign_high_sequence_and_drains_after_first_usable() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let packet = packet();
        let key = packet.public_key();
        let foreign = MutableItem::new(
            mainline::SigningKey::from_bytes(&[8; 32]),
            packet.encoded_packet(),
            i64::MAX,
            None,
        );
        let malformed = MutableItem::new(
            mainline::SigningKey::from_bytes(&[7; 32]),
            b"not DNS",
            i64::MAX,
            None,
        );
        let stale = MutableItem::new(
            mainline::SigningKey::from_bytes(&[7; 32]),
            packet.encoded_packet(),
            1,
            None,
        );
        let expected = packet.timestamp();
        let calls = Arc::new(AtomicUsize::new(0));
        let validator_calls = calls.clone();
        let (items_tx, mut items_rx) = mpsc::unbounded_channel();
        let items = futures_util::stream::poll_fn(move |cx| items_rx.poll_recv(cx));
        let (result_tx, result_rx) = oneshot::channel();
        let slots = Arc::new(Semaphore::new(1));
        let permit = slots.clone().try_acquire_owned().unwrap();
        let owner = tokio::spawn(async move {
            let _permit = permit;
            drain_dht(
                items,
                key,
                move |candidate| {
                    validator_calls.fetch_add(1, Ordering::SeqCst);
                    if candidate.timestamp() != expected {
                        return Err(Error::Expired);
                    }
                    Ok(candidate.clone())
                },
                result_tx,
            )
            .await;
        });
        for item in [foreign, malformed, stale, to_mutable(&packet).unwrap()] {
            items_tx.send(item).unwrap();
        }
        // The stream remains open: a valid result must not wait for traversal EOF.
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), result_rx)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            packet
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(slots.available_permits(), 0);
        assert!(!owner.is_finished());
        items_tx.send(to_mutable(&packet).unwrap()).unwrap();
        drop(items_tx);
        owner.await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(slots.available_permits(), 1);
    }

    #[tokio::test]
    async fn disconnected_dht_result_still_drains_without_running_validator() {
        let packet = packet();
        let key = packet.public_key();
        let (items_tx, mut items_rx) = mpsc::unbounded_channel();
        let items = futures_util::stream::poll_fn(move |cx| items_rx.poll_recv(cx));
        let (result_tx, result_rx) = oneshot::channel::<Result<(), Error>>();
        drop(result_rx);
        let owner = tokio::spawn(drain_dht(
            items,
            key,
            |_| panic!("cancelled caller must not validate"),
            result_tx,
        ));
        items_tx.send(to_mutable(&packet).unwrap()).unwrap();
        drop(items_tx);
        owner.await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_calls_and_http_winners_cannot_exceed_twenty_dht_owners() {
        let blackhole = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let gateway = Gateway::new(Reply::Store(Arc::new(Mutex::new(None)))).await;
        let mut network = Network::new(gateway.config()).unwrap();
        // Keep admitted actor queries pending for the duration of this test.
        network.dht = Some(
            mainline::Dht::builder()
                .bootstrap(&[blackhole.local_addr().unwrap()])
                .request_timeout(Duration::from_secs(30))
                .build()
                .unwrap()
                .as_async(),
        );
        let mut dht_only = network.clone();
        dht_only.gateways.clear();
        dht_only.http = None;
        for index in 0..10 {
            let key = iroh::SecretKey::from_bytes(&[index + 20; 32]).public();
            let mut call = Box::pin(dht_only.resolve(&key));
            tokio::select! {
                biased;
                _ = &mut call => panic!("blackhole DHT unexpectedly completed"),
                _ = tokio::task::yield_now() => {}
            }
            drop(call);
            assert_eq!(
                network.dht_queries.available_permits(),
                19 - usize::from(index)
            );
        }
        for index in 0..10 {
            let packet = SignedPacket::from_txt_strings(
                &iroh::SecretKey::from_bytes(&[index + 40; 32]),
                "_test",
                ["local"],
                300,
            )
            .unwrap();
            network.publish(&packet).await.unwrap();
            assert_eq!(
                network.dht_queries.available_permits(),
                9 - usize::from(index)
            );
        }
        let packet = packet();
        // Full actor slots are skipped immediately; HTTP remains usable without queuing.
        tokio::time::timeout(Duration::from_secs(2), async {
            assert!(matches!(
                dht_only.resolve(&packet.public_key()).await,
                Err(Error::Network)
            ));
            for _ in 0..25 {
                network.publish(&packet).await.unwrap();
            }
            assert_eq!(network.resolve(&packet.public_key()).await.unwrap(), packet);
        })
        .await
        .unwrap();
        assert_eq!(network.dht_queries.available_permits(), 0);
        assert_eq!(network.operations.available_permits(), 20);
        assert_eq!(dht_only.operations.available_permits(), 20);
    }
}
