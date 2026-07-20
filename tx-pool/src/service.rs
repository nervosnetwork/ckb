//! Tx-pool background service

pub(crate) mod builder;
pub(crate) mod controller;
pub(crate) mod dispatch;
pub(crate) mod message;

pub use builder::TxPoolServiceBuilder;
pub use controller::TxPoolController;
pub(crate) use message::{
    AsyncRequest, BlockAssemblerMessage, DeferredTask, Message, SyncRequest, TestAcceptTxResult,
};

#[cfg(feature = "internal")]
pub(crate) use builder::spawn_deferred_worker;
pub(crate) use dispatch::process;
pub(crate) use message::{
    BlockTemplateArgs, BlockTemplateResult, FeeEstimatesResult, FetchTxsWithCyclesResult,
    GetTransactionWithStatusResult, GetTxStatusResult, SubmitTxResult,
};

use crate::block_assembler::BlockAssembler;
use crate::callback::Callbacks;
use crate::component::entry::TxEntry;
use crate::component::orphan::OrphanPool;
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
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
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
#[derive(Debug)]
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

#[derive(Clone)]
pub(crate) struct TxPoolService {
    pub(crate) tx_pool: Arc<RwLock<TxPool>>,
    pub(crate) orphan: Arc<RwLock<OrphanPool>>,
    pub(crate) consensus: Arc<Consensus>,
    pub(crate) tx_pool_config: Arc<TxPoolConfig>,
    pub(crate) block_assembler: Option<BlockAssembler>,
    pub(crate) callbacks: Arc<Callbacks>,
    pub(crate) network: crate::network::TxPoolNetworkHandle,
    pub(crate) tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
    pub(crate) block_assembler_sender: mpsc::Sender<BlockAssemblerMessage>,
    pub(crate) aux: AuxServices,
    /// The pipeline queues (pre-check, ordered-resolve, verify) and the
    /// in-flight RBF gate, bundled behind a single `Arc` because they share
    /// the same lifecycle and the same lock hierarchy.
    pub(crate) queues: Arc<crate::component::pipeline_queues::PipelineQueues>,
    /// Chunk command receiver used by the synchronous reorg recovery path so
    /// that detached transactions are not verified while the pipeline is
    /// suspended.
    pub(crate) chunk_rx: watch::Receiver<ChunkCommand>,
    /// Bounded channel for deferred side-effects (recovery tx re-enqueue,
    /// verify cache updates). A single background worker drains this channel,
    /// preventing unbounded task accumulation under high RBF frequency.
    pub(crate) deferred_sender: mpsc::Sender<DeferredTask>,
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

            if let Err(e) = block_assembler.reset_template(snapshot, true).await {
                error!("block_assembler reset_template error {}", e);
            }
            block_assembler.notify().await;
        }
    }

    pub async fn update_block_assembler_after_tx_pool_reorg(&self) {
        if let Some(ref block_assembler) = self.block_assembler {
            match block_assembler.update_full(&self.tx_pool).await {
                Ok(true) => block_assembler.notify().await,
                Ok(false) => debug!("block_assembler update_full skipped (tip mismatch)"),
                Err(e) => error!("block_assembler update failed {:?}", e),
            }
        }
    }

    #[cfg(feature = "internal")]
    pub async fn plug_entry(&self, entries: Vec<TxEntry>, target: PlugTarget) {
        {
            let mut tx_pool = self.tx_pool.write().await;
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
            if self.block_assembler_sender.send(msg).await.is_err() {
                error!("block_assembler receiver dropped");
            }
        }
    }
}

#[cfg(test)]
mod tests;
