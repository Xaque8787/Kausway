//! Authorized devices, invitations, and policy, persisted as one JSON file in the data directory.
//!
//! Every mutation is written to disk (atomically, 0600) before it becomes visible in memory.
//! Mutations hold an exclusive lock on `state.lock` and start from the file on disk, so the running
//! host and admin commands (invite, revoke) can change state at the same time without losing writes.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iroh::{EndpointAddr, PublicKey};
use kausway_rendezvous::{Code, SealedInvite, Secrets, SignedPacket, fit_packet};
use kausway_transport::invite::InviteLink;
use kausway_transport::pairing::{
    self, DEFAULT_INVITATION_TTL_SECS, FailureLimiter, Invitation, RedeemError,
};
use kausway_transport::policy::TransportPolicy;
use kausway_transport::websocket::{OriginHeader, WebSocketSettings};
use serde::{Deserialize, Serialize};

use crate::fsutil::{restrict_permissions, write_private_atomic};

pub const STATE_FILE: &str = "state.json";
const STATE_LOCK_FILE: &str = "state.lock";
pub const STATE_VERSION: u32 = 2;
const MAX_RENDEZVOUS_INVITATIONS: usize = 20;
const REFRESH_SECS: u64 = 240;
pub const MIN_INVITATION_TTL_SECS: u64 = 60;
pub const MAX_INVITATION_TTL_SECS: u64 = 24 * 60 * 60;
const MAX_LABEL_CHARS: usize = 80;
const MAX_DEVICE_NAME_CHARS: usize = 64;
/// Spent or expired invitations are kept this long for the admin UI, then pruned.
const INVITATION_RETENTION_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// The device's Iroh endpoint id. This is the credential checked on every connection.
    pub endpoint_id: String,
    pub name: String,
    pub paired_at: u64,
    pub invitation_id: String,
    #[serde(default)]
    pub revoked_at: Option<u64>,
}

impl Device {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateFile {
    pub version: u32,
    #[serde(default)]
    pub policy: TransportPolicy,
    #[serde(default)]
    pub websocket: WebSocketSettings,
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(default)]
    pub invitations: Vec<Invitation>,
    #[serde(default)]
    pub pairing_failures: FailureLimiter,
}

impl Default for StateFile {
    fn default() -> Self {
        Self {
            version: 1,
            policy: TransportPolicy::default(),
            websocket: WebSocketSettings::default(),
            devices: Vec::new(),
            invitations: Vec::new(),
            pairing_failures: FailureLimiter::default(),
        }
    }
}

// Keep recovery material out of StateFile, snapshots, and every public control response.
#[derive(Clone, Default, Serialize, Deserialize)]
struct StoredState {
    #[serde(flatten)]
    state: StateFile,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    rendezvous: BTreeMap<String, Pending>,
}

impl Deref for StoredState {
    type Target = StateFile;
    fn deref(&self) -> &StateFile {
        &self.state
    }
}

impl DerefMut for StoredState {
    fn deref_mut(&mut self) -> &mut StateFile {
        &mut self.state
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Pending {
    secrets: Secrets,
    original: SealedInvite,
    host_id: PublicKey,
    sequence: u64,
    // Retrying a sequence must reuse exactly the same signed bytes, including the nonce.
    packet: Option<Vec<u8>>,
    address: Option<EndpointAddr>,
    next_attempt_at: u64,
    failures: u32,
    published_at: Option<u64>,
    #[serde(default)]
    last_error: Option<String>,
}

/// Publication metadata only. Never contains a code, token, key or encrypted recovery copy.
#[derive(Debug, Clone, Serialize)]
pub struct RendezvousStatus {
    pub state: &'static str,
    pub published_at: Option<u64>,
    pub next_attempt_at: u64,
    pub error: Option<String>,
}

impl Pending {
    pub(crate) fn secrets(&self) -> &Secrets {
        &self.secrets
    }
    pub(crate) fn original(&self) -> &SealedInvite {
        &self.original
    }
    pub(crate) fn sequence(&self) -> u64 {
        self.sequence
    }
    #[cfg(test)]
    pub(crate) fn next_attempt_at(&self) -> u64 {
        self.next_attempt_at
    }
    #[cfg(test)]
    pub(crate) fn failures(&self) -> u32 {
        self.failures
    }
}

pub(crate) struct Publication {
    pub(crate) id: String,
    pub(crate) sequence: u64,
    pub(crate) packet: SignedPacket,
    pub(crate) address: EndpointAddr,
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("could not access {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path} is not valid host state: {source}")]
    Corrupt {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{path} has state version {found}; this build supports up to {max}", max = STATE_VERSION)]
    UnsupportedVersion { path: PathBuf, found: u32 },
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairOutcome {
    AlreadyAuthorized,
    Paired,
}

pub struct HostState {
    path: PathBuf,
    inner: Mutex<StoredState>,
}

impl HostState {
    /// Opens or creates the state file. `configured_policy`, when set, replaces the stored policy.
    pub fn open(
        path: &Path,
        configured_policy: Option<TransportPolicy>,
    ) -> Result<Self, StateError> {
        let _lock = lock_state(path)?;
        let mut file = load(path)?;
        if let Some(policy) = configured_policy {
            file.policy = policy;
        }
        prune_invitations(&mut file, now_secs());
        persist(path, &file)?;
        Ok(Self {
            path: path.to_path_buf(),
            inner: Mutex::new(file),
        })
    }

    /// Picks up changes made by other processes. Readers never see a partial file.
    pub fn reload(&self) -> Result<(), StateError> {
        let _file_lock = lock_state_bounded(&self.path)?;
        let mut guard = self.lock();
        *guard = load(&self.path)?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn snapshot(&self) -> StateFile {
        self.lock().state.clone()
    }

    pub fn policy(&self) -> TransportPolicy {
        self.lock().policy
    }

    pub fn set_policy(&self, policy: TransportPolicy) -> Result<(), StateError> {
        self.update(|file| file.policy = policy)
    }

    pub fn websocket(&self) -> WebSocketSettings {
        self.lock().websocket
    }

    /// Changes the stored WebSocket settings; `None` keeps the current value.
    pub fn set_websocket(
        &self,
        enabled: Option<bool>,
        origin_header: Option<OriginHeader>,
    ) -> Result<WebSocketSettings, StateError> {
        self.update(|file| {
            if let Some(enabled) = enabled {
                file.websocket.enabled = enabled;
            }
            if let Some(origin_header) = origin_header {
                file.websocket.origin_header = origin_header;
            }
            file.websocket
        })
    }

    /// Apply validated live configuration together in one state persistence operation.
    pub fn set_live_configuration(
        &self,
        policy: Option<TransportPolicy>,
        enabled: Option<bool>,
        origin: Option<OriginHeader>,
    ) -> Result<(), StateError> {
        self.update(|file| {
            if let Some(policy) = policy {
                file.policy = policy;
            }
            if let Some(enabled) = enabled {
                file.websocket.enabled = enabled;
            }
            if let Some(origin) = origin {
                file.websocket.origin_header = origin;
            }
        })
    }

    pub fn is_authorized(&self, endpoint_id: &str) -> bool {
        self.lock()
            .devices
            .iter()
            .any(|device| device.endpoint_id == endpoint_id && device.is_active())
    }

    /// Creates an invitation and returns it with the plaintext token, which is never stored.
    pub fn create_invitation(
        &self,
        label: &str,
        ttl_secs: Option<u64>,
        now: u64,
    ) -> Result<(Invitation, String), StateError> {
        let (invitation, token) = new_invitation(label, ttl_secs, now)?;
        let stored = invitation.clone();
        self.update(move |file| file.invitations.push(stored))?;
        Ok((invitation, token))
    }

    /// Creates both invitation and private recovery state in one atomic transaction.
    /// The returned token can always build a normal URL, even when DHT fitting fails.
    pub fn create_rendezvous_invitation(
        &self,
        label: &str,
        ttl_secs: Option<u64>,
        now: u64,
        addr: EndpointAddr,
    ) -> Result<(Invitation, String, Code), StateError> {
        let (invitation, token) = new_invitation(label, ttl_secs, now)?;
        let code = Code::generate();
        let secrets = code.secrets();
        let original = SealedInvite::seal(
            &secrets,
            &InviteLink::new(addr.clone(), token.clone()),
            invitation.expires_at,
        )
        .map_err(|error| StateError::Invalid(error.to_string()))?;
        self.update(|file| -> Result<(), StateError> {
            prune_invitations(file, now);
            if file.rendezvous.len() >= MAX_RENDEZVOUS_INVITATIONS {
                return Err(StateError::Invalid(
                    "at most 20 active rendezvous invitations are allowed".into(),
                ));
            }
            file.version = STATE_VERSION;
            file.rendezvous.insert(
                invitation.id.clone(),
                Pending {
                    secrets,
                    original,
                    host_id: addr.id,
                    sequence: 0,
                    packet: None,
                    address: None,
                    next_attempt_at: now,
                    failures: 0,
                    published_at: None,
                    last_error: None,
                },
            );
            file.invitations.push(invitation.clone());
            Ok(())
        })??;
        Ok((invitation, token, code))
    }

    pub(crate) fn pending_rendezvous(&self) -> BTreeMap<String, Pending> {
        self.lock().rendezvous.clone()
    }

    pub fn rendezvous_status(&self, id: &str, now: u64) -> Option<RendezvousStatus> {
        let file = self.lock();
        if !file
            .invitations
            .iter()
            .any(|invite| invite.id == id && !invite.is_redeemed() && now < invite.expires_at)
        {
            return None;
        }
        file.rendezvous.get(id).map(|pending| RendezvousStatus {
            state: if pending.failures > 0 {
                "retrying"
            } else if pending.published_at.is_some() {
                "published"
            } else {
                "pending"
            },
            published_at: pending.published_at,
            next_attempt_at: pending.next_attempt_at,
            error: pending.last_error.clone(),
        })
    }

    /// Durably reserve sequences and bytes before handing work to the network worker.
    pub(crate) fn rendezvous_publications(
        &self,
        addr: &EndpointAddr,
        now: u64,
    ) -> Result<Vec<Publication>, StateError> {
        self.update_bounded(|file| {
            prune_invitations(file, now);
            let invitations = &file.state.invitations;
            let mut work = Vec::new();
            file.rendezvous.retain(|id, pending| {
                let Some(invitation) = invitations.iter().find(|invite| invite.id == *id) else {
                    return false;
                };
                let Ok(mut invite) = pending.original().open(pending.secrets(), now) else {
                    return false;
                };
                if pending.host_id != addr.id
                    || invite.addr.id != addr.id
                    || pending.original.expires_at().ok() != Some(invitation.expires_at)
                    || !invitation.matches(&invite.token)
                {
                    return false;
                }
                // Never lower the durable sequence after wall-clock rollback. Wait until the
                // resolver's clock window accepts it, including when the address changed.
                if sequence_is_future(pending.sequence(), now) {
                    pending.next_attempt_at = pending
                        .sequence()
                        .saturating_sub(kausway_rendezvous::CLOCK_SKEW_SECS * 1_000_000)
                        .div_ceil(1_000_000);
                    return true;
                }
                let changed = pending.address.as_ref() != Some(addr);
                if !changed && now < pending.next_attempt_at {
                    return true;
                }
                let stale = now
                    .saturating_mul(1_000_000)
                    .saturating_sub(pending.sequence())
                    > kausway_rendezvous::MAX_RECORD_AGE_SECS * 1_000_000;
                if changed || stale || pending.packet.is_none() || pending.published_at.is_some() {
                    let sequence = now
                        .saturating_mul(1_000_000)
                        .max(pending.sequence().saturating_add(1));
                    invite.addr = addr.clone();
                    match fit_packet(
                        pending.secrets(),
                        &invite,
                        invitation.expires_at,
                        sequence,
                        now,
                    ) {
                        Ok(packet) => {
                            pending.sequence = sequence;
                            pending.packet = Some(packet.as_bytes().to_vec());
                            pending.address = Some(addr.clone());
                            pending.published_at = None;
                            pending.last_error = None;
                            pending.failures = 0;
                        }
                        Err(error) => {
                            pending.last_error = Some(error.code().into());
                            pending.address = Some(addr.clone());
                            pending.packet = None;
                            pending.failures = pending.failures.saturating_add(1);
                            pending.next_attempt_at =
                                now.saturating_add(retry_secs(pending.failures));
                            return true;
                        }
                    }
                }
                if let Some(packet) = pending
                    .packet
                    .as_ref()
                    .and_then(|bytes| SignedPacket::from_bytes(bytes).ok())
                {
                    work.push(Publication {
                        id: id.clone(),
                        sequence: pending.sequence(),
                        packet,
                        address: addr.clone(),
                    });
                } else {
                    pending.packet = None;
                }
                true
            });
            work
        })
    }

    /// Reload and check immediately before publishing, and while an operation is in flight.
    pub(crate) fn rendezvous_is_current(
        &self,
        id: &str,
        sequence: u64,
        now: u64,
    ) -> Result<bool, StateError> {
        let _file_lock = lock_state_bounded(&self.path)?;
        let mut file = self.lock();
        *file = load(&self.path)?;
        Ok(sequence_is_current(sequence, now)
            && file
                .rendezvous
                .get(id)
                .is_some_and(|pending| pending.sequence() == sequence)
            && file
                .invitations
                .iter()
                .any(|invite| invite.id == id && !invite.is_redeemed() && now < invite.expires_at))
    }

    /// A late acknowledgement must never recreate canceled or superseded work.
    pub(crate) fn rendezvous_result(
        &self,
        id: &str,
        sequence: u64,
        now: u64,
        success: bool,
    ) -> Result<(), StateError> {
        self.update_bounded(|file| {
            prune_invitations(file, now);
            if let Some(pending) = file.rendezvous.get_mut(id).filter(|pending| {
                pending.sequence() == sequence && sequence_is_current(sequence, now)
            }) {
                if success {
                    pending.published_at = Some(now);
                    pending.last_error = None;
                    pending.failures = 0;
                    pending.next_attempt_at = now.saturating_add(REFRESH_SECS);
                } else {
                    pending.last_error = Some("publication_failed".into());
                    pending.failures = pending.failures.saturating_add(1);
                    pending.next_attempt_at = now.saturating_add(retry_secs(pending.failures));
                }
            }
        })
    }

    /// Authorizes `endpoint_id`, redeeming `token` if the device is not already authorized.
    ///
    /// Failed attempts are persisted so the rate limit survives restarts.
    pub fn pair(
        &self,
        endpoint_id: &str,
        device_name: &str,
        token: Option<&str>,
        now: u64,
    ) -> Result<Result<PairOutcome, RedeemError>, StateError> {
        let name = sanitize_device_name(device_name);
        self.update(|file| -> Result<PairOutcome, RedeemError> {
            prune_invitations(file, now);
            if file
                .devices
                .iter()
                .any(|device| device.endpoint_id == endpoint_id && device.is_active())
            {
                return Ok(PairOutcome::AlreadyAuthorized);
            }
            let token = token.ok_or(RedeemError::Invalid)?;
            let StateFile {
                invitations,
                pairing_failures,
                devices,
                ..
            } = &mut file.state;
            let index = pairing::redeem(invitations, pairing_failures, token, endpoint_id, now)?;
            let invitation_id = invitations[index].id.clone();
            devices.retain(|device| device.endpoint_id != endpoint_id);
            devices.push(Device {
                endpoint_id: endpoint_id.to_owned(),
                name,
                paired_at: now,
                invitation_id: invitation_id.clone(),
                revoked_at: None,
            });
            file.rendezvous.remove(&invitation_id);
            Ok(PairOutcome::Paired)
        })
    }

    /// Revokes a device. Returns false when no active device has that id.
    pub fn revoke_device(&self, endpoint_id: &str, now: u64) -> Result<bool, StateError> {
        self.update(|file| {
            match file
                .devices
                .iter_mut()
                .find(|device| device.endpoint_id == endpoint_id && device.is_active())
            {
                Some(device) => {
                    device.revoked_at = Some(now);
                    true
                }
                None => false,
            }
        })
    }

    /// Cancels an unredeemed invitation. Returns false when none has that id.
    pub fn cancel_invitation(&self, id: &str) -> Result<bool, StateError> {
        self.update(|file| {
            let before = file.invitations.len();
            file.invitations
                .retain(|invitation| invitation.id != id || invitation.is_redeemed());
            let removed = file.invitations.len() != before;
            if removed {
                file.rendezvous.remove(id);
            }
            removed
        })
    }

    /// Applies `change` to the current file, persists it, then publishes it. On write failure nothing
    /// changes.
    fn update<T>(&self, change: impl FnOnce(&mut StoredState) -> T) -> Result<T, StateError> {
        self.update_locked(change, lock_state(&self.path)?)
    }

    fn update_bounded<T>(
        &self,
        change: impl FnOnce(&mut StoredState) -> T,
    ) -> Result<T, StateError> {
        self.update_locked(change, lock_state_bounded(&self.path)?)
    }

    fn update_locked<T>(
        &self,
        change: impl FnOnce(&mut StoredState) -> T,
        _file_lock: File,
    ) -> Result<T, StateError> {
        let mut guard = self.lock();
        let mut next = load(&self.path)?;
        let before = serde_json::to_vec(&next).expect("state is serializable");
        let result = change(&mut next);
        prune_invitations(&mut next, now_secs());
        if serde_json::to_vec(&next).expect("state is serializable") != before {
            persist(&self.path, &next)?;
        }
        *guard = next;
        Ok(result)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StoredState> {
        // A panic while holding the lock cannot leave a half-applied change: `update` only
        // publishes fully persisted copies, so the inner value is still consistent.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn prune_invitations(file: &mut StoredState, now: u64) {
    let invitations = &file.state.invitations;
    file.rendezvous.retain(|id, _| {
        invitations
            .iter()
            .any(|invite| invite.id == *id && !invite.is_redeemed() && now < invite.expires_at)
    });
    file.invitations
        .retain(|inv| now < inv.expires_at.saturating_add(INVITATION_RETENTION_SECS));
}

fn new_invitation(
    label: &str,
    ttl_secs: Option<u64>,
    now: u64,
) -> Result<(Invitation, String), StateError> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > MAX_LABEL_CHARS {
        return Err(StateError::Invalid(format!(
            "label must be 1 to {MAX_LABEL_CHARS} characters"
        )));
    }
    let ttl = ttl_secs.unwrap_or(DEFAULT_INVITATION_TTL_SECS);
    if !(MIN_INVITATION_TTL_SECS..=MAX_INVITATION_TTL_SECS).contains(&ttl) {
        return Err(StateError::Invalid(format!(
            "ttl_secs must be between {MIN_INVITATION_TTL_SECS} and {MAX_INVITATION_TTL_SECS}"
        )));
    }
    let token = pairing::generate_token();
    let id = format!("inv-{}", &pairing::hash_token(&token)[..12]);
    Ok((
        Invitation::new(id, label.to_owned(), &token, now, ttl),
        token,
    ))
}

fn sequence_is_future(sequence: u64, now: u64) -> bool {
    sequence
        > now
            .saturating_mul(1_000_000)
            .saturating_add(kausway_rendezvous::CLOCK_SKEW_SECS * 1_000_000)
}

fn sequence_is_current(sequence: u64, now: u64) -> bool {
    sequence <= i64::MAX as u64
        && !sequence_is_future(sequence, now)
        && now.saturating_mul(1_000_000).saturating_sub(sequence)
            <= kausway_rendezvous::MAX_RECORD_AGE_SECS * 1_000_000
}

fn retry_secs(failures: u32) -> u64 {
    // Bounded exponential backoff with jitter; generated only when recording a failure.
    (2u64.saturating_pow(failures.min(6))).min(60) + rand::random_range(0..=3)
}

fn sanitize_device_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DEVICE_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "Unnamed device".into()
    } else {
        cleaned.to_owned()
    }
}

fn load(path: &Path) -> Result<StoredState, StateError> {
    match fs::read(path) {
        Ok(bytes) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if fs::metadata(path)
                    .map_err(|source| io_error(path, source))?
                    .permissions()
                    .mode()
                    & 0o777
                    != 0o600
                {
                    restrict_permissions(path).map_err(|source| io_error(path, source))?;
                }
            }
            #[cfg(not(unix))]
            restrict_permissions(path).map_err(|source| io_error(path, source))?;
            let mut file: StoredState =
                serde_json::from_slice(&bytes).map_err(|source| StateError::Corrupt {
                    path: path.to_path_buf(),
                    source,
                })?;
            if file.version > STATE_VERSION {
                return Err(StateError::UnsupportedVersion {
                    path: path.to_path_buf(),
                    found: file.version,
                });
            }
            if !file.rendezvous.is_empty() {
                file.version = STATE_VERSION;
            }
            Ok(file)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(StoredState::default()),
        Err(source) => Err(io_error(path, source)),
    }
}

/// Blocks until this process holds the state write lock. Released when the file is dropped.
fn lock_state(state_path: &Path) -> Result<File, StateError> {
    let file = open_lock_file(state_path)?;
    file.lock()
        .map_err(|source| io_error(&state_path.with_file_name(STATE_LOCK_FILE), source))?;
    Ok(file)
}

fn lock_state_bounded(state_path: &Path) -> Result<File, StateError> {
    let file = open_lock_file(state_path)?;
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(io_error(
                    &state_path.with_file_name(STATE_LOCK_FILE),
                    io::Error::new(io::ErrorKind::TimedOut, "rendezvous state lock timed out"),
                ));
            }
            Err(std::fs::TryLockError::Error(source)) => {
                return Err(io_error(
                    &state_path.with_file_name(STATE_LOCK_FILE),
                    source,
                ));
            }
        }
    }
}

fn open_lock_file(state_path: &Path) -> Result<File, StateError> {
    let lock_path = state_path.with_file_name(STATE_LOCK_FILE);
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|source| io_error(&lock_path, source))
}

fn persist(path: &Path, file: &StoredState) -> Result<(), StateError> {
    let bytes = serde_json::to_vec_pretty(file).expect("state is always serializable");
    write_private_atomic(path, &bytes).map_err(|source| io_error(path, source))
}

fn io_error(path: &Path, source: io::Error) -> StateError {
    StateError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::test_support::TempDir;

    fn open(dir: &TempDir) -> HostState {
        HostState::open(&dir.0.join(STATE_FILE), None).unwrap()
    }

    fn address() -> EndpointAddr {
        EndpointAddr::new(iroh::SecretKey::from_bytes(&[7; 32]).public())
            .with_ip_addr("192.0.2.10:4321".parse().unwrap())
    }

    #[test]
    fn normal_state_keeps_version_one_and_worker_reads_do_not_write() {
        let dir = TempDir::create();
        let state = open(&dir);
        let now = now_secs();
        state.create_invitation("Normal URL", None, now).unwrap();
        let before = fs::read(state.path()).unwrap();
        let modified = fs::metadata(state.path()).unwrap().modified().unwrap();
        assert_eq!(state.snapshot().version, 1);
        assert!(!String::from_utf8_lossy(&before).contains("rendezvous"));
        for _ in 0..3 {
            assert!(
                state
                    .rendezvous_publications(&address(), now)
                    .unwrap()
                    .is_empty()
            );
            assert!(!state.rendezvous_is_current("missing", 1, now).unwrap());
        }
        assert_eq!(fs::read(state.path()).unwrap(), before);
        assert_eq!(
            fs::metadata(state.path()).unwrap().modified().unwrap(),
            modified
        );
        assert_eq!(open(&dir).snapshot().version, 1);
    }

    #[test]
    fn rendezvous_restarts_retry_identical_bytes_and_never_extend_expiry() {
        let dir = TempDir::create();
        let state = open(&dir);
        let now = now_secs();
        let addr = address();
        let (invitation, token, code) = state
            .create_rendezvous_invitation("Phone", None, now, addr.clone())
            .unwrap();
        let original = InviteLink::new(addr.clone(), token);
        let work = state
            .rendezvous_publications(&addr, now)
            .unwrap()
            .pop()
            .unwrap();
        let first = work.packet.as_bytes().to_vec();
        let sequence = work.sequence;
        let pending = state.pending_rendezvous();
        assert_eq!(pending[&invitation.id].sequence(), sequence);
        assert_eq!(
            pending[&invitation.id]
                .original()
                .open(&code.secrets(), now)
                .unwrap(),
            original
        );
        assert_eq!(
            kausway_rendezvous::decode_packet(&code.secrets(), &work.packet, now).unwrap(),
            original
        );
        state
            .rendezvous_result(&invitation.id, sequence, now, false)
            .unwrap();
        drop(state);
        let state = open(&dir);
        let retry = state
            .rendezvous_publications(&addr, now + 10)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(retry.sequence, sequence);
        assert_eq!(retry.packet.as_bytes(), first);
        state
            .rendezvous_result(&invitation.id, sequence, now + 10, true)
            .unwrap();
        assert!(
            state
                .rendezvous_publications(&addr, now + 249)
                .unwrap()
                .is_empty()
        );
        let refresh = state
            .rendezvous_publications(&addr, now + 250)
            .unwrap()
            .pop()
            .unwrap();
        assert!(refresh.sequence > sequence);
        assert_eq!(
            state.snapshot().invitations[0].expires_at,
            invitation.expires_at
        );
        assert_eq!(
            state.pending_rendezvous()[&invitation.id]
                .original()
                .expires_at()
                .unwrap(),
            invitation.expires_at
        );
        assert_eq!(
            kausway_rendezvous::decode_packet(&code.secrets(), &refresh.packet, now + 250).unwrap(),
            original
        );
    }

    #[test]
    fn rollback_defers_future_sequence_without_decreasing_or_acknowledging_it() {
        let dir = TempDir::create();
        let state = open(&dir);
        let now = now_secs();
        let (invite, token, code) = state
            .create_rendezvous_invitation("Phone", None, now, address())
            .unwrap();
        let work = state
            .rendezvous_publications(&address(), now)
            .unwrap()
            .pop()
            .unwrap();
        state
            .rendezvous_result(&invite.id, work.sequence, now, false)
            .unwrap();
        let rollback = now - kausway_rendezvous::CLOCK_SKEW_SECS - 1;
        assert!(
            !state
                .rendezvous_is_current(&invite.id, work.sequence, rollback)
                .unwrap()
        );
        assert!(
            state
                .rendezvous_publications(&address(), rollback)
                .unwrap()
                .is_empty()
        );
        state
            .rendezvous_result(&invite.id, work.sequence, rollback, true)
            .unwrap();
        let pending = state.pending_rendezvous();
        assert_eq!(pending[&invite.id].sequence(), work.sequence);
        assert!(pending[&invite.id].published_at.is_none());
        assert_eq!(
            pending[&invite.id]
                .original()
                .open(&code.secrets(), rollback)
                .unwrap()
                .token,
            token
        );
        let recovered = state
            .rendezvous_publications(&address(), now)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(recovered.sequence, work.sequence);
        assert_eq!(recovered.packet.as_bytes(), work.packet.as_bytes());
    }

    #[test]
    fn rendezvous_state_and_snapshots_keep_plaintext_secrets_out() {
        let dir = TempDir::create();
        let state = open(&dir);
        let now = now_secs();
        let (invite, token, code) = state
            .create_rendezvous_invitation("Phone", None, now, address())
            .unwrap();
        state.rendezvous_publications(&address(), now).unwrap();
        let disk = fs::read_to_string(state.path()).unwrap();
        let snapshot = serde_json::to_string(&state.snapshot()).unwrap();
        let debug = format!(
            "{:?} {:?} {:?}",
            state.snapshot(),
            state.pending_rendezvous(),
            code
        );
        for text in [&disk, &snapshot, &debug] {
            assert!(!text.contains(&token));
            assert!(!text.contains(&code.expose()));
            assert!(!text.contains(&code.expose().replace('-', "")));
        }
        assert!(disk.contains("signing_seed")); // Derived keys are private recovery material.
        for text in [&snapshot, &debug] {
            assert!(!text.contains("signing_seed"));
            assert!(!text.contains("encryption_key"));
        }
        assert!(!snapshot.contains("rendezvous"));
        assert!(disk.contains(&invite.token_hash));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(state.path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn code_and_url_share_single_use_and_late_results_do_not_resurrect() {
        for redeem_url_first in [false, true] {
            let dir = TempDir::create();
            let state = open(&dir);
            let now = now_secs();
            let (invite, token, code) = state
                .create_rendezvous_invitation("Phone", None, now, address())
                .unwrap();
            let work = state
                .rendezvous_publications(&address(), now)
                .unwrap()
                .pop()
                .unwrap();
            let url = InviteLink::new(address(), token).encode();
            let normal = InviteLink::decode(&url).unwrap();
            let resolved =
                kausway_rendezvous::decode_packet(&code.secrets(), &work.packet, now).unwrap();
            let (first, second) = if redeem_url_first {
                (normal, resolved)
            } else {
                (resolved, normal)
            };
            assert_eq!(
                state.pair("phone-a", "A", Some(&first.token), now).unwrap(),
                Ok(PairOutcome::Paired)
            );
            assert!(state.pending_rendezvous().is_empty());
            assert_eq!(
                state
                    .pair("phone-b", "B", Some(&second.token), now)
                    .unwrap(),
                Err(RedeemError::AlreadyUsed)
            );
            state
                .rendezvous_result(&invite.id, work.sequence, now, true)
                .unwrap();
            assert!(open(&dir).pending_rendezvous().is_empty());
            assert!(
                state
                    .rendezvous_publications(&address(), now + 300)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn rendezvous_limit_cancel_expiry_and_version_protection_are_atomic() {
        let dir = TempDir::create();
        let state = open(&dir);
        let now = now_secs();
        for _ in 0..MAX_RENDEZVOUS_INVITATIONS {
            state
                .create_rendezvous_invitation("Phone", Some(60), now, address())
                .unwrap();
        }
        assert!(
            state
                .create_rendezvous_invitation("Excess", None, now, address())
                .is_err()
        );
        assert_eq!(state.snapshot().invitations.len(), 20);
        let first = state.snapshot().invitations[0].id.clone();
        assert!(state.cancel_invitation(&first).unwrap());
        state.rendezvous_result(&first, 0, now, true).unwrap();
        assert!(!state.pending_rendezvous().contains_key(&first));
        state
            .create_rendezvous_invitation("Replacement", Some(60), now, address())
            .unwrap();
        assert_eq!(state.pending_rendezvous().len(), 20);
        assert!(
            state
                .rendezvous_publications(&address(), now + 60)
                .unwrap()
                .is_empty()
        );
        assert!(state.pending_rendezvous().is_empty());
        assert_eq!(open(&dir).snapshot().version, 2);
        state
            .create_invitation("Normal again", None, now + 60)
            .unwrap();
        assert_eq!(state.snapshot().version, 2);
        // Capacity is released by expiry even if the maintenance worker has not run.
        state
            .create_rendezvous_invitation("After expiry", None, now + 60, address())
            .unwrap();
        assert_eq!(state.pending_rendezvous().len(), 1);
    }

    #[test]
    fn address_updates_keep_code_token_identity_original_and_expiry() {
        let dir = TempDir::create();
        let state = open(&dir);
        let now = now_secs();
        let original_addr = address();
        let (invite, token, code) = state
            .create_rendezvous_invitation("Phone", None, now, original_addr.clone())
            .unwrap();
        let first = state
            .rendezvous_publications(&original_addr, now)
            .unwrap()
            .pop()
            .unwrap();
        let mut next_addr = original_addr.clone();
        next_addr.addrs.clear();
        next_addr = next_addr.with_ip_addr("192.0.2.20:5432".parse().unwrap());
        let next = state
            .rendezvous_publications(&next_addr, now)
            .unwrap()
            .pop()
            .unwrap();
        assert!(next.sequence > first.sequence);
        // A stale successful publication cannot acknowledge the newer address.
        state
            .rendezvous_result(&invite.id, first.sequence, now, true)
            .unwrap();
        assert!(
            state.pending_rendezvous()[&invite.id]
                .published_at
                .is_none()
        );
        let recovered =
            kausway_rendezvous::decode_packet(&code.secrets(), &next.packet, now).unwrap();
        assert_eq!(recovered.addr, next_addr);
        assert_eq!(recovered.token, token);
        let pending = state.pending_rendezvous();
        assert_eq!(
            pending[&invite.id]
                .original()
                .open(&code.secrets(), now)
                .unwrap()
                .addr,
            original_addr
        );
        assert_eq!(
            pending[&invite.id].original().expires_at().unwrap(),
            invite.expires_at
        );
        let different_host = EndpointAddr::new(iroh::SecretKey::from_bytes(&[8; 32]).public());
        assert!(
            state
                .rendezvous_publications(&different_host, now)
                .unwrap()
                .is_empty()
        );
        assert!(state.pending_rendezvous().is_empty());
        assert_eq!(state.snapshot().invitations.len(), 1);
    }

    #[test]
    fn fitting_failure_preserves_normal_url_and_obeys_backoff() {
        let dir = TempDir::create();
        let state = open(&dir);
        let now = now_secs();
        let mut addr = address();
        addr.addrs.insert(iroh::TransportAddr::Relay(
            format!("https://relay.example.com/{}", "a".repeat(1500))
                .parse()
                .unwrap(),
        ));
        let (invite, token, code) = state
            .create_rendezvous_invitation("Large address", None, now, addr.clone())
            .unwrap();
        let url = InviteLink::new(addr.clone(), token.clone()).encode();
        assert_eq!(InviteLink::decode(&url).unwrap().token, token);
        assert!(
            state
                .rendezvous_publications(&addr, now)
                .unwrap()
                .is_empty()
        );
        let pending = state.pending_rendezvous();
        let pending = &pending[&invite.id];
        assert_eq!(pending.failures, 1);
        assert!(pending.next_attempt_at > now);
        assert_eq!(
            pending
                .original()
                .open(&code.secrets(), now)
                .unwrap()
                .encode(),
            url
        );
        assert!(
            state
                .rendezvous_publications(&addr, now)
                .unwrap()
                .is_empty()
        );
        assert_eq!(state.pending_rendezvous()[&invite.id].failures, 1);
        assert_eq!(
            state.pair("phone", "Phone", Some(&token), now).unwrap(),
            Ok(PairOutcome::Paired)
        );
        assert!(state.pending_rendezvous().is_empty());
    }

    #[test]
    fn rendezvous_uses_normal_validation_without_leaving_half_an_invitation() {
        let dir = TempDir::create();
        let state = open(&dir);
        let now = now_secs();
        for (label, ttl) in [
            ("", None),
            ("  ", None),
            ("Phone", Some(1)),
            ("Phone", Some(MAX_INVITATION_TTL_SECS + 1)),
        ] {
            assert!(
                state
                    .create_rendezvous_invitation(label, ttl, now, address())
                    .is_err()
            );
        }
        assert!(state.snapshot().invitations.is_empty());
        assert!(state.pending_rendezvous().is_empty());
        assert_eq!(state.snapshot().version, 1);
    }

    #[test]
    fn pairing_persists_across_restarts() {
        let dir = TempDir::create();
        let now = now_secs();
        let state = open(&dir);
        let (_, token) = state
            .create_invitation("Living room TV", None, now)
            .unwrap();
        assert_eq!(
            state.pair("endpoint-a", "TV", Some(&token), now).unwrap(),
            Ok(PairOutcome::Paired)
        );
        drop(state);

        let state = open(&dir);
        assert!(state.is_authorized("endpoint-a"));
        assert_eq!(
            state.pair("endpoint-a", "TV", None, now).unwrap(),
            Ok(PairOutcome::AlreadyAuthorized)
        );
    }

    #[test]
    fn cancelled_invitations_cannot_be_redeemed() {
        let dir = TempDir::create();
        let now = now_secs();
        let state = open(&dir);
        let (invitation, token) = state.create_invitation("Phone", None, now).unwrap();
        assert!(state.cancel_invitation(&invitation.id).unwrap());
        assert!(!state.cancel_invitation(&invitation.id).unwrap());
        assert_eq!(
            state
                .pair("endpoint-a", "Phone", Some(&token), now)
                .unwrap(),
            Err(RedeemError::Invalid)
        );
    }

    #[test]
    fn plaintext_tokens_are_never_written() {
        let dir = TempDir::create();
        let state = open(&dir);
        let (_, token) = state.create_invitation("Phone", None, now_secs()).unwrap();
        let on_disk = fs::read_to_string(state.path()).unwrap();
        assert!(!on_disk.contains(&token));
        assert!(on_disk.contains(&pairing::hash_token(&token)));
    }

    #[test]
    fn invitations_are_single_use_across_devices() {
        let dir = TempDir::create();
        let now = now_secs();
        let state = open(&dir);
        let (_, token) = state.create_invitation("Phone", None, now).unwrap();
        assert!(
            state
                .pair("endpoint-a", "A", Some(&token), now)
                .unwrap()
                .is_ok()
        );
        assert_eq!(
            state.pair("endpoint-b", "B", Some(&token), now).unwrap(),
            Err(RedeemError::AlreadyUsed)
        );
        assert!(!state.is_authorized("endpoint-b"));
    }

    #[test]
    fn unknown_device_without_token_is_denied() {
        let dir = TempDir::create();
        let state = open(&dir);
        assert_eq!(
            state.pair("stranger", "X", None, now_secs()).unwrap(),
            Err(RedeemError::Invalid)
        );
    }

    #[test]
    fn revoked_devices_lose_access_and_can_repair_with_new_invitation() {
        let dir = TempDir::create();
        let now = now_secs();
        let state = open(&dir);
        let (_, first) = state.create_invitation("TV", None, now).unwrap();
        state
            .pair("endpoint-a", "TV", Some(&first), now)
            .unwrap()
            .unwrap();
        assert!(state.revoke_device("endpoint-a", now).unwrap());
        assert!(!state.is_authorized("endpoint-a"));
        assert!(!state.revoke_device("endpoint-a", now).unwrap());

        let (_, second) = state.create_invitation("TV again", None, now).unwrap();
        assert!(
            state
                .pair("endpoint-a", "TV", Some(&second), now)
                .unwrap()
                .is_ok()
        );
        assert!(state.is_authorized("endpoint-a"));
        assert_eq!(state.snapshot().devices.len(), 1);
    }

    #[test]
    fn rate_limit_survives_restart() {
        let dir = TempDir::create();
        let now = now_secs();
        let state = open(&dir);
        let (_, token) = state.create_invitation("TV", None, now).unwrap();
        for _ in 0..pairing::MAX_FAILURES_PER_WINDOW {
            let _ = state.pair("attacker", "X", Some("wrong"), now).unwrap();
        }
        drop(state);
        let state = open(&dir);
        assert_eq!(
            state.pair("endpoint-a", "TV", Some(&token), now).unwrap(),
            Err(RedeemError::RateLimited)
        );
    }

    #[test]
    fn validates_invitation_parameters() {
        let dir = TempDir::create();
        let state = open(&dir);
        let now = now_secs();
        assert!(state.create_invitation("  ", None, now).is_err());
        assert!(state.create_invitation(&"x".repeat(81), None, now).is_err());
        assert!(state.create_invitation("TV", Some(10), now).is_err());
        assert!(
            state
                .create_invitation("TV", Some(MAX_INVITATION_TTL_SECS + 1), now)
                .is_err()
        );
    }

    #[test]
    fn configured_policy_overrides_stored_policy() {
        let dir = TempDir::create();
        let path = dir.0.join(STATE_FILE);
        HostState::open(&path, None)
            .unwrap()
            .set_policy(TransportPolicy::RelayOnly)
            .unwrap();
        assert_eq!(
            HostState::open(&path, None).unwrap().policy(),
            TransportPolicy::RelayOnly
        );
        assert_eq!(
            HostState::open(&path, Some(TransportPolicy::P2pOnly))
                .unwrap()
                .policy(),
            TransportPolicy::P2pOnly
        );
    }

    #[test]
    fn websocket_settings_default_on_and_persist() {
        let dir = TempDir::create();
        let path = dir.0.join(STATE_FILE);
        let state = HostState::open(&path, None).unwrap();
        assert_eq!(state.websocket(), WebSocketSettings::default());
        let changed = state
            .set_websocket(Some(false), Some(OriginHeader::PassThrough))
            .unwrap();
        assert!(!changed.enabled);
        let kept = state.set_websocket(None, None).unwrap();
        assert_eq!(kept, changed);
        assert_eq!(HostState::open(&path, None).unwrap().websocket(), changed);
    }

    #[test]
    fn rejects_corrupt_and_future_state() {
        let dir = TempDir::create();
        let path = dir.0.join(STATE_FILE);
        fs::write(&path, b"{not json").unwrap();
        assert!(matches!(
            HostState::open(&path, None),
            Err(StateError::Corrupt { .. })
        ));
        fs::write(&path, br#"{"version": 99}"#).unwrap();
        assert!(matches!(
            HostState::open(&path, None),
            Err(StateError::UnsupportedVersion { found: 99, .. })
        ));
    }

    #[test]
    fn separate_handles_see_each_others_changes() {
        let dir = TempDir::create();
        let now = now_secs();
        let host = open(&dir);
        let admin = open(&dir);
        let (_, token) = admin.create_invitation("TV", None, now).unwrap();
        assert_eq!(
            host.pair("endpoint-a", "TV", Some(&token), now).unwrap(),
            Ok(PairOutcome::Paired)
        );
        assert!(admin.revoke_device("endpoint-a", now).unwrap());
        assert!(host.is_authorized("endpoint-a"));
        host.reload().unwrap();
        assert!(!host.is_authorized("endpoint-a"));
        let (_, second) = admin.create_invitation("Phone", None, now).unwrap();
        host.create_invitation("Tablet", None, now).unwrap();
        admin.reload().unwrap();
        assert_eq!(admin.snapshot().invitations.len(), 3);
        assert!(
            admin
                .snapshot()
                .invitations
                .iter()
                .any(|inv| inv.token_hash == pairing::hash_token(&second))
        );
    }

    #[test]
    fn device_names_are_sanitized() {
        assert_eq!(sanitize_device_name("  TV\u{0007}\n "), "TV");
        assert_eq!(sanitize_device_name("\n"), "Unnamed device");
        assert_eq!(
            sanitize_device_name(&"a".repeat(100)).len(),
            MAX_DEVICE_NAME_CHARS
        );
    }
}
