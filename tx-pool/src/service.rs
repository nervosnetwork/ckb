use crate::block_assembler::{BlockAssembler, TemplateCache};
use crate::component::{commit_txs_scanner::CommitTxsScanner, entry::TxEntry};
use crate::config::BlockAssemblerConfig;
use crate::config::TxPoolConfig;
use crate::error::{BlockAssemblerError, PoolError};
use crate::pool::{TxPool, TxPoolInfo};
use ckb_dao::DaoCalculator;
use ckb_jsonrpc_types::{
    BlockNumber as JsonBlockNumber, BlockTemplate, Cycle as JsonCycle,
    EpochNumber as JsonEpochNumber, Timestamp as JsonTimestamp, Unsigned, Version as JsonVersion,
};
use ckb_logger::{debug_target, error, info};
use ckb_notify::NotifyController;
use ckb_script::ScriptConfig;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        cell::{
            get_related_dep_out_points, resolve_transaction, OverlayCellProvider,
            ResolvedTransaction, TransactionsProvider,
        },
        service::{Request, DEFAULT_CHANNEL_SIZE, SIGNAL_CHANNEL_SIZE},
        BlockView, Capacity, Cycle, ScriptHashType, TransactionView, Version,
    },
    packed::{self, Byte32, OutPoint, ProposalShortId, Script},
    prelude::*,
};
use ckb_util::LinkedHashSet;
use ckb_util::Mutex;
use ckb_verification::{ContextualTransactionVerifier, TransactionVerifier};
use crossbeam_channel::{self, select, Receiver, Sender};
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use lru_cache::LruCache;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use std::cmp;
use std::collections::{HashMap, HashSet, VecDeque};
use std::iter;
use std::sync::{atomic::Ordering, Arc};
use std::thread;

type BlockTemplateResult = Result<BlockTemplate, FailureError>;
type BlockTemplateRequest =
    Request<(Option<u64>, Option<u64>, Option<Version>), BlockTemplateResult>;

type SubmitTxsResult = Result<Vec<Cycle>, PoolError>;
type SubmitTxs = Request<Vec<TransactionView>, SubmitTxsResult>;

const BLOCK_ASSEMBLER_SUBSCRIBER: &str = "block_assembler";

type ChainReorgResult = ();
type ChainReorg = Request<
    (
        VecDeque<BlockView>,
        VecDeque<BlockView>,
        HashSet<ProposalShortId>,
        Arc<Snapshot>,
    ),
    ChainReorgResult,
>;

type FreshProposalsFilterRequst = Request<Vec<ProposalShortId>, Vec<ProposalShortId>>;
type FetchTxsRequst = Request<HashSet<ProposalShortId>, HashMap<ProposalShortId, TransactionView>>;
type FetchTxsWithCyclesRequst =
    Request<HashSet<ProposalShortId>, HashMap<ProposalShortId, (TransactionView, Cycle)>>;
type GetTxPoolInfoRequst = Request<(), TxPoolInfo>;
type FetchTxRPCRequst = Request<ProposalShortId, Option<(bool, TransactionView)>>;

enum TxStatus {
    Fresh,
    Gap,
    Proposed,
}

#[derive(Clone)]
pub struct TxPoolController {
    block_template_sender: Sender<BlockTemplateRequest>,
    submit_txs_sender: Sender<SubmitTxs>,
    chain_reorg_sender: Sender<ChainReorg>,
    fresh_proposals_filter_sender: Sender<FreshProposalsFilterRequst>,
    fetch_txs_sender: Sender<FetchTxsRequst>,
    fetch_txs_with_cycles_sender: Sender<FetchTxsWithCyclesRequst>,
    get_tx_pool_info_sender: Sender<GetTxPoolInfoRequst>,
    fetch_tx_for_rpc_sender: Sender<FetchTxRPCRequst>,
    stop: StopHandler<()>,
}

impl TxPoolController {
    pub fn get_block_template(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> BlockTemplateResult {
        Request::call(
            &self.block_template_sender,
            (bytes_limit, proposals_limit, max_version),
        )
        .expect("get_block_template() failed")
    }

    pub fn submit_txs(&self, txs: Vec<TransactionView>) -> SubmitTxsResult {
        Request::call(&self.submit_txs_sender, txs).expect("submit_txs() failed")
    }

    pub fn fresh_proposals_filter(&self, proposals: Vec<ProposalShortId>) -> Vec<ProposalShortId> {
        Request::call(&self.fresh_proposals_filter_sender, proposals)
            .expect("fresh_proposals_filter() failed")
    }

    pub fn get_tx_pool_info(&self) -> TxPoolInfo {
        Request::call(&self.get_tx_pool_info_sender, ()).expect("get_tx_pool_info() failed")
    }

    pub fn fetch_tx_for_rpc(&self, id: ProposalShortId) -> Option<(bool, TransactionView)> {
        Request::call(&self.fetch_tx_for_rpc_sender, id).expect("fetch_tx_for_rpc() failed")
    }

    pub fn fetch_txs(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> HashMap<ProposalShortId, TransactionView> {
        Request::call(&self.fetch_txs_sender, short_ids).expect("fetch_txs() failed")
    }

    pub fn fetch_txs_with_cycles(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> HashMap<ProposalShortId, (TransactionView, Cycle)> {
        Request::call(&self.fetch_txs_with_cycles_sender, short_ids)
            .expect("fetch_txs_with_cycles() failed")
    }

    pub fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposal_id: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
    ) {
        Request::call(
            &self.chain_reorg_sender,
            (
                detached_blocks,
                attached_blocks,
                detached_proposal_id,
                snapshot,
            ),
        )
        .expect("update_tx_pool_for_reorg() failed")
    }
}

impl Drop for TxPoolController {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

struct TxPoolReceivers {
    block_template_receiver: Receiver<BlockTemplateRequest>,
    submit_txs_receiver: Receiver<SubmitTxs>,
    chain_reorg_receiver: Receiver<ChainReorg>,
    fresh_proposals_filter_receiver: Receiver<FreshProposalsFilterRequst>,
    fetch_txs_receiver: Receiver<FetchTxsRequst>,
    fetch_txs_with_cycles_receiver: Receiver<FetchTxsWithCyclesRequst>,
    get_tx_pool_info_receiver: Receiver<GetTxPoolInfoRequst>,
    fetch_tx_for_rpc_receiver: Receiver<FetchTxRPCRequst>,
}

pub struct TxPoolServiceBuiler {
    service: Option<TxPoolService>,
}

impl TxPoolServiceBuiler {
    pub fn new(
        tx_pool_config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        script_config: ScriptConfig,
        block_assembler_config: Option<BlockAssemblerConfig>,
        txs_verify_cache: Arc<Mutex<LruCache<Byte32, Cycle>>>,
    ) -> TxPoolServiceBuiler {
        let tx_pool = TxPool::new(tx_pool_config, snapshot, script_config);
        let block_assembler = block_assembler_config.map(BlockAssembler::new);

        TxPoolServiceBuiler {
            service: Some(TxPoolService {
                tx_pool,
                block_assembler,
                txs_verify_cache,
            }),
        }
    }

    pub fn start(mut self, notify: &NotifyController) -> TxPoolController {
        let (signal_sender, signal_receiver) =
            crossbeam_channel::bounded::<()>(SIGNAL_CHANNEL_SIZE);
        let (block_template_sender, block_template_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (submit_txs_sender, submit_txs_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (chain_reorg_sender, chain_reorg_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (fresh_proposals_filter_sender, fresh_proposals_filter_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (fetch_txs_sender, fetch_txs_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (fetch_txs_with_cycles_sender, fetch_txs_with_cycles_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (get_tx_pool_info_sender, get_tx_pool_info_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (fetch_tx_for_rpc_sender, fetch_tx_for_rpc_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);

        let thread_builder = thread::Builder::new().name("TX-POOL".to_string());

        let receivers = TxPoolReceivers {
            block_template_receiver,
            submit_txs_receiver,
            chain_reorg_receiver,
            fresh_proposals_filter_receiver,
            fetch_txs_receiver,
            fetch_txs_with_cycles_receiver,
            get_tx_pool_info_receiver,
            fetch_tx_for_rpc_receiver,
        };

        let new_uncle_receiver = notify.subscribe_new_uncle(BLOCK_ASSEMBLER_SUBSCRIBER);

        let service = self.service.take().expect("tx pool service start once");

        let thread = thread_builder
            .spawn(move || {
                let mut service = service;
                loop {
                    select! {
                        recv(signal_receiver) -> _ => {
                            break;
                        },
                        recv(receivers.chain_reorg_receiver) -> msg => match msg {
                            Ok(Request { responder, arguments: (detached_blocks, attached_blocks, detached_proposal_id, snapshot ) }) => {
                                let _ = responder.send(
                                    service.update_tx_pool_for_reorg(
                                        detached_blocks,
                                        attached_blocks,
                                        detached_proposal_id,
                                        snapshot,
                                    )
                                );
                            },
                            _ => {
                                error!("chain_reorg_receiver closed");
                                break;
                            },
                        },
                        recv(new_uncle_receiver) -> msg => match msg {
                            Ok(uncle_block) => {
                                service.block_assembler.as_mut().map(|block_assembler| {
                                    block_assembler.candidate_uncles.insert(uncle_block);
                                    block_assembler.last_uncles_updated_at
                                        .store(unix_time_as_millis(), Ordering::SeqCst);
                                });
                            }
                            _ => {
                                error!("new_uncle_receiver closed");
                                break;
                            }
                        },
                        recv(receivers.block_template_receiver) -> msg => match msg {
                            Ok(Request { responder, arguments: (bytes_limit, proposals_limit,  max_version) }) => {
                                let _ = responder.send(
                                    service.get_block_template(
                                        bytes_limit,
                                        proposals_limit,
                                        max_version,
                                    )
                                );
                            },
                            _ => {
                                error!("block_template_receiver closed");
                                break;
                            },
                        },
                        recv(receivers.submit_txs_receiver) -> msg => match msg {
                            Ok(Request { responder, arguments: txs }) => {
                                let _ = responder.send(service.submit_txs(txs));
                            },
                            _ => {
                                error!("submit_txs_receiver closed");
                                break;
                            },
                        },
                        recv(receivers.fresh_proposals_filter_receiver) -> msg => match msg {
                            Ok(Request { responder, arguments: proposals }) => {
                                let _ = responder.send(service.fresh_proposals_filter(proposals));
                            },
                            _ => {
                                error!("fresh_proposals_filter_receiver closed");
                                break;
                            },
                        },
                        recv(receivers.fetch_txs_receiver) -> msg => match msg {
                            Ok(Request { responder, arguments: short_ids }) => {
                                let _ = responder.send(service.fetch_txs(short_ids));
                            },
                            _ => {
                                error!("fetch_txs_receiver closed");
                                break;
                            },
                        },
                        recv(receivers.fetch_txs_with_cycles_receiver) -> msg => match msg {
                            Ok(Request { responder, arguments: short_ids }) => {
                                let _ = responder.send(service.fetch_txs_with_cycles(short_ids));
                            },
                            _ => {
                                error!("fetch_txs_with_cycles_receiver closed");
                                break;
                            },
                        },
                        recv(receivers.get_tx_pool_info_receiver) -> msg => match msg {
                            Ok(Request { responder, arguments: _ }) => {
                                let _ = responder.send(service.get_tx_pool_info());
                            },
                            _ => {
                                error!("get_tx_pool_info_receiver closed");
                                break;
                            },
                        },
                        recv(receivers.fetch_tx_for_rpc_receiver) -> msg => match msg {
                            Ok(Request { responder, arguments: id }) => {
                                let _ = responder.send(service.fetch_tx_for_rpc(&id));
                            },
                            _ => {
                                error!("fetch_tx_for_rpc_receiver closed");
                                break;
                            },
                        },
                    }
                }
            }).expect("Start MinerAgent failed");

        let stop = StopHandler::new(SignalSender::Crossbeam(signal_sender), thread);

        TxPoolController {
            block_template_sender,
            submit_txs_sender,
            chain_reorg_sender,
            fresh_proposals_filter_sender,
            fetch_txs_sender,
            fetch_txs_with_cycles_sender,
            get_tx_pool_info_sender,
            fetch_tx_for_rpc_sender,
            stop,
        }
    }
}

pub struct TxPoolService {
    tx_pool: TxPool,
    block_assembler: Option<BlockAssembler>,
    txs_verify_cache: Arc<Mutex<LruCache<Byte32, Cycle>>>,
}

impl TxPoolService {
    pub fn new(
        tx_pool: TxPool,
        block_assembler: Option<BlockAssembler>,
        txs_verify_cache: Arc<Mutex<LruCache<Byte32, Cycle>>>,
    ) -> Self {
        Self {
            tx_pool,
            block_assembler,
            txs_verify_cache,
        }
    }

    fn fresh_proposals_filter(&self, mut proposals: Vec<ProposalShortId>) -> Vec<ProposalShortId> {
        proposals.retain(|id| !self.tx_pool.contains_proposal_id(&id));
        proposals
    }

    fn get_tx_pool_info(&self) -> TxPoolInfo {
        self.tx_pool.info()
    }

    fn fetch_tx_for_rpc(&self, id: &ProposalShortId) -> Option<(bool, TransactionView)> {
        self.tx_pool
            .proposed()
            .get(id)
            .map(|entry| (true, entry.transaction.clone()))
            .or_else(|| {
                self.tx_pool
                    .get_tx_without_conflict(&id)
                    .map(|tx| (false, tx))
            })
    }

    fn fetch_txs(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> HashMap<ProposalShortId, TransactionView> {
        short_ids
            .into_iter()
            .filter_map(|short_id| {
                if let Some(tx) = self.tx_pool.get_tx_from_pool_or_store(&short_id) {
                    Some((short_id, tx))
                } else {
                    None
                }
            })
            .collect()
    }

    fn fetch_txs_with_cycles(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> HashMap<ProposalShortId, (TransactionView, Cycle)> {
        short_ids
            .into_iter()
            .filter_map(|short_id| {
                self.tx_pool
                    .get_tx_with_cycles(&short_id)
                    .and_then(|(tx, cycles)| cycles.map(|cycles| (short_id, (tx, cycles))))
            })
            .collect()
    }

    fn submit_txs(&mut self, txs: Vec<TransactionView>) -> SubmitTxsResult {
        debug_assert!(!txs.is_empty(), "txs should not be empty!");
        let snapshot = self.tx_pool.snapshot();
        let mut txs_provider = TransactionsProvider::default();
        let resolved = txs
            .iter()
            .map(|tx| {
                let ret = self.resolve_tx(snapshot, &txs_provider, tx);
                txs_provider.insert(tx);
                ret
            })
            .collect::<Result<Vec<(ResolvedTransaction<'_>, usize, Capacity, TxStatus)>, _>>()?;

        let verified_cycles = self.verify_rtxs(snapshot, &resolved[..])?;

        for ((tx, (rtx, tx_size, fee, status)), cycle) in txs
            .iter()
            .zip(resolved.into_iter())
            .zip(verified_cycles.iter())
        {
            let related_dep_out_points = rtx.related_dep_out_points();
            let entry = TxEntry::new(tx.clone(), *cycle, fee, tx_size, related_dep_out_points);
            if match status {
                TxStatus::Fresh => self.tx_pool.add_pending(entry),
                TxStatus::Gap => self.tx_pool.add_gap(entry),
                TxStatus::Proposed => self.tx_pool.add_proposed(entry),
            } {
                self.tx_pool.update_statics_for_add_tx(tx_size, *cycle);
            }
        }

        Ok(verified_cycles)
    }

    fn verify_rtxs<'a>(
        &self,
        snapshot: &Snapshot,
        rtxs: &'a [(ResolvedTransaction<'_>, usize, Capacity, TxStatus)],
    ) -> Result<Vec<Cycle>, PoolError> {
        let mut txs_verify_cache = self.txs_verify_cache.lock();
        let tip_header = snapshot.tip_header();
        let tip_number = tip_header.number();
        let epoch_number = tip_header.epoch();
        let consensus = snapshot.consensus();
        let verified = rtxs
            .par_iter()
            .map(|(tx, _, _, _)| {
                let tx_hash = tx.transaction.hash();
                if let Some(cycles) = txs_verify_cache.get(&tx_hash) {
                    if self.tx_pool.reach_cycles_limit(*cycles) {
                        Err(PoolError::LimitReached)
                    } else {
                        ContextualTransactionVerifier::new(
                            &tx,
                            snapshot,
                            tip_number + 1,
                            epoch_number,
                            tip_header.hash(),
                            consensus,
                        )
                        .verify()
                        .map_err(PoolError::InvalidTx)
                        .map(|_| (tx_hash, *cycles))
                    }
                } else {
                    TransactionVerifier::new(
                        &tx,
                        snapshot,
                        tip_number + 1,
                        epoch_number,
                        tip_header.hash(),
                        consensus,
                        &self.tx_pool.script_config,
                        snapshot,
                    )
                    .verify(consensus.max_block_cycles())
                    .map_err(PoolError::InvalidTx)
                    .and_then(|cycles| {
                        if self.tx_pool.reach_cycles_limit(cycles) {
                            Err(PoolError::LimitReached)
                        } else {
                            Ok((tx_hash, cycles))
                        }
                    })
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut ret = Vec::with_capacity(verified.len());
        for (hash, cycles) in verified {
            txs_verify_cache.insert(hash, cycles);
            ret.push(cycles);
        }

        Ok(ret)
    }

    fn resolve_tx<'a, 'b>(
        &self,
        snapshot: &Snapshot,
        txs_provider: &'b TransactionsProvider<'b>,
        tx: &'a TransactionView,
    ) -> Result<(ResolvedTransaction<'a>, usize, Capacity, TxStatus), PoolError> {
        let tx_size = tx.serialized_size();
        if self.tx_pool.reach_size_limit(tx_size) {
            return Err(PoolError::LimitReached);
        }

        let short_id = tx.proposal_short_id();
        if snapshot.proposals().contains_proposed(&short_id) {
            self.resolve_tx_from_proposed(snapshot, txs_provider, tx)
                .and_then(|rtx| {
                    let fee = self.tx_pool.calculate_transaction_fee(snapshot, &rtx);
                    fee.map(|fee| (rtx, tx_size, fee, TxStatus::Proposed))
                })
        } else {
            self.resolve_tx_from_pending_and_proposed(snapshot, txs_provider, tx)
                .and_then(|rtx| {
                    let status = if snapshot.proposals().contains_gap(&short_id) {
                        TxStatus::Gap
                    } else {
                        TxStatus::Fresh
                    };
                    let fee = self.tx_pool.calculate_transaction_fee(snapshot, &rtx);
                    fee.map(|fee| (rtx, tx_size, fee, status))
                })
        }
    }

    fn resolve_tx_from_proposed<'a, 'b>(
        &self,
        snapshot: &Snapshot,
        txs_provider: &'b TransactionsProvider<'b>,
        tx: &'a TransactionView,
    ) -> Result<ResolvedTransaction<'a>, PoolError> {
        let cell_provider = OverlayCellProvider::new(&self.tx_pool.proposed, snapshot);
        let provider = OverlayCellProvider::new(txs_provider, &cell_provider);
        resolve_transaction(tx, &mut HashSet::new(), &provider, snapshot)
            .map_err(PoolError::UnresolvableTransaction)
    }

    fn resolve_tx_from_pending_and_proposed<'a, 'b>(
        &self,
        snapshot: &Snapshot,
        txs_provider: &'b TransactionsProvider<'b>,
        tx: &'a TransactionView,
    ) -> Result<ResolvedTransaction<'a>, PoolError> {
        let proposed_provider = OverlayCellProvider::new(&self.tx_pool.proposed, snapshot);
        let gap_and_proposed_provider =
            OverlayCellProvider::new(&self.tx_pool.gap, &proposed_provider);
        let pending_and_proposed_provider =
            OverlayCellProvider::new(&self.tx_pool.pending, &gap_and_proposed_provider);
        let provider = OverlayCellProvider::new(txs_provider, &pending_and_proposed_provider);
        resolve_transaction(tx, &mut HashSet::new(), &provider, snapshot)
            .map_err(PoolError::UnresolvableTransaction)
    }

    pub fn update_tx_pool_for_reorg(
        &mut self,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposal_id: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
    ) {
        self.tx_pool.snapshot = Arc::clone(&snapshot);
        let mut detached = LinkedHashSet::default();
        let mut attached = LinkedHashSet::default();

        for blk in detached_blocks {
            detached.extend(blk.transactions().iter().skip(1).cloned())
        }

        for blk in attached_blocks {
            attached.extend(blk.transactions().iter().skip(1).cloned())
        }

        let retain: Vec<TransactionView> = detached.difference(&attached).cloned().collect();

        let txs_iter = attached.iter().map(|tx| {
            let get_cell_data = |out_point: &OutPoint| {
                snapshot
                    .get_cell_data(&out_point.tx_hash(), out_point.index().unpack())
                    .map(|result| result.0)
            };
            let related_out_points =
                get_related_dep_out_points(tx, get_cell_data).expect("Get dep out points failed");
            (tx, related_out_points)
        });
        self.tx_pool.remove_expired(detached_proposal_id.iter());
        self.tx_pool.remove_committed_txs_from_proposed(txs_iter);
        {
            let mut txs_verify_cache = self.txs_verify_cache.lock();

            for tx in retain {
                self.tx_pool
                    .readd_dettached_tx(&snapshot, &mut txs_verify_cache, tx);
            }
        }

        for tx in &attached {
            self.tx_pool.try_proposed_orphan_by_ancestor(tx);
        }

        let mut entries = Vec::new();
        let mut gaps = Vec::new();

        // pending ---> gap ----> proposed
        // try move gap to proposed
        let mut removed: Vec<ProposalShortId> = Vec::with_capacity(self.tx_pool.gap.size());
        for id in self.tx_pool.gap.sorted_keys() {
            if snapshot.proposals().contains_proposed(&id) {
                let entry = self.tx_pool.gap.get(&id).expect("exists");
                entries.push((Some(entry.cycles), entry.size, entry.transaction.to_owned()));
                removed.push(id.clone());
            }
        }
        removed.into_iter().for_each(|id| {
            self.tx_pool.gap.remove_entry_and_descendants(&id);
        });

        // try move pending to proposed
        let mut removed: Vec<ProposalShortId> = Vec::with_capacity(self.tx_pool.pending.size());
        for id in self.tx_pool.pending.sorted_keys() {
            let entry = self.tx_pool.pending.get(&id).expect("exists");
            if snapshot.proposals().contains_proposed(&id) {
                entries.push((Some(entry.cycles), entry.size, entry.transaction.to_owned()));
                removed.push(id.clone());
            } else if snapshot.proposals().contains_gap(&id) {
                gaps.push((Some(entry.cycles), entry.size, entry.transaction.to_owned()));
                removed.push(id.clone());
            }
        }
        removed.into_iter().for_each(|id| {
            self.tx_pool.pending.remove_entry_and_descendants(&id);
        });

        // try move conflict to proposed
        for entry in self.tx_pool.conflict.entries() {
            if snapshot.proposals().contains_proposed(entry.key()) {
                let entry = entry.remove();
                entries.push((entry.cycles, entry.size, entry.transaction));
            } else if snapshot.proposals().contains_gap(entry.key()) {
                let entry = entry.remove();
                gaps.push((entry.cycles, entry.size, entry.transaction));
            }
        }

        for (cycles, size, tx) in entries {
            let tx_hash = tx.hash().to_owned();
            if let Err(e) = self.tx_pool.proposed_tx_and_descendants(cycles, size, tx) {
                debug_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to add proposed tx {}, reason: {:?}",
                    tx_hash,
                    e
                );
            }
        }

        for (cycles, size, tx) in gaps {
            debug_target!(
                crate::LOG_TARGET_TX_POOL,
                "tx proposed, add to gap {}",
                tx.hash()
            );
            let tx_hash = tx.hash().to_owned();
            if let Err(e) = self.tx_pool.gap_tx(cycles, size, tx) {
                debug_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to add tx to gap {}, reason: {:?}",
                    tx_hash,
                    e
                );
            }
        }
    }

    fn get_block_template(
        &mut self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> BlockTemplateResult {
        if self.block_assembler.is_none() {
            return Err(BlockAssemblerError::Disabled.into());
        }

        let block_assembler = self.block_assembler.as_mut().unwrap();
        let snapshot = self.tx_pool.snapshot();
        let last_txs_updated_at = self.tx_pool.get_last_txs_updated_at();
        let consensus = snapshot.consensus();

        let cycles_limit = consensus.max_block_cycles();
        let (bytes_limit, proposals_limit, version) =
            block_assembler.transform_params(consensus, bytes_limit, proposals_limit, max_version);
        let uncles_count_limit = consensus.max_uncles_num() as u32;

        let last_uncles_updated_at = block_assembler.load_last_uncles_updated_at();

        // try get cache
        let tip_header = snapshot.get_tip_header().expect("get tip header");
        let tip_hash = tip_header.hash();
        let candidate_number = tip_header.number() + 1;
        let current_time = cmp::max(unix_time_as_millis(), tip_header.timestamp() + 1);
        if let Some(template_cache) = block_assembler.template_caches.get(&(
            tip_header.hash().unpack(),
            cycles_limit,
            bytes_limit,
            version,
        )) {
            // check template cache outdate time
            if !template_cache.is_outdate(current_time) {
                let mut template = template_cache.template.clone();
                template.current_time = JsonTimestamp(current_time);
                return Ok(template);
            }

            if !template_cache.is_modified(last_uncles_updated_at, last_txs_updated_at) {
                let mut template = template_cache.template.clone();
                template.current_time = JsonTimestamp(current_time);
                return Ok(template);
            }
        }

        let last_epoch = snapshot.get_current_epoch_ext().expect("current epoch ext");
        let next_epoch_ext = snapshot.next_epoch_ext(consensus, &last_epoch, &tip_header);
        let current_epoch = next_epoch_ext.unwrap_or(last_epoch);
        let uncles = block_assembler.prepare_uncles(&snapshot, candidate_number, &current_epoch);

        let block_assembler_config = &block_assembler.config;

        let cellbase_lock_args = block_assembler_config
            .args
            .clone()
            .into_iter()
            .map(Into::into)
            .collect::<Vec<packed::Bytes>>();

        let hash_type: ScriptHashType = block_assembler_config.hash_type.clone().into();
        let cellbase_lock = Script::new_builder()
            .args(cellbase_lock_args.pack())
            .code_hash(block_assembler_config.code_hash.pack())
            .hash_type(hash_type.pack())
            .build();

        let cellbase = block_assembler.build_cellbase(&snapshot, &tip_header, cellbase_lock)?;

        let (proposals, entries, last_txs_updated_at) = {
            let proposals = self.tx_pool.get_proposals(proposals_limit as usize);
            let txs_size_limit = block_assembler.calculate_txs_size_limit(
                bytes_limit,
                cellbase.data(),
                &uncles,
                &proposals,
            )?;

            let (entries, size, cycles) = CommitTxsScanner::new(self.tx_pool.proposed())
                .txs_to_commit(txs_size_limit, cycles_limit);
            if !entries.is_empty() {
                info!(
                    "[get_block_template] candidate txs count: {}, size: {}/{}, cycles:{}/{}",
                    entries.len(),
                    size,
                    txs_size_limit,
                    cycles,
                    cycles_limit
                );
            }
            (proposals, entries, last_txs_updated_at)
        };

        let mut txs = iter::once(&cellbase).chain(entries.iter().map(|entry| &entry.transaction));

        let mut seen_inputs = HashSet::new();
        let transactions_provider = TransactionsProvider::new(txs.clone());
        let overlay_cell_provider = OverlayCellProvider::new(&transactions_provider, snapshot);

        let rtxs = txs
            .try_fold(vec![], |mut rtxs, tx| {
                match resolve_transaction(tx, &mut seen_inputs, &overlay_cell_provider, snapshot) {
                    Ok(rtx) => {
                        rtxs.push(rtx);
                        Ok(rtxs)
                    }
                    Err(e) => Err(e),
                }
            })
            .map_err(|_| BlockAssemblerError::InvalidInput)?;
        // Generate DAO fields here
        let dao = DaoCalculator::new(consensus, snapshot).dao_field(&rtxs, &tip_header)?;

        // Should recalculate current time after create cellbase (create cellbase may spend a lot of time)
        let current_time = cmp::max(unix_time_as_millis(), tip_header.timestamp() + 1);
        let template = BlockTemplate {
            version: JsonVersion(version),
            difficulty: current_epoch.difficulty().clone(),
            current_time: JsonTimestamp(current_time),
            number: JsonBlockNumber(candidate_number),
            epoch: JsonEpochNumber(current_epoch.number()),
            parent_hash: tip_hash.unpack(),
            cycles_limit: JsonCycle(cycles_limit),
            bytes_limit: Unsigned(bytes_limit),
            uncles_count_limit: Unsigned(uncles_count_limit.into()),
            uncles: uncles
                .into_iter()
                .map(BlockAssembler::transform_uncle)
                .collect(),
            transactions: entries
                .iter()
                .map(|entry| BlockAssembler::transform_tx(entry, false, None))
                .collect(),
            proposals: proposals.into_iter().map(Into::into).collect(),
            cellbase: BlockAssembler::transform_cellbase(&cellbase, None),
            work_id: Unsigned(block_assembler.work_id.fetch_add(1, Ordering::SeqCst) as u64),
            dao: dao.into(),
        };

        block_assembler.template_caches.insert(
            (tip_hash.unpack(), cycles_limit, bytes_limit, version),
            TemplateCache {
                time: current_time,
                uncles_updated_at: last_uncles_updated_at,
                txs_updated_at: last_txs_updated_at,
                template: template.clone(),
            },
        );

        Ok(template)
    }
}
