use crate::pool::TxPool;
use crate::service::ChainReorgArgs;
use ckb_logger::debug_target;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    core::{cell::get_related_dep_out_points, BlockView, TransactionView},
    packed::{Byte32, OutPoint, ProposalShortId},
    prelude::*,
};
use ckb_util::LinkedHashSet;
use ckb_verification::cache::CacheEntry;
use futures::future::Future;
use std::collections::HashMap;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct ChainReorgProcess {
    pub tx_pool: Lock<TxPool>,
    pub txs_verify_cache: HashMap<Byte32, CacheEntry>,
    pub args: Option<ChainReorgArgs>,
}

impl ChainReorgProcess {
    pub fn new(
        tx_pool: Lock<TxPool>,
        txs_verify_cache: HashMap<Byte32, CacheEntry>,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposal_id: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
    ) -> ChainReorgProcess {
        ChainReorgProcess {
            tx_pool,
            txs_verify_cache,
            args: Some((
                detached_blocks,
                attached_blocks,
                detached_proposal_id,
                snapshot,
            )),
        }
    }
}

impl Future for ChainReorgProcess {
    type Item = HashMap<Byte32, CacheEntry>;
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(mut guard) => {
                let (detached_blocks, attached_blocks, detached_proposal_id, snapshot) =
                    self.args.take().expect("cannot poll twice");
                let ret = update_tx_pool_for_reorg(
                    &mut guard,
                    &self.txs_verify_cache,
                    detached_blocks,
                    attached_blocks,
                    detached_proposal_id,
                    snapshot,
                );

                Ok(Async::Ready(ret))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}

pub fn update_tx_pool_for_reorg(
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
        tx_pool.fee_estimator.process_block(
            blk.header().number(),
            blk.transactions().iter().skip(1).map(|tx| tx.hash()),
        );
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
    for entry in tx_pool.conflict.entries() {
        if snapshot.proposals().contains_proposed(entry.key()) {
            let entry = entry.remove();
            entries.push((entry.cache_entry, entry.size, entry.transaction));
        } else if snapshot.proposals().contains_gap(entry.key()) {
            let entry = entry.remove();
            gaps.push((entry.cache_entry, entry.size, entry.transaction));
        }
    }

    for (cycles, size, tx) in entries {
        let tx_hash = tx.hash();
        if let Err(e) = tx_pool.proposed_tx_and_descendants(cycles, size, tx) {
            debug_target!(
                crate::LOG_TARGET_TX_POOL,
                "Failed to add proposed tx {}, reason: {}",
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
        let tx_hash = tx.hash();
        if let Err(e) = tx_pool.gap_tx(cycles, size, tx) {
            debug_target!(
                crate::LOG_TARGET_TX_POOL,
                "Failed to add tx to gap {}, reason: {}",
                tx_hash,
                e
            );
        }
    }

    to_update_cache
}
