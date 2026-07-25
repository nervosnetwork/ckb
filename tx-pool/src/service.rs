//! Tx-pool background service

pub(crate) mod builder;
pub(crate) mod controller;
pub(crate) mod dispatch;
pub(crate) mod effects;
pub(crate) mod message;
pub(crate) mod pipeline_ops;
pub(crate) mod workers;

pub use builder::TxPoolServiceBuilder;
pub use controller::TxPoolController;
pub(crate) use message::{
    AsyncRequest, BlockAssemblerMessage, Message, SyncRequest, TestAcceptTxResult,
    VerifyCacheUpdate,
};

pub(crate) use dispatch::process;
pub(crate) use message::{
    BlockTemplateArgs, BlockTemplateResult, FeeEstimatesResult, FetchTxsWithCyclesResult,
    GetTransactionWithStatusResult, GetTxStatusResult, SubmitTxResult,
};
#[cfg(feature = "internal")]
pub(crate) use workers::spawn_verify_cache_worker;

use crate::block_assembler::BlockAssembler;
use crate::callback::Callbacks;
use crate::component::entry::TxEntry;
use crate::component::pool_map::Status;
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
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{BlockView, Capacity, Cycle, TransactionView, UncleBlockView, tx_pool::TxStatus},
    packed::{Byte32, ProposalShortId},
};
use ckb_verification::cache::TxVerificationCache;
use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::{RwLock, mpsc, watch};

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
    exhausted: AtomicBool,
}

impl PipelineEpoch {
    pub(crate) fn current(&self) -> Option<u64> {
        if self.exhausted.load(Ordering::Acquire) {
            None
        } else {
            Some(self.value.load(Ordering::Acquire))
        }
    }

    pub(crate) fn is_current(&self, epoch: u64) -> bool {
        !self.exhausted.load(Ordering::Acquire) && self.value.load(Ordering::Acquire) == epoch
    }

    /// Invalidate every job admitted before this call.
    pub(crate) fn advance(&self) -> Option<u64> {
        if self.exhausted.load(Ordering::Acquire) {
            return None;
        }
        match self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            }) {
            Ok(previous) => Some(previous + 1),
            Err(_) => {
                self.exhausted.store(true, Ordering::Release);
                None
            }
        }
    }
}

/// Pipeline state: the structures a transaction travels through before it
/// reaches the pool.
#[derive(Clone)]
pub(crate) struct PipelineState {
    /// Single authoritative pre-pool lifecycle owner.
    pub(crate) runtime: Arc<crate::component::pipeline_runtime::PipelineRuntime>,
    /// Administrative generation shared by every pipeline stage.
    pub(crate) epoch: Arc<PipelineEpoch>,
    /// Chunk command receiver used by the synchronous reorg recovery path so
    /// that detached transactions are not verified while the pipeline is
    /// suspended.
    pub(crate) chunk_rx: watch::Receiver<ChunkCommand>,
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
    pub(crate) effects: Arc<effects::EffectQueue>,
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
    pending: AtomicU64,
    proposed: AtomicU64,
    uncle: AtomicU64,
}

#[derive(Default)]
pub(crate) struct BlockAssemblerResetJournal {
    state: std::sync::Mutex<BlockAssemblerResetState>,
}

#[derive(Default)]
struct BlockAssemblerResetState {
    next_generation: u64,
    pending: Option<(u64, Arc<Snapshot>)>,
}

impl BlockAssemblerResetJournal {
    fn mark(&self, snapshot: Arc<Snapshot>) {
        let mut state = self
            .state
            .lock()
            .expect("block-assembler reset journal mutex poisoned");
        state.next_generation = state
            .next_generation
            .checked_add(1)
            .expect("block-assembler reset generation exhausted");
        let generation = state.next_generation;
        state.pending = Some((generation, snapshot));
    }

    fn load(&self) -> Option<(u64, Arc<Snapshot>)> {
        self.state
            .lock()
            .expect("block-assembler reset journal mutex poisoned")
            .pending
            .clone()
    }

    fn complete(&self, completed_generation: u64) {
        let mut state = self
            .state
            .lock()
            .expect("block-assembler reset journal mutex poisoned");
        if state
            .pending
            .as_ref()
            .is_some_and(|(generation, _)| *generation == completed_generation)
        {
            state.pending.take();
        }
    }

    fn is_pending(&self) -> bool {
        self.state
            .lock()
            .expect("block-assembler reset journal mutex poisoned")
            .pending
            .is_some()
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

    fn mark(&self, message: &BlockAssemblerMessage) {
        let Some(slot) = self.slot(message) else {
            return;
        };
        slot.fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
            generation.checked_add(1)
        })
        .expect("block-assembler dirty generation exhausted");
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
    pub(crate) fn mark_block_assembler_dirty(&self, message: &BlockAssemblerMessage) {
        self.block_assembler_dirty.mark(message);
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

    /// A high-priority full swap may intentionally overwrite optimistic
    /// proposal/transaction updates that completed while it was building.
    /// Reissue both level-triggered generations after that swap so an update
    /// acknowledged immediately before the full writer cannot be lost.
    pub(crate) fn mark_block_assembler_full_reconcile(&self) {
        self.block_assembler_dirty
            .mark(&BlockAssemblerMessage::Pending);
        self.block_assembler_dirty
            .mark(&BlockAssemblerMessage::Proposed);
    }

    pub(crate) fn mark_block_assembler_reset(&self, snapshot: Arc<Snapshot>) {
        self.block_assembler_reset.mark(snapshot);
    }

    /// Load the latest required reset without consuming it. The reset journal
    /// is level-triggered authority: a failed template rebuild must leave the
    /// snapshot available for the interval consumer's next attempt.
    pub(crate) fn load_block_assembler_reset(&self) -> Option<(u64, Arc<Snapshot>)> {
        self.block_assembler_reset.load()
    }

    pub(crate) fn block_assembler_reset_pending(&self) -> bool {
        self.block_assembler_reset.is_pending()
    }

    /// Acknowledge only the exact loaded generation. Pointer identity is not
    /// sufficient because the same snapshot Arc may be journaled again while
    /// an earlier rebuild is off-lock.
    pub(crate) fn complete_block_assembler_reset(&self, completed_generation: u64) {
        self.block_assembler_reset.complete(completed_generation);
    }
}

/// Bounded internal ban fence for controller messages and worker leases that
/// raced with network-level peer eviction. The network service owns the
/// durable three-day ban; tx-pool only needs enough markers to cover its
/// bounded channel, active handlers and coordinator residents. A reconnect
/// churn attack must not turn those transient markers into an unbounded
/// three-day HashMap.
pub(crate) struct BannedPeerSet {
    entries: std::sync::Mutex<lru::LruCache<ckb_network::PeerIndex, std::time::Instant>>,
}

impl BannedPeerSet {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: std::sync::Mutex::new(lru::LruCache::new(capacity.max(1))),
        }
    }

    pub(crate) fn record(&self, peer: ckb_network::PeerIndex, duration: std::time::Duration) {
        let expires_at = std::time::Instant::now()
            .checked_add(duration)
            .unwrap_or_else(std::time::Instant::now);
        self.entries.lock().unwrap().put(peer, expires_at);
    }

    pub(crate) fn contains(&self, peer: ckb_network::PeerIndex) -> bool {
        let mut entries = self.entries.lock().unwrap();
        match entries.peek(&peer).copied() {
            Some(expires_at) if expires_at > std::time::Instant::now() => true,
            Some(_) => {
                entries.pop(&peer);
                false
            }
            None => false,
        }
    }
}

impl Default for BannedPeerSet {
    fn default() -> Self {
        Self::new(DEFAULT_CHANNEL_SIZE)
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
    /// Held while the lock-free section of a reorg (retained-transaction
    /// recovery) is in progress. `save_pool` acquires it before persisting
    /// so the file always represents a complete recovery point: a snapshot
    /// taken mid-recovery would silently lose the detached transactions
    /// that have not been re-added yet.
    pub(crate) recovery_lock: Arc<tokio::sync::Mutex<()>>,
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

/// Location and metadata of a transaction found in the pipeline queues.
pub(crate) enum PipelineTxLocation {
    /// In the pre-check queue (awaiting initial resolution).
    PreChecking { tx: TransactionView },
    /// In the ordered resolve queue (not yet resolved/verified).
    Ordered { tx: TransactionView },
    /// In the verify queue (resolved, awaiting verification).
    Verifying {
        tx: TransactionView,
        fee: Capacity,
        status: Status,
    },
    /// In the orphan pool (missing inputs).
    Orphan { tx: TransactionView, cycle: Cycle },
}

/// Result of looking up a transaction in the tx-pool or pipeline queues.
pub(crate) enum ResolvedTxLocation {
    /// Accepted in the main pool.
    Pool {
        status: Status,
        entry: TxEntry,
        /// Computed under the same pool read guard as `status` and `entry`, so
        /// one RPC response never combines metadata from different mutations.
        min_replace_fee: Option<Capacity>,
    },
    /// In one of the pipeline queues.
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
            let inserted = block_assembler.candidate_uncles.lock().await.insert(uncle);
            if inserted {
                self.journal_block_assembler_message(BlockAssemblerMessage::Uncle);
            }
        }
    }

    /// Rebuild mining state only after the matching pool/coordinator reorg
    /// transaction and detached-tx recovery have completed. Publishing the
    /// new template first exposes a future snapshot when the retained reorg
    /// head later fails its preflight.
    pub async fn refresh_block_assembler_after_tx_pool_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
    ) -> Result<(), String> {
        if let Some(ref block_assembler) = self.block_assembler {
            {
                let mut candidate_uncles = block_assembler.candidate_uncles.lock().await;
                for detached_block in detached_blocks {
                    candidate_uncles.insert(detached_block.as_uncle());
                }
            }

            // Consume the generation-tagged authority journal instead of the
            // reorg handler's captured snapshot. A later clear may have won
            // after tx-pool recovery released `recovery_lock`; applying the
            // captured snapshot here would resurrect a stale template.
            match crate::block_assembler::process_reset(self.clone(), false).await {
                crate::block_assembler::ResetApply::Retry => {
                    return Err("block assembler authoritative reset remains pending".to_string());
                }
                crate::block_assembler::ResetApply::Superseded => {
                    // This reorg refresh was superseded (for example by a
                    // later clear). The newer generation is now the sole
                    // template authority; retrying the retained old reorg
                    // would resurrect stale chain state.
                    return Ok(());
                }
                crate::block_assembler::ResetApply::Idle
                | crate::block_assembler::ResetApply::Applied => {}
            }
            match block_assembler.update_full(&self.pool.tx_pool).await {
                Ok(true) => self.journal_block_assembler_full_reconcile(),
                Ok(false) => {
                    if self.relay.block_assembler_reset_pending() {
                        return Ok(());
                    }
                    return Err(
                        "block assembler full rebuild observed a tx-pool tip mismatch".to_string(),
                    );
                }
                Err(e) => error!("block_assembler update failed {:?}", e),
            }
            // A reset is already a valid blank template if the full rebuild
            // fails; publish at most once after the complete refresh attempt.
            if !self.relay.block_assembler_reset_pending() {
                block_assembler.notify().await;
            }
        }
        Ok(())
    }

    #[cfg(feature = "internal")]
    pub async fn plug_entry(&self, entries: Vec<TxEntry>, target: PlugTarget) {
        {
            let mut tx_pool = self.pool.tx_pool.write().await;
            match target {
                PlugTarget::Pending => {
                    for entry in entries {
                        tx_pool
                            .add_pending(entry)
                            .expect("Plug entry add_pending error");
                    }
                }
                PlugTarget::Proposed => {
                    for entry in entries {
                        tx_pool
                            .add_proposed(entry)
                            .expect("Plug entry add_proposed error");
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
    }
}

#[cfg(test)]
mod tests;
