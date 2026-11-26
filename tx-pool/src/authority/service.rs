//! Runtime resources, shared commit helpers and generation coordination.
//! Worker execution and submission implementations share this single Pool.
pub(crate) use super::model::Error;
pub(crate) use super::notice::{Batch, Endpoints};
pub(crate) use super::relay::{AuthorityRelaySink as RelaySink, RelayDrain};
#[cfg(feature = "internal")]
pub use super::store::VerifyObserverHook;
use super::{
    budget::ActivePermit,
    chain, ingress,
    jobs::{self, Job, Resolution, Verified},
    membership,
    model::{Entry, FullReason, Phase, Premises, Source},
    notice::{Class, Effect},
    packing::Selection,
    query,
    queue::{WorkSelection, WorkStage},
    relay::production_authority_relay_mailbox,
    store::{Captured, Store},
    template::Driver,
    waiting,
};
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
use ckb_util::parking_lot::Mutex;
use ckb_verification::cache::TxVerificationCache;
pub(in crate::authority) use compute::CapacitySignal;
pub(crate) use compute::{ComputePool, LocalStep, LocalTask, PushErrorKind};
#[cfg(feature = "internal")]
use std::collections::BTreeSet;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
pub(crate) use submission::LocalFastOutcome;
use tokio::{
    sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, RwLock, Semaphore, mpsc, watch},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

mod compute;
mod execution;
mod submission;

#[cfg(test)]
#[path = "tests/execution.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PauseScope {
    External,
    Background,
    BlockScripts,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PauseFlags {
    external: bool,
    block_background: bool,
    block_script: bool,
    stopped: bool,
}

impl PauseFlags {
    fn command(self, paused: bool) -> ChunkCommand {
        if self.stopped {
            ChunkCommand::Stop
        } else if paused {
            ChunkCommand::Suspend
        } else {
            ChunkCommand::Resume
        }
    }

    fn background(self) -> ChunkCommand {
        self.command(self.external || self.block_background || self.block_script)
    }

    fn local(self) -> ChunkCommand {
        self.command(self.external || self.block_script)
    }
}

struct VerificationControlInner {
    flags: Mutex<PauseFlags>,
    background: watch::Sender<ChunkCommand>,
    local: watch::Sender<ChunkCommand>,
}

/// Scoped pause control separating background queue workers from local RPC
/// submission (`External` and `BlockScripts` pause both; `Background` pauses
/// only queue workers; `stop()` is terminal).
#[derive(Clone)]
pub(crate) struct VerificationControl(Arc<VerificationControlInner>);

impl VerificationControl {
    pub(crate) fn channel(
        command: ChunkCommand,
    ) -> (
        Self,
        watch::Receiver<ChunkCommand>,
        watch::Receiver<ChunkCommand>,
    ) {
        let initial = PauseFlags {
            external: matches!(command, ChunkCommand::Suspend),
            stopped: matches!(command, ChunkCommand::Stop),
            ..PauseFlags::default()
        };
        let (background, bg_rx) = watch::channel(initial.background());
        let (local, local_rx) = watch::channel(initial.local());
        (
            Self(Arc::new(VerificationControlInner {
                flags: Mutex::new(initial),
                background,
                local,
            })),
            bg_rx,
            local_rx,
        )
    }

    pub(crate) fn set_paused(
        &self,
        scope: PauseScope,
        paused: bool,
    ) -> Result<(), watch::error::SendError<ChunkCommand>> {
        let mut flags = self.0.flags.lock();
        if flags.stopped || self.0.background.is_closed() {
            return Err(watch::error::SendError(if paused {
                ChunkCommand::Suspend
            } else {
                ChunkCommand::Resume
            }));
        }
        match scope {
            PauseScope::External => flags.external = paused,
            PauseScope::Background => flags.block_background = paused,
            PauseScope::BlockScripts => flags.block_script = paused,
        }
        for (sender, next) in [
            (&self.0.background, flags.background()),
            (&self.0.local, flags.local()),
        ] {
            sender.send_if_modified(|current| {
                let changed = *current != next;
                *current = next;
                changed
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn suspend(&self) -> Result<(), watch::error::SendError<ChunkCommand>> {
        self.set_paused(PauseScope::External, true)
    }

    #[cfg(test)]
    pub(crate) fn resume(&self) -> Result<(), watch::error::SendError<ChunkCommand>> {
        self.set_paused(PauseScope::External, false)
    }

    fn stop(&self) {
        let mut flags = self.0.flags.lock();
        flags.stopped = true;
        self.0.background.send_replace(ChunkCommand::Stop);
        self.0.local.send_replace(ChunkCommand::Stop);
    }

    #[cfg(feature = "internal")]
    pub(crate) fn subscribe(&self) -> watch::Receiver<ChunkCommand> {
        self.0.background.subscribe()
    }

    #[cfg(feature = "internal")]
    pub(crate) fn subscribe_local(&self) -> watch::Receiver<ChunkCommand> {
        self.0.local.subscribe()
    }
}

/// Computation admitted through the pool's capacity and pause gate. The owned
/// permit stays with the caller while synchronous or asynchronous work runs.
pub(super) struct ComputePermit {
    permit: Option<OwnedSemaphorePermit>,
    signal: Option<compute::CapacitySignal>,
}

impl ComputePermit {
    fn new(permit: OwnedSemaphorePermit, signal: compute::CapacitySignal) -> Self {
        Self {
            permit: Some(permit),
            signal: Some(signal),
        }
    }
}

impl Drop for ComputePermit {
    fn drop(&mut self) {
        // Release the semaphore permit before notifying capacity waiters so an
        // awakened worker immediately observes the freed slot.
        drop(self.permit.take());
        if let Some(signal) = self.signal.take() {
            signal.notify();
        }
    }
}

pub(super) enum FastCapacityError {
    Contended,
    Suspended,
    Other(Error),
}

#[cfg(test)]
impl ComputePermit {
    /// Preserve runtime capacity for controls while synchronous work runs.
    pub(super) fn run<T>(&self, operation: impl FnOnce() -> T) -> T {
        block_offload(operation)
    }
}

pub(crate) struct Pool {
    pub(crate) config: Arc<TxPoolConfig>,
    pub(super) store: Arc<Store>,
    cache: Arc<RwLock<TxVerificationCache>>,
    pub(crate) verification: VerificationControl,
    commands: watch::Receiver<ChunkCommand>,
    local_commands: watch::Receiver<ChunkCommand>,
    direct: Arc<Semaphore>,
    cpu: Arc<Semaphore>,
    local_headroom_cpu: Option<Arc<Semaphore>>,
    compute: compute::ComputePool,
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
    fn runtime_workers(handle: &Handle) -> Result<usize, Error> {
        let runtime = handle.clone().into_inner();
        if runtime.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
            return Err(Error::Full("multi-thread runtime required".into()));
        }
        Ok(runtime.metrics().num_workers().max(1))
    }

    pub(crate) fn new(
        config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        handle: &Handle,
        cache: Arc<RwLock<TxVerificationCache>>,
        assembler: Option<BlockAssembler>,
        recent: Option<Arc<RecentReject>>,
        estimator: FeeEstimator,
    ) -> Result<(Arc<Self>, RelaySink, RelayDrain), Error> {
        let runtime_workers = Self::runtime_workers(handle)?;
        let store = Store::with_runtime_workers(snapshot, &config, Some(runtime_workers))?;
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
        let runtime_workers = Self::runtime_workers(handle)?;
        let plan = super::budget::ConcurrencyPlan::derive(
            store.budget.limits.workers,
            Some(runtime_workers),
        )?;
        let permits = plan.bg_compute;
        let local_headroom = plan
            .local_headroom
            .min(store.budget.limits.active_jobs.saturating_sub(permits));
        let total_compute = permits.saturating_add(local_headroom);
        // Bound concurrent local submissions entering `direct_capacity()` to at
        // most `min(total_compute + 1, active_jobs)` in strict FIFO order so
        // callers never bounce off `budget.active_jobs` and re-enter the back
        // of the compute queue.
        let direct_slots = total_compute
            .saturating_add(1)
            .min(store.budget.limits.active_jobs)
            .max(1);
        let (verification, commands, local_commands) =
            VerificationControl::channel(ChunkCommand::Resume);
        let (relay, receiver) = production_authority_relay_mailbox(
            crate::service::DEFAULT_CHANNEL_SIZE,
            store.budget.limits.per_job.edges,
        )
        .map_err(|_| Error::Full("relay mailbox configuration".into()))?;
        let drain = RelayDrain::new(receiver, &store);
        let template = assembler.map(|assembler| {
            Driver::new(
                Arc::clone(&store),
                assembler,
                Arc::clone(&cache),
                config.max_ancestors_count,
            )
        });
        let compute = compute::ComputePool::new(total_compute);
        store.budget.attach_capacity_signal(compute.signal());
        Ok((
            Arc::new(Self {
                config: Arc::new(config),
                store,
                cache,
                verification,
                commands,
                local_commands,
                direct: Arc::new(Semaphore::new(direct_slots)),
                cpu: Arc::new(Semaphore::new(permits)),
                local_headroom_cpu: (local_headroom > 0)
                    .then(|| Arc::new(Semaphore::new(local_headroom))),
                compute,
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

    #[cfg(feature = "internal")]
    pub(crate) fn set_verify_observer(&self, hook: Option<super::store::VerifyObserverHook>) {
        *self.store.verify_observer.lock() = hook;
    }

    pub(crate) fn stop(&self) {
        self.store.stop();
        self.verification.stop();
        self.stopped.cancel();
        self.compute.notify_capacity();
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

    #[cfg(test)]
    pub(crate) fn set_before_commit_hook(&self, hook: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.store.before_commit.lock() = hook;
    }

    #[cfg(test)]
    pub(crate) fn set_compute_spawn_hook(&self, hook: Option<compute::SpawnHook>) {
        self.compute.set_spawn_hook(hook);
    }

    #[cfg(test)]
    pub(crate) fn owner_hashes(&self) -> std::collections::BTreeSet<Byte32> {
        self.store
            .capture_all()
            .owners
            .into_iter()
            .map(|entry| entry.hash())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn lock_state_write_for_test(&self) -> impl Drop + '_ {
        self.store.state_write()
    }

    #[cfg(test)]
    pub(crate) fn assert_state_invariants_for_test(&self) {
        use super::eviction::EvictionRank;
        use super::model::Status;
        let state = self.store.state_read();
        assert_eq!(
            state.owners.len(),
            state.proposals.len(),
            "owners and proposals index cardinality must match"
        );
        let accepted_entries: Vec<&Arc<Entry>> = state
            .owners
            .values()
            .filter(|entry| entry.accepted().is_some())
            .collect();
        assert_eq!(
            state.usage.accepted.items,
            accepted_entries.len(),
            "accepted usage item count must match accepted owners"
        );
        assert_eq!(
            state.eviction_bounds.len(),
            accepted_entries.len(),
            "eviction_bounds must track every accepted owner"
        );
        let mut expected_gap = 0usize;
        let mut expected_proposed = 0usize;
        for entry in &accepted_entries {
            let bound = EvictionRank::lower_bound(entry).expect("accepted owner has bound");
            assert!(
                state.eviction_bounds.contains(&bound),
                "eviction_bounds missing accepted owner"
            );
            let status = entry.accepted().unwrap().status(&state.snapshot);
            expected_gap += usize::from(status == Status::Gap);
            expected_proposed += usize::from(status == Status::Proposed);
        }
        assert_eq!(state.gap_count, expected_gap, "gap_count mismatch");
        assert_eq!(
            state.proposed_count, expected_proposed,
            "proposed_count mismatch"
        );
        let recomputed = membership::aggregates(
            accepted_entries.into_iter(),
            self.config.max_ancestors_count,
        )
        .expect("accepted graph must be acyclic and within ancestor limit");
        for (hash, totals) in &recomputed {
            let live = state
                .descendant_aggregate(hash)
                .expect("live descendant aggregate exists");
            assert_eq!(
                live, totals.descendants,
                "incremental descendant aggregate diverged from exact recomputation"
            );
            assert_eq!(
                state.descendants.contains_key(hash),
                totals.descendants.count > 1,
                "leaves must be omitted from descendants map"
            );
        }
        assert_eq!(
            state.descendants.len(),
            recomputed
                .values()
                .filter(|t| t.descendants.count > 1)
                .count(),
            "descendants map must contain no stale entries"
        );
    }

    pub(crate) fn open(&self) -> Result<(), Error> {
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
        signal: &CancellationToken,
    ) -> (JoinSet<Result<(), Error>>, JoinHandle<Result<(), Error>>) {
        let publisher =
            Arc::clone(&self.store.outbox).spawn_dedicated(handle, endpoints, signal.clone());
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
            driver.start(&mut tasks);
        }
        (tasks, publisher)
    }

    async fn compute_with(
        &self,
        channel: &watch::Receiver<ChunkCommand>,
    ) -> Result<ComputePermit, Error> {
        loop {
            let cpu = tokio::select! {
                permit = Arc::clone(&self.cpu).acquire_owned() => permit.map_err(|_| Error::Closed)?,
                _ = self.stopped.cancelled() => return Err(Error::Closed),
            };
            match self.channel_ready(channel) {
                Ok(()) => return Ok(ComputePermit::new(cpu, self.compute.signal())),
                Err(Error::Full(_)) => drop(cpu),
                Err(error) => return Err(error),
            }
            let mut commands = channel.clone();
            tokio::select! {
                ready = commands.wait_for(|command| *command != ChunkCommand::Suspend) => {
                    ready.map_err(|_| Error::Closed)?;
                },
                _ = self.stopped.cancelled() => return Err(Error::Closed),
            }
        }
    }

    fn channel_ready(&self, channel: &watch::Receiver<ChunkCommand>) -> Result<(), Error> {
        self.open()?;
        match *channel.borrow() {
            ChunkCommand::Resume => Ok(()),
            ChunkCommand::Suspend => Err(Error::Full("verification is suspended".into())),
            ChunkCommand::Stop => Err(Error::Closed),
        }
    }

    async fn compute_background(&self) -> Result<ComputePermit, Error> {
        self.compute_with(&self.commands).await
    }

    async fn compute_local(&self) -> Result<ComputePermit, Error> {
        if let Some(headroom) = &self.local_headroom_cpu
            && self.channel_ready(&self.local_commands).is_ok()
            && let Ok(permit) = Arc::clone(headroom).try_acquire_owned()
        {
            match self.channel_ready(&self.local_commands) {
                Ok(()) => return Ok(ComputePermit::new(permit, self.compute.signal())),
                Err(Error::Full(_)) => drop(permit),
                Err(error) => return Err(error),
            }
        }
        self.compute_with(&self.local_commands).await
    }

    fn try_direct_capacity(&self) -> Result<(ComputePermit, ActivePermit), Error> {
        self.open()?;
        let cpu = Arc::clone(&self.cpu)
            .try_acquire_owned()
            .map_err(|_| Error::Full("active computation".into()))?;
        self.channel_ready(&self.local_commands)?;
        let memory = self.store.budget.active(Source::Local)?;
        Ok((ComputePermit::new(cpu, self.compute.signal()), memory))
    }

    fn try_fast_direct_capacity(&self) -> Result<(ComputePermit, ActivePermit), FastCapacityError> {
        self.open().map_err(FastCapacityError::Other)?;
        match self.channel_ready(&self.local_commands) {
            Ok(()) => {}
            Err(Error::Full(_)) => return Err(FastCapacityError::Suspended),
            Err(error) => return Err(FastCapacityError::Other(error)),
        }
        let cpu = match Arc::clone(&self.cpu).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => match &self.local_headroom_cpu {
                Some(headroom) => match Arc::clone(headroom).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => return Err(FastCapacityError::Contended),
                },
                None => return Err(FastCapacityError::Contended),
            },
        };
        let memory = match self.store.budget.active(Source::Local) {
            Ok(memory) => memory,
            Err(Error::Full(_)) => return Err(FastCapacityError::Contended),
            Err(error) => return Err(FastCapacityError::Other(error)),
        };
        Ok((ComputePermit::new(cpu, self.compute.signal()), memory))
    }

    async fn acquire_direct_slot(&self) -> Result<OwnedSemaphorePermit, Error> {
        self.open()?;
        let permit = tokio::select! {
            permit = Arc::clone(&self.direct).acquire_owned() => permit.map_err(|_| Error::Closed)?,
            _ = self.stopped.cancelled() => return Err(Error::Closed),
        };
        self.open()?;
        Ok(permit)
    }

    async fn direct_capacity(&self) -> Result<(ComputePermit, ActivePermit), Error> {
        loop {
            let changed = self.store.budget.active_changed.notified();
            tokio::pin!(changed);
            self.open()?;
            let cpu = self.compute_local().await?;
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

    /// Retry only when the bounded notice outbox is temporarily saturated.
    async fn commit_attempt<T>(
        &self,
        mut attempt: impl FnMut() -> Result<T, Error>,
    ) -> Result<T, Error> {
        match attempt() {
            Err(Error::Full(FullReason::NoticeOutbox)) => {}
            result => return result,
        }
        loop {
            let changed = self.store.changed.notified();
            let room = self.store.outbox.room.notified();
            tokio::pin!(changed, room);
            match attempt() {
                Err(Error::Full(FullReason::NoticeOutbox)) => {
                    tokio::select! {
                        _ = &mut changed => {},
                        _ = &mut room => {}
                    }
                }
                result => return result,
            }
        }
    }

    pub(crate) fn push_local_fast(
        &self,
        task: LocalTask,
    ) -> Result<(), (PushErrorKind, LocalTask)> {
        self.compute.push_local(task)
    }

    pub(crate) async fn wait_batch(&self, batch: Arc<Batch>) -> Result<(), Error> {
        batch.wait(&self.store.outbox).await
    }

    async fn published(&self, batch: Option<Arc<Batch>>) -> Result<(), Error> {
        if let Some(batch) = batch {
            batch.wait(&self.store.outbox).await?;
        }
        Ok(())
    }

    async fn settle_publish(&self, batch: Option<Arc<Batch>>, assist: bool) -> Result<(), Error> {
        // Assist publication directly when local direct slots are not saturated
        // (eliminating a wakeup hop for serial and low-concurrency RPCs). Under
        // saturation, delegate to the single dedicated publisher task so RPC
        // worker threads do not churn `block_in_place` / OS threads.
        if assist && self.direct.available_permits() > 0 {
            self.publish_assisted(batch).await
        } else {
            self.published(batch).await
        }
    }

    /// Wait for `batch` to publish while cooperatively lending this task's
    /// thread to `outbox.try_publish()` whenever the exclusive publisher lease
    /// is idle, cutting an extra async wakeup hop on local RPC and reorg paths.
    async fn publish_assisted(&self, batch: Option<Arc<Batch>>) -> Result<(), Error> {
        let Some(batch) = batch else {
            return Ok(());
        };
        let wait = batch.wait(&self.store.outbox);
        tokio::pin!(wait);
        std::future::poll_fn(|context| {
            use std::task::Poll;
            let result = wait.as_mut().poll(context);
            if result.is_pending() {
                match self.store.outbox.try_publish() {
                    Ok(true) => return wait.as_mut().poll(context),
                    Ok(false) => {}
                    Err(error) => return Poll::Ready(Err(error)),
                }
            }
            result
        })
        .await
    }

    async fn reconcile(&self, command: &ChainReorgArgs) -> Result<(), Error> {
        let (batch, recovered) = self
            .commit_attempt(
                || match chain::commit_reconcile(&self.store, command, &self.config) {
                    Err(Error::Full(FullReason::NoticeOutbox)) => {
                        Err(Error::Full(FullReason::NoticeOutbox))
                    }
                    Err(Error::Full(_)) => {
                        chain::commit_recover_bounded(&self.store, command).map(|b| (b, true))
                    }
                    result => result.map(|b| (b, false)),
                },
            )
            .await?;
        if recovered {
            ckb_logger::warn!(
                "tx-pool committed bounded chain recovery after resource refusal; some pool transactions may have been discarded"
            );
        }
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
        self.publish_assisted(batch).await
    }

    async fn clear(
        &self,
        snapshot: Option<Arc<Snapshot>>,
        scope: chain::ClearScope,
    ) -> Result<(), Error> {
        let batch = self
            .commit_attempt(|| chain::commit_clear(&self.store, snapshot.clone(), scope))
            .await?;
        self.publish_assisted(batch).await
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
            let room = self.store.outbox.room.notified();
            tokio::pin!(changed, room);
            if self.store.is_faulted() {
                return Err(Error::Fault("maintenance generation"));
            }
            if self.store.is_stopped() {
                return Ok(());
            }
            let wake_result = match waiting::commit_wake(&self.store, &mut wake_cursor) {
                Ok(Some(_)) | Err(Error::Stale) => Ok(true),
                Ok(None) => Ok(false),
                Err(Error::Full(reason)) => Err(reason),
                Err(error) => return Err(error),
            };
            tokio::select! {
                biased;
                _ = self.stopped.cancelled() => return Ok(()),
                _ = expiry.tick() => {
                    self.store.publish_entry_metrics();
                    self.store.publish_budget_metrics();
                    self.store.outbox.publish_metrics();
                    let age = u64::from(self.config.expiry_hours)
                        .checked_mul(3_600_000)
                        .ok_or(Error::Fault("expiry configuration"))?;
                    let before = ckb_systemtime::unix_time_as_millis().saturating_sub(age);
                    for entry in self.store.expired(Instant::now(), before, 32) {
                        let result = self.commit_attempt(|| {
                            if let Some(value) = entry.accepted() {
                                membership::commit_removal(
                                    &self.store,
                                    &entry,
                                    &self.config,
                                    Some(Reject::Expiry(value.timestamp)),
                                )
                            } else {
                                let hash = entry.hash_bytes();
                                let (_, batch) = self.store.commit_with(|tx| {
                                    if !tx
                                        .state
                                        .owner(&hash)
                                        .is_some_and(|current| Arc::ptr_eq(current, &entry))
                                    {
                                        return Err(Error::Stale);
                                    }
                                    tx.commit_single(
                                        hash,
                                        Some(Arc::clone(&entry)),
                                        None,
                                        vec![Effect::relay(TxVerificationResult::Reject {
                                            tx_hash: entry.hash(),
                                        })],
                                        Class::Remote,
                                    )
                                })?;
                                Ok(batch)
                            }
                        }).await;
                        match result {
                            Ok(_) | Err(Error::Stale) | Err(Error::Full(_)) => {},
                            Err(error) => return Err(error),
                        }
                    }
                    tokio::task::yield_now().await;
                },
                _ = tokio::task::yield_now(), if wake_result == Ok(true) => {},
                _ = &mut changed, if wake_result != Ok(true) => {},
                _ = &mut room, if wake_result == Err(FullReason::NoticeOutbox) => {},
            }
        }
    }

    async fn read_permit(&self) -> Result<tokio::sync::SemaphorePermit<'_>, Error> {
        let _permit = tokio::select! {
            permit = self.reads.acquire() => permit.map_err(|_| Error::Closed)?,
            _ = self.stopped.cancelled() => return Err(Error::Closed)
        };
        self.open()?;
        Ok(_permit)
    }

    #[cfg(any(test, feature = "internal"))]
    async fn read<T>(&self, query: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        let _permit = self.read_permit().await?;
        block_offload(query)
    }

    /// Run a bounded read query inline when a read slot is immediately free,
    /// falling back to waiting on `self.reads` without duplicating sync/async paths.
    fn read_blocking<T>(
        &self,
        handle: &Handle,
        query: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        let _permit = match self.reads.try_acquire() {
            Ok(permit) => {
                self.open()?;
                permit
            }
            Err(_) => block_offload(|| handle.block_on(self.read_permit()))?,
        };
        block_offload(query)
    }

    pub(crate) fn pool_info_sync(&self) -> Result<ckb_types::core::tx_pool::TxPoolInfo, Error> {
        self.open()?;
        query::summary(&self.store, &self.config)
    }

    #[cfg(test)]
    pub(crate) async fn pool_info(&self) -> Result<ckb_types::core::tx_pool::TxPoolInfo, Error> {
        self.read(|| query::summary(&self.store, &self.config))
            .await
    }

    pub(crate) fn pool_ids_blocking(
        &self,
        handle: &Handle,
    ) -> Result<ckb_types::core::tx_pool::TxPoolIds, Error> {
        self.read_blocking(handle, || Ok(query::ids(&self.store)))
    }

    #[cfg(test)]
    pub(crate) async fn pool_ids(&self) -> Result<ckb_types::core::tx_pool::TxPoolIds, Error> {
        self.read(|| Ok(query::ids(&self.store))).await
    }

    pub(crate) fn input_snapshot_blocking(
        &self,
        handle: &Handle,
    ) -> Result<crate::TxPoolInputSnapshot, Error> {
        self.read_blocking(handle, || self.input_snapshots.capture(&self.store))
    }

    pub(crate) fn entry_info_blocking(
        &self,
        handle: &Handle,
    ) -> Result<ckb_types::core::tx_pool::TxPoolEntryInfo, Error> {
        self.read_blocking(handle, || query::entry_info(&self.store, &self.config))
    }

    pub(crate) fn detail_blocking(
        &self,
        handle: &Handle,
        hash: &Byte32,
    ) -> Result<ckb_types::core::tx_pool::PoolTxDetailInfo, Error> {
        self.read_blocking(handle, || query::detail(&self.store, hash, &self.config))
    }

    #[cfg(test)]
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

    #[cfg(test)]
    pub(crate) fn set_template_response_hook(&self, hook: Option<Arc<dyn Fn() + Send + Sync>>) {
        if let Some(template) = &self.template {
            template.set_build_response_hook(hook);
        }
    }

    #[cfg(test)]
    pub(crate) fn current_block_template(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<Option<ckb_jsonrpc_types::BlockTemplate>, Error> {
        self.template
            .as_ref()
            .ok_or(Error::Full("block assembler disabled".into()))?
            .current(deadline)
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

    fn finish_transaction_lookup(
        &self,
        hash: &Byte32,
        found: Option<TransactionWithStatus>,
    ) -> Result<TransactionWithStatus, AnyError> {
        if let Some(result) = found {
            return Ok(result);
        }
        Ok(self.recent_reject(hash)?.map_or_else(
            TransactionWithStatus::with_unknown,
            TransactionWithStatus::with_rejected,
        ))
    }

    pub(crate) fn transaction_blocking(
        &self,
        handle: &Handle,
        hash: &Byte32,
    ) -> Result<TransactionWithStatus, AnyError> {
        let found = self.read_blocking(handle, || {
            query::transaction(&self.store, hash, &self.config)
        })?;
        self.finish_transaction_lookup(hash, found)
    }

    #[cfg(test)]
    pub(crate) async fn transaction(
        &self,
        hash: &Byte32,
    ) -> Result<TransactionWithStatus, AnyError> {
        let found = {
            let _permit = self.read_permit().await?;
            query::transaction(&self.store, hash, &self.config)?
        };
        self.finish_transaction_lookup(hash, found)
    }

    pub(crate) fn recent_count(&self) -> Option<u64> {
        self.recent
            .as_ref()
            .map(|recent| recent.get_estimate_total_keys_num())
    }

    pub(crate) fn estimate_fee_blocking(
        &self,
        handle: &Handle,
        mode: EstimateMode,
        fallback: bool,
    ) -> Result<ckb_types::core::FeeRate, AnyError> {
        let estimated = self.read_blocking(handle, || {
            Ok(self
                .estimator
                .estimate_fee_rate_with_samples(mode, || query::fee_samples(&self.store)))
        })?;
        match estimated {
            Ok(rate) => Ok(rate),
            Err(error) if !fallback => Err(error.into()),
            Err(_) => {
                let target = FeeEstimator::target_blocks_for_estimate_mode(mode);
                Ok(self.read_blocking(handle, || {
                    query::estimate_fee(&self.store, &self.config, target)
                })?)
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
                .for_block()
                .map_err(Error::from)?
                .pack_transactions(super::packing::TemplatePackingLimits::new(
                    bytes.unwrap_or_else(|| snapshot.consensus().max_block_bytes()) as usize,
                    snapshot.consensus().max_block_cycles(),
                ))
                .map_err(Error::from)
        })
        .await
    }
}
