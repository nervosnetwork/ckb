//! Construction and joined ownership of the pool's background tasks and direct
//! request lifecycle gate.

use crate::{
    authority::service::{Endpoints, Error, Pool, RelaySink},
    block_assembler::{BlockAssembler, BoundedCandidateUncle},
    callback::{Callbacks, PendingCallback, ProposedCallback, RejectCallback},
    component::recent_reject::RecentReject,
    constants::{PIPELINE_SHUTDOWN_TIMEOUT_SECONDS, SECONDS_PER_DAY},
    network::{TxPoolNetwork, TxPoolNetworkHandle},
    persisted::PreparedReplay,
    service::{
        AdministrationGate, CHAIN_CONTROL_CHANNEL_SIZE, ChainControl, ChainReorgPayloadLimit,
        DEFAULT_CHANNEL_SIZE, READ_CHANNEL_SIZE, RequestGate, TxPoolController,
        TxVerificationResultReceiver,
    },
};
use ckb_app_config::{BlockAssemblerConfig, TxPoolConfig};
use ckb_async_runtime::Handle;
use ckb_error::{AnyError, OtherError};
use ckb_fee_estimator::FeeEstimator;
use ckb_logger::{error, info, warn};
use ckb_snapshot::Snapshot;
use ckb_stop_handler::new_tokio_exit_rx;
use ckb_verification::cache::TxVerificationCache;
use futures_util::FutureExt;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{RwLock, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

/// Closes the shared `RequestGate` if an unstarted `TxPoolServiceBuilder` is
/// dropped before `start()` transfers ownership into `GenerationGuard`.
struct PrestartGate(Option<Arc<RequestGate>>);

impl PrestartGate {
    fn into_gate(mut self) -> Arc<RequestGate> {
        self.0.take().unwrap_or_else(|| RequestGate::new(0, 0))
    }
}

impl Drop for PrestartGate {
    fn drop(&mut self) {
        if let Some(gate) = self.0.take() {
            gate.close();
        }
    }
}

/// Cancellation of the generation future must also close request intake and
/// reach tasks inside synchronous work that cannot observe JoinSet abort yet.
struct GenerationGuard<'a> {
    pool: &'a Pool,
    signal: &'a CancellationToken,
    gate: &'a RequestGate,
    completed: bool,
}

impl Drop for GenerationGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.gate.close();
            self.signal.cancel();
            self.pool.fault();
            self.pool.close_outbox();
        }
    }
}

/// Configures and starts the bounded transaction-pool service.
pub struct TxPoolServiceBuilder {
    pool: Arc<Pool>,
    relay: RelaySink,
    callbacks: Callbacks,
    gate: PrestartGate,
    chain_receiver: mpsc::Receiver<ChainControl>,
    pub(crate) signal_receiver: CancellationToken,
    pub(crate) handle: Handle,
    recent_reject: Option<Arc<RecentReject>>,
    estimator: FeeEstimator,
}

impl TxPoolServiceBuilder {
    #[cfg(test)]
    pub(crate) fn pool_for_test(&self) -> Arc<Pool> {
        Arc::clone(&self.pool)
    }

    /// Construct the bounded controller and sole committed relay receiver.
    pub fn new(
        config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        assembler: Option<BlockAssemblerConfig>,
        cache: Arc<RwLock<TxVerificationCache>>,
        handle: &Handle,
        estimator: FeeEstimator,
    ) -> Result<(Self, TxPoolController, TxVerificationResultReceiver), AnyError> {
        let (chain_control_sender, chain_receiver) = mpsc::channel(CHAIN_CONTROL_CHANNEL_SIZE);
        let signal_receiver = new_tokio_exit_rx().child_token();
        let gate = RequestGate::new(DEFAULT_CHANNEL_SIZE, READ_CHANNEL_SIZE);
        let candidate_uncle_payload_limit =
            BoundedCandidateUncle::payload_limit(snapshot.consensus().max_block_bytes())
                .ok_or_else(|| {
                    OtherError::new("uncle residency bound is not representable".to_owned())
                })?;
        let chain_reorg_payload_limit =
            ChainReorgPayloadLimit::from_config(&config).ok_or_else(|| {
                OtherError::new("reorg residency bound is not representable".to_owned())
            })?;
        let assembler = assembler.and_then(|config| {
            BlockAssembler::new(config, Arc::clone(&snapshot))
                .inspect_err(|error| error!("failed to initialize block assembler: {error}"))
                .ok()
        });
        let recent_reject = Self::build_recent_reject(&config).map(Arc::new);
        let (pool, relay, drain) = Pool::new(
            config,
            snapshot,
            handle,
            cache,
            assembler,
            recent_reject.clone(),
            estimator.clone(),
        )?;
        let controller = TxPoolController {
            pool: Arc::downgrade(&pool),
            gate: Arc::clone(&gate),
            chain_control_sender,
            verification_command: pool.verification.clone(),
            handle: handle.clone(),
            administration_gate: AdministrationGate::new(),
            chain_reorg_payload_limit,
            candidate_uncle_payload_limit,
            signal: signal_receiver.clone(),
        };
        Ok((
            Self {
                pool,
                relay,
                callbacks: Callbacks::new(),
                gate: PrestartGate(Some(gate)),
                chain_receiver,
                signal_receiver,
                handle: handle.clone(),
                recent_reject,
                estimator,
            },
            controller,
            TxVerificationResultReceiver::from_authority(drain),
        ))
    }

    /// Register notification of committed pending transactions.
    ///
    /// The callback must follow the [Callbacks] contract and return promptly.
    pub fn register_pending(&mut self, callback: PendingCallback) {
        self.callbacks.register_pending(callback);
    }

    /// Register notification of committed proposed transactions.
    ///
    /// The callback must follow the [Callbacks] contract and return promptly.
    pub fn register_proposed(&mut self, callback: ProposedCallback) {
        self.callbacks.register_proposed(callback);
    }

    /// Register notification of committed transaction rejection.
    ///
    /// The callback must follow the [Callbacks] contract and return promptly.
    pub fn register_reject(&mut self, callback: RejectCallback) {
        self.callbacks.register_reject(callback);
    }

    /// Return the optional recent rejection database.
    pub fn recent_reject(&self) -> Option<Arc<RecentReject>> {
        self.recent_reject.clone()
    }

    pub(crate) fn build_recent_reject(config: &TxPoolConfig) -> Option<RecentReject> {
        if config.recent_reject.as_os_str().is_empty() {
            warn!("Recent reject database is disabled!");
            return None;
        }
        let ttl =
            i32::from(config.keep_rejected_tx_hashes_days.max(1)).saturating_mul(SECONDS_PER_DAY);
        RecentReject::new(
            &config.recent_reject,
            config.keep_rejected_tx_hashes_count,
            ttl,
        )
        .inspect_err(|error| {
            error!(
                "Failed to open recent reject database {:?}: {error}",
                config.recent_reject
            )
        })
        .ok()
    }

    /// Start the service; its generation task joins all owned workers on stop.
    pub fn start<N: TxPoolNetwork>(self, network: N) {
        drop(self.start_inner(network));
    }

    #[cfg(any(test, feature = "internal"))]
    pub(crate) fn start_with_handle<N: TxPoolNetwork>(
        self,
        network: N,
    ) -> tokio::task::JoinHandle<()> {
        self.start_inner(network)
    }

    fn start_inner<N: TxPoolNetwork>(self, network: N) -> tokio::task::JoinHandle<()> {
        crate::verification::calibration::initialize();
        let handle = self.handle.clone();
        let network: TxPoolNetworkHandle = Arc::new(network);
        handle.spawn(self.run(network))
    }

    async fn run(self, network: TxPoolNetworkHandle) {
        let Self {
            pool,
            relay,
            callbacks,
            gate,
            chain_receiver,
            signal_receiver: signal,
            handle,
            recent_reject,
            estimator,
        } = self;
        let gate = gate.into_gate();
        let mut generation = GenerationGuard {
            pool: &pool,
            signal: &signal,
            gate: &gate,
            completed: false,
        };
        let config = Arc::clone(&pool.config);
        let persisted = match tokio::task::spawn_blocking(move || {
            crate::persisted::load_persistence_snapshot(&config)
                .and_then(crate::persisted::PersistenceSnapshot::prepare_replay)
        })
        .await
        {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                error!("failed to prepare tx-pool persistence: {error}");
                PreparedReplay::default()
            }
            Err(error) => {
                error!("persistence loader failed to join: {error}");
                return;
            }
        };
        let endpoints = Endpoints::new(
            network,
            relay,
            Arc::new(callbacks),
            recent_reject,
            estimator,
        );
        let (mut background, publisher) =
            pool.start_background(&handle, endpoints, chain_receiver, &signal);
        let mut publisher = Some(publisher);
        let mut startup_complete = false;
        {
            // Completion releases replay immediately; leaving this scope on
            // cancellation also drops its remaining payloads before draining.
            let replay = pool.replay(persisted).fuse();
            tokio::pin!(replay);
            // Nested replay callbacks execute directly against `Pool` via the
            // reserved read lane. External requests wait until `gate.open()`.
            loop {
                tokio::select! {
                    result = &mut replay, if !startup_complete => {
                        match result {
                            Ok((loaded, rejected)) => {
                                info!("Persistent tx-pool data loaded: {loaded} accepted, {rejected} rejected");
                                startup_complete = true;
                                if signal.is_cancelled() || pool.is_faulted() {
                                    break;
                                }
                                gate.open();
                            }
                            Err(error) => {
                                error!("tx-pool persistence replay failed: {error}");
                                pool.fault();
                                break;
                            }
                        }
                    },
                    _ = signal.cancelled() => break,
                    result = publisher_wait(&mut publisher) => {
                        error!("tx-pool publisher exited before drain: {result:?}");
                        pool.fault();
                        break;
                    },
                    result = background.join_next() => {
                        if !startup_complete || !pool.is_stopped() {
                            crate::metrics::record_failure(crate::metrics::FailureBoundary::WorkerExit);
                            error!("tx-pool background task exited: {result:?}");
                            pool.fault();
                        } else if !matches!(result, Some(Ok(Ok(())))) {
                            pool.fault();
                        }
                        break;
                    },
                }
            }
        }
        gate.close();
        signal.cancel();
        pool.stop();
        let timeout = Duration::from_secs(PIPELINE_SHUTDOWN_TIMEOUT_SECONDS);
        let drain = async {
            let drained = gate.wait_drained().fuse();
            tokio::pin!(drained);
            let mut requests_drained = false;
            while !requests_drained || !background.is_empty() {
                tokio::select! {
                    _ = &mut drained, if !requests_drained => {
                        requests_drained = true;
                    }
                    result = background.join_next(), if !background.is_empty() => {
                        if !matches!(result, Some(Ok(Ok(())))) {
                            error!("tx-pool task failed while joining: {result:?}");
                            pool.fault();
                        }
                    }
                }
            }
        };
        if tokio::time::timeout(timeout, drain).await.is_err() {
            pool.fault();
            background.abort_all();
            while background.join_next().await.is_some() {}
        }
        pool.close_outbox();
        if publisher.is_some() {
            match tokio::time::timeout(timeout, publisher_wait(&mut publisher)).await {
                Ok(Ok(Ok(()))) => {}
                Ok(result) => {
                    error!("tx-pool publisher failed while draining: {result:?}");
                    pool.fault();
                }
                Err(_) => {
                    pool.fault();
                    if let Some(publisher) = &publisher {
                        publisher.abort();
                    }
                    let _ = publisher_wait(&mut publisher).await;
                }
            }
        }
        if startup_complete && pool.persistence_eligible() {
            if let Err(error) = pool.save().await {
                error!("failed to save tx-pool: {error}");
            }
        } else {
            warn!(
                "TxPool did not reach a complete state and publication boundary; skipping persistence"
            );
        }
        info!("TxPool service exited");
        generation.completed = true;
    }
}

/// Never take the handle before awaiting: select/timeout cancellation must
/// retain the join capability. Completion and clearing its slot cannot suspend.
async fn publisher_wait(
    publisher: &mut Option<JoinHandle<Result<(), Error>>>,
) -> Result<Result<(), Error>, tokio::task::JoinError> {
    let result = match publisher.as_mut() {
        Some(handle) => handle.await,
        None => std::future::pending().await,
    };
    *publisher = None;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelling_a_publisher_wait_preserves_its_join() {
        let (release, held) = tokio::sync::oneshot::channel();
        let mut publisher = Some(tokio::spawn(async move {
            held.await.unwrap();
            Ok(())
        }));
        {
            let wait = publisher_wait(&mut publisher);
            tokio::pin!(wait);
            assert!(futures_util::poll!(wait).is_pending());
        }
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), publisher_wait(&mut publisher))
            .await
            .expect("the cancelled wait must leave a joinable publisher")
            .unwrap()
            .unwrap();
    }
}
