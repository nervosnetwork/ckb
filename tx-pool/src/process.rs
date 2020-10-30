use crate::block_assembler::{BlockAssembler, BlockTemplateCacheKey, TemplateCache};
use crate::component::commit_txs_scanner::CommitTxsScanner;
use crate::component::entry::TxEntry;
use crate::error::{BlockAssemblerError, Reject};
use crate::pool::TxPool;
use crate::service::TxPoolService;
use ckb_app_config::BlockAssemblerConfig;
use ckb_dao::DaoCalculator;
use ckb_error::{Error, InternalErrorKind};
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::{debug, info};
use ckb_notify::PoolTransactionEntry;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        cell::{
            get_related_dep_out_points, resolve_transaction, OverlayCellProvider,
            ResolvedTransaction, TransactionsProvider,
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
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{atomic::AtomicU64, Arc};
use std::{cmp, iter};
use tokio::task::block_in_place;

/// TODO(doc): @zhangsoledad
pub enum PlugTarget {
    /// TODO(doc): @zhangsoledad
    Pending,
    /// TODO(doc): @zhangsoledad
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
    ) -> Result<TransactionView, FailureError> {
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
        let last_epoch = snapshot.get_current_epoch_ext().expect("current epoch ext");
        let next_epoch_ext = snapshot.next_epoch_ext(consensus, &last_epoch, tip_header);
        let current_epoch = next_epoch_ext.unwrap_or(last_epoch);
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
    ) -> Result<(HashSet<ProposalShortId>, Vec<TxEntry>, u64), FailureError> {
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
    ) -> Result<BlockTemplate, FailureError> {
        let consensus = snapshot.consensus();
        let tip_header = snapshot.tip_header();
        let tip_hash = tip_header.hash();
        let mut txs = iter::once(&cellbase).chain(entries.iter().map(|entry| &entry.transaction));
        let mut seen_inputs = HashSet::new();
        let transactions_provider = TransactionsProvider::new(txs.clone());
        let overlay_cell_provider = OverlayCellProvider::new(&transactions_provider, snapshot);

        let rtxs = txs
            .try_fold(vec![], |mut rtxs, tx| {
                resolve_transaction(
                    tx.clone(),
                    &mut seen_inputs,
                    &overlay_cell_provider,
                    snapshot,
                )
                .map(|rtx| {
                    rtxs.push(rtx);
                    rtxs
                })
            })
            .map_err(|_| BlockAssemblerError::InvalidInput)?;

        // Generate DAO fields here
        let dao = DaoCalculator::new(consensus, snapshot).dao_field(&rtxs, tip_header)?;

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
            transactions: entries
                .iter()
                .map(|entry| BlockAssembler::transform_tx(entry, false, None))
                .collect(),
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
    ) -> Result<BlockTemplate, FailureError> {
        if self.block_assembler.is_none() && block_assembler_config.is_none() {
            Err(InternalErrorKind::Config
                .reason("BlockAssembler disabled")
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

            let cellbase = block_in_place(|| {
                self.build_block_template_cellbase(&snapshot, &block_assembler.config)
            })?;

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

            let block_template = block_in_place(|| {
                self.build_block_template(
                    &snapshot,
                    entries,
                    proposals,
                    cellbase,
                    work_id,
                    current_epoch,
                    uncles,
                    bytes_limit,
                    version,
                )
            })?;

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

    async fn pre_resolve_txs(&self, txs: &[TransactionView]) -> Result<PreResolvedTxs, Error> {
        let tx_pool = self.tx_pool.read().await;

        debug_assert!(!txs.is_empty(), "txs should not be empty!");
        let snapshot = tx_pool.cloned_snapshot();
        let tip_hash = snapshot.tip_hash();

        check_transaction_hash_collision(&tx_pool, txs)?;

        let mut txs_provider = TransactionsProvider::default();
        let resolved = txs
            .iter()
            .map(|tx| {
                let ret = resolve_tx(&tx_pool, &snapshot, &txs_provider, tx.clone());
                txs_provider.insert(tx);
                ret
            })
            .collect::<Result<Vec<(ResolvedTransaction, usize, Capacity, TxStatus)>, _>>()?;

        let (rtxs, status) = resolved
            .into_iter()
            .map(|(rtx, tx_size, fee, status)| (rtx, (tx_size, fee, status)))
            .unzip();

        Ok((tip_hash, snapshot, rtxs, status))
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

    async fn submit_txs(
        &self,
        txs: Vec<(ResolvedTransaction, CacheEntry)>,
        pre_resolve_tip: Byte32,
        status: Vec<(usize, Capacity, TxStatus)>,
    ) -> Result<(), Error> {
        let mut tx_pool = self.tx_pool.write().await;
        let snapshot = tx_pool.snapshot();

        if pre_resolve_tip != snapshot.tip_hash() {
            let mut txs_provider = TransactionsProvider::default();

            for (tx, _) in &txs {
                resolve_tx(&tx_pool, snapshot, &txs_provider, tx.transaction.clone())?;
                txs_provider.insert(&tx.transaction);
            }
        }

        for ((rtx, cache_entry), (tx_size, fee, status)) in txs.into_iter().zip(status.into_iter())
        {
            if tx_pool.reach_cycles_limit(cache_entry.cycles) {
                return Err(Reject::Full("cycles".to_owned(), tx_pool.config.max_cycles).into());
            }

            let min_fee = tx_pool.config.min_fee_rate.fee(tx_size);
            // reject txs which fee lower than min fee rate
            if fee < min_fee {
                return Err(Reject::LowFeeRate(min_fee.as_u64(), fee.as_u64()).into());
            }

            let related_dep_out_points = rtx.related_dep_out_points();
            let entry = TxEntry::new(
                rtx.transaction.clone(),
                cache_entry.cycles,
                fee,
                tx_size,
                related_dep_out_points,
            );
            let inserted = match status {
                TxStatus::Fresh => tx_pool.add_pending(entry)?,
                TxStatus::Gap => tx_pool.add_gap(entry)?,
                TxStatus::Proposed => tx_pool.add_proposed(entry)?,
            };
            if inserted {
                let notify_tx_entry = PoolTransactionEntry {
                    transaction: rtx.transaction,
                    cycles: cache_entry.cycles,
                    size: tx_size,
                    fee,
                };
                self.notify_controller
                    .notify_new_transaction(notify_tx_entry);
                tx_pool.update_statics_for_add_tx(tx_size, cache_entry.cycles);
            }
        }
        Ok(())
    }

    fn non_contextual_verify(&self, txs: &[TransactionView]) -> Result<(), Error> {
        for tx in txs {
            NonContextualTransactionVerifier::new(tx, &self.consensus).verify()?;

            // cellbase is only valid in a block, not as a loose transaction
            if tx.is_cellbase() {
                return Err(Reject::Malformed("cellbase like".to_owned()).into());
            }
        }
        Ok(())
    }

    pub(crate) async fn process_txs(
        &self,
        txs: Vec<TransactionView>,
    ) -> Result<Vec<CacheEntry>, Error> {
        // non contextual verify first
        self.non_contextual_verify(&txs)?;

        let max_tx_verify_cycles = self.tx_pool_config.max_tx_verify_cycles;
        let (tip_hash, snapshot, rtxs, status) = self.pre_resolve_txs(&txs).await?;
        let fetched_cache = self.fetch_txs_verify_cache(txs.iter()).await;

        let verified =
            block_in_place(|| verify_rtxs(&snapshot, rtxs, &fetched_cache, max_tx_verify_cycles))?;

        let updated_cache = verified
            .iter()
            .map(|(tx, cycles)| (tx.transaction.hash(), *cycles))
            .collect::<Vec<_>>();
        let cycles_vec = verified.iter().map(|(_, cycles)| *cycles).collect();

        self.submit_txs(verified, tip_hash, status).await?;

        let txs_verify_cache = Arc::clone(&self.txs_verify_cache);
        tokio::spawn(async move {
            let mut guard = txs_verify_cache.write().await;
            for (k, v) in updated_cache {
                guard.put(k, v);
            }
        });
        Ok(cycles_vec)
    }

    pub(crate) async fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposal_id: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
    ) {
        let mut detached_txs = HashSet::new();
        let mut attached_txs = HashSet::new();
        for blk in &detached_blocks {
            detached_txs.extend(blk.transactions().iter().skip(1).cloned())
        }
        for blk in &attached_blocks {
            attached_txs.extend(blk.transactions().iter().skip(1).cloned())
        }
        let fetched_cache = self
            .fetch_txs_verify_cache(detached_txs.difference(&attached_txs))
            .await;
        let mut tx_pool = self.tx_pool.write().await;
        let updated_cache = block_in_place(|| {
            _update_tx_pool_for_reorg(
                &mut tx_pool,
                &fetched_cache,
                detached_blocks,
                attached_blocks,
                detached_proposal_id,
                snapshot,
            )
        });

        let txs_verify_cache = Arc::clone(&self.txs_verify_cache);
        tokio::spawn(async move {
            let mut guard = txs_verify_cache.write().await;
            for (k, v) in updated_cache {
                guard.put(k, v);
            }
        });
    }

    pub(crate) async fn clear_pool(&self, new_snapshot: Arc<Snapshot>) {
        let mut tx_pool = self.tx_pool.write().await;
        let config = tx_pool.config;
        let last_txs_updated_at = Arc::new(AtomicU64::new(0));
        *tx_pool = TxPool::new(config, new_snapshot, last_txs_updated_at);
    }
}

type PreResolvedTxs = (
    Byte32,
    Arc<Snapshot>,
    Vec<ResolvedTransaction>,
    Vec<(usize, Capacity, TxStatus)>,
);

type ResolveResult = Result<(ResolvedTransaction, usize, Capacity, TxStatus), Error>;

fn check_transaction_hash_collision(
    tx_pool: &TxPool,
    txs: &[TransactionView],
) -> Result<(), Error> {
    for tx in txs {
        let short_id = tx.proposal_short_id();
        if tx_pool.contains_proposal_id(&short_id) {
            return Err(Reject::Duplicated(tx.hash()).into());
        }
    }
    Ok(())
}

fn resolve_tx<'a>(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    txs_provider: &'a TransactionsProvider<'a>,
    tx: TransactionView,
) -> ResolveResult {
    let tx_size = tx.data().serialized_size_in_block();
    if tx_pool.reach_size_limit(tx_size) {
        return Err(Reject::Full("size".to_owned(), tx_pool.config.max_mem_size as u64).into());
    }

    let short_id = tx.proposal_short_id();
    if snapshot.proposals().contains_proposed(&short_id) {
        resolve_tx_from_proposed(tx_pool, snapshot, txs_provider, tx).and_then(|rtx| {
            let fee = tx_pool.calculate_transaction_fee(snapshot, &rtx);
            fee.map(|fee| (rtx, tx_size, fee, TxStatus::Proposed))
        })
    } else {
        resolve_tx_from_pending_and_proposed(tx_pool, snapshot, txs_provider, tx).and_then(|rtx| {
            let status = if snapshot.proposals().contains_gap(&short_id) {
                TxStatus::Gap
            } else {
                TxStatus::Fresh
            };
            let fee = tx_pool.calculate_transaction_fee(snapshot, &rtx);
            fee.map(|fee| (rtx, tx_size, fee, status))
        })
    }
}

fn resolve_tx_from_proposed<'a>(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    txs_provider: &'a TransactionsProvider<'a>,
    tx: TransactionView,
) -> Result<ResolvedTransaction, Error> {
    let cell_provider = OverlayCellProvider::new(&tx_pool.proposed, snapshot);
    let provider = OverlayCellProvider::new(txs_provider, &cell_provider);
    resolve_transaction(tx, &mut HashSet::new(), &provider, snapshot)
}

fn resolve_tx_from_pending_and_proposed<'a>(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    txs_provider: &'a TransactionsProvider<'a>,
    tx: TransactionView,
) -> Result<ResolvedTransaction, Error> {
    let proposed_provider = OverlayCellProvider::new(&tx_pool.proposed, snapshot);
    let gap_and_proposed_provider = OverlayCellProvider::new(&tx_pool.gap, &proposed_provider);
    let pending_and_proposed_provider =
        OverlayCellProvider::new(&tx_pool.pending, &gap_and_proposed_provider);
    let provider = OverlayCellProvider::new(txs_provider, &pending_and_proposed_provider);
    resolve_transaction(tx, &mut HashSet::new(), &provider, snapshot)
}

fn verify_rtxs(
    snapshot: &Snapshot,
    txs: Vec<ResolvedTransaction>,
    txs_verify_cache: &HashMap<Byte32, CacheEntry>,
    max_tx_verify_cycles: Cycle,
) -> Result<Vec<(ResolvedTransaction, CacheEntry)>, Error> {
    let tip_header = snapshot.tip_header();
    let tip_number = tip_header.number();
    let epoch = tip_header.epoch();
    let consensus = snapshot.consensus();

    txs.into_iter()
        .map(|tx| {
            let tx_hash = tx.transaction.hash();
            if let Some(cache_entry) = txs_verify_cache.get(&tx_hash) {
                TimeRelativeTransactionVerifier::new(
                    &tx,
                    snapshot,
                    tip_number + 1,
                    epoch,
                    tip_header.hash(),
                    consensus,
                )
                .verify()
                .map(|_| (tx, *cache_entry))
            } else {
                ContextualTransactionVerifier::new(
                    &tx,
                    snapshot,
                    tip_number + 1,
                    epoch,
                    tip_header.hash(),
                    consensus,
                    snapshot,
                )
                .verify(max_tx_verify_cycles)
                .map(|cycles| (tx, cycles))
            }
        })
        .collect::<Result<Vec<_>, _>>()
}

fn _update_tx_pool_for_reorg(
    tx_pool: &mut TxPool,
    txs_verify_cache: &HashMap<Byte32, CacheEntry>,
    detached_blocks: VecDeque<BlockView>,
    attached_blocks: VecDeque<BlockView>,
    detached_proposal_id: HashSet<ProposalShortId>,
    snapshot: Arc<Snapshot>,
) -> HashMap<Byte32, CacheEntry> {
    tx_pool.snapshot = Arc::clone(&snapshot);
    let mut detached = LinkedHashSet::default();
    let mut attached = LinkedHashSet::default();

    for blk in detached_blocks {
        detached.extend(blk.transactions().iter().skip(1).cloned())
    }

    for blk in attached_blocks {
        attached.extend(blk.transactions().iter().skip(1).cloned());
    }

    let retain: Vec<TransactionView> = detached.difference(&attached).cloned().collect();

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
    // that involves `remove_committed_txs_from_proposed` before `remove_expired`.
    tx_pool.remove_committed_txs_from_proposed(txs_iter);
    tx_pool.remove_expired(detached_proposal_id.iter());

    let to_update_cache = retain
        .into_iter()
        .filter_map(|tx| tx_pool.readd_dettached_tx(&snapshot, txs_verify_cache, tx))
        .collect();

    for tx in &attached {
        tx_pool.try_proposed_orphan_by_ancestor(tx);
    }

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
                entry.size,
                entry.transaction.to_owned(),
            ));
            removed.push(key.id.clone());
        }
    }
    removed.into_iter().for_each(|id| {
        tx_pool.gap.remove_entry_and_descendants(&id);
    });

    // try move pending to proposed
    let mut removed: Vec<ProposalShortId> = Vec::with_capacity(tx_pool.pending.size());
    for key in tx_pool.pending.keys_sorted_by_fee_and_relation() {
        let entry = tx_pool.pending.get(&key.id).expect("exists");
        if snapshot.proposals().contains_proposed(&key.id) {
            entries.push((
                Some(CacheEntry::new(entry.cycles, entry.fee)),
                entry.size,
                entry.transaction.to_owned(),
            ));
            removed.push(key.id.clone());
        } else if snapshot.proposals().contains_gap(&key.id) {
            gaps.push((
                Some(CacheEntry::new(entry.cycles, entry.fee)),
                entry.size,
                entry.transaction.to_owned(),
            ));
            removed.push(key.id.clone());
        }
    }
    removed.into_iter().for_each(|id| {
        tx_pool.pending.remove_entry(&id);
    });

    // try move conflict to proposed
    let mut removed_conflict = Vec::with_capacity(tx_pool.conflict.len());
    for (key, entry) in tx_pool.conflict.iter() {
        if snapshot.proposals().contains_proposed(key) {
            removed_conflict.push(key.clone());
            entries.push((entry.cache_entry, entry.size, entry.transaction.clone()));
        } else if snapshot.proposals().contains_gap(key) {
            removed_conflict.push(key.clone());
            gaps.push((entry.cache_entry, entry.size, entry.transaction.clone()));
        }
    }
    for removed_key in removed_conflict {
        tx_pool.conflict.pop(&removed_key);
    }

    for (cycles, size, tx) in entries {
        let tx_hash = tx.hash();
        if let Err(e) = tx_pool.proposed_tx_and_descendants(cycles, size, tx) {
            debug!("Failed to add proposed tx {}, reason: {}", tx_hash, e);
        }
    }

    for (cycles, size, tx) in gaps {
        debug!("tx proposed, add to gap {}", tx.hash());
        let tx_hash = tx.hash();
        if let Err(e) = tx_pool.gap_tx(cycles, size, tx) {
            debug!("Failed to add tx to gap {}, reason: {}", tx_hash, e);
        }
    }

    to_update_cache
}
