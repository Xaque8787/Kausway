//! Platform-independent client engine. No terminal signals or process-global configuration.
/// Client release identity, also used by CLI/SDK wrappers independently of the host package.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

use iroh::{Endpoint, EndpointAddr};
use kausway_rendezvous::{
    Code,
    backend::{Network, NetworkConfig},
    decode_packet,
};
use kausway_transport::{
    client::{ClientError, ClientSession},
    invite::InviteLink,
    net::{EndpointOptions, RelayChoice, bind_endpoint},
};
use serde::Serialize;
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{Mutex, OnceCell};
use tokio_util::sync::CancellationToken;

pub mod fsutil;
pub mod identity;
mod proxy;

#[derive(Clone)]
pub struct ClientConfig {
    pub data_dir: PathBuf,
    pub name: String,
    pub relay: RelayChoice,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid invitation")]
    InvalidInvitation,
    #[error("pairing cancelled")]
    Cancelled,
    #[error("short-code discovery failed: {0}; try the full invitation URL")]
    Rendezvous(kausway_rendezvous::Error),
    #[error("host rejected authorization ({0})")]
    Denied(String),
    #[error("no paired host")]
    NotPaired,
    #[error("client is shut down")]
    Closed,
    #[error("invalid configuration: {0}")]
    Configuration(String),
    #[error("storage failure: {0}")]
    Storage(String),
    #[error("transport failure: {0}")]
    Transport(String),
}
impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInvitation => "invalid_invitation",
            Self::Cancelled => "cancelled",
            Self::Rendezvous(error) => match error {
                kausway_rendezvous::Error::NotFound => "rendezvous_not_found",
                kausway_rendezvous::Error::Timeout => "rendezvous_timeout",
                kausway_rendezvous::Error::Expired => "rendezvous_expired",
                kausway_rendezvous::Error::InvalidRecord | kausway_rendezvous::Error::TooLarge => {
                    "rendezvous_invalid_record"
                }
                _ => "rendezvous_unavailable",
            },
            Self::Denied(_) => "authorization_denied",
            Self::NotPaired => "not_paired",
            Self::Closed => "closed",
            Self::Configuration(_) => "configuration",
            Self::Storage(_) => "storage",
            Self::Transport(_) => "transport",
        }
    }
}
impl From<ClientError> for Error {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::Denied(code) => Self::Denied(code),
            other => Self::Transport(other.to_string()),
        }
    }
}
fn storage(e: impl std::fmt::Display) -> Error {
    Error::Storage(e.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingStage {
    Resolving,
    Connecting,
}

struct PairingProgress<'a>(&'a std::sync::Mutex<Option<PairingStage>>);
impl Drop for PairingProgress<'_> {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProxyConnection {
    pub proxy_url: String,
}
#[derive(Debug)]
pub struct PairingResult {
    pub outcome: kausway_transport::protocol::AuthOutcome,
    pub host_id: String,
    pub transport_policy: String,
}
#[derive(Debug, Serialize)]
pub struct Status {
    pub state: String,
    pub authorization: String,
    pub path: String,
    pub rtt_ms: Option<f64>,
    pub transport_policy: Option<String>,
    pub policy_enforcement: Option<String>,
    pub proxy_url: Option<String>,
    pub last_error: Option<String>,
    pub pairing_stage: Option<PairingStage>,
}
struct Inner {
    proxy: Option<proxy::Proxy>,
    closed: bool,
    last_error: Option<String>,
    denied: bool,
    directory_lock: Option<fsutil::DataDirLock>,
}
/// One owner per data directory. Concurrent operations are serialized. Dropping this object
/// aborts its listener; call shutdown to close sessions and await task termination explicitly.
pub struct Client {
    config: ClientConfig,
    endpoint: Endpoint,
    inner: Mutex<Inner>,
    pair_gate: Mutex<()>,
    pair_progress: std::sync::Mutex<Option<PairingStage>>,
    pair_cancel: std::sync::Mutex<CancellationToken>,
    shutdown: CancellationToken,
    rendezvous_config: NetworkConfig,
    rendezvous: OnceCell<Network>,
}
impl Client {
    pub async fn new(config: ClientConfig) -> Result<Self, Error> {
        Self::new_with_rendezvous(config, NetworkConfig::default()).await
    }
    /// Overrides discovery only. Full URL pairing and saved-host reconnects never initialize it.
    pub async fn new_with_rendezvous(
        config: ClientConfig,
        rendezvous_config: NetworkConfig,
    ) -> Result<Self, Error> {
        rendezvous_config
            .validate()
            .map_err(|_| Error::Configuration("invalid rendezvous settings".into()))?;
        if config.name.is_empty() || config.name.len() > 128 {
            return Err(Error::Configuration(
                "device name must be 1..128 bytes".into(),
            ));
        }
        fsutil::ensure_private_dir(&config.data_dir).map_err(storage)?;
        let lock = fsutil::DataDirLock::acquire(&config.data_dir).map_err(storage)?;
        let key = identity::load_or_create(&config.data_dir.join("client.key")).map_err(storage)?;
        let endpoint = bind_endpoint(
            key,
            vec![],
            EndpointOptions {
                relay: Some(config.relay.clone()),
                relay_transport_only: false,
            },
        )
        .await
        .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self {
            config,
            endpoint,
            pair_gate: Mutex::new(()),
            pair_progress: std::sync::Mutex::new(None),
            pair_cancel: std::sync::Mutex::new(CancellationToken::new()),
            shutdown: CancellationToken::new(),
            rendezvous_config,
            rendezvous: OnceCell::new(),
            inner: Mutex::new(Inner {
                proxy: None,
                closed: false,
                last_error: None,
                denied: false,
                directory_lock: Some(lock),
            }),
        })
    }
    /// Accept a full invitation URL or K1 short code. URLs bypass discovery entirely.
    pub async fn pair(&self, input: &str) -> Result<PairingResult, Error> {
        self.pair_with_progress(input, |_| {}).await
    }

    pub fn pairing_stage(&self) -> Option<PairingStage> {
        *self.pair_progress.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn cancel_pairing(&self) {
        self.pair_cancel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .cancel();
    }

    pub async fn pair_with_progress(
        &self,
        input: &str,
        progress: impl Fn(PairingStage),
    ) -> Result<PairingResult, Error> {
        let _pair = tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => return Err(Error::Closed),
            guard = self.pair_gate.lock() => guard,
        };
        if self.inner.lock().await.closed {
            return Err(Error::Closed);
        }
        let cancel = self.shutdown.child_token();
        *self.pair_cancel.lock().unwrap_or_else(|p| p.into_inner()) = cancel.clone();
        let _progress = PairingProgress(&self.pair_progress);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => if self.shutdown.is_cancelled() { Err(Error::Closed) } else { Err(Error::Cancelled) },
            result = self.pair_input(input, &progress) => result,
        }
    }

    async fn pair_input(
        &self,
        input: &str,
        progress: &impl Fn(PairingStage),
    ) -> Result<PairingResult, Error> {
        let invite = if input
            .trim()
            .starts_with(kausway_transport::invite::LINK_PREFIX)
        {
            InviteLink::decode(input).map_err(|_| Error::InvalidInvitation)?
        } else {
            let code = Code::parse(input).map_err(|_| Error::InvalidInvitation)?;
            *self.pair_progress.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(PairingStage::Resolving);
            progress(PairingStage::Resolving);
            tokio::time::timeout(self.rendezvous_config.timeout, async {
                let network = self
                    .rendezvous
                    .get_or_try_init(|| Network::new_async(self.rendezvous_config.clone()))
                    .await
                    .map_err(Error::Rendezvous)?;
                let secrets = code.secrets();
                let key = secrets.public_key();
                network
                    .resolve_validated(&key, move |packet| {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|time| time.as_secs())
                            .unwrap_or(0);
                        decode_packet(&secrets, packet, now)
                    })
                    .await
                    .map_err(Error::Rendezvous)
            })
            .await
            .map_err(|_| Error::Rendezvous(kausway_rendezvous::Error::Timeout))??
        };
        let mut inner = self.inner.lock().await;
        if inner.closed {
            return Err(Error::Closed);
        }
        *self.pair_progress.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(PairingStage::Connecting);
        progress(PairingStage::Connecting);
        if let Some(mut proxy) = inner.proxy.take() {
            proxy.stop().await;
        }
        let session = tokio::time::timeout(
            Duration::from_secs(30),
            ClientSession::connect(
                &self.endpoint,
                invite.addr.clone(),
                &self.config.name,
                Some(&invite.token),
            ),
        )
        .await
        .map_err(|_| Error::Transport("pairing timed out".into()))??;
        let result = serde_json::to_vec_pretty(&invite.addr)
            .map_err(storage)
            .and_then(|bytes| {
                fsutil::write_private_atomic(&self.config.data_dir.join("host.json"), &bytes)
                    .map_err(storage)
            });
        let paired = PairingResult {
            outcome: session.outcome(),
            host_id: invite.addr.id.to_string(),
            transport_policy: session.transport_policy().as_str().into(),
        };
        session.close().await;
        result.map(|()| {
            inner.last_error = None;
            inner.denied = false;
            paired
        })
    }
    /// Obtain a separate session, primarily for CLI fetch/status. The caller closes it.
    pub async fn connect(&self) -> Result<ClientSession, Error> {
        let inner = self.inner.lock().await;
        if inner.closed {
            return Err(Error::Closed);
        }
        connect(&self.config, &self.endpoint).await
    }
    /// Loopback only. SDK callers use port zero and a capability-protected URL. CLI callers
    /// may opt out of URL protection to preserve their historical local URL behavior.
    pub async fn start_proxy(
        &self,
        listen: SocketAddr,
        protect_url: bool,
    ) -> Result<ProxyConnection, Error> {
        if !listen.ip().is_loopback() {
            return Err(Error::Configuration("proxy must bind loopback".into()));
        }
        let mut inner = self.inner.lock().await;
        if inner.closed {
            return Err(Error::Closed);
        }
        if let Some(proxy) = &inner.proxy {
            return Ok(proxy.connection.clone());
        }
        let proxy = proxy::Proxy::start(
            self.config.clone(),
            self.endpoint.clone(),
            listen,
            protect_url,
        )
        .await;
        let proxy = match proxy {
            Ok(proxy) => {
                inner.last_error = None;
                inner.denied = false;
                proxy
            }
            Err(error) => {
                inner.last_error = Some(error.to_string());
                inner.denied = matches!(error, Error::Denied(_));
                return Err(error);
            }
        };
        let connection = proxy.connection.clone();
        inner.proxy = Some(proxy);
        Ok(connection)
    }
    pub async fn stop_proxy(&self) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        if let Some(mut proxy) = inner.proxy.take() {
            proxy.stop().await;
        }
        Ok(())
    }
    pub async fn forget_pairing(&self) -> Result<(), Error> {
        self.cancel_pairing();
        let mut inner = self.inner.lock().await;
        if inner.closed {
            return Err(Error::Closed);
        }
        if let Some(mut proxy) = inner.proxy.take() {
            proxy.stop().await;
        }
        inner.last_error = None;
        inner.denied = false;
        match std::fs::remove_file(self.config.data_dir.join("host.json")) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(storage(e)),
        }
    }
    pub async fn notify_network_change(&self) -> Result<(), Error> {
        let inner = self.inner.lock().await;
        if inner.closed {
            return Err(Error::Closed);
        }
        self.endpoint.network_change().await;
        Ok(())
    }
    pub async fn status(&self) -> Status {
        let inner = self.inner.lock().await;
        let mut status = Status {
            state: if inner.closed { "closed" } else { "idle" }.into(),
            authorization: if inner.denied { "denied" } else { "unknown" }.into(),
            path: "unknown".into(),
            rtt_ms: None,
            transport_policy: None,
            policy_enforcement: None,
            proxy_url: None,
            last_error: inner.last_error.clone(),
            pairing_stage: self.pairing_stage(),
        };
        if let Some(proxy) = &inner.proxy {
            status.proxy_url = Some(proxy.connection.proxy_url.clone());
            let slot = proxy.state.session.lock().await;
            status.last_error = proxy.state.last_error.lock().await.clone();
            if let Some(session) = slot.as_ref() {
                let connected = session.connection().close_reason().is_none();
                status.state = if connected {
                    "connected"
                } else {
                    "disconnected"
                }
                .into();
                status.authorization = match session.connection().close_reason() {
                    None => "authorized",
                    Some(iroh::endpoint::ConnectionError::ApplicationClosed(close))
                        if close.error_code.into_inner()
                            == u64::from(kausway_transport::protocol::close_code::REVOKED) =>
                    {
                        "revoked"
                    }
                    Some(iroh::endpoint::ConnectionError::ApplicationClosed(close))
                        if close.error_code.into_inner()
                            == u64::from(
                                kausway_transport::protocol::close_code::NOT_AUTHORIZED,
                            ) =>
                    {
                        "denied"
                    }
                    _ => "unknown",
                }
                .into();
                let path = session.path();
                status.path = format!("{:?}", path.kind).to_lowercase();
                status.rtt_ms = path.rtt_ms;
                status.transport_policy = Some(session.transport_policy().as_str().into());
                status.policy_enforcement = Some(session.transport_policy().enforcement().into());
                if !connected {
                    status.last_error = session.connection().close_reason().map(|e| e.to_string());
                }
            }
        }
        status
    }
    pub async fn shutdown(&self) {
        self.shutdown.cancel();
        self.cancel_pairing();
        let mut inner = self.inner.lock().await;
        if let Some(mut proxy) = inner.proxy.take() {
            proxy.stop().await;
        }
        self.endpoint.close().await;
        inner.closed = true;
        inner.directory_lock.take();
    }
}
fn saved_host(config: &ClientConfig) -> Result<EndpointAddr, Error> {
    let bytes = std::fs::read(config.data_dir.join("host.json")).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::NotPaired
        } else {
            storage(e)
        }
    })?;
    serde_json::from_slice(&bytes).map_err(storage)
}
async fn connect(config: &ClientConfig, endpoint: &Endpoint) -> Result<ClientSession, Error> {
    tokio::time::timeout(
        Duration::from_secs(30),
        ClientSession::connect(endpoint, saved_host(config)?, &config.name, None),
    )
    .await
    .map_err(|_| Error::Transport("connection timed out".into()))?
    .map_err(Error::from)
}
