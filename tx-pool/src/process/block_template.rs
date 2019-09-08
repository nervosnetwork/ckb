use crate::block_assembler::{
    BlockAssembler, BlockTemplateCacheKey, CandidateUncles, TemplateCache,
};
use crate::component::commit_txs_scanner::CommitTxsScanner;
use crate::component::entry::TxEntry;
use crate::config::BlockAssemblerConfig;
use crate::error::BlockAssemblerError;
use crate::pool::TxPool;
use ckb_dao::DaoCalculator;
use ckb_jsonrpc_types::{
    BlockNumber as JsonBlockNumber, BlockTemplate, Cycle as JsonCycle,
    EpochNumber as JsonEpochNumber, Timestamp as JsonTimestamp, Unsigned, Version as JsonVersion,
};
use ckb_logger::info;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        cell::{resolve_transaction, OverlayCellProvider, TransactionsProvider},
        Cycle, EpochExt, ScriptHashType, TransactionView, UncleBlockView, Version,
    },
    packed::{self, ProposalShortId, Script},
    prelude::*,
};
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use futures::future::Future;
use lru_cache::LruCache;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::{cmp, iter};
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

type Args = (u64, u64, Version);

pub struct BlockTemplateCacheProcess {
    pub template_caches: Lock<LruCache<BlockTemplateCacheKey, TemplateCache>>,
    pub last_txs_updated_at: Arc<AtomicU64>,
    pub last_uncles_updated_at: Arc<AtomicU64>,
    pub snapshot: Arc<Snapshot>,
    pub args: Args,
}

impl BlockTemplateCacheProcess {
    pub fn new(
        template_caches: Lock<LruCache<BlockTemplateCacheKey, TemplateCache>>,
        last_txs_updated_at: Arc<AtomicU64>,
        last_uncles_updated_at: Arc<AtomicU64>,
        snapshot: Arc<Snapshot>,
        args: Args,
    ) -> Self {
        BlockTemplateCacheProcess {
            template_caches,
            last_txs_updated_at,
            last_uncles_updated_at,
            snapshot,
            args,
        }
    }
}

impl Future for BlockTemplateCacheProcess {
    type Item = BlockTemplate;
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.template_caches.poll_lock() {
            Async::Ready(guard) => {
                let (bytes_limit, proposals_limit, version) = self.args;
                let tip_header = self.snapshot.get_tip_header().expect("get tip header");
                let tip_hash = tip_header.hash();
                let current_time = cmp::max(unix_time_as_millis(), tip_header.timestamp() + 1);

                let last_uncles_updated_at = self.last_uncles_updated_at.load(Ordering::SeqCst);
                let last_txs_updated_at = self.last_txs_updated_at.load(Ordering::SeqCst);
                if let Some(template_cache) =
                    guard.get(&(tip_hash, bytes_limit, proposals_limit, version))
                {
                    // check template cache outdate time
                    if !template_cache.is_outdate(current_time) {
                        let mut template = template_cache.template.clone();
                        template.current_time = JsonTimestamp(current_time);
                        return Ok(Async::Ready(template));
                    }

                    if !template_cache.is_modified(last_uncles_updated_at, last_txs_updated_at) {
                        let mut template = template_cache.template.clone();
                        template.current_time = JsonTimestamp(current_time);
                        return Ok(Async::Ready(template));
                    }
                }
                Err(())
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}

pub struct BuildCellbaseProcess {
    pub snapshot: Arc<Snapshot>,
    pub config: Arc<BlockAssemblerConfig>,
}

impl BuildCellbaseProcess {
    pub fn new(snapshot: Arc<Snapshot>, config: Arc<BlockAssemblerConfig>) -> Self {
        BuildCellbaseProcess { snapshot, config }
    }
}

impl Future for BuildCellbaseProcess {
    type Item = TransactionView;
    type Error = FailureError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let tip_header = self.snapshot.get_tip_header().expect("get tip header");
        let cellbase_lock_args = self
            .config
            .args
            .clone()
            .into_iter()
            .map(Into::into)
            .collect::<Vec<packed::Bytes>>();

        let hash_type: ScriptHashType = self.config.hash_type.clone().into();
        let cellbase_lock = Script::new_builder()
            .args(cellbase_lock_args.pack())
            .code_hash(self.config.code_hash.pack())
            .hash_type(hash_type.pack())
            .build();

        let cellbase = BlockAssembler::build_cellbase(
            &self.config,
            &self.snapshot,
            &tip_header,
            cellbase_lock,
        )?;

        Ok(Async::Ready(cellbase))
    }
}

pub struct PrepareUnclesProcess {
    pub snapshot: Arc<Snapshot>,
    pub last_uncles_updated_at: Arc<AtomicU64>,
    pub candidate_uncles: Lock<CandidateUncles>,
}

impl Future for PrepareUnclesProcess {
    type Item = (Vec<UncleBlockView>, EpochExt, u64);
    type Error = FailureError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.candidate_uncles.poll_lock() {
            Async::Ready(mut guard) => {
                let consensus = self.snapshot.consensus();
                let tip_header = self.snapshot.get_tip_header().expect("get tip header");
                let last_epoch = self
                    .snapshot
                    .get_current_epoch_ext()
                    .expect("current epoch ext");
                let next_epoch_ext =
                    self.snapshot
                        .next_epoch_ext(consensus, &last_epoch, &tip_header);
                let current_epoch = next_epoch_ext.unwrap_or(last_epoch);
                let candidate_number = tip_header.number() + 1;
                let uncles = BlockAssembler::prepare_uncles(
                    &self.snapshot,
                    candidate_number,
                    &current_epoch,
                    &mut guard,
                );
                let last_uncles_updated_at = self.last_uncles_updated_at.load(Ordering::SeqCst);
                Ok(Async::Ready((
                    uncles,
                    current_epoch,
                    last_uncles_updated_at,
                )))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}

pub struct PackageTxsProcess {
    pub tx_pool: Lock<TxPool>,
    pub bytes_limit: u64,
    pub proposals_limit: u64,
    pub max_block_cycles: Cycle,
    pub last_txs_updated_at: Arc<AtomicU64>,
    pub cellbase: TransactionView,
    pub uncles: Vec<UncleBlockView>,
}

impl Future for PackageTxsProcess {
    type Item = (HashSet<ProposalShortId>, Vec<TxEntry>, u64);
    type Error = FailureError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(guard) => {
                let proposals = guard.get_proposals(self.proposals_limit as usize);

                let txs_size_limit = BlockAssembler::calculate_txs_size_limit(
                    self.bytes_limit,
                    self.cellbase.data(),
                    &self.uncles,
                    &proposals,
                )?;

                let proposals = guard.get_proposals(self.proposals_limit as usize);
                let (entries, size, cycles) = CommitTxsScanner::new(guard.proposed())
                    .txs_to_commit(txs_size_limit, self.max_block_cycles);
                if !entries.is_empty() {
                    info!(
                        "[get_block_template] candidate txs count: {}, size: {}/{}, cycles:{}/{}",
                        entries.len(),
                        size,
                        txs_size_limit,
                        cycles,
                        self.max_block_cycles
                    );
                }
                let last_txs_updated_at = self.last_txs_updated_at.load(Ordering::SeqCst);
                Ok(Async::Ready((proposals, entries, last_txs_updated_at)))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}

pub struct BlockTemplateBuilder {
    pub snapshot: Arc<Snapshot>,
    pub entries: Vec<TxEntry>,
    pub proposals: HashSet<ProposalShortId>,
    pub cellbase: TransactionView,
    pub work_id: Arc<AtomicUsize>,
    pub current_epoch: EpochExt,
    pub uncles: Vec<UncleBlockView>,
    pub args: Args,
    pub uncles_updated_at: u64,
    pub txs_updated_at: u64,
}

impl Future for BlockTemplateBuilder {
    type Item = (BlockTemplate, u64, u64);
    type Error = FailureError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let consensus = self.snapshot.consensus();
        let tip_header = self.snapshot.get_tip_header().expect("get tip header");
        let snapshot: &Snapshot = &self.snapshot;
        let tip_hash = tip_header.hash();
        let mut txs =
            iter::once(&self.cellbase).chain(self.entries.iter().map(|entry| &entry.transaction));
        let mut seen_inputs = HashSet::new();
        let transactions_provider = TransactionsProvider::new(txs.clone());
        let overlay_cell_provider = OverlayCellProvider::new(&transactions_provider, snapshot);

        let rtxs = txs
            .try_fold(vec![], |mut rtxs, tx| {
                resolve_transaction(tx, &mut seen_inputs, &overlay_cell_provider, snapshot).map(
                    |rtx| {
                        rtxs.push(rtx);
                        rtxs
                    },
                )
            })
            .map_err(|_| BlockAssemblerError::InvalidInput)?;

        // Generate DAO fields here
        let dao = DaoCalculator::new(consensus, snapshot).dao_field(&rtxs, &tip_header)?;

        let candidate_number = tip_header.number() + 1;
        let (bytes_limit, _, version) = self.args;
        let cycles_limit = consensus.max_block_cycles();
        let uncles_count_limit = consensus.max_uncles_num() as u32;

        // Should recalculate current time after create cellbase (create cellbase may spend a lot of time)
        let current_time = cmp::max(unix_time_as_millis(), candidate_number);
        Ok(Async::Ready((
            BlockTemplate {
                version: JsonVersion(version),
                difficulty: self.current_epoch.difficulty().clone(),
                current_time: JsonTimestamp(current_time),
                number: JsonBlockNumber(candidate_number),
                epoch: JsonEpochNumber(self.current_epoch.number()),
                parent_hash: tip_hash.unpack(),
                cycles_limit: JsonCycle(cycles_limit),
                bytes_limit: Unsigned(bytes_limit),
                uncles_count_limit: Unsigned(uncles_count_limit.into()),
                uncles: self
                    .uncles
                    .iter()
                    .map(BlockAssembler::transform_uncle)
                    .collect(),
                transactions: self
                    .entries
                    .iter()
                    .map(|entry| BlockAssembler::transform_tx(entry, false, None))
                    .collect(),
                proposals: self.proposals.iter().cloned().map(Into::into).collect(),
                cellbase: BlockAssembler::transform_cellbase(&self.cellbase, None),
                work_id: Unsigned(self.work_id.fetch_add(1, Ordering::SeqCst) as u64),
                dao: dao.into(),
            },
            self.uncles_updated_at,
            self.txs_updated_at,
        )))
    }
}

pub struct UpdateBlockTemplateCache {
    template_caches: Lock<LruCache<BlockTemplateCacheKey, TemplateCache>>,
    key: Option<BlockTemplateCacheKey>,
    uncles_updated_at: u64,
    txs_updated_at: u64,
    template: Option<BlockTemplate>,
}

impl UpdateBlockTemplateCache {
    pub fn new(
        template_caches: Lock<LruCache<BlockTemplateCacheKey, TemplateCache>>,
        key: BlockTemplateCacheKey,
        uncles_updated_at: u64,
        txs_updated_at: u64,
        template: BlockTemplate,
    ) -> Self {
        UpdateBlockTemplateCache {
            template_caches,
            key: Some(key),
            uncles_updated_at,
            txs_updated_at,
            template: Some(template),
        }
    }
}

impl Future for UpdateBlockTemplateCache {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.template_caches.poll_lock() {
            Async::Ready(mut guard) => {
                let key = self.key.take().expect("cannot poll twice");
                let template = self.template.take().expect("cannot poll twice");
                guard.insert(
                    key,
                    TemplateCache {
                        time: template.current_time.0,
                        uncles_updated_at: self.uncles_updated_at,
                        txs_updated_at: self.txs_updated_at,
                        template,
                    },
                );
                Ok(Async::Ready(()))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
