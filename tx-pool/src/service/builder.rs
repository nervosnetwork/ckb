//! Construction and structured ownership for the unified tx-pool service.

use super::dispatch::process_retained_ingress_batch;
use crate::{
    authority::service::{
        AuthorityGeneration, AuthorityGenerationEvent, AuthorityPersistenceError, AuthorityService,
        AuthorityServiceBootstrap, AuthorityServiceInputs, AuthorityServiceStartError,
        AuthorityShutdownOutcome, AuthorityVerificationControl,
    },
    block_assembler::BlockAssembler,
    callback::{Callbacks, PendingCallback, ProposedCallback, RejectCallback},
    component::recent_reject::RecentReject,
    constants::{
        MESSAGE_CONCURRENCY_MULTIPLIER, PIPELINE_SHUTDOWN_TIMEOUT_SECONDS, SECONDS_PER_DAY,
    },
    network::{TxPoolNetwork, TxPoolNetworkHandle},
    service::{
        AdministrationGate, BoundedTransaction, CHAIN_CONTROL_CHANNEL_SIZE, ChainControl,
        ChainReorgPayloadLimit, DEFAULT_CHANNEL_SIZE, Message, Notify, RemoteTxSubmission, Request,
        TxPoolController, TxVerificationResultReceiver, process,
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
use ckb_types::{packed::Byte32, prelude::Entity};
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

fn service_cancellation_token(process_exit: &CancellationToken) -> CancellationToken {
    process_exit.child_token()
}

const RETAINED_INGRESS_APPLY_ITEMS: usize = crate::constants::MAX_POOL_MUTATION_CANDIDATES;
const RETAINED_INGRESS_BYTES: usize = ckb_constant::sync::MAX_RELAY_TXS_BYTES_PER_BATCH;

/// Stack-owned, immediately available retained-ingress prefix.
///
/// It is not a queue or semantic owner. The controller channel remains the
/// only waiting owner; the dispatcher merely moves an already available
/// homogeneous prefix into one existing handler task. Remote batches are also
/// peer-homogeneous so a malformed item has one unambiguous revocation cohort.
pub(super) enum RetainedIngressBatch {
    Remote {
        peer: ckb_network::PeerIndex,
        submissions: Vec<(BoundedTransaction, ckb_types::core::Cycle)>,
        responders: Vec<tokio::sync::oneshot::Sender<()>>,
        bytes: usize,
    },
    Proposal {
        transactions: Vec<BoundedTransaction>,
        bytes: usize,
    },
}

enum RetainedIngressAppend {
    Consumed,
    Lookahead(Message),
}

impl RetainedIngressBatch {
    fn try_new(message: Message) -> Result<Self, Message> {
        match message {
            Message::SubmitRemoteTx(request) => {
                let tx_bytes = request.arguments.transaction.payload_bytes();
                if tx_bytes > RETAINED_INGRESS_BYTES {
                    return Err(Message::SubmitRemoteTx(request));
                }
                let mut submissions = Vec::new();
                let mut responders = Vec::new();
                if submissions.try_reserve(1).is_err() || responders.try_reserve(1).is_err() {
                    return Err(Message::SubmitRemoteTx(request));
                }
                let Request {
                    responder,
                    arguments:
                        RemoteTxSubmission {
                            transaction,
                            declared_cycles,
                            peer,
                        },
                } = request;
                submissions.push((transaction, declared_cycles));
                responders.push(responder);
                Ok(Self::Remote {
                    peer,
                    submissions,
                    responders,
                    bytes: tx_bytes,
                })
            }
            Message::NotifyTxs(Notify { arguments }) if !arguments.is_empty() => {
                let bytes = arguments.total_bytes();
                Ok(Self::Proposal {
                    transactions: arguments.into_transactions(),
                    bytes,
                })
            }
            message => Err(message),
        }
    }

    fn can_drain(&self) -> bool {
        match self {
            Self::Remote {
                submissions, bytes, ..
            } => {
                submissions.len() < RETAINED_INGRESS_APPLY_ITEMS && *bytes < RETAINED_INGRESS_BYTES
            }
            Self::Proposal {
                transactions,
                bytes,
            } => {
                transactions.len() < RETAINED_INGRESS_APPLY_ITEMS && *bytes < RETAINED_INGRESS_BYTES
            }
        }
    }

    fn append(&mut self, message: Message) -> RetainedIngressAppend {
        match (self, message) {
            (
                Self::Remote {
                    peer,
                    submissions,
                    responders,
                    bytes,
                },
                Message::SubmitRemoteTx(request),
            ) if request.arguments.peer == *peer
                && submissions.len() < RETAINED_INGRESS_APPLY_ITEMS =>
            {
                let tx_bytes = request.arguments.transaction.payload_bytes();
                let Some(next_bytes) = bytes.checked_add(tx_bytes) else {
                    return RetainedIngressAppend::Lookahead(Message::SubmitRemoteTx(request));
                };
                if next_bytes > RETAINED_INGRESS_BYTES
                    || submissions.try_reserve(1).is_err()
                    || responders.try_reserve(1).is_err()
                {
                    return RetainedIngressAppend::Lookahead(Message::SubmitRemoteTx(request));
                }
                let Request {
                    responder,
                    arguments:
                        RemoteTxSubmission {
                            transaction,
                            declared_cycles,
                            ..
                        },
                } = request;
                submissions.push((transaction, declared_cycles));
                responders.push(responder);
                *bytes = next_bytes;
                RetainedIngressAppend::Consumed
            }
            (
                Self::Proposal {
                    transactions,
                    bytes,
                },
                Message::NotifyTxs(Notify { arguments }),
            ) => {
                if arguments.is_empty() {
                    return RetainedIngressAppend::Consumed;
                }
                let Some(next_count) = transactions.len().checked_add(arguments.transactions.len())
                else {
                    return RetainedIngressAppend::Lookahead(Message::NotifyTxs(Notify::new(
                        arguments,
                    )));
                };
                let Some(next_bytes) = bytes.checked_add(arguments.total_bytes()) else {
                    return RetainedIngressAppend::Lookahead(Message::NotifyTxs(Notify::new(
                        arguments,
                    )));
                };
                if next_count > RETAINED_INGRESS_APPLY_ITEMS
                    || next_bytes > RETAINED_INGRESS_BYTES
                    || transactions
                        .try_reserve(arguments.transactions.len())
                        .is_err()
                {
                    return RetainedIngressAppend::Lookahead(Message::NotifyTxs(Notify::new(
                        arguments,
                    )));
                }
                transactions.extend(arguments.into_transactions());
                *bytes = next_bytes;
                RetainedIngressAppend::Consumed
            }
            (_, message) => RetainedIngressAppend::Lookahead(message),
        }
    }
}

fn spawn_message_handler(
    service: &AuthorityService,
    receiver: &mut mpsc::Receiver<Message>,
    handlers: &mut JoinSet<Result<(), crate::authority::service::AuthorityGenerationInvalidity>>,
    lookahead: &mut Option<Message>,
    message: Message,
) {
    let task_service = service.clone();
    match RetainedIngressBatch::try_new(message) {
        Ok(mut batch) => {
            while batch.can_drain() {
                let Ok(message) = receiver.try_recv() else {
                    break;
                };
                match batch.append(message) {
                    RetainedIngressAppend::Consumed => {}
                    RetainedIngressAppend::Lookahead(message) => {
                        *lookahead = Some(message);
                        break;
                    }
                }
            }
            handlers
                .spawn(async move { process_retained_ingress_batch(task_service, batch).await });
        }
        Err(message) => {
            handlers.spawn(async move { process(task_service, message).await });
        }
    }
}

/// Builder for one unified-authority tx-pool generation.
pub struct TxPoolServiceBuilder {
    bootstrap: AuthorityServiceBootstrap,
    pub(crate) block_assembler: Option<BlockAssembler>,
    pub(crate) txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
    pub(crate) callbacks: Callbacks,
    pub(crate) receiver: mpsc::Receiver<Message>,
    pub(crate) chain_control_receiver: mpsc::Receiver<ChainControl>,
    pub(crate) signal_receiver: CancellationToken,
    pub(crate) handle: Handle,
    pub(crate) verification_control: AuthorityVerificationControl,
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
        let (chain_control_sender, chain_control_receiver) =
            mpsc::channel(CHAIN_CONTROL_CHANNEL_SIZE);
        let process_exit = new_tokio_exit_rx();
        let signal_receiver = service_cancellation_token(&process_exit);
        let (verification_control, verification_command) =
            AuthorityVerificationControl::channel(ChunkCommand::Resume);
        let started = Arc::new(AtomicBool::new(false));
        let administration_gate = AdministrationGate::new();
        let candidate_uncle_payload_limit = usize::try_from(snapshot.consensus().max_block_bytes())
            .map_err(|_| {
                OtherError::new(
                    "consensus maximum block bytes do not fit the host index width".to_owned(),
                )
            })?
            // The protocol bound covers serialized uncle bytes. The residency
            // carrier additionally retains its fixed cached hash.
            .checked_add(Byte32::default().as_slice().len())
            .ok_or_else(|| {
                OtherError::new(
                    "candidate-uncle residency bound does not fit the host index width".to_owned(),
                )
            })?;
        let chain_reorg_payload_limit = ChainReorgPayloadLimit::from_config(&tx_pool_config)
            .ok_or_else(|| {
                OtherError::new(
                    "combined tx-pool reorg residency bound does not fit the host index width"
                        .to_owned(),
                )
            })?;
        let controller = TxPoolController {
            sender,
            chain_control_sender,
            handle: handle.clone(),
            verification_command,
            started: Arc::clone(&started),
            administration_gate,
            chain_reorg_payload_limit,
            candidate_uncle_payload_limit,
            signal: signal_receiver.clone(),
        };

        let block_assembler = block_assembler_config.and_then(|config| {
            BlockAssembler::new(config, Arc::clone(&snapshot))
                .inspect_err(|error| error!("failed to initialize block assembler: {error}"))
                .ok()
        });
        let recent_reject = Self::build_recent_reject(&tx_pool_config).map(Arc::new);
        let (bootstrap, relay_receiver) =
            AuthorityService::prepare(handle, tx_pool_config, snapshot).map_err(|error| {
                OtherError::new(format!(
                    "invalid unified tx-pool authority configuration: {error:?}"
                ))
            })?;

        Ok((
            Self {
                bootstrap,
                block_assembler,
                txs_verify_cache,
                callbacks: Callbacks::new(),
                receiver,
                chain_control_receiver,
                signal_receiver,
                handle: handle.clone(),
                verification_control,
                started,
                fee_estimator,
                recent_reject,
            },
            controller,
            TxVerificationResultReceiver::from_authority(relay_receiver),
        ))
    }

    /// Registers the callback invoked for committed pending transactions.
    pub fn register_pending(&mut self, callback: PendingCallback) {
        self.callbacks.register_pending(callback);
    }

    /// Registers the callback invoked for committed proposed transactions.
    pub fn register_proposed(&mut self, callback: ProposedCallback) {
        self.callbacks.register_proposed(callback);
    }

    /// Registers the callback invoked for committed transaction rejections.
    pub fn register_reject(&mut self, callback: RejectCallback) {
        self.callbacks.register_reject(callback);
    }

    /// Returns the configured recent-rejection index, when enabled.
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

    /// Internal test/benchmark variant that exposes the generation owner.
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
        handle.spawn(async move { self.run(network).await })
    }

    async fn run(self, network: TxPoolNetworkHandle) {
        let config = self.bootstrap.config();
        if config.max_tx_pool_resident_size < config.max_tx_pool_size {
            warn!(
                "max_tx_pool_resident_size ({}) < max_tx_pool_size ({}): clamping the accepted-pool residency budget up to max_tx_pool_size",
                config.max_tx_pool_resident_size, config.max_tx_pool_size
            );
        }
        let handler_limit = match config
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
            bootstrap,
            block_assembler,
            txs_verify_cache,
            callbacks,
            receiver,
            chain_control_receiver,
            signal_receiver,
            handle,
            verification_control,
            started,
            fee_estimator,
            recent_reject,
        } = self;

        let persisted_config = bootstrap.config().clone();
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
                bootstrap,
                block_assembler,
                verification_cache: txs_verify_cache,
                callbacks,
                network,
                persistence_writer: Arc::new(crate::persisted::PersistenceWriter::default()),
                recent_reject,
                fee_estimator,
                chain_control_receiver,
                verification_control,
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
                error!("failed to replay tx-pool persistence: {error:?}");
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
        let mut lookahead = None;
        loop {
            if handlers.len() < handler_limit
                && let Some(message) = lookahead.take()
            {
                spawn_message_handler(
                    &service,
                    &mut receiver,
                    &mut handlers,
                    &mut lookahead,
                    message,
                );
                continue;
            }
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
                        spawn_message_handler(
                            &service,
                            &mut receiver,
                            &mut handlers,
                            &mut lookahead,
                            message,
                        );
                    }
                    None => break,
                }
            }
        }

        started.store(false, Ordering::Release);
        generation.begin_shutdown();
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

#[cfg(test)]
#[path = "tests/builder.rs"]
mod tests;
