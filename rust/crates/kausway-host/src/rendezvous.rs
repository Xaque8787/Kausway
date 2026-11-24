//! Host-owned, opt-in invitation publication. No backend exists until there is work.
//!
//! Canceling stops retries and refreshes. BEP44 has no deletion operation, so already
//! published ciphertext may remain cached until expiry; pairing still enforces revocation.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use iroh::{Endpoint, EndpointAddr};
use kausway_rendezvous::{
    Error, SignedPacket,
    backend::{Network, NetworkConfig},
};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::state::{HostState, Publication, StateError, now_secs};

const POLL: Duration = Duration::from_secs(1);
const RECHECK: Duration = Duration::from_millis(250);
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone, Copy)]
struct Timing {
    poll: Duration,
    recheck: Duration,
    publish_timeout: Duration,
}

const TIMING: Timing = Timing {
    poll: POLL,
    recheck: RECHECK,
    publish_timeout: PUBLISH_TIMEOUT,
};

trait Publisher: Send + Sync + 'static {
    fn publish(&self, packet: &SignedPacket) -> impl Future<Output = Result<(), Error>> + Send;
}

impl Publisher for Network {
    async fn publish(&self, packet: &SignedPacket) -> Result<(), Error> {
        Network::publish(self, packet).await
    }
}

pub(crate) async fn run(
    state: Arc<HostState>,
    endpoint: Endpoint,
    shutdown: CancellationToken,
    config: NetworkConfig,
) {
    run_with(
        state,
        move || endpoint.addr(),
        shutdown,
        move || Network::new_async(config.clone()),
        now_secs,
        TIMING,
    )
    .await;
}

async fn blocking<T: Send + 'static>(
    state: &Arc<HostState>,
    change: impl FnOnce(Arc<HostState>) -> Result<T, StateError> + Send + 'static,
) -> Option<T> {
    let state = state.clone();
    match tokio::task::spawn_blocking(move || change(state)).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(_)) => {
            warn!("could not update rendezvous state");
            None
        }
        Err(_) => {
            warn!("rendezvous state task failed");
            None
        }
    }
}

async fn run_with<P: Publisher, F: Future<Output = Result<P, Error>> + Send>(
    state: Arc<HostState>,
    address: impl Fn() -> EndpointAddr,
    shutdown: CancellationToken,
    factory: impl Fn() -> F + Send + Sync + 'static,
    clock: impl Fn() -> u64,
    timing: Timing,
) {
    let mut publisher = None;
    let mut interval = tokio::time::interval(timing.poll);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            _ = interval.tick() => {}
        }
        let addr = address();
        let now = clock();
        let work = tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            work = blocking(&state, move |state| state.rendezvous_publications(&addr, now)) => work,
        };
        let Some(work) = work else {
            continue;
        };
        if state.pending_rendezvous().is_empty() {
            publisher = None; // Stop public DHT activity when the last invitation retires.
        }
        // Sequential publication bounds concurrency to one, and the state cap bounds the queue.
        for publication in work {
            if shutdown.is_cancelled() {
                return;
            }
            if address() != publication.address || !current(&state, &publication, clock()).await {
                continue;
            }
            let deadline = tokio::time::Instant::now() + timing.publish_timeout;
            if publisher.is_none() {
                let created = tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => return,
                    result = tokio::time::timeout_at(deadline, factory()) => result.unwrap_or(Err(Error::Timeout)),
                };
                match created {
                    Ok(network) => publisher = Some(network),
                    _ => {
                        record_result(&state, &publication, clock(), false).await;
                        continue;
                    }
                }
            }
            // Construction can take time; reload once more before issuing any network request.
            if address() != publication.address || !current(&state, &publication, clock()).await {
                continue;
            }
            // Drop the publication future before recording a timeout or scheduling a retry.
            let success = {
                let network = publisher.as_ref().expect("created above");
                let operation = network.publish(&publication.packet);
                tokio::pin!(operation);
                let timeout = tokio::time::sleep_until(deadline);
                tokio::pin!(timeout);
                let mut recheck = tokio::time::interval(timing.recheck);
                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.cancelled() => return,
                        _ = &mut timeout => break Some(false),
                        result = &mut operation => break Some(result.is_ok()),
                        _ = recheck.tick() => {
                            if address() != publication.address || !current(&state, &publication, clock()).await {
                                break None;
                            }
                        }
                    }
                }
            };
            if let Some(success) = success {
                // Keep the packet borrow inside the publication operation's scope.
                record_result(&state, &publication, clock(), success).await;
            }
        }
    }
}

async fn current(state: &Arc<HostState>, publication: &Publication, now: u64) -> bool {
    let id = publication.id.clone();
    let sequence = publication.sequence;
    blocking(state, move |state| {
        state.rendezvous_is_current(&id, sequence, now)
    })
    .await
    .unwrap_or(false)
}

async fn record_result(state: &Arc<HostState>, publication: &Publication, now: u64, success: bool) {
    let id = publication.id.clone();
    let sequence = publication.sequence;
    blocking(state, move |state| {
        state.rendezvous_result(&id, sequence, now, success)
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::test_support::TempDir;
    use crate::state::STATE_FILE;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    };

    const FAST: Timing = Timing {
        poll: Duration::from_millis(10),
        recheck: Duration::from_millis(10),
        publish_timeout: Duration::from_millis(150),
    };

    #[derive(Default)]
    struct MockState {
        creates: AtomicUsize,
        drops: AtomicUsize,
        active: AtomicUsize,
        maximum: AtomicUsize,
        fail: AtomicBool,
        stall: AtomicBool,
        packets: Mutex<Vec<Vec<u8>>>,
    }

    impl MockState {
        fn calls(&self) -> usize {
            self.packets.lock().unwrap().len()
        }
    }

    struct MockPublisher(Arc<MockState>);

    impl Drop for MockPublisher {
        fn drop(&mut self) {
            self.0.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Active(Arc<MockState>);

    impl Drop for Active {
        fn drop(&mut self) {
            self.0.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl Publisher for MockPublisher {
        async fn publish(&self, packet: &SignedPacket) -> Result<(), Error> {
            let count = self.0.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.0.maximum.fetch_max(count, Ordering::SeqCst);
            let _active = Active(self.0.clone());
            self.0
                .packets
                .lock()
                .unwrap()
                .push(packet.as_bytes().to_vec());
            if self.0.stall.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if self.0.fail.swap(false, Ordering::SeqCst) {
                Err(Error::Network)
            } else {
                Ok(())
            }
        }
    }

    fn address() -> EndpointAddr {
        EndpointAddr::new(iroh::SecretKey::from_bytes(&[7; 32]).public())
            .with_ip_addr("192.0.2.10:4321".parse().unwrap())
    }

    fn start(
        state: Arc<HostState>,
        clock: Arc<AtomicU64>,
        mock: Arc<MockState>,
        shutdown: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(run_with(
            state,
            address,
            shutdown,
            move || {
                mock.creates.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(MockPublisher(mock.clone())))
            },
            move || clock.load(Ordering::SeqCst),
            FAST,
        ))
    }

    async fn until(mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("worker made progress within deadline");
    }

    async fn stop(shutdown: CancellationToken, worker: tokio::task::JoinHandle<()>) {
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn lazy_backend_idle_does_not_write_and_retry_reuses_durable_packet() {
        let dir = TempDir::create();
        let state = Arc::new(HostState::open(&dir.0.join(STATE_FILE), None).unwrap());
        let now = now_secs();
        let clock = Arc::new(AtomicU64::new(now));
        let mock = Arc::new(MockState::default());
        mock.fail.store(true, Ordering::SeqCst);
        let shutdown = CancellationToken::new();
        let modified = std::fs::metadata(state.path()).unwrap().modified().unwrap();
        let worker = start(state.clone(), clock.clone(), mock.clone(), shutdown.clone());
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(mock.creates.load(Ordering::SeqCst), 0);
        assert_eq!(
            std::fs::metadata(state.path()).unwrap().modified().unwrap(),
            modified
        );
        let (invite, _, code) = state
            .create_rendezvous_invitation("Phone", None, now, address())
            .unwrap();
        until(|| {
            state
                .pending_rendezvous()
                .get(&invite.id)
                .is_some_and(|pending| pending.failures() == 1)
        })
        .await;
        let first = mock.packets.lock().unwrap()[0].clone();
        let sequence = state.pending_rendezvous()[&invite.id].sequence();
        // A retry is deferred across a clock rollback and keeps the same durable bytes.
        clock.store(
            now - kausway_rendezvous::CLOCK_SKEW_SECS - 1,
            Ordering::SeqCst,
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(mock.calls(), 1);
        clock.store(now + 10, Ordering::SeqCst);
        until(|| mock.calls() == 2).await;
        until(|| state.pending_rendezvous()[&invite.id].next_attempt_at() > now + 10).await;
        assert_eq!(mock.packets.lock().unwrap()[1], first);
        assert_eq!(state.pending_rendezvous()[&invite.id].sequence(), sequence);
        let packet = SignedPacket::from_bytes(&first).unwrap();
        assert_eq!(
            kausway_rendezvous::decode_packet(&code.secrets(), &packet, now + 10)
                .unwrap()
                .addr,
            address()
        );
        state.cancel_invitation(&invite.id).unwrap();
        until(|| mock.drops.load(Ordering::SeqCst) == 1).await;
        clock.store(now + 500, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(mock.calls(), 2);
        stop(shutdown, worker).await;
        assert_eq!(mock.maximum.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_drops_inflight_publication_and_backend_without_refresh() {
        let dir = TempDir::create();
        let state = Arc::new(HostState::open(&dir.0.join(STATE_FILE), None).unwrap());
        let now = now_secs();
        let (invite, _, _) = state
            .create_rendezvous_invitation("Phone", None, now, address())
            .unwrap();
        let clock = Arc::new(AtomicU64::new(now));
        let mock = Arc::new(MockState::default());
        mock.stall.store(true, Ordering::SeqCst);
        let shutdown = CancellationToken::new();
        let worker = start(state.clone(), clock.clone(), mock.clone(), shutdown.clone());
        until(|| mock.active.load(Ordering::SeqCst) == 1).await;
        state.cancel_invitation(&invite.id).unwrap();
        until(|| mock.active.load(Ordering::SeqCst) == 0 && mock.drops.load(Ordering::SeqCst) == 1)
            .await;
        clock.store(now + 500, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(mock.calls(), 1);
        assert!(state.pending_rendezvous().is_empty());
        stop(shutdown, worker).await;
    }

    #[tokio::test]
    async fn stalled_backend_initialization_is_bounded_and_shutdown_remains_responsive() {
        let dir = TempDir::create();
        let state = Arc::new(HostState::open(&dir.0.join(STATE_FILE), None).unwrap());
        let now = now_secs();
        let (invite, _, _) = state
            .create_rendezvous_invitation("Phone", None, now, address())
            .unwrap();
        let shutdown = CancellationToken::new();
        let worker = tokio::spawn(run_with(
            state.clone(),
            address,
            shutdown.clone(),
            std::future::pending::<Result<MockPublisher, Error>>,
            move || now,
            FAST,
        ));
        until(|| state.pending_rendezvous()[&invite.id].failures() == 1).await;
        stop(shutdown, worker).await;
        assert_eq!(
            state.rendezvous_status(&invite.id, now).unwrap().state,
            "retrying"
        );
    }

    #[tokio::test]
    async fn publication_timeout_retries_then_shutdown_cancels_inflight_work() {
        let dir = TempDir::create();
        let state = Arc::new(HostState::open(&dir.0.join(STATE_FILE), None).unwrap());
        let now = now_secs();
        let (invite, _, _) = state
            .create_rendezvous_invitation("Phone", None, now, address())
            .unwrap();
        let clock = Arc::new(AtomicU64::new(now));
        let mock = Arc::new(MockState::default());
        mock.stall.store(true, Ordering::SeqCst);
        let shutdown = CancellationToken::new();
        let worker = start(state.clone(), clock.clone(), mock.clone(), shutdown.clone());
        until(|| state.pending_rendezvous()[&invite.id].failures() == 1).await;
        assert_eq!(mock.active.load(Ordering::SeqCst), 0);
        clock.store(now + 10, Ordering::SeqCst);
        until(|| mock.calls() == 2 && mock.active.load(Ordering::SeqCst) == 1).await;
        stop(shutdown, worker).await;
        assert_eq!(mock.active.load(Ordering::SeqCst), 0);
        assert_eq!(mock.maximum.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn external_file_lock_does_not_hold_worker_or_reload_indefinitely() {
        let dir = TempDir::create();
        let state = Arc::new(HostState::open(&dir.0.join(STATE_FILE), None).unwrap());
        let now = now_secs();
        state
            .create_rendezvous_invitation("Phone", None, now, address())
            .unwrap();
        let mock = Arc::new(MockState::default());
        mock.stall.store(true, Ordering::SeqCst);
        let shutdown = CancellationToken::new();
        let worker = start(
            state.clone(),
            Arc::new(AtomicU64::new(now)),
            mock.clone(),
            shutdown.clone(),
        );
        until(|| mock.active.load(Ordering::SeqCst) == 1).await;
        let held = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.0.join("state.lock"))
            .unwrap();
        held.lock().unwrap();
        let reload = {
            let state = state.clone();
            tokio::task::spawn_blocking(move || state.reload())
        };
        // Let an in-flight publication reach its periodic preflight while the OS lock is held.
        tokio::time::sleep(Duration::from_millis(30)).await;
        stop(shutdown, worker).await;
        assert_eq!(mock.active.load(Ordering::SeqCst), 0);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), reload)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        drop(held);
    }
}
