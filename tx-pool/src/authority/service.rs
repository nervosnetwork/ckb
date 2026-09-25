//! Runtime resources, shared commit helpers and generation coordination.
//! Worker execution and submission implementations share this single Pool.
pub(crate) use super::model::Error;
pub(crate) use super::notice::Endpoints;
pub(crate) use super::relay::{AuthorityRelaySink as RelaySink, RelayDrain};
use super::{
    budget::ActivePermit,
    chain, ingress,
    jobs::{self, Job, Resolution, Verified},
    membership,
    model::{DependencyKey, Entry, FullReason, Phase, Source},
    notice::{Batch, Class, Effect},
    packing::Selection,
    query,
    queue::{WorkSelection, WorkStage},
    relay::production_authority_relay_mailbox,
    store::{Captured, Plan, ReadSet, Store},
    template::Driver,
    waiting,
};
use crate::verification::ComputeMode;
use crate::{
    block_assembler::{BlockAssembler, BoundedCandidateUncle},
    component::recent_reject::RecentReject,
    error::Reject,
    persisted::{PersistenceSnapshot, PreparedReplay, write_snapshot},
    service::{
        BoundedTransaction, ChainControl, ChainReorgArgs, LocalRemovalCompetingProgress, Request,
        TxVerificationResult, respond,
    },
    util::block_offload,
    verification::non_contextual_verify,
};
use ckb_app_config::TxPoolConfig;
use ckb_async_runtime::Handle;
use ckb_error::AnyError;
use ckb_fee_estimator::FeeEstimator;
use ckb_network::PeerIndex;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        Cycle, EstimateMode, TransactionView,
        error::OutPointError,
        tx_pool::{EntryCompleted, TransactionWithStatus, TxStatus},
    },
    packed::{Byte32, ProposalShortId},
};
use ckb_verification::cache::TxVerificationCache;
#[cfg(feature = "internal")]
use std::collections::BTreeSet;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, RwLock, Semaphore, mpsc, watch},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

mod admission;
mod execution;
mod submission;

#[cfg(test)]
#[path = "tests/execution.rs"]
mod tests;

/// Stop is terminal within the same watch update that excludes later resume.
/// Public control cannot resurrect a generation while its workers are joining.
#[derive(Clone)]
pub(crate) struct VerificationControl(watch::Sender<ChunkCommand>);
impl VerificationControl {
    pub(crate) fn channel(command: ChunkCommand) -> (Self, watch::Receiver<ChunkCommand>) {
        let (sender, receiver) = watch::channel(command);
        (Self(sender), receiver)
    }

    fn set(&self, command: ChunkCommand) -> Result<(), watch::error::SendError<ChunkCommand>> {
        let mut stopped = false;
        self.0.send_if_modified(|current| {
            stopped = *current == ChunkCommand::Stop;
            if stopped || *current == command {
                false
            } else {
                *current = command.clone();
                true
            }
        });
        if stopped || self.0.is_closed() {
            Err(watch::error::SendError(command))
        } else {
            Ok(())
        }
    }

    pub(crate) fn suspend(&self) -> Result<(), watch::error::SendError<ChunkCommand>> {
        self.set(ChunkCommand::Suspend)
    }

    pub(crate) fn resume(&self) -> Result<(), watch::error::SendError<ChunkCommand>> {
        self.set(ChunkCommand::Resume)
    }

    fn stop(&self) {
        self.0.send_replace(ChunkCommand::Stop);
    }

    #[cfg(feature = "internal")]
    pub(crate) fn subscribe(&self) -> watch::Receiver<ChunkCommand> {
        self.0.subscribe()
    }
}

/// Computation admitted through the pool's capacity and pause gate. The owned
/// permit stays with the caller while synchronous or asynchronous work runs.
pub(super) struct ComputePermit {
    _permit: OwnedSemaphorePermit,
    mode: ComputeMode,
}

impl ComputePermit {
    /// Preserve runtime capacity for controls while synchronous work runs.
    pub(super) fn run<T>(&self, operation: impl FnOnce() -> T) -> T {
        self.mode.run(operation)
    }

    pub(super) fn mode(&self) -> ComputeMode {
        self.mode
    }
}

pub(crate) struct Pool {
    pub(crate) config: Arc<TxPoolConfig>,
    pub(super) store: Arc<Store>,
    cache: Arc<RwLock<TxVerificationCache>>,
    pub(crate) verification: VerificationControl,
    commands: watch::Receiver<ChunkCommand>,
    mode: ComputeMode,
    cpu: Arc<Semaphore>,
    reads: Semaphore,
    input_snapshots: query::InputSnapshotCache,
    template: Option<Arc<Driver>>,
    recent: Option<Arc<RecentReject>>,
    estimator: FeeEstimator,
    // Held from before snapshot capture through the blocking disk write.
    save_gate: Arc<AsyncMutex<()>>,
    stopped: CancellationToken,
}
impl Pool {
    pub(crate) fn new(
        config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        handle: &Handle,
        cache: Arc<RwLock<TxVerificationCache>>,
        assembler: Option<BlockAssembler>,
        recent: Option<Arc<RecentReject>>,
        estimator: FeeEstimator,
    ) -> Result<(Arc<Self>, RelaySink, RelayDrain), Error> {
        let store = Store::new(snapshot, &config)?;
        Self::with_store(config, store, handle, cache, assembler, recent, estimator)
    }

    fn with_store(
        config: TxPoolConfig,
        store: Arc<Store>,
        handle: &Handle,
        cache: Arc<RwLock<TxVerificationCache>>,
        assembler: Option<BlockAssembler>,
        recent: Option<Arc<RecentReject>>,
        estimator: FeeEstimator,
    ) -> Result<(Arc<Self>, RelaySink, RelayDrain), Error> {
        let runtime = handle.clone().into_inner();
        if runtime.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
            return Err(Error::Full("multi-thread runtime required".into()));
        }
        let workers = runtime.metrics().num_workers().max(1);
        let permits = store
            .budget
            .limits
            .workers
            .saturating_add(1)
            .min(workers.saturating_sub(1))
            .max(1);
        let mode = if workers == 1 {
            ComputeMode::YieldRuntimeWorker
        } else {
            ComputeMode::Inline
        };
        let (verification, commands) = VerificationControl::channel(ChunkCommand::Resume);
        let (relay, receiver) = production_authority_relay_mailbox(
            crate::service::DEFAULT_CHANNEL_SIZE,
            store.budget.limits.per_job.edges,
        )
        .map_err(|_| Error::Full("relay mailbox configuration".into()))?;
        let drain = RelayDrain::new(receiver, &store);
        let template = assembler.map(|assembler| {
            Driver::new(Arc::clone(&store), assembler, config.max_ancestors_count)
        });
        Ok((
            Arc::new(Self {
                config: Arc::new(config),
                store,
                cache,
                verification,
                commands,
                mode,
                cpu: Arc::new(Semaphore::new(permits)),
                reads: Semaphore::new(2),
                input_snapshots: query::InputSnapshotCache::default(),
                template,
                recent,
                estimator,
                save_gate: Arc::new(AsyncMutex::new(())),
                stopped: CancellationToken::new(),
            }),
            relay,
            drain,
        ))
    }

    pub(crate) fn stop(&self) {
        self.store.stop();
        self.verification.stop();
        self.stopped.cancel();
    }

    pub(crate) fn fault(&self) {
        self.store.fault();
        self.stop();
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.store.is_stopped()
    }

    pub(crate) fn is_faulted(&self) -> bool {
        self.store.is_faulted()
    }

    fn open(&self) -> Result<(), Error> {
        if self.store.is_faulted() {
            Err(Error::Fault("closed generation"))
        } else if self.store.is_stopped() {
            Err(Error::Closed)
        } else {
            Ok(())
        }
    }

    pub(crate) fn close_outbox(&self) {
        self.store.outbox.close();
    }

    pub(crate) fn persistence_eligible(&self) -> bool {
        !self.store.is_faulted() && self.store.outbox.drained()
    }

    #[expect(
        clippy::type_complexity,
        reason = "The caller joins workers first and the sole publisher last."
    )]
    pub(crate) fn start_background(
        self: &Arc<Self>,
        handle: &Handle,
        endpoints: Endpoints,
        chain: mpsc::Receiver<ChainControl>,
    ) -> (JoinSet<Result<(), Error>>, JoinHandle<Result<(), Error>>) {
        let publisher = handle.spawn(Arc::clone(&self.store.outbox).run(endpoints));
        let mut tasks = JoinSet::new();
        let workers = self.store.budget.limits.workers;
        if workers == 0 {
            ckb_logger::warn!(
                "max_tx_verify_workers = 0 disables background transaction resolution and verification; network transactions and test submissions remain queued; synchronous local RPC submissions still work"
            );
        }
        for (stage, count) in [
            (WorkStage::Resolve, workers.min(1)),
            (WorkStage::Verify, workers),
        ] {
            for index in 0..count {
                tasks.spawn(Arc::clone(self).worker(stage, index));
            }
        }
        tasks.spawn(Arc::clone(self).maintain());
        tasks.spawn(Arc::clone(self).chain_loop(chain));
        if let Some(driver) = &self.template {
            tasks.spawn(Arc::clone(driver).run());
            tasks.spawn(Arc::clone(driver).notify());
        }
        (tasks, publisher)
    }

    async fn compute(&self) -> Result<ComputePermit, Error> {
        loop {
            let cpu = tokio::select! {
                permit = Arc::clone(&self.cpu).acquire_owned() => permit.map_err(|_| Error::Closed)?,
                _ = self.stopped.cancelled() => return Err(Error::Closed),
            };
            // Check after acquiring capacity: a block may have arrived while
            // this request was waiting. No queued job or active memory is
            // selected until all computation sources pass this same gate.
            match self.computation_ready() {
                Ok(()) => {
                    return Ok(ComputePermit {
                        _permit: cpu,
                        mode: self.mode,
                    });
                }
                Err(Error::Full(_)) => drop(cpu),
                Err(error) => return Err(error),
            }
            // Subscribe only on suspension; ordinary jobs need no additional
            // command receiver. wait_for observes the current level as well as
            // later updates, including a resume before its first poll.
            let mut commands = self.commands.clone();
            tokio::select! {
                ready = commands.wait_for(|command| *command != ChunkCommand::Suspend) => {
                    ready.map_err(|_| Error::Closed)?;
                },
                _ = self.stopped.cancelled() => return Err(Error::Closed),
            }
        }
    }

    fn computation_ready(&self) -> Result<(), Error> {
        self.open()?;
        match *self.commands.borrow() {
            ChunkCommand::Resume => Ok(()),
            ChunkCommand::Suspend => Err(Error::Full("verification is suspended".into())),
            ChunkCommand::Stop => Err(Error::Closed),
        }
    }

    fn try_direct_capacity(&self) -> Result<(ComputePermit, ActivePermit), Error> {
        self.open()?;
        let cpu = Arc::clone(&self.cpu)
            .try_acquire_owned()
            .map_err(|_| Error::Full("active computation".into()))?;
        self.computation_ready()?;
        let memory = self.store.budget.active(Source::Local)?;
        Ok((
            ComputePermit {
                _permit: cpu,
                mode: self.mode,
            },
            memory,
        ))
    }

    async fn direct_capacity(&self) -> Result<(ComputePermit, ActivePermit), Error> {
        loop {
            let changed = self.store.budget.active_changed.notified();
            tokio::pin!(changed);
            self.open()?;
            let cpu = self.compute().await?;
            match self.store.budget.active(Source::Local) {
                Ok(memory) => return Ok((cpu, memory)),
                Err(Error::Full(_)) => drop(cpu),
                Err(error) => return Err(error),
            }
            tokio::select! {
                _ = &mut changed => {},
                _ = self.stopped.cancelled() => return Err(Error::Closed)
            }
        }
    }

    async fn commit(
        &self,
        mut prepare: impl FnMut() -> Result<Plan, Error>,
    ) -> Result<Option<Arc<Batch>>, Error> {
        self.commit_attempt(|| prepare().and_then(|plan| self.store.apply(plan)))
            .await
    }

    /// Create broadcasts before planning: notify_waiters is observed even before
    /// the first poll, without registering a waiter on the successful path.
    async fn commit_attempt<T>(
        &self,
        mut attempt: impl FnMut() -> Result<T, Error>,
    ) -> Result<T, Error> {
        loop {
            let changed = self.store.changed.notified();
            let room = self.store.outbox.room.notified();
            tokio::pin!(changed, room);
            // These planners and Apply operate on bounded in-memory facts.
            // Disk providers, VM execution and callbacks own their blocking cuts.
            match attempt() {
                Err(Error::Full(FullReason::NoticeOutbox | FullReason::ChainTransition)) => {
                    tokio::select! {
                        _ = &mut changed => {},
                        _ = &mut room => {}
                    }
                }
                result => return result,
            }
        }
    }

    async fn published(&self, batch: Option<Arc<Batch>>) -> Result<(), Error> {
        if let Some(batch) = batch {
            batch.wait(&self.store.outbox).await?;
        }
        Ok(())
    }

    async fn reconcile(&self, command: &ChainReorgArgs) -> Result<(), Error> {
        let _pause = self.store.begin_chain()?;
        let applied = self
            .commit_attempt(|| {
                let (plan, recovery) = match chain::reconcile(&self.store, command, &self.config) {
                    Err(Error::Full(_)) => (chain::recover_bounded(&self.store, command)?, true),
                    result => (result?, false),
                };
                self.store.apply(plan).map(|batch| (batch, recovery))
            })
            .await;
        let (batch, recovered) = match applied {
            Err(Error::Full(_)) => {
                let batch = self
                    .commit(|| chain::recover_bounded(&self.store, command))
                    .await?;
                (batch, true)
            }
            result => result?,
        };
        if recovered {
            ckb_logger::warn!(
                "tx-pool committed bounded chain recovery after resource refusal; some pool transactions may have been discarded"
            );
        }
        // Detached blocks are optional template candidates after the chain
        // commit. The command already bounds their retained payload, and the
        // candidate cache independently bounds its population and backing.
        if let (Some(template), Some(fork)) = (&self.template, command.fork()) {
            let maximum = BoundedCandidateUncle::payload_limit(
                command.snapshot().consensus().max_block_bytes(),
            )
            .unwrap_or(usize::MAX);
            for block in &fork.detached_blocks {
                match BoundedCandidateUncle::try_new(block.as_uncle(), maximum) {
                    Ok(uncle) => template.uncle(uncle),
                    Err(error) => ckb_logger::warn!("detached uncle unavailable: {error:?}"),
                }
            }
        }
        self.published(batch).await
    }

    async fn clear(
        &self,
        snapshot: Option<Arc<Snapshot>>,
        scope: chain::ClearScope,
    ) -> Result<(), Error> {
        let _pause = self.store.begin_chain()?;
        let batch = self
            .commit(|| chain::clear(&self.store, snapshot.clone(), scope))
            .await?;
        self.published(batch).await
    }

    async fn chain_loop(
        self: Arc<Self>,
        mut receiver: mpsc::Receiver<ChainControl>,
    ) -> Result<(), Error> {
        loop {
            let command = tokio::select! {
                command = receiver.recv() => command,
                _ = self.stopped.cancelled() => None
            };
            let Some(command) = command else {
                self.stop();
                receiver.close();
                while receiver.try_recv().is_ok() {}
                return Ok(());
            };
            let (result, responder, _admission) = match command {
                ChainControl::Reconcile(Request {
                    arguments,
                    responder,
                }) => (self.reconcile(&arguments).await, responder, None),
                ChainControl::UpdateIBDState(Request {
                    arguments,
                    responder,
                }) => {
                    block_offload(|| self.estimator.update_ibd_state(arguments));
                    (Ok(()), responder, None)
                }
                ChainControl::ClearPool(command) => {
                    let (
                        admission,
                        Request {
                            arguments,
                            responder,
                        },
                    ) = command.into_parts();
                    (
                        self.clear(Some(arguments), chain::ClearScope::All).await,
                        responder,
                        Some(admission),
                    )
                }
                ChainControl::ClearPipeline(command) => {
                    let (admission, Request { responder, .. }) = command.into_parts();
                    (
                        self.clear(None, chain::ClearScope::Unaccepted).await,
                        responder,
                        Some(admission),
                    )
                }
            };
            match result {
                Ok(()) => respond(responder, (), "chain_control"),
                Err(Error::Closed) => drop(responder),
                Err(error) => {
                    drop(responder);
                    return Err(error);
                }
            }
        }
    }

    async fn maintain(self: Arc<Self>) -> Result<(), Error> {
        let mut expiry = tokio::time::interval(Duration::from_secs(1));
        expiry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut wake_cursor = None;
        loop {
            let changed = self.store.changed.notified();
            tokio::pin!(changed);
            if self.store.is_faulted() {
                return Err(Error::Fault("maintenance generation"));
            }
            if self.store.is_stopped() {
                return Ok(());
            }
            // The empty wake check is memory-only; waiting::wake offloads its
            // actual snapshot lookups at the provider boundary.
            let progress = match waiting::wake(&self.store, &mut wake_cursor) {
                Ok(Some(plan)) => match self.store.apply(plan) {
                    Ok(_) | Err(Error::Stale) => true,
                    Err(Error::Full(_)) => false,
                    Err(error) => return Err(error),
                },
                Ok(None) => false,
                Err(Error::Stale) => true,
                Err(error) => return Err(error),
            };
            // Due expiry gets a turn after each wake page. A progress turn must
            // yield; its own ready change notification must not bypass that.
            tokio::select! {
                biased;
                _ = self.stopped.cancelled() => return Ok(()),
                _ = expiry.tick() => {
                    self.store.budget.publish_metrics();
                    self.store.outbox.publish_metrics();
                    let age = u64::from(self.config.expiry_hours)
                        .checked_mul(3_600_000)
                        .ok_or(Error::Fault("expiry configuration"))?;
                    let before = ckb_systemtime::unix_time_as_millis().saturating_sub(age);
                    for entry in self.store.expired(Instant::now(), before, 32) {
                        let result = self.commit(|| {
                            if let Some(value) = entry.accepted() {
                                membership::removal(
                                    &self.store,
                                    &entry,
                                    &self.config,
                                    Some(Reject::Expiry(value.timestamp))
                                )
                            } else {
                                let (view, _) = self.store.snapshot();
                                let mut plan = Plan::new(view, Class::Remote, Default::default());
                                plan.edit(
                                    Some(Arc::clone(&entry)),
                                    None,
                                    Some(Effect::relay(TxVerificationResult::Reject {
                                        tx_hash: entry.hash(),
                                    })),
                                )?;
                                Ok(plan)
                            }
                        }).await;
                        match result {
                            Ok(_) | Err(Error::Stale) | Err(Error::Full(_)) => {},
                            Err(error) => return Err(error)
                        }
                    }
                    // Cleanup can itself overrun the interval. Return to the
                    // executor before another already-due maintenance turn.
                    tokio::task::yield_now().await;
                },
                _ = tokio::task::yield_now(), if progress => {},
                _ = &mut changed, if !progress => {},
            }
        }
    }

    async fn read<T>(&self, query: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        let _permit = tokio::select! {
            permit = self.reads.acquire() => permit.map_err(|_| Error::Closed)?,
            _ = self.stopped.cancelled() => return Err(Error::Closed)
        };
        self.open()?;
        block_offload(query)
    }

    pub(crate) async fn pool_info(&self) -> Result<ckb_types::core::tx_pool::TxPoolInfo, Error> {
        self.read(|| query::summary(&self.store, &self.config))
            .await
    }

    pub(crate) async fn pool_ids(&self) -> Result<ckb_types::core::tx_pool::TxPoolIds, Error> {
        self.read(|| Ok(query::ids(&self.store))).await
    }

    pub(crate) async fn input_snapshot(&self) -> Result<crate::TxPoolInputSnapshot, Error> {
        self.read(|| self.input_snapshots.capture(&self.store))
            .await
    }

    pub(crate) async fn entry_info(
        &self,
    ) -> Result<ckb_types::core::tx_pool::TxPoolEntryInfo, Error> {
        self.read(|| query::entry_info(&self.store, &self.config))
            .await
    }

    pub(crate) async fn detail(
        &self,
        hash: &Byte32,
    ) -> Result<ckb_types::core::tx_pool::PoolTxDetailInfo, Error> {
        self.read(|| query::detail(&self.store, hash, &self.config))
            .await
    }

    pub(crate) fn live_cell(
        &self,
        point: &ckb_types::packed::OutPoint,
        with_data: bool,
    ) -> Result<ckb_types::core::cell::CellStatus, AnyError> {
        self.open()?;
        Ok(block_offload(|| {
            query::live_cell(&self.store, point, with_data)
        }))
    }

    pub(crate) fn fresh_proposals(
        &self,
        ids: Vec<ProposalShortId>,
    ) -> Result<Vec<ProposalShortId>, Error> {
        self.open()?;
        Ok(self.store.filter_fresh_proposals(ids))
    }

    pub(crate) fn compact_transactions(
        &self,
        ids: &[ProposalShortId],
    ) -> Result<std::collections::HashMap<ProposalShortId, TransactionView>, Error> {
        self.open()?;
        Ok(block_offload(|| {
            query::compact_transactions(&self.store, ids)
        }))
    }

    pub(crate) fn accepted_with_cycles(
        &self,
        ids: &[Byte32],
    ) -> Result<query::AcceptedTransactionsWithCycles, Error> {
        self.open()?;
        Ok(query::accepted_with_cycles(&self.store, ids))
    }

    pub(crate) async fn block_template(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<ckb_jsonrpc_types::BlockTemplate, Error> {
        self.template
            .as_ref()
            .ok_or(Error::Full("block assembler disabled".into()))?
            .read(deadline)
            .await
    }

    pub(crate) fn uncle(&self, uncle: BoundedCandidateUncle) {
        if let Some(template) = &self.template {
            template.uncle(uncle);
        }
    }

    fn recent_reject(&self, hash: &Byte32) -> Result<Option<String>, AnyError> {
        if let Some(reject) = self.store.outbox.pending_reject(hash) {
            return Ok(Some(reject));
        }
        self.recent
            .as_ref()
            .map(|recent| recent.get(hash))
            .transpose()
            .map(Option::flatten)
    }

    pub(crate) fn transaction_status(
        &self,
        hash: &Byte32,
    ) -> Result<(TxStatus, Option<Cycle>), AnyError> {
        self.open()?;
        if let Some(result) = query::transaction_status(&self.store, hash) {
            return Ok(result);
        }
        Ok(self
            .recent_reject(hash)?
            .map_or((TxStatus::Unknown, None), |reason| {
                (TxStatus::Rejected(reason), None)
            }))
    }

    pub(crate) async fn transaction(
        &self,
        hash: &Byte32,
    ) -> Result<TransactionWithStatus, AnyError> {
        if let Some(result) = self
            .read(|| query::transaction(&self.store, hash, &self.config))
            .await?
        {
            return Ok(result);
        }
        Ok(self.recent_reject(hash)?.map_or_else(
            TransactionWithStatus::with_unknown,
            TransactionWithStatus::with_rejected,
        ))
    }

    pub(crate) fn recent_count(&self) -> Option<u64> {
        self.recent
            .as_ref()
            .map(|recent| recent.get_estimate_total_keys_num())
    }

    pub(crate) async fn estimate_fee(
        &self,
        mode: EstimateMode,
        fallback: bool,
    ) -> Result<ckb_types::core::FeeRate, AnyError> {
        let estimated = self
            .read(|| {
                Ok(self
                    .estimator
                    .estimate_fee_rate_with_samples(mode, || query::fee_samples(&self.store)))
            })
            .await?;
        match estimated {
            Ok(rate) => Ok(rate),
            Err(error) if !fallback => Err(error.into()),
            Err(_) => {
                let target = FeeEstimator::target_blocks_for_estimate_mode(mode);
                Ok(self
                    .read(|| query::estimate_fee(&self.store, &self.config, target))
                    .await?)
            }
        }
    }

    pub(crate) async fn save(&self) -> Result<(), AnyError> {
        let save_guard = Arc::clone(&self.save_gate).lock_owned().await;
        if self.store.is_faulted() {
            return Err(Error::Fault("persistence generation").into());
        }
        let saved = block_offload(|| {
            let Captured {
                owners, snapshot, ..
            } = self.store.capture_all();
            // Capture held every owner read guard, so a mutation that faults
            // before that cut is visible here. Later mutations cannot alter
            // this immutable, already-complete snapshot.
            if self.store.is_faulted() {
                return Err(Error::Fault("persistence generation"));
            }
            let accepted = Selection::new(&owners, &snapshot, self.config.max_ancestors_count)?
                .replay_transactions()?;
            let recovery = owners
                .iter()
                .filter(|entry| {
                    entry.accepted().is_none() && matches!(entry.source, Source::Recovery)
                })
                .map(|entry| entry.transaction.as_ref().clone())
                .collect();
            Ok(PersistenceSnapshot { accepted, recovery })
        })?;
        let base = self.config.persisted_data.clone();
        tokio::task::spawn_blocking(move || {
            // Caller cancellation must not permit another full capture while writing.
            let _save_guard = save_guard;
            write_snapshot(&base, saved)
        })
        .await??;
        Ok(())
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Loaded and rejected counts partition the bounded input vector."
    )]
    pub(crate) async fn replay(
        &self,
        transactions: PreparedReplay,
    ) -> Result<(usize, usize), AnyError> {
        let mut loaded = 0;
        let mut rejected = 0;
        for transaction in transactions {
            let transaction = match BoundedTransaction::try_new(transaction) {
                Ok(transaction) => transaction,
                Err(_) => {
                    rejected += 1;
                    continue;
                }
            };
            match self.submit_local(transaction).await? {
                Ok(_) => loaded += 1,
                Err(_) => rejected += 1,
            }
        }
        Ok((loaded, rejected))
    }

    #[cfg(feature = "internal")]
    pub(crate) async fn package_transactions(
        &self,
        bytes: Option<u64>,
    ) -> Result<Vec<crate::TxEntry>, Error> {
        self.read(|| {
            let Captured {
                snapshot, owners, ..
            } = self.store.capture_accepted();
            let selection = Selection::new(&owners, &snapshot, self.config.max_ancestors_count)?;
            selection
                .pack_transactions(super::packing::TemplatePackingLimits::new(
                    bytes.unwrap_or_else(|| snapshot.consensus().max_block_bytes()) as usize,
                    snapshot.consensus().max_block_cycles(),
                ))
                .map_err(Error::from)
        })
        .await
    }
}
