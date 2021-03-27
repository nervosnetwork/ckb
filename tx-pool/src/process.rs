use crate::block_assembler::{BlockAssembler, BlockTemplateCacheKey, TemplateCache};
use crate::callback::Callbacks;
use crate::component::commit_txs_scanner::CommitTxsScanner;
use crate::component::entry::TxEntry;
use crate::error::Reject;
use crate::pool::TxPool;
use crate::service::TxPoolService;
use crate::util::{
    check_tx_cycle_limit, check_tx_fee, check_tx_size_limit, check_txid_collision,
    non_contextual_verify, verify_rtx,
};
use ckb_app_config::BlockAssemblerConfig;
use ckb_dao::DaoCalculator;
use ckb_error::{AnyError, Error, InternalErrorKind};
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::{debug, error, info};
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        cell::{
            get_related_dep_out_points, resolve_transaction, OverlayCellChecker,
            OverlayCellProvider, ResolvedTransaction, TransactionsChecker,
        },
        BlockView, Capacity, Cycle, EpochExt, ScriptHashType, TransactionView, UncleBlockView,
        Version,
    },
    packed::{Byte32, CellbaseWitness, OutPoint, ProposalShortId, Script},
    prelude::*,
};
use ckb_util::LinkedHashSet;
use ckb_verification::{
    cache::CacheEntry, ContextualTransactionVerifier, NonContextualTransactionVerifier,
    TimeRelativeTransactionVerifier,
};
use faketime::unix_time_as_millis;
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{atomic::AtomicU64, Arc};
use std::{cmp, iter};
use tokio::task::block_in_place;

/// A list for plug target for `plug_entry` method
pub enum PlugTarget {
    /// Pending pool
    Pending,
    /// Proposed pool
    Proposed,
}

pub enum TxStatus {
    Fresh,
    Gap,
    Proposed,
}

impl TxPoolService {
    async fn get_block_template_cache(
        &self,
        bytes_limit: u64,
        proposals_limit: u64,
        version: Version,
        snapshot: &Snapshot,
        block_assembler: &BlockAssembler,
    ) -> Option<BlockTemplate> {
        let tip_header = snapshot.tip_header();
        let tip_hash = tip_header.hash();
        let current_time = cmp::max(unix_time_as_millis(), tip_header.timestamp() + 1);

        let last_uncles_updated_at = block_assembler
            .last_uncles_updated_at
            .load(Ordering::SeqCst);
        let last_txs_updated_at = self.last_txs_updated_at.load(Ordering::SeqCst);
        if let Some(template_cache) = block_assembler.template_caches.lock().await.peek(&(
            tip_hash,
            bytes_limit,
            proposals_limit,
            version,
        )) {
            // check template cache outdate time
            if !template_cache.is_outdate(current_time) {
                let mut template = template_cache.template.clone();
                template.current_time = current_time.into();
                return Some(template);
            }

            if !template_cache.is_modified(last_uncles_updated_at, last_txs_updated_at) {
                let mut template = template_cache.template.clone();
                template.current_time = current_time.into();
                return Some(template);
            }
        }

        None
    }

    fn build_block_template_cellbase(
        &self,
        snapshot: &Snapshot,
        config: &BlockAssemblerConfig,
    ) -> Result<TransactionView, AnyError> {
        let hash_type: ScriptHashType = config.hash_type.clone().into();
        let cellbase_lock = Script::new_builder()
            .args(config.args.as_bytes().pack())
            .code_hash(config.code_hash.pack())
            .hash_type(hash_type.into())
            .build();
        let cellbase_witness = CellbaseWitness::new_builder()
            .lock(cellbase_lock)
            .message(config.message.as_bytes().pack())
            .build();

        BlockAssembler::build_cellbase(snapshot, snapshot.tip_header(), cellbase_witness)
    }

    async fn prepare_block_template_uncles(
        &self,
        snapshot: &Snapshot,
        block_assembler: &BlockAssembler,
    ) -> (Vec<UncleBlockView>, EpochExt, u64) {
        let consensus = snapshot.consensus();
        let tip_header = snapshot.tip_header();
        let current_epoch = consensus
            .next_epoch_ext(tip_header, &snapshot.as_data_provider())
            .expect("tip header's epoch should be stored")
            .epoch();
        let candidate_number = tip_header.number() + 1;

        let mut guard = block_assembler.candidate_uncles.lock().await;
        let uncles =
            BlockAssembler::prepare_uncles(snapshot, candidate_number, &current_epoch, &mut guard);
        let last_uncles_updated_at = block_assembler
            .last_uncles_updated_at
            .load(Ordering::SeqCst);
        (uncles, current_epoch, last_uncles_updated_at)
    }

    async fn package_txs_for_block_template(
        &self,
        bytes_limit: u64,
        proposals_limit: u64,
        max_block_cycles: Cycle,
        cellbase: &TransactionView,
        uncles: &[UncleBlockView],
    ) -> Result<(HashSet<ProposalShortId>, Vec<TxEntry>, u64), AnyError> {
        let guard = self.tx_pool.read().await;
        let uncle_proposals = uncles
            .iter()
            .flat_map(|u| u.data().proposals().into_iter())
            .collect();
        let proposals = guard.get_proposals(proposals_limit as usize, &uncle_proposals);

        let txs_size_limit = BlockAssembler::calculate_txs_size_limit(
            bytes_limit,
            cellbase.data(),
            uncles,
            &proposals,
        )?;

        let (entries, size, cycles) = CommitTxsScanner::new(guard.proposed()).txs_to_commit(
            txs_size_limit,
            max_block_cycles,
            guard.config.min_fee_rate,
        );
        if !entries.is_empty() {
            info!(
                "[get_block_template] candidate txs count: {}, size: {}/{}, cycles:{}/{}",
                entries.len(),
                size,
                txs_size_limit,
                cycles,
                max_block_cycles
            );
        }
        let last_txs_updated_at = self.last_txs_updated_at.load(Ordering::SeqCst);
        Ok((proposals, entries, last_txs_updated_at))
    }

    #[allow(clippy::too_many_arguments)]
    fn build_block_template(
        &self,
        snapshot: &Snapshot,
        entries: Vec<TxEntry>,
        proposals: HashSet<ProposalShortId>,
        cellbase: TransactionView,
        work_id: u64,
        current_epoch: EpochExt,
        uncles: Vec<UncleBlockView>,
        bytes_limit: u64,
        version: Version,
    ) -> Result<BlockTemplate, AnyError> {
        let consensus = snapshot.consensus();
        let tip_header = snapshot.tip_header();
        let tip_hash = tip_header.hash();
        let mut template_txs = Vec::with_capacity(entries.len());
        let mut seen_inputs = HashSet::new();

        let transactions_checker = TransactionsChecker::new(
            iter::once(&cellbase).chain(entries.iter().map(|entry| entry.transaction())),
        );

        let dummy_cellbase_entry = TxEntry::dummy_resolve(cellbase.clone(), 0, Capacity::zero(), 0);
        let entries_iter = iter::once(dummy_cellbase_entry).chain(entries.into_iter());

        let overlay_cell_checker = OverlayCellChecker::new(&transactions_checker, snapshot);

        let rtxs: Vec<_> = block_in_place(|| {
            entries_iter.enumerate().filter_map(|(index, entry)| {
                if let Err(err) = entry.rtx.check(&mut seen_inputs, &overlay_cell_checker, snapshot) {
                    error!(
                        "resolve transactions when build block template, tip_number: {}, tip_hash: {}, error: {:?}",
                        tip_header.number(), tip_hash, err
                    );
                    None
                } else {
                    if index != 0 {
                        template_txs.push(BlockAssembler::transform_tx(&entry, false, None))
                    }
                    Some(entry.rtx)
                }
            }).collect()
        });

        // Generate DAO fields here
        let dao = DaoCalculator::new(consensus, &snapshot.as_data_provider())
            .dao_field(&rtxs, tip_header)?;

        let candidate_number = tip_header.number() + 1;
        let cycles_limit = consensus.max_block_cycles();
        let uncles_count_limit = consensus.max_uncles_num() as u32;

        // Should recalculate current time after create cellbase (create cellbase may spend a lot of time)
        let current_time = cmp::max(unix_time_as_millis(), tip_header.timestamp() + 1);

        Ok(BlockTemplate {
            version: version.into(),
            compact_target: current_epoch.compact_target().into(),
            current_time: current_time.into(),
            number: candidate_number.into(),
            epoch: current_epoch.number_with_fraction(candidate_number).into(),
            parent_hash: tip_hash.unpack(),
            cycles_limit: cycles_limit.into(),
            bytes_limit: bytes_limit.into(),
            uncles_count_limit: u64::from(uncles_count_limit).into(),
            uncles: uncles.iter().map(BlockAssembler::transform_uncle).collect(),
            transactions: template_txs,
            proposals: proposals.iter().cloned().map(Into::into).collect(),
            cellbase: BlockAssembler::transform_cellbase(&cellbase, None),
            work_id: work_id.into(),
            dao: dao.into(),
        })
    }

    async fn update_block_template_cache(
        &self,
        block_assembler: &BlockAssembler,
        key: BlockTemplateCacheKey,
        uncles_updated_at: u64,
        txs_updated_at: u64,
        template: BlockTemplate,
    ) {
        block_assembler.template_caches.lock().await.put(
            key,
            TemplateCache {
                time: template.current_time.into(),
                uncles_updated_at,
                txs_updated_at,
                template,
            },
        );
    }

    pub(crate) async fn get_block_template(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
        block_assembler_config: Option<BlockAssemblerConfig>,
    ) -> Result<BlockTemplate, AnyError> {
        if self.block_assembler.is_none() && block_assembler_config.is_none() {
            Err(InternalErrorKind::Config
                .other("BlockAssembler disabled")
                .into())
        } else {
            let block_assembler = block_assembler_config
                .map(BlockAssembler::new)
                .unwrap_or_else(|| self.block_assembler.clone().unwrap());
            let snapshot = self.snapshot();
            let consensus = snapshot.consensus();
            let cycles_limit = consensus.max_block_cycles();
            let (bytes_limit, proposals_limit, version) = BlockAssembler::transform_params(
                consensus,
                bytes_limit,
                proposals_limit,
                max_version,
            );

            if let Some(cache) = self
                .get_block_template_cache(
                    bytes_limit,
                    proposals_limit,
                    version,
                    &snapshot,
                    &block_assembler,
                )
                .await
            {
                return Ok(cache);
            }

            let cellbase =
                self.build_block_template_cellbase(&snapshot, &block_assembler.config)?;

            let (uncles, current_epoch, uncles_updated_at) = self
                .prepare_block_template_uncles(&snapshot, &block_assembler)
                .await;

            let (proposals, entries, txs_updated_at) = self
                .package_txs_for_block_template(
                    bytes_limit,
                    proposals_limit,
                    cycles_limit,
                    &cellbase,
                    &uncles,
                )
                .await?;

            let work_id = block_assembler.work_id.fetch_add(1, Ordering::SeqCst);

            let block_template = self.build_block_template(
                &snapshot,
                entries,
                proposals,
                cellbase,
                work_id,
                current_epoch,
                uncles,
                bytes_limit,
                version,
            )?;

            self.update_block_template_cache(
                &block_assembler,
                (snapshot.tip_hash(), bytes_limit, proposals_limit, version),
                uncles_updated_at,
                txs_updated_at,
                block_template.clone(),
            )
            .await;

            Ok(block_template)
        }
    }

    async fn fetch_tx_verify_cache(&self, hash: &Byte32) -> Option<CacheEntry> {
        let guard = self.txs_verify_cache.read().await;
        guard.peek(hash).cloned()
    }

    async fn fetch_txs_verify_cache(
        &self,
        txs: impl Iterator<Item = &TransactionView>,
    ) -> HashMap<Byte32, CacheEntry> {
        let guard = self.txs_verify_cache.read().await;
        txs.filter_map(|tx| {
            let hash = tx.hash();
            guard.peek(&hash).cloned().map(|value| (hash, value))
        })
        .collect()
    }

    async fn submit_entry(
        &self,
        verified: CacheEntry,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        mut status: TxStatus,
    ) -> Result<(), Reject> {
        let mut tx_pool = self.tx_pool.write().await;

        check_tx_cycle_limit(&tx_pool, verified.cycles)?;

        let snapshot = tx_pool.snapshot();
        // if tip changed, resolve again
        if pre_resolve_tip != snapshot.tip_hash() {
            // destructuring assignments are not currently supported
            status = check_rtx(&tx_pool, snapshot, &entry.rtx)?;
        }

        _submit_entry(&mut tx_pool, status, entry, &self.callbacks)
    }

    async fn pre_check(&self, tx: TransactionView) -> Result<PreCheckedTx, Reject> {
        // Acquire read lock for cheap check
        let tx_pool = self.tx_pool.read().await;
        let snapshot = tx_pool.cloned_snapshot();
        let tip_hash = snapshot.tip_hash();

        let tx_size = tx.data().serialized_size_in_block();

        // reject if pool reach size limit
        // TODO: tx evict strategy
        check_tx_size_limit(&tx_pool, tx_size)?;

        // reject collision id
        check_txid_collision(&tx_pool, &tx)?;

        let (rtx, status) = resolve_tx(&tx_pool, &snapshot, tx)?;

        let fee = check_tx_fee(&tx_pool, &snapshot, &rtx, tx_size)?;

        Ok((tip_hash, snapshot, rtx, status, fee, tx_size))
    }

    pub(crate) async fn process_tx(
        &self,
        tx: TransactionView,
        max_cycles: Option<Cycle>,
    ) -> Result<CacheEntry, Reject> {
        let remote = max_cycles.is_some();

        let ret = self._process_tx(tx, max_cycles).await?;

        // if let Some(hash) = self.orphan.find_by_previous(&tx) {
        //     process_orphan_tx().await
        // }
        Ok(ret)
    }

    // pub(crate) async fn process_orphan_tx(
    //     &self,
    //     tx: &TransactionView,
    // ) -> Result<CacheEntry, Reject> {
    //     if let Some(hash) = self.orphan_pool.find_by_previous(&tx) {
    //         let mut orphan: VecDeque<packed::Byte32> = VecDeque::new();
    //         orphan.push_back(hash);

    //         while let Some(tx_hash) = orphan.pop_front() {
    //             if let Some(tx) = self.orphan_pool.get(&tx_hash).await {
    //                 match self._process_tx(tx.clone()).await {
    //                     Ok(ret) => match ret {
    //                         Ok(_) => {
    //                             orphan_pool.remove_orphan_tx(&tx_hash);
    //                             broadcast_tx(relayer, tx_hash, peer);
    //                             if let Some(hash) = self.orphan_pool.find_by_previous(&tx) {
    //                                 orphan.push_back(hash);
    //                             }
    //                         }
    //                         Err(err) => {
    //                             if !is_missing_input(&err) {
    //                                 orphan_pool.remove_orphan_tx(&tx_hash);
    //                             }
    //                             if is_malformed(&err) {
    //                                 ban_malformed(nc, &err, peer);
    //                                 break;
    //                             }
    //                         }
    //                     },
    //                     Err(err) => {
    //                         debug_target!(
    //                             crate::LOG_TARGET_RELAY,
    //                             "process_orphan_tx internal error {}",
    //                             err
    //                         );
    //                         break;
    //                     }
    //                 }
    //             }
    //         }
    //     }
    // }

    pub(crate) async fn _process_tx(
        &self,
        tx: TransactionView,
        max_cycles: Option<Cycle>,
    ) -> Result<CacheEntry, Reject> {
        let tx_hash = tx.hash();

        // non contextual verify first
        non_contextual_verify(&self.consensus, &tx)?;

        let (tip_hash, snapshot, rtx, status, fee, tx_size) = self.pre_check(tx).await?;

        let verify_cache = self.fetch_tx_verify_cache(&tx_hash).await;
        let max_cycles = max_cycles.unwrap_or(self.tx_pool_config.max_tx_verify_cycles);
        let verified = verify_rtx(&snapshot, &rtx, verify_cache, max_cycles)?;

        let entry = TxEntry::new(rtx, verified.cycles, fee, tx_size);

        self.submit_entry(verified, tip_hash, entry, status).await?;

        if verify_cache.is_none() {
            // update cache
            let txs_verify_cache = Arc::clone(&self.txs_verify_cache);
            tokio::spawn(async move {
                let mut guard = txs_verify_cache.write().await;
                guard.put(tx_hash, verified);
            });
        }

        Ok(verified)
    }

    pub(crate) async fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposal_id: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
    ) {
        let mut detached = LinkedHashSet::default();
        let mut attached = LinkedHashSet::default();

        for blk in detached_blocks {
            detached.extend(blk.transactions().into_iter().skip(1))
        }

        for blk in attached_blocks {
            attached.extend(blk.transactions().into_iter().skip(1));
        }
        let retain: Vec<TransactionView> = detached.difference(&attached).cloned().collect();

        let fetched_cache = self.fetch_txs_verify_cache(retain.iter()).await;

        let mut tx_pool = self.tx_pool.write().await;
        _update_tx_pool_for_reorg(
            &mut tx_pool,
            attached,
            detached_proposal_id,
            snapshot,
            &self.callbacks,
        );

        self.readd_dettached_tx(&mut tx_pool, retain, fetched_cache)
    }

    fn readd_dettached_tx(
        &self,
        tx_pool: &mut TxPool,
        txs: Vec<TransactionView>,
        fetched_cache: HashMap<Byte32, CacheEntry>,
    ) {
        let max_cycles = self.tx_pool_config.max_tx_verify_cycles;
        for tx in txs {
            let tx_size = tx.data().serialized_size_in_block();
            let tx_hash = tx.hash();
            if let Ok((rtx, status)) = resolve_tx(tx_pool, tx_pool.snapshot(), tx) {
                if let Ok(fee) = check_tx_fee(tx_pool, tx_pool.snapshot(), &rtx, tx_size) {
                    let verify_cache = fetched_cache.get(&tx_hash).cloned();
                    if let Ok(verified) =
                        verify_rtx(tx_pool.snapshot(), &rtx, verify_cache, max_cycles)
                    {
                        let entry = TxEntry::new(rtx, verified.cycles, fee, tx_size);
                        _submit_entry(tx_pool, status, entry, &self.callbacks);
                    }
                }
            }
        }
    }

    pub(crate) async fn clear_pool(&mut self, new_snapshot: Arc<Snapshot>) {
        let mut tx_pool = self.tx_pool.write().await;
        let config = tx_pool.config;
        self.last_txs_updated_at = Arc::new(AtomicU64::new(0));
        *tx_pool = TxPool::new(config, new_snapshot, Arc::clone(&self.last_txs_updated_at));
    }
}

type PreCheckedTx = (
    Byte32,
    Arc<Snapshot>,
    ResolvedTransaction,
    TxStatus,
    Capacity,
    usize,
);

type ResolveResult = Result<(ResolvedTransaction, TxStatus), Reject>;

fn check_rtx(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
) -> Result<TxStatus, Reject> {
    let short_id = rtx.transaction.proposal_short_id();
    if snapshot.proposals().contains_proposed(&short_id) {
        tx_pool
            .check_rtx_from_proposed(rtx)
            .map(|_| TxStatus::Proposed)
    } else {
        tx_pool.check_rtx_from_pending_and_proposed(rtx).map(|_| {
            if snapshot.proposals().contains_gap(&short_id) {
                TxStatus::Gap
            } else {
                TxStatus::Fresh
            }
        })
    }
}

fn resolve_tx(tx_pool: &TxPool, snapshot: &Snapshot, tx: TransactionView) -> ResolveResult {
    let short_id = tx.proposal_short_id();
    if snapshot.proposals().contains_proposed(&short_id) {
        tx_pool
            .resolve_tx_from_proposed(tx)
            .map(|rtx| (rtx, TxStatus::Proposed))
    } else {
        tx_pool.resolve_tx_from_pending_and_proposed(tx).map(|rtx| {
            let status = if snapshot.proposals().contains_gap(&short_id) {
                TxStatus::Gap
            } else {
                TxStatus::Fresh
            };
            (rtx, status)
        })
    }
}

fn _submit_entry(
    tx_pool: &mut TxPool,
    status: TxStatus,
    entry: TxEntry,
    callbacks: &Callbacks,
) -> Result<(), Reject> {
    match status {
        TxStatus::Fresh => {
            tx_pool.add_pending(entry.clone())?;
            callbacks.call_pending(tx_pool, entry);
        }
        TxStatus::Gap => {
            tx_pool.add_gap(entry.clone())?;
            callbacks.call_pending(tx_pool, entry);
        }
        TxStatus::Proposed => {
            tx_pool.add_proposed(entry.clone())?;
            callbacks.call_proposed(tx_pool, entry, true);
        }
    }
    Ok(())
}

fn _update_tx_pool_for_reorg(
    tx_pool: &mut TxPool,
    attached: LinkedHashSet<TransactionView>,
    detached_proposal_id: HashSet<ProposalShortId>,
    snapshot: Arc<Snapshot>,
    callbacks: &Callbacks,
) {
    tx_pool.snapshot = Arc::clone(&snapshot);

    let txs_iter = attached.iter().map(|tx| {
        let get_cell_data = |out_point: &OutPoint| {
            snapshot
                .get_cell_data(out_point)
                .map(|(data, _data_hash)| data)
        };
        let related_out_points =
            get_related_dep_out_points(tx, get_cell_data).expect("Get dep out points failed");
        (tx, related_out_points)
    });
    // NOTE: `remove_expired` will try to re-put the given expired/detached proposals into
    // pending-pool if they can be found within txpool. As for a transaction
    // which is both expired and committed at the one time(commit at its end of commit-window),
    // we should treat it as a committed and not re-put into pending-pool. So we should ensure
    // that involves `remove_committed_txs` before `remove_expired`.
    tx_pool.remove_committed_txs(txs_iter, callbacks);
    tx_pool.remove_expired(detached_proposal_id.iter(), callbacks);

    let mut entries = Vec::new();
    let mut gaps = Vec::new();

    // pending ---> gap ----> proposed
    // try move gap to proposed
    let mut removed: Vec<ProposalShortId> = Vec::with_capacity(tx_pool.gap.size());
    for key in tx_pool.gap.keys_sorted_by_fee_and_relation() {
        if snapshot.proposals().contains_proposed(&key.id) {
            let entry = tx_pool.gap.get(&key.id).expect("exists");
            entries.push((
                Some(CacheEntry::new(entry.cycles, entry.fee)),
                entry.clone(),
            ));
            removed.push(key.id.clone());
        }
    }
    removed.into_iter().for_each(|id| {
        tx_pool.gap.remove_entry(&id);
    });

    // try move pending to proposed
    let mut removed: Vec<ProposalShortId> = Vec::with_capacity(tx_pool.pending.size());
    for key in tx_pool.pending.keys_sorted_by_fee_and_relation() {
        let entry = tx_pool.pending.get(&key.id).expect("exists");
        if snapshot.proposals().contains_proposed(&key.id) {
            entries.push((
                Some(CacheEntry::new(entry.cycles, entry.fee)),
                entry.clone(),
            ));
            removed.push(key.id.clone());
        } else if snapshot.proposals().contains_gap(&key.id) {
            gaps.push((
                Some(CacheEntry::new(entry.cycles, entry.fee)),
                entry.clone(),
            ));
            removed.push(key.id.clone());
        }
    }
    removed.into_iter().for_each(|id| {
        tx_pool.pending.remove_entry(&id);
    });

    for (cycles, entry) in entries {
        let tx_hash = entry.transaction().hash();
        if let Err(e) = tx_pool.proposed_rtx(cycles, entry.size, entry.rtx.clone()) {
            debug!("Failed to add proposed tx {}, reason: {}", tx_hash, e);
            callbacks.call_reject(tx_pool, entry, e.clone());
        } else {
            callbacks.call_proposed(tx_pool, entry, false);
        }
    }

    for (cycles, entry) in gaps {
        debug!("tx proposed, add to gap {}", entry.transaction().hash());
        let tx_hash = entry.transaction().hash();
        if let Err(e) = tx_pool.gap_rtx(cycles, entry.size, entry.rtx.clone()) {
            debug!("Failed to add tx to gap {}, reason: {}", tx_hash, e);
            callbacks.call_reject(tx_pool, entry, e.clone());
        }
    }
}
