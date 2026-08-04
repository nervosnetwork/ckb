//! Construction and structured ownership for the unified tx-pool service.

use crate::{
    authority::service::{
        AuthorityGeneration, AuthorityGenerationEvent, AuthorityPersistenceError, AuthorityService,
        AuthorityServiceInputs, AuthorityServiceStartError, AuthorityShutdownOutcome,
    },
    block_assembler::BlockAssembler,
    callback::{Callbacks, PendingCallback, ProposedCallback, RejectCallback},
    component::recent_reject::RecentReject,
    constants::{
        MESSAGE_CONCURRENCY_MULTIPLIER, PIPELINE_SHUTDOWN_TIMEOUT_SECONDS, SECONDS_PER_DAY,
    },
    network::{TxPoolNetwork, TxPoolNetworkHandle},
    service::{
        ChainReorgArgs, DEFAULT_CHANNEL_SIZE, Message, Notify, TxPoolController,
        TxVerificationResultReceiver, process,
    },
};
use ckb_app_config::{BlockAssemblerConfig, TxPoolConfig};
use ckb_async_runtime::Handle;
use ckb_error::{AnyError, OtherError};
use ckb_fee_estimator::FeeEstimator;
use ckb_logger::{error, info, warn};
use ckb_script::ChunkCommand;
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
    sync::{RwLock, mpsc, watch},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

fn service_cancellation_token(process_exit: &CancellationToken) -> CancellationToken {
    process_exit.child_token()
}

/// Builder for one unified-authority tx-pool generation.
pub struct TxPoolServiceBuilder {
    pub(crate) tx_pool_config: TxPoolConfig,
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) block_assembler: Option<BlockAssembler>,
    pub(crate) txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
    pub(crate) callbacks: Callbacks,
    pub(crate) receiver: mpsc::Receiver<Message>,
    pub(crate) reorg_receiver: mpsc::Receiver<Notify<ChainReorgArgs>>,
    pub(crate) signal_receiver: CancellationToken,
    pub(crate) handle: Handle,
    relay_sink: crate::authority::service::AuthorityRelaySink,
    pub(crate) chunk_rx: watch::Receiver<ChunkCommand>,
    pub(crate) started: Arc<AtomicBool>,
    pub(crate) fee_estimator: FeeEstimator,
    pub(crate) recent_reject: Option<Arc<RecentReject>>,
}

impl TxPoolServiceBuilder {
    /// Create every bounded controller and relay capability before startup.
    ///
    /// The relay receiver is returned linearly with the builder: sync consumes
    /// the committed authority stream directly, so there is no forwarding
    /// task or unbounded compatibility channel.
    pub fn new(
        tx_pool_config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        block_assembler_config: Option<BlockAssemblerConfig>,
        txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
        handle: &Handle,
        fee_estimator: FeeEstimator,
    ) -> Result<
        (
            TxPoolServiceBuilder,
            TxPoolController,
            TxVerificationResultReceiver,
        ),
        AnyError,
    > {
        let (sender, receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let (reorg_sender, reorg_receiver) = mpsc::channel(crate::service::REORG_CHANNEL_SIZE);
        let process_exit = new_tokio_exit_rx();
        let signal_receiver = service_cancellation_token(&process_exit);
        let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);
        let started = Arc::new(AtomicBool::new(false));
        let controller = TxPoolController {
            sender,
            reorg_sender,
            handle: handle.clone(),
            chunk_tx: Arc::new(chunk_tx),
            started: Arc::clone(&started),
            signal: signal_receiver.clone(),
        };

        let block_assembler = block_assembler_config.and_then(|config| {
            BlockAssembler::new(config, Arc::clone(&snapshot))
                .inspect_err(|error| error!("failed to initialize block assembler: {error}"))
                .ok()
        });
        let recent_reject = Self::build_recent_reject(&tx_pool_config).map(Arc::new);
        let (relay_sink, relay_receiver) =
            AuthorityService::prepare_relay(&tx_pool_config, &snapshot).map_err(|error| {
                OtherError::new(format!(
                    "invalid unified tx-pool relay configuration: {error:?}"
                ))
            })?;

        Ok((
            Self {
                tx_pool_config,
                snapshot,
                block_assembler,
                txs_verify_cache,
                callbacks: Callbacks::new(),
                receiver,
                reorg_receiver,
                signal_receiver,
                handle: handle.clone(),
                relay_sink,
                chunk_rx,
                started,
                fee_estimator,
                recent_reject,
            },
            controller,
            TxVerificationResultReceiver::from_authority(relay_receiver),
        ))
    }

    pub fn register_pending(&mut self, callback: PendingCallback) {
        self.callbacks.register_pending(callback);
    }

    pub fn register_proposed(&mut self, callback: ProposedCallback) {
        self.callbacks.register_proposed(callback);
    }

    pub fn register_reject(&mut self, callback: RejectCallback) {
        self.callbacks.register_reject(callback);
    }

    pub fn recent_reject(&self) -> Option<Arc<RecentReject>> {
        self.recent_reject.clone()
    }

    pub(crate) fn build_recent_reject(config: &TxPoolConfig) -> Option<RecentReject> {
        if config.recent_reject.as_os_str().is_empty() {
            warn!("Recent reject database is disabled!");
            return None;
        }
        let ttl = i32::from(u8::max(1, config.keep_rejected_tx_hashes_days))
            .saturating_mul(SECONDS_PER_DAY);
        match RecentReject::new(
            &config.recent_reject,
            config.keep_rejected_tx_hashes_count,
            ttl,
        ) {
            Ok(recent_reject) => Some(recent_reject),
            Err(error) => {
                error!(
                    "Failed to open the recent reject database {:?} {error}",
                    config.recent_reject
                );
                None
            }
        }
    }

    /// Start a detached production generation.
    pub fn start<N: TxPoolNetwork>(self, network: N) {
        drop(self.start_inner(network));
    }

    /// Test/benchmark variant that exposes the generation owner.
    #[cfg(any(test, feature = "internal"))]
    pub(crate) fn start_with_handle<N: TxPoolNetwork>(
        self,
        network: N,
    ) -> tokio::task::JoinHandle<()> {
        self.start_inner(network)
    }

    fn start_inner<N: TxPoolNetwork>(self, network: N) -> tokio::task::JoinHandle<()> {
        let handle = self.handle.clone();
        let network: TxPoolNetworkHandle = Arc::new(network);
        handle.spawn(async move { self.run(network).await })
    }

    async fn run(self, network: TxPoolNetworkHandle) {
        if self.tx_pool_config.max_tx_pool_resident_size < self.tx_pool_config.max_tx_pool_size {
            warn!(
                "max_tx_pool_resident_size ({}) < max_tx_pool_size ({}): clamping the accepted-pool residency budget up to max_tx_pool_size",
                self.tx_pool_config.max_tx_pool_resident_size, self.tx_pool_config.max_tx_pool_size
            );
        }
        let handler_limit = match self
            .tx_pool_config
            .max_tx_verify_workers
            .max(1)
            .checked_mul(MESSAGE_CONCURRENCY_MULTIPLIER)
        {
            Some(limit) if limit > 0 => limit,
            _ => {
                error!("tx-pool dispatcher concurrency bound is not representable");
                return;
            }
        };

        let Self {
            tx_pool_config,
            snapshot,
            block_assembler,
            txs_verify_cache,
            callbacks,
            receiver,
            reorg_receiver,
            signal_receiver,
            handle,
            relay_sink,
            chunk_rx,
            started,
            fee_estimator,
            recent_reject,
        } = self;

        let persisted_config = tx_pool_config.clone();
        let persisted = match tokio::task::spawn_blocking(move || {
            crate::persisted::load_persistence_snapshot(&persisted_config)
        })
        .await
        {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                error!("failed to load tx-pool persistence: {error}");
                crate::persisted::PersistenceSnapshot::default()
            }
            Err(error) => {
                error!("tx-pool persistence loader failed to join: {error}");
                crate::persisted::PersistenceSnapshot::default()
            }
        };

        let assembly = AuthorityService::assemble(
            &handle,
            AuthorityServiceInputs {
                config: tx_pool_config,
                snapshot,
                block_assembler,
                verification_cache: txs_verify_cache,
                callbacks,
                network,
                relay_sink,
                persistence_writer: Arc::new(crate::persisted::PersistenceWriter::default()),
                recent_reject,
                fee_estimator,
                reorg_receiver,
                chunk_rx,
                cancel: signal_receiver.clone(),
            },
        )
        .await;
        let assembly = match assembly {
            Ok(assembly) => assembly,
            Err(error) => {
                log_start_error(error);
                return;
            }
        };
        let service = assembly.service;
        let mut generation = assembly.generation;

        match service.replay_persisted(persisted).await {
            Ok(report) if report.loaded > 0 || report.stale > 0 => {
                info!(
                    "Persistent tx-pool data loaded: {} accepted, {} stale",
                    report.loaded, report.stale
                );
            }
            Ok(_) => {}
            Err(AuthorityPersistenceError::Replay(error)) => {
                log_replay_error(&AuthorityPersistenceError::Replay(error));
                if let Err(fault) = AuthorityService::settle_operation_error(error) {
                    generation.invalidate(fault);
                }
                let _ = generation
                    .shutdown(Duration::from_secs(PIPELINE_SHUTDOWN_TIMEOUT_SECONDS))
                    .await;
                return;
            }
            Err(error) => log_replay_error(&error),
        }
        if signal_receiver.is_cancelled() {
            let _ = generation
                .shutdown(Duration::from_secs(PIPELINE_SHUTDOWN_TIMEOUT_SECONDS))
                .await;
            return;
        }

        started.store(true, Ordering::Release);
        Self::run_dispatcher(
            service,
            generation,
            receiver,
            signal_receiver,
            started,
            handler_limit,
        )
        .await;
    }

    async fn run_dispatcher(
        service: AuthorityService,
        mut generation: AuthorityGeneration,
        mut receiver: mpsc::Receiver<Message>,
        signal: CancellationToken,
        started: Arc<AtomicBool>,
        handler_limit: usize,
    ) {
        let mut handlers = JoinSet::new();
        let mut handler_clean = true;
        loop {
            tokio::select! {
                _ = signal.cancelled() => break,
                event = generation.next_event() => match event {
                    AuthorityGenerationEvent::DerivedDegraded => {}
                    AuthorityGenerationEvent::ShutdownRequested => break,
                    AuthorityGenerationEvent::GenerationInvalid => {
                        handler_clean = false;
                        break;
                    }
                },
                completed = handlers.join_next(), if !handlers.is_empty() => {
                    match completed {
                        Some(Ok(Ok(()))) => {}
                        Some(Ok(Err(error))) => {
                            error!("tx-pool message handler reached a structural fault: {error:?}");
                            generation.invalidate(error);
                            handler_clean = false;
                            break;
                        }
                        Some(Err(error)) => {
                            error!("tx-pool message handler task failed: {error}");
                            crate::metrics::record_failure(if error.is_panic() {
                                crate::metrics::FailureBoundary::HandlerUnwind
                            } else {
                                crate::metrics::FailureBoundary::WorkerExit
                            });
                            handler_clean = false;
                            break;
                        }
                        None => {}
                    }
                }
                message = receiver.recv(), if handlers.len() < handler_limit => match message {
                    Some(message) => {
                        let service = service.clone();
                        handlers.spawn(async move { process(service, message).await });
                    }
                    None => break,
                }
            }
        }

        started.store(false, Ordering::Release);
        signal.cancel();
        receiver.close();
        while receiver.try_recv().is_ok() {}

        let handler_timeout = Duration::from_secs(PIPELINE_SHUTDOWN_TIMEOUT_SECONDS);
        let drain_handlers = async {
            while let Some(result) = handlers.join_next().await {
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        error!("tx-pool handler failed while draining: {error:?}");
                        generation.invalidate(error);
                        handler_clean = false;
                    }
                    Err(error) => {
                        error!("tx-pool handler task failed while draining: {error}");
                        crate::metrics::record_failure(if error.is_panic() {
                            crate::metrics::FailureBoundary::HandlerUnwind
                        } else {
                            crate::metrics::FailureBoundary::WorkerExit
                        });
                        handler_clean = false;
                    }
                }
            }
        };
        if tokio::time::timeout(handler_timeout, drain_handlers)
            .await
            .is_err()
        {
            handlers.abort_all();
            while handlers.join_next().await.is_some() {}
            crate::metrics::record_failure(crate::metrics::FailureBoundary::WorkerExit);
            handler_clean = false;
        }

        let shutdown = generation.shutdown(handler_timeout).await;
        if handler_clean && shutdown == AuthorityShutdownOutcome::PersistenceEligible {
            info!("TxPool is saving, please wait...");
            if let Err(error) = service.save_pool().await {
                error!("failed to save tx-pool: {error:?}");
            }
        } else {
            warn!(
                "TxPool shutdown did not reach a complete authority/effect boundary; skipping persistence"
            );
        }
        info!("TxPool service exited");
    }
}

fn log_start_error(error: AuthorityServiceStartError) {
    error!("tx-pool service startup failed: {error:?}");
}

fn log_replay_error(error: &AuthorityPersistenceError) {
    error!("failed to replay tx-pool persistence: {error:?}");
}
