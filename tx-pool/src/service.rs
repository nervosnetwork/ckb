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
use ckb_logger::{debug, error};
use ckb_network::PeerIndex;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
#[cfg(test)]
use ckb_stop_handler::new_tokio_exit_rx;
use ckb_types::{
    core::{BlockView, Capacity, Cycle, TransactionView, UncleBlockView, tx_pool::TxStatus},
    packed::{Byte32, ProposalShortId},
};
use ckb_verification::cache::TxVerificationCache;
use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};
#[cfg(test)]
use std::time::Duration;
use tokio::sync::{RwLock, mpsc, watch};

/// Default bounded channel capacity for internal tx-pool message queues.
///
/// Provides back-pressure between producers (network, RPC, block sync) and
/// consumers (pre-check, resolve, verify workers). 512 is large enough to
/// absorb short bursts without allowing an unbounded backlog.
pub(crate) const DEFAULT_CHANNEL_SIZE: usize = 512;

/// Bounded channel capacity for block-assembler update notifications.
///
/// The block assembler receives notifications for new block templates. 100
/// slots is sufficient because consumers drain these quickly and stale
/// notifications are acceptable to drop.
pub(crate) const BLOCK_ASSEMBLER_CHANNEL_SIZE: usize = 100;

const BLOCK_ASSEMBLER_DIRTY_PENDING: u8 = 1 << 0;
const BLOCK_ASSEMBLER_DIRTY_PROPOSED: u8 = 1 << 1;
const BLOCK_ASSEMBLER_DIRTY_UNCLE: u8 = 1 << 2;

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

    #[cfg(test)]
    pub(crate) fn set_for_test(&self, value: u64) {
        self.value.store(value, Ordering::Release);
        self.exhausted.store(false, Ordering::Release);
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
    pub(crate) block_assembler_dirty: Arc<AtomicU8>,
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

impl RelayState {
    pub(crate) fn mark_block_assembler_dirty(&self, message: &BlockAssemblerMessage) {
        let bit = match message {
            BlockAssemblerMessage::Pending => BLOCK_ASSEMBLER_DIRTY_PENDING,
            BlockAssemblerMessage::Proposed => BLOCK_ASSEMBLER_DIRTY_PROPOSED,
            BlockAssemblerMessage::Uncle => BLOCK_ASSEMBLER_DIRTY_UNCLE,
            BlockAssemblerMessage::Reset(_) => return,
        };
        self.block_assembler_dirty.fetch_or(bit, Ordering::Release);
    }

    pub(crate) fn take_block_assembler_dirty(&self) -> Vec<BlockAssemblerMessage> {
        let dirty = self.block_assembler_dirty.swap(0, Ordering::AcqRel);
        let mut messages = Vec::with_capacity(3);
        if dirty & BLOCK_ASSEMBLER_DIRTY_PENDING != 0 {
            messages.push(BlockAssemblerMessage::Pending);
        }
        if dirty & BLOCK_ASSEMBLER_DIRTY_PROPOSED != 0 {
            messages.push(BlockAssemblerMessage::Proposed);
        }
        if dirty & BLOCK_ASSEMBLER_DIRTY_UNCLE != 0 {
            messages.push(BlockAssemblerMessage::Uncle);
        }
        messages
    }
}

/// Shared set of recently banned peers (ban time per peer). Pruned
/// opportunistically on insert.
pub(crate) type BannedPeers =
    Arc<std::sync::Mutex<std::collections::HashMap<ckb_network::PeerIndex, std::time::Instant>>>;

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
    Pool { status: Status, entry: TxEntry },
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
            {
                block_assembler.candidate_uncles.lock().await.insert(uncle);
            }
            if self
                .relay
                .block_assembler_sender
                .send(BlockAssemblerMessage::Uncle)
                .await
                .is_err()
            {
                error!("block_assembler receiver dropped");
            }
        }
    }

    pub async fn update_block_assembler_before_tx_pool_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        snapshot: Arc<Snapshot>,
    ) {
        if let Some(ref block_assembler) = self.block_assembler {
            {
                let mut candidate_uncles = block_assembler.candidate_uncles.lock().await;
                for detached_block in detached_blocks {
                    candidate_uncles.insert(detached_block.as_uncle());
                }
            }

            if let Err(e) = block_assembler.reset_template(snapshot).await {
                error!("block_assembler reset_template error {}", e);
            }
            block_assembler.notify().await;
        }
    }

    pub async fn update_block_assembler_after_tx_pool_reorg(&self) {
        if let Some(ref block_assembler) = self.block_assembler {
            match block_assembler.update_full(&self.pool.tx_pool).await {
                Ok(true) => block_assembler.notify().await,
                Ok(false) => debug!("block_assembler update_full skipped (tip mismatch)"),
                Err(e) => error!("block_assembler update failed {:?}", e),
            }
        }
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
            if self.relay.block_assembler_sender.send(msg).await.is_err() {
                error!("block_assembler receiver dropped");
            }
        }
    }
}

#[cfg(test)]
mod tests;
