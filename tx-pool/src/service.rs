//! Tx-pool background service

pub(crate) mod builder;
pub(crate) mod controller;
pub(crate) mod dispatch;
pub(crate) mod effects;
pub(crate) mod message;
pub(crate) mod pipeline_ops;
pub(crate) mod stages;
pub(crate) mod workers;

pub use builder::TxPoolServiceBuilder;
pub use controller::TxPoolController;
pub(crate) use message::{
    AsyncRequest, BlockAssemblerMessage, Message, NotifyTxBatch, SyncRequest, TestAcceptTxResult,
    VerifyCacheUpdate,
};

pub(crate) use dispatch::process;
pub(crate) use message::{
    BlockTemplateArgs, BlockTemplateResult, FeeEstimatesResult, FetchTxsWithCyclesResult,
    GetTransactionWithStatusResult, GetTxStatusResult, SubmitTxResult,
};
#[cfg(feature = "internal")]
pub(crate) use workers::spawn_verify_cache_worker;

use crate::block_assembler::{BlockAssembler, ResetEpoch};
use crate::callback::Callbacks;
use crate::component::entry::TxEntry;
use crate::component::pool_map::Status;
pub(crate) use crate::component::pre_pool::PipelineTxLocation;
use crate::component::recent_reject::RecentReject;
use crate::pool::TxPool;
#[cfg(feature = "internal")]
use crate::process::PlugTarget;
use ckb_app_config::TxPoolConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_channel::oneshot;
use ckb_fee_estimator::FeeEstimator;
use ckb_logger::error;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{BlockView, Capacity, UncleBlockView, tx_pool::TxStatus},
    packed::{Byte32, ProposalShortId},
};
use ckb_util::Mutex;
use ckb_verification::cache::TxVerificationCache;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::num::NonZeroU32;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::{RwLock, mpsc};

/// Default bounded channel capacity for internal tx-pool message queues.
///
/// Provides back-pressure between producers (network, RPC, block sync) and
/// consumers (pre-check, resolve, verify workers). 512 is large enough to
/// absorb short bursts without allowing an unbounded backlog.
pub(crate) const DEFAULT_CHANNEL_SIZE: usize = 512;

/// Reorg messages are ordered authoritative deltas. The sender already
/// applies backpressure and the retained handler must process them strictly
/// one at a time, so a single buffered block tree avoids unbounded/deep-fork
/// backing retention without reducing useful concurrency.
pub(crate) const REORG_CHANNEL_SIZE: usize = 1;

/// Bounded channel capacity for block-assembler update notifications.
///
/// The block assembler receives notifications for new block templates. 100
/// slots is sufficient because consumers drain these quickly and stale
/// notifications are acceptable to drop.
pub(crate) const BLOCK_ASSEMBLER_CHANNEL_SIZE: usize = 100;

pub(crate) trait OneshotSender<R: fmt::Debug> {
    fn send(self, value: R) -> Result<(), R>;
}

impl<R: fmt::Debug> OneshotSender<R> for oneshot::Sender<R> {
    fn send(self, value: R) -> Result<(), R> {
        oneshot::Sender::send(&self, value).map_err(|e| e.0)
    }
}

impl<R: fmt::Debug> OneshotSender<R> for tokio::sync::oneshot::Sender<R> {
    fn send(self, value: R) -> Result<(), R> {
        tokio::sync::oneshot::Sender::send(self, value)
    }
}

pub(crate) fn respond<R: fmt::Debug, S: OneshotSender<R>>(
    responder: S,
    value: R,
    message: &'static str,
) {
    if let Err(e) = responder.send(value) {
        error!("Responder sending {} failed {:?}", message, e);
    }
}

pub(crate) struct Request<R, A> {
    pub responder: R,
    pub arguments: A,
}

impl<R, A> Request<R, A> {
    pub(crate) fn call(arguments: A, responder: R) -> Request<R, A> {
        Request {
            responder,
            arguments,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Notify<A> {
    pub arguments: A,
}

impl<A> Notify<A> {
    pub(crate) fn new(arguments: A) -> Notify<A> {
        Notify { arguments }
    }
}

pub(crate) type ChainReorgArgs = (
    VecDeque<BlockView>,
    VecDeque<BlockView>,
    HashSet<ProposalShortId>,
    Arc<Snapshot>,
);

/// tx verification result
#[derive(Clone, Debug)]
pub enum TxVerificationResult {
    /// tx is verified
    Ok {
        /// original peer
        original_peer: Option<PeerIndex>,
        /// transaction hash
        tx_hash: Byte32,
    },
    /// tx parent is unknown
    UnknownParents {
        /// original peer
        peer: PeerIndex,
        /// parents hashes
        parents: HashSet<Byte32>,
    },
    /// tx is rejected
    Reject {
        /// transaction hash
        tx_hash: Byte32,
    },
    /// Constant-size reconciliation after an authoritative generation swap
    /// deliberately discards optional pre-pool residents.
    GenerationReset,
}

/// Auxiliary read-mostly services bundled to keep [`TxPoolService`] field
/// count down. All three share the service lifecycle.
///
/// The `Arc`s here are kept deliberately: `txs_verify_cache` is shared with
/// `Shared` outside the tx-pool, and `recent_reject` is captured by the
/// registered reject callback in `shared_builder`.
#[derive(Clone)]
pub(crate) struct AuxServices {
    pub(crate) txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
    /// Recent-reject database (RocksDB with TTL), owned by the service rather
    /// than `TxPool` so that `put` / `get` never need the tx-pool write lock.
    pub(crate) recent_reject: Option<Arc<RecentReject>>,
    pub(crate) fee_estimator: FeeEstimator,
}

/// Pool-core state: the main pool and the chain context it validates
/// against.
#[derive(Clone)]
pub(crate) struct PoolCore {
    pub(crate) tx_pool: Arc<RwLock<TxPool>>,
    pub(crate) consensus: Arc<Consensus>,
    pub(crate) tx_pool_config: Arc<TxPoolConfig>,
    pub(crate) dispatcher_capacity: DispatcherCapacity,
}

/// Dispatcher concurrency validated once before any service task is spawned.
///
/// Tokio's semaphore stores a `usize` permit count while its atomic
/// `acquire_many` operation accepts `u32`. Keeping both representations in a
/// private validated value makes zero and truncating conversions
/// unrepresentable in the running service.
#[derive(Clone, Copy)]
pub(crate) struct DispatcherCapacity {
    permits: usize,
    acquire_many: NonZeroU32,
}

impl DispatcherCapacity {
    pub(crate) fn new(permits: usize) -> Option<Self> {
        Some(Self {
            permits,
            acquire_many: NonZeroU32::new(u32::try_from(permits).ok()?)?,
        })
    }

    pub(crate) fn permits(self) -> usize {
        self.permits
    }

    pub(crate) fn acquire_many(self) -> u32 {
        self.acquire_many.get()
    }
}

/// Monotonic administrative generation for the asynchronous pipeline.
///
/// `clear_pool` and `clear_pipeline` advance this generation before they
/// remove queued state. Every job carries the generation in which it was
/// admitted and workers re-check it at each ownership boundary and at the
/// final pool commit. This turns clear into a linearizable cancellation
/// barrier for jobs that had already been popped by a worker.
///
/// Generation exhaustion is fail-closed. Wrapping to zero would make an
/// ancient job current again, so once `u64::MAX` is reached no later job is
/// accepted or committed.
#[derive(Debug, Default)]
pub(crate) struct PipelineEpoch {
    value: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PipelineEpochExhausted;

impl std::fmt::Display for PipelineEpochExhausted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("tx-pool pipeline epoch exhausted")
    }
}

impl std::error::Error for PipelineEpochExhausted {}

impl PipelineEpoch {
    pub(crate) fn current(&self) -> Option<u64> {
        let value = self.value.load(Ordering::Acquire);
        (value != u64::MAX).then_some(value)
    }

    pub(crate) fn is_current(&self, epoch: u64) -> bool {
        epoch != u64::MAX && self.value.load(Ordering::Acquire) == epoch
    }

    /// Invalidate every job admitted before this call.
    pub(crate) fn advance(&self) -> Option<u64> {
        match self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            }) {
            Ok(previous) => previous.checked_add(1).filter(|next| *next != u64::MAX),
            Err(_) => None,
        }
    }
}

/// Pipeline state: the structures a transaction travels through before it
/// reaches the pool.
#[derive(Clone)]
pub(crate) struct PipelineState {
    /// Single authoritative pre-pool lifecycle owner.
    pub(crate) kernel: Arc<crate::component::pre_pool::PrePool>,
    /// Administrative generation shared by every pipeline stage.
    pub(crate) epoch: Arc<PipelineEpoch>,
    /// Bounded best-effort verification-cache update channel. Executable
    /// transaction lifecycle and conflict recovery never use this channel.
    pub(crate) verify_cache_sender: mpsc::Sender<VerifyCacheUpdate>,
}

/// Relay/notification state: how verification outcomes leave the node.
#[derive(Clone)]
pub(crate) struct RelayState {
    pub(crate) network: crate::network::TxPoolNetworkHandle,
    pub(crate) tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
    pub(crate) block_assembler_sender: mpsc::Sender<BlockAssemblerMessage>,
    /// Level-triggered update journal paired with the bounded notification
    /// channel. A full channel may drop a wake edge, but it cannot erase the
    /// authoritative dirty bit consumed on the next assembler pass.
    pub(crate) block_assembler_dirty: Arc<BlockAssemblerDirtyJournal>,
    /// Latest management reset retained independently of the bounded wake
    /// channel. `clear_pool` writes this while still holding the pool lock, so
    /// a later accepted transaction cannot be overwritten by an older reset.
    pub(crate) block_assembler_reset: Arc<BlockAssemblerResetJournal>,
    pub(crate) callbacks: Arc<Callbacks>,
    /// Bounded stable-state journal. Its publisher is independent of the
    /// controller dispatcher, so a callback may synchronously re-enter the
    /// controller without consuming the permit needed to serve itself.
    pub(crate) effects: Arc<effects::EffectJournal>,
    /// Peers banned within the ban window. Workers check jobs against this
    /// set so that a banned peer's in-flight jobs (popped from a queue
    /// before the ban) do not keep flowing into the pool afterwards.
    pub(crate) banned_peers: BannedPeers,
}

/// Generation-tagged, level-triggered template work. Consumers load without
/// consuming and acknowledge only the exact generation they applied. A
/// failed update therefore remains retryable, while a producer racing with an
/// older completion cannot have its newer work erased.
#[derive(Default)]
pub(crate) struct BlockAssemblerDirtyJournal {
    next_generation: AtomicU64,
    pending: AtomicU64,
    proposed: AtomicU64,
    uncle: AtomicU64,
}

#[derive(Default)]
pub(crate) struct BlockAssemblerResetJournal {
    state: Mutex<BlockAssemblerResetState>,
}

/// Exact reset work loaded from the level-triggered authority journal.
/// Keeping generation and snapshot in one opaque value prevents callers from
/// acknowledging or publishing a snapshot under a different generation.
#[derive(Clone)]
pub(crate) struct PendingBlockAssemblerReset {
    generation: ResetEpoch,
    snapshot: Arc<Snapshot>,
}

impl PendingBlockAssemblerReset {
    pub(crate) fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    pub(crate) fn generation(&self) -> ResetEpoch {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockAssemblerJournalError {
    GenerationExhausted,
}

#[derive(Default)]
struct BlockAssemblerResetState {
    next_generation: ResetEpoch,
    pending: Option<PendingBlockAssemblerReset>,
}

impl BlockAssemblerResetJournal {
    fn mark(&self, snapshot: Arc<Snapshot>) -> Result<(), BlockAssemblerJournalError> {
        let mut state = self.state.lock();
        state.next_generation = state
            .next_generation
            .next()
            .ok_or(BlockAssemblerJournalError::GenerationExhausted)?;
        let generation = state.next_generation;
        state.pending = Some(PendingBlockAssemblerReset {
            generation,
            snapshot,
        });
        Ok(())
    }

    fn load(&self) -> Option<PendingBlockAssemblerReset> {
        self.state.lock().pending.clone()
    }

    /// Linearize publication with consumption of the exact reset token. The
    /// closure runs while the journal lock is held and must not block. A newer
    /// generation either wins before this check (so the stale closure never
    /// runs) or is installed after the committed publication and remains
    /// pending.
    pub(crate) fn try_apply<T>(
        &self,
        pending: &PendingBlockAssemblerReset,
        apply: impl FnOnce() -> T,
    ) -> Option<T> {
        let mut state = self.state.lock();
        if !state
            .pending
            .as_ref()
            .is_some_and(|current| current.generation == pending.generation)
        {
            return None;
        }
        let result = apply();
        state.pending.take();
        Some(result)
    }

    fn is_pending(&self) -> bool {
        self.state.lock().pending.is_some()
    }

    pub(crate) fn is_current(&self, pending: &PendingBlockAssemblerReset) -> bool {
        self.state
            .lock()
            .pending
            .as_ref()
            .is_some_and(|current| current.generation == pending.generation)
    }
}

impl BlockAssemblerDirtyJournal {
    fn slot(&self, message: &BlockAssemblerMessage) -> Option<&AtomicU64> {
        match message {
            BlockAssemblerMessage::Pending => Some(&self.pending),
            BlockAssemblerMessage::Proposed => Some(&self.proposed),
            BlockAssemblerMessage::Uncle => Some(&self.uncle),
            BlockAssemblerMessage::Reset => None,
        }
    }

    fn next_generation(&self) -> Result<u64, BlockAssemblerJournalError> {
        self.next_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .and_then(|generation| generation.checked_add(1).ok_or(generation))
            .map_err(|_| BlockAssemblerJournalError::GenerationExhausted)
    }

    fn mark(&self, message: &BlockAssemblerMessage) -> Result<(), BlockAssemblerJournalError> {
        let Some(slot) = self.slot(message) else {
            return Ok(());
        };
        let generation = self.next_generation()?;
        slot.fetch_max(generation, Ordering::AcqRel);
        Ok(())
    }

    fn mark_full_reconcile(&self) -> Result<(), BlockAssemblerJournalError> {
        let generation = self.next_generation()?;
        self.pending.fetch_max(generation, Ordering::AcqRel);
        self.proposed.fetch_max(generation, Ordering::AcqRel);
        self.uncle.fetch_max(generation, Ordering::AcqRel);
        Ok(())
    }

    fn load(&self) -> Vec<(BlockAssemblerMessage, u64)> {
        [
            (BlockAssemblerMessage::Pending, &self.pending),
            (BlockAssemblerMessage::Proposed, &self.proposed),
            (BlockAssemblerMessage::Uncle, &self.uncle),
        ]
        .into_iter()
        .filter_map(|(message, slot)| {
            let generation = slot.load(Ordering::Acquire);
            (generation != 0).then_some((message, generation))
        })
        .collect()
    }

    fn complete(&self, message: &BlockAssemblerMessage, generation: u64) {
        if let Some(slot) = self.slot(message) {
            // Failure means a producer installed a newer generation. Leaving
            // that value intact is the desired level-triggered behavior.
            let _ = slot.compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}

impl RelayState {
    pub(crate) fn mark_block_assembler_dirty(
        &self,
        message: &BlockAssemblerMessage,
    ) -> Result<(), BlockAssemblerJournalError> {
        self.block_assembler_dirty.mark(message)
    }

    pub(crate) fn load_block_assembler_dirty(&self) -> Vec<(BlockAssemblerMessage, u64)> {
        self.block_assembler_dirty.load()
    }

    pub(crate) fn complete_block_assembler_dirty(
        &self,
        message: &BlockAssemblerMessage,
        generation: u64,
    ) {
        self.block_assembler_dirty.complete(message, generation);
    }

    /// A high-priority full/reset swap may intentionally overwrite any
    /// optimistic partial update that completed while it was building.
    /// Reissue every level-triggered generation after that swap so an update
    /// acknowledged immediately before the replacement cannot be lost.
    pub(crate) fn mark_block_assembler_full_reconcile(
        &self,
    ) -> Result<(), BlockAssemblerJournalError> {
        self.block_assembler_dirty.mark_full_reconcile()
    }

    pub(crate) fn mark_block_assembler_reset(
        &self,
        snapshot: Arc<Snapshot>,
    ) -> Result<(), BlockAssemblerJournalError> {
        self.block_assembler_reset.mark(snapshot)
    }

    /// Load the latest required reset without consuming it. The reset journal
    /// is level-triggered authority: a failed template rebuild must leave the
    /// snapshot available for the interval consumer's next attempt.
    pub(crate) fn load_block_assembler_reset(&self) -> Option<PendingBlockAssemblerReset> {
        self.block_assembler_reset.load()
    }

    pub(crate) fn block_assembler_reset_pending(&self) -> bool {
        self.block_assembler_reset.is_pending()
    }
}

/// Internal ban fence for controller messages and worker leases that raced
/// with network-level peer eviction.
///
/// An unexpired fence must never be evicted for capacity: doing so would turn
/// peer churn into an integrity bypass for an older queued ingress. Entries
/// are pruned by the same expiry as the network ban. Their cardinality is
/// therefore coupled to the network's existing durable ban set instead of to
/// tx-pool transaction residency, and does not create an independent
/// process-lifetime retention class.
pub(crate) struct BannedPeerSet {
    state: Mutex<BanFenceState>,
}

struct BanFenceState {
    entries: HashMap<ckb_network::PeerIndex, BanFenceDeadline>,
    expirations: BTreeSet<(std::time::Instant, ckb_network::PeerIndex)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BanFenceDeadline {
    At(std::time::Instant),
    ProcessLifetime,
}

impl BanFenceDeadline {
    fn after(now: std::time::Instant, duration: std::time::Duration) -> Self {
        now.checked_add(duration)
            .map_or(Self::ProcessLifetime, Self::At)
    }
}

impl BannedPeerSet {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(BanFenceState {
                entries: HashMap::new(),
                expirations: BTreeSet::new(),
            }),
        }
    }

    pub(crate) fn record(&self, peer: ckb_network::PeerIndex, duration: std::time::Duration) {
        let now = std::time::Instant::now();
        let deadline = BanFenceDeadline::after(now, duration);
        let mut state = self.state.lock();
        state.prune(now);
        if let Some(BanFenceDeadline::At(previous)) = state.entries.insert(peer, deadline) {
            state.expirations.remove(&(previous, peer));
        }
        if let BanFenceDeadline::At(deadline) = deadline {
            state.expirations.insert((deadline, peer));
        }
    }

    pub(crate) fn contains(&self, peer: ckb_network::PeerIndex) -> bool {
        let mut state = self.state.lock();
        let now = std::time::Instant::now();
        state.prune(now);
        state.entries.contains_key(&peer)
    }
}

impl BanFenceState {
    fn prune(&mut self, now: std::time::Instant) {
        while let Some((deadline, peer)) = self.expirations.first().copied() {
            if deadline > now {
                break;
            }
            self.expirations.remove(&(deadline, peer));
            self.entries.remove(&peer);
        }
    }
}

impl Default for BannedPeerSet {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) type BannedPeers = Arc<BannedPeerSet>;

#[derive(Clone)]
pub(crate) struct TxPoolService {
    /// Main pool and chain context.
    pub(crate) pool: PoolCore,
    /// Pipeline structures between entry and the pool.
    pub(crate) pipeline: PipelineState,
    /// Outbound notification channels (network, relayer, callbacks).
    pub(crate) relay: RelayState,
    pub(crate) aux: AuxServices,
    pub(crate) block_assembler: Option<BlockAssembler>,
    /// Orders immutable persistence snapshots without holding an async guard
    /// across blocking file I/O.
    pub(crate) persistence_writer: Arc<crate::persisted::PersistenceWriter>,
}

/// Outcome of an administrative `remove_tx` attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoveTxOutcome {
    /// The transaction was found and removed.
    Removed,
    /// A worker has popped the transaction and is processing it right now
    /// (not yet terminal): it cannot be removed mid-flight, but it exists.
    InProgress,
    /// Not found anywhere.
    NotFound,
}

/// Result of looking up a transaction in the accepted pool or pre-pool.
pub(crate) enum ResolvedTxLocation {
    /// Accepted in the main pool.
    Pool {
        status: Status,
        entry: TxEntry,
        /// Computed under the same pool read guard as `status` and `entry`, so
        /// one RPC response never combines metadata from different mutations.
        min_replace_fee: Option<Capacity>,
    },
    /// In one of the pre-pool locations.
    Pipeline(PipelineTxLocation),
    /// Not found in either place; the caller should check recent rejects.
    NotFound,
}

/// Map the internal pool status to the RPC-visible tx status.
pub(crate) fn map_pool_status(status: Status) -> TxStatus {
    if status == Status::Proposed {
        TxStatus::Proposed
    } else {
        TxStatus::Pending
    }
}

impl TxPoolService {
    pub fn should_notify_block_assembler(&self) -> bool {
        self.block_assembler.is_some()
    }

    pub async fn receive_candidate_uncle(&self, uncle: UncleBlockView) {
        if let Some(ref block_assembler) = self.block_assembler {
            let inserted = block_assembler.candidate_uncles.lock().insert(uncle);
            if inserted {
                self.journal_block_assembler_message(BlockAssemblerMessage::Uncle);
            }
        }
    }

    /// Rebuild mining state only after the matching pool/kernel reorg
    /// transaction and detached-tx recovery have completed. Publishing the
    /// new template first exposes a future snapshot when the retained reorg
    /// head later fails its preflight.
    pub async fn refresh_block_assembler_after_tx_pool_reorg(
        &self,
        candidate_uncles: Vec<UncleBlockView>,
        expected_snapshot: Arc<Snapshot>,
    ) {
        if let Some(ref block_assembler) = self.block_assembler {
            if self.pool.tx_pool.read().await.snapshot().tip_hash() != expected_snapshot.tip_hash()
            {
                return;
            }
            // Phase one has already committed the matching pool/kernel reorg,
            // so detached candidates can enter their bounded optional cache
            // before reset publication is arbitrated. The assembler loop and
            // this phase may both consume the same reset generation; retaining
            // candidates first prevents the loser from dropping the only
            // phase-two payload on its `Superseded` exit. A reset prepared
            // before this insertion is still repaired by `update_full`, which
            // derives from the candidate authority, or by the reissued Uncle
            // dirty generation.
            {
                let mut retained = block_assembler.candidate_uncles.lock();
                for uncle in candidate_uncles {
                    retained.insert(uncle);
                }
            }
            // Consume the generation-tagged authority journal instead of the
            // reorg handler's captured snapshot. A later clear may have won
            // while retained payloads were validating; applying the captured
            // snapshot here would resurrect stale template authority.
            match crate::block_assembler::process_reset(
                self.clone(),
                crate::block_assembler::ResetNotification::SuppressUntilFull,
            )
            .await
            {
                crate::block_assembler::ResetApply::Retry => {
                    // The assembler loop owns the latest reset and retries it
                    // level-wise. Keep the pool-derived deltas dirty, but do
                    // not retain this reorg channel head behind a deterministic
                    // template/cellbase failure.
                    self.journal_block_assembler_full_reconcile();
                    return;
                }
                crate::block_assembler::ResetApply::Superseded => {
                    // This reorg refresh was superseded (for example by a
                    // later clear). The newer generation is now the sole
                    // template authority; retrying the retained old reorg
                    // would resurrect stale chain state.
                    return;
                }
                crate::block_assembler::ResetApply::Idle
                | crate::block_assembler::ResetApply::Applied => {}
            }
            if self.pool.tx_pool.read().await.snapshot().tip_hash() != expected_snapshot.tip_hash()
            {
                let current = self.pool.tx_pool.read().await.cloned_snapshot();
                self.journal_block_assembler_reset(current);
                self.journal_block_assembler_full_reconcile();
                return;
            }
            match block_assembler.update_full(&self.pool.tx_pool).await {
                Ok(true) => {}
                Ok(false) => {
                    ckb_logger::debug!(
                        "block assembler full rebuild observed a tx-pool tip mismatch; retaining level-triggered reconcile"
                    );
                }
                Err(e) => {
                    ckb_logger::error!(
                        "block assembler full rebuild failed; retaining level-triggered reconcile: {e:?}"
                    );
                }
            }
            // A full attempt consumes an arbitrary snapshot of optimistic
            // partial/uncle dirtiness. Re-publish that level-triggered work at
            // this single exit regardless of success, tip mismatch or build
            // failure; the three outcomes differ only in diagnostics.
            self.journal_block_assembler_full_reconcile();
            // A reset is already a valid blank template if the full rebuild
            // fails; publish at most once after the complete refresh attempt.
            if !self.relay.block_assembler_reset_pending() {
                block_assembler.notify().await;
            }
        }
    }

    #[cfg(feature = "internal")]
    pub async fn plug_entry(
        &self,
        entries: Vec<TxEntry>,
        target: PlugTarget,
    ) -> Result<(), crate::error::Reject> {
        {
            let mut tx_pool = self.pool.tx_pool.write().await;
            match target {
                PlugTarget::Pending => {
                    for entry in entries {
                        tx_pool.pool_map.plug_entry(entry, Status::Pending)?;
                    }
                }
                PlugTarget::Proposed => {
                    for entry in entries {
                        tx_pool.pool_map.plug_entry(entry, Status::Proposed)?;
                    }
                }
            };
        }

        if self.should_notify_block_assembler() {
            let msg = match target {
                PlugTarget::Pending => BlockAssemblerMessage::Pending,
                PlugTarget::Proposed => BlockAssemblerMessage::Proposed,
            };
            self.journal_block_assembler_message(msg);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
