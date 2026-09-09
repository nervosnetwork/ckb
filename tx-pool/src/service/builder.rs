//! Construction and joined ownership of the pool's fixed task population.
use crate::{
    authority::service::{Endpoints, Pool, RelaySink},
    block_assembler::{BlockAssembler, BoundedCandidateUncle},
    callback::{Callbacks, PendingCallback, ProposedCallback, RejectCallback},
    component::recent_reject::RecentReject,
    constants::{
        MESSAGE_CONCURRENCY_MULTIPLIER, PIPELINE_SHUTDOWN_TIMEOUT_SECONDS, SECONDS_PER_DAY,
    },
    network::{TxPoolNetwork, TxPoolNetworkHandle},
    service::{
        AdministrationGate, CHAIN_CONTROL_CHANNEL_SIZE, ChainControl, ChainReorgPayloadLimit,
        DEFAULT_CHANNEL_SIZE, Message, TxPoolController, TxVerificationResultReceiver, process,
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
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{RwLock, mpsc},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

/// Read callbacks have their own small channel and handlers, so ordinary
/// requests awaiting publication cannot prevent the publisher's nested read.
/// External dry-runs and template waits use ordinary handlers. Callback
/// dry-runs use this reserve and never wait for publishing jobs' CPU capacity.
pub(super) const READ_CHANNEL_SIZE: usize = 16;
const READ_HANDLERS: usize = 2;

/// Configures and starts the bounded transaction-pool service.
pub struct TxPoolServiceBuilder {
    pool: Arc<Pool>,
    relay: RelaySink,
    callbacks: Callbacks,
    receiver: mpsc::Receiver<Message>,
    query_receiver: mpsc::Receiver<Message>,
    chain_receiver: mpsc::Receiver<ChainControl>,
    pub(crate) signal_receiver: CancellationToken,
    pub(crate) handle: Handle,
    started: Arc<AtomicBool>,
    recent_reject: Option<Arc<RecentReject>>,
    estimator: FeeEstimator,
}
impl TxPoolServiceBuilder {
    #[cfg(all(test, feature = "internal"))]
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
        let (sender, receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let (query_sender, query_receiver) = mpsc::channel(READ_CHANNEL_SIZE);
        let (chain_control_sender, chain_receiver) = mpsc::channel(CHAIN_CONTROL_CHANNEL_SIZE);
        let signal_receiver = new_tokio_exit_rx().child_token();
        let started = Arc::new(AtomicBool::new(false));
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
            sender,
            query_sender,
            chain_control_sender,
            verification_command: pool.verification.clone(),
            handle: handle.clone(),
            started: Arc::clone(&started),
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
                receiver,
                query_receiver,
                chain_receiver,
                signal_receiver,
                handle: handle.clone(),
                started,
                recent_reject,
                estimator,
            },
            controller,
            TxVerificationResultReceiver::from_authority(drain),
        ))
    }
    /// Register notification of committed pending transactions.
    ///
    /// The callback must return promptly and must not wait for pool mutations,
    /// including on helper threads. Publication and shutdown wait for its return.
    pub fn register_pending(&mut self, callback: PendingCallback) {
        self.callbacks.register_pending(callback);
    }
    /// Register notification of committed proposed transactions.
    ///
    /// The callback must return promptly and must not wait for pool mutations,
    /// including on helper threads. Publication and shutdown wait for its return.
    pub fn register_proposed(&mut self, callback: ProposedCallback) {
        self.callbacks.register_proposed(callback);
    }
    /// Register notification of committed transaction rejection.
    ///
    /// The callback must return promptly and must not wait for pool mutations,
    /// including on helper threads. Publication and shutdown wait for its return.
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
    #[cfg(feature = "internal")]
    pub(crate) fn start_with_handle<N: TxPoolNetwork>(
        self,
        network: N,
    ) -> tokio::task::JoinHandle<()> {
        self.start_inner(network)
    }
    fn start_inner<N: TxPoolNetwork>(self, network: N) -> tokio::task::JoinHandle<()> {
        let handle = self.handle.clone();
        let network: TxPoolNetworkHandle = Arc::new(network);
        handle.spawn(self.run(network))
    }
    async fn run(self, network: TxPoolNetworkHandle) {
        let Self {
            pool,
            relay,
            callbacks,
            mut receiver,
            mut query_receiver,
            chain_receiver,
            signal_receiver: signal,
            handle,
            started,
            recent_reject,
            estimator,
        } = self;
        let Some(handler_limit) = pool
            .config
            .max_tx_verify_workers
            .max(1)
            .checked_mul(MESSAGE_CONCURRENCY_MULTIPLIER)
        else {
            error!("tx-pool handler bound is not representable");
            return;
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
                Vec::new()
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
        let (mut background, mut publisher) =
            pool.start_background(&handle, endpoints, chain_receiver);
        let mut publisher_finished = false;
        let mut startup_complete = false;
        let mut queries = JoinSet::new();
        {
            let replay = pool.replay(persisted);
            tokio::pin!(replay);
            // Replayed admissions publish callbacks too. Serve their bounded
            // read channel while replay is waiting for those callbacks.
            loop {
                tokio::select! {
                    result = &mut replay => {
                        match result {
                            Ok((loaded, stale)) => {
                                info!("Persistent tx-pool data loaded: {loaded} accepted, {stale} stale");
                                startup_complete = true;
                            }
                            Err(error) => {
                                error!("tx-pool persistence replay failed: {error}");
                                pool.fault();
                            }
                        }
                        break;
                    },
                    _ = signal.cancelled() => break,
                    result = background.join_next() => {
                        crate::metrics::record_failure(crate::metrics::FailureBoundary::WorkerExit);
                        error!("tx-pool background task exited during replay: {result:?}");
                        pool.fault();
                        break;
                    },
                    result = &mut publisher => {
                        publisher_finished = true;
                        error!("tx-pool publisher exited during replay: {result:?}");
                        pool.fault();
                        break;
                    },
                    result = queries.join_next(), if !queries.is_empty() => {
                        if !matches!(result, Some(Ok(Ok(())))) {
                            error!("tx-pool read handler failed during replay: {result:?}");
                            pool.fault();
                            break;
                        }
                    },
                    message = query_receiver.recv(), if queries.len() < READ_HANDLERS => match message {
                        Some(message) => {
                            queries.spawn(process(Arc::clone(&pool), message));
                        },
                        None => break,
                    },
                }
            }
        }
        let mut handlers = JoinSet::new();
        if startup_complete && !signal.is_cancelled() && !pool.is_faulted() {
            started.store(true, Ordering::Release);
            loop {
                tokio::select! {
                    _ = signal.cancelled() => break,
                    result = &mut publisher, if !publisher_finished => {
                        publisher_finished = true;
                        error!("tx-pool publisher exited before drain: {result:?}");
                        pool.fault();
                        break;
                    },
                    result = background.join_next(), if !background.is_empty() => {
                        if !pool.is_stopped() {
                            crate::metrics::record_failure(crate::metrics::FailureBoundary::WorkerExit);
                            error!("tx-pool background task exited: {result:?}");
                            pool.fault();
                        } else if !matches!(result, Some(Ok(Ok(())))) {
                            pool.fault();
                        }
                        break;
                    },
                    result = handlers.join_next(), if !handlers.is_empty() => {
                        if !matches!(result, Some(Ok(Ok(())))) {
                            crate::metrics::record_failure(crate::metrics::FailureBoundary::HandlerUnwind);
                            error!("tx-pool message handler failed: {result:?}");
                            pool.fault();
                            break;
                        }
                    },
                    result = queries.join_next(), if !queries.is_empty() => {
                        if !matches!(result, Some(Ok(Ok(())))) {
                            crate::metrics::record_failure(crate::metrics::FailureBoundary::HandlerUnwind);
                            error!("tx-pool read handler failed: {result:?}");
                            pool.fault();
                            break;
                        }
                    },
                    message = receiver.recv(), if handlers.len() < handler_limit => match message {
                        Some(message) => {
                            handlers.spawn(process(Arc::clone(&pool), message));
                        },
                        None => break,
                    },
                    message = query_receiver.recv(), if queries.len() < READ_HANDLERS => match message {
                        Some(message) => {
                            queries.spawn(process(Arc::clone(&pool), message));
                        },
                        None => break,
                    },
                }
            }
        }
        started.store(false, Ordering::Release);
        signal.cancel();
        pool.stop();
        receiver.close();
        query_receiver.close();
        while receiver.try_recv().is_ok() {}
        while query_receiver.try_recv().is_ok() {}
        let timeout = Duration::from_secs(PIPELINE_SHUTDOWN_TIMEOUT_SECONDS);
        let drain = async {
            while !handlers.is_empty() || !queries.is_empty() || !background.is_empty() {
                let result = tokio::select! {
                    result = handlers.join_next(), if !handlers.is_empty() => result,
                    result = queries.join_next(), if !queries.is_empty() => result,
                    result = background.join_next(), if !background.is_empty() => result,
                };
                if !matches!(result, Some(Ok(Ok(())))) {
                    error!("tx-pool task failed while joining: {result:?}");
                    pool.fault();
                }
            }
        };
        if tokio::time::timeout(timeout, drain).await.is_err() {
            pool.fault();
            handlers.abort_all();
            queries.abort_all();
            background.abort_all();
            while handlers.join_next().await.is_some() {}
            while queries.join_next().await.is_some() {}
            while background.join_next().await.is_some() {}
        }
        pool.close_outbox();
        if !publisher_finished {
            match tokio::time::timeout(timeout, &mut publisher).await {
                Ok(Ok(Ok(()))) => {}
                Ok(result) => {
                    error!("tx-pool publisher failed while draining: {result:?}");
                    pool.fault();
                }
                Err(_) => {
                    pool.fault();
                    publisher.abort();
                    let _ = publisher.await;
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
    }
}
