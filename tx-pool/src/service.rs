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
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::pool_map::Status;
use crate::component::recent_reject::RecentReject;
use crate::pool::TxPool;
use ckb_app_config::TxPoolConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_channel::oneshot;
use ckb_error::AnyError;
use ckb_fee_estimator::FeeEstimator;
use ckb_logger::error;
use ckb_network::PeerIndex;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
#[cfg(test)]
use ckb_stop_handler::new_tokio_exit_rx;
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        BlockView, Capacity, Cycle, TransactionView, UncleBlockView,
        cell::{CellProvider, CellStatus, OverlayCellProvider},
        tx_pool::{TRANSACTION_SIZE_LIMIT, TxPoolInfo, TxStatus},
    },
    packed::{Byte32, OutPoint, ProposalShortId},
};
use ckb_verification::cache::TxVerificationCache;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
#[cfg(test)]
use std::time::Duration;
use tokio::sync::{RwLock, mpsc, watch};

use crate::pool_cell::PoolCell;
#[cfg(feature = "internal")]
use crate::process::PlugTarget;

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

#[derive(Clone)]
pub(crate) struct TxPoolService {
    pub(crate) tx_pool: Arc<RwLock<TxPool>>,
    pub(crate) orphan: Arc<RwLock<OrphanPool>>,
    pub(crate) consensus: Arc<Consensus>,
    pub(crate) tx_pool_config: Arc<TxPoolConfig>,
    pub(crate) block_assembler: Option<BlockAssembler>,
    pub(crate) txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
    pub(crate) callbacks: Arc<Callbacks>,
    pub(crate) network: crate::network::TxPoolNetworkHandle,
    pub(crate) tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
    pub(crate) block_assembler_sender: mpsc::Sender<BlockAssemblerMessage>,
    pub(crate) fee_estimator: FeeEstimator,
    /// Lock-free recent-reject database (RocksDB with TTL).
    /// Owned by the service rather than `TxPool` so that `put` / `get` never
    /// need the tx-pool write lock.
    pub(crate) recent_reject: Option<Arc<RecentReject>>,
    /// The three pipeline queues (pre-check, ordered-resolve, verify) bundled
    /// together because they share the same lifecycle and are always accessed
    /// as a unit.
    pub(crate) queues: crate::component::pipeline_queues::PipelineQueues,
    /// Chunk command receiver used by the synchronous reorg recovery path so
    /// that detached transactions are not verified while the pipeline is
    /// suspended.
    pub(crate) chunk_rx: watch::Receiver<ChunkCommand>,
    /// Fee-ordering gate for conflicting RBF replacements that are concurrently
    /// in flight through the pipeline.  Ensures the highest-fee candidate wins.
    pub(crate) rbf_candidates: Arc<RwLock<crate::component::rbf_candidates::RbfCandidates>>,
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
    /// Search the pipeline queues for a transaction by short id.
    pub(crate) async fn find_tx_in_pipeline(
        &self,
        id: &ProposalShortId,
    ) -> Option<PipelineTxLocation> {
        if let Some(tx) = self.queues.pre_check_queue.get_tx(id) {
            return Some(PipelineTxLocation::PreChecking { tx });
        }
        {
            let ordered = self.queues.ordered_resolve_queue.read().await;
            if let Some(tx) = ordered.get_tx(id) {
                return Some(PipelineTxLocation::Ordered { tx: tx.clone() });
            }
        }
        {
            let verify_queue = self.queues.verify_queue.read().await;
            if let Some(resolved) = verify_queue.get_tx_by_id(id) {
                return Some(PipelineTxLocation::Verifying {
                    tx: resolved.tx.clone(),
                    fee: resolved.fee,
                    status: resolved.status,
                });
            }
        }
        {
            let orphan = self.orphan.read().await;
            if let Some(entry) = orphan.get(id) {
                return Some(PipelineTxLocation::Orphan {
                    tx: entry.tx.clone(),
                    cycle: entry.cycle,
                });
            }
        }
        None
    }

    /// Tx-pool information
    pub(crate) async fn info(&self) -> TxPoolInfo {
        let tx_pool = self.tx_pool.read().await;
        let orphan = self.orphan.read().await;
        let verify_queue = self.queues.verify_queue.read().await;
        let tip_header = tx_pool.snapshot.tip_header();
        TxPoolInfo {
            tip_hash: tip_header.hash(),
            tip_number: tip_header.number(),
            pending_size: tx_pool.pool_map.pending_size(),
            proposed_size: tx_pool.pool_map.proposed_size(),
            orphan_size: orphan.len(),
            total_tx_size: tx_pool.pool_map.stats.total_tx_size.get(),
            total_tx_cycles: tx_pool.pool_map.stats.total_tx_cycles.get(),
            min_fee_rate: self.tx_pool_config.min_fee_rate,
            min_rbf_rate: self.tx_pool_config.min_rbf_rate,
            last_txs_updated_at: tx_pool.pool_map.get_max_update_time(),
            tx_size_limit: TRANSACTION_SIZE_LIMIT,
            max_tx_pool_size: self.tx_pool_config.max_tx_pool_size as u64,
            verify_queue_size: verify_queue.len(),
        }
    }

    pub(crate) fn get_total_recent_reject_num(&self) -> Option<u64> {
        self.recent_reject
            .as_ref()
            .map(|r| r.get_estimate_total_keys_num())
    }

    /// Look up a transaction hash in the recent-reject database.
    ///
    /// Returns `on_rejected(record)` if the tx was recently rejected,
    /// `on_unknown()` if it is unknown or there is no recent-reject db.
    pub(crate) async fn lookup_recent_reject<T>(
        &self,
        hash: &Byte32,
        on_rejected: impl FnOnce(String) -> T,
        on_unknown: impl FnOnce() -> T,
    ) -> Result<T, AnyError> {
        if let Some(ref db) = self.recent_reject {
            match db.get(hash) {
                Ok(Some(record)) => Ok(on_rejected(record)),
                Ok(_) => Ok(on_unknown()),
                Err(err) => Err(err),
            }
        } else {
            Ok(on_unknown())
        }
    }

    /// Get Live Cell Status
    pub(crate) async fn get_live_cell(&self, out_point: OutPoint, eager_load: bool) -> CellStatus {
        let tx_pool = self.tx_pool.read().await;
        let snapshot = tx_pool.snapshot();
        let pool_cell = PoolCell::new(&tx_pool.pool_map, false);
        let provider = OverlayCellProvider::new(&pool_cell, snapshot);

        match provider.cell(&out_point, false) {
            CellStatus::Live(mut cell_meta) => {
                if eager_load && let Some((data, data_hash)) = snapshot.get_cell_data(&out_point) {
                    cell_meta.mem_cell_data = Some(data);
                    cell_meta.mem_cell_data_hash = Some(data_hash);
                }
                CellStatus::live_cell(cell_meta)
            }
            _ => CellStatus::Unknown,
        }
    }

    pub fn should_notify_block_assembler(&self) -> bool {
        self.block_assembler.is_some()
    }

    /// Excludes proposals that already exist in the tx-pool or any pipeline queue.
    ///
    /// Any proposal that appears in any of the following structures is considered
    /// "already exists" and will be filtered out:
    /// - already accepted and stored in the main pool (`pool_map`),
    /// - waiting for missing parents in the `orphan` pool,
    /// - waiting for resolution/verification in the `ordered_resolve_queue`,
    /// - undergoing pre-check in the `pre_check_queue`,
    /// - currently being verified (`verify_queue`).
    ///
    /// # Returns
    ///
    /// A new `Vec<ProposalShortId>` containing only the proposals that are **completely new**.
    pub async fn exclude_existing_proposal(
        &self,
        mut proposals: Vec<ProposalShortId>,
    ) -> Vec<ProposalShortId> {
        {
            let ordered = self.queues.ordered_resolve_queue.read().await;
            proposals.retain(|id| !ordered.contains_key(id));
        }
        proposals.retain(|id| !self.queues.pre_check_queue.contains_key(id));
        {
            let verify_queue = self.queues.verify_queue.read().await;
            proposals.retain(|id| !verify_queue.contains_key(id));
        }
        {
            let orphan = self.orphan.read().await;
            proposals.retain(|id| !orphan.contains_key(id));
        }
        {
            let tx_pool = self.tx_pool.read().await;
            proposals.retain(|id| !tx_pool.contains_proposal_id(id));
        }
        proposals
    }

    /// Retrieves transactions required for compact block reconstruction.
    ///
    /// During compact block relay, a node may receive a block that contains transactions
    /// still being verified and not yet present in the main mempool. This method searches
    /// **both** primary locations where a transaction can reside when its short ID is known:
    ///
    /// 1. `pool_map` – the main mempool (already accepted transactions)
    /// 2. `verify_queue` – transactions currently undergoing background validation
    /// 3. `orphan_pool`   – Orphan transactions that are waiting for missing parents
    ///
    /// # Returns
    /// A map containing only the transactions that were found, keyed by their short ID.
    /// Missing entries are simply omitted (caller should treat absence as "need to request")
    /// Returning a `HashMap` allows the caller (compact block reconstructor) to:
    /// - Immediately obtain all locally-available transactions in a single call
    /// - Quickly identify which short IDs are missing
    pub async fn get_tx_for_compact_block(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> HashMap<ProposalShortId, TransactionView> {
        let mut txs = HashMap::with_capacity(short_ids.len());
        {
            let verify_queue = self.queues.verify_queue.read().await;
            txs.extend(short_ids.iter().filter_map(|short_id| {
                verify_queue
                    .get_tx_by_id(short_id)
                    .map(|resolved| (short_id.to_owned(), resolved.tx.to_owned()))
            }));
        }
        {
            let orphan = self.orphan.read().await;
            txs.extend(short_ids.iter().filter_map(|short_id| {
                orphan
                    .get(short_id)
                    .map(|entry| (short_id.to_owned(), entry.tx.to_owned()))
            }));
        }
        {
            let tx_pool = self.tx_pool.read().await;
            txs.extend(short_ids.iter().filter_map(|short_id| {
                tx_pool
                    .get_tx_from_pool_or_store(short_id)
                    .map(|tx| (short_id.to_owned(), tx))
            }));
        }
        txs
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
            if let Err(e) = block_assembler.update_full(&self.tx_pool).await {
                error!("block_assembler update failed {:?}", e);
            }
            block_assembler.notify().await;
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
