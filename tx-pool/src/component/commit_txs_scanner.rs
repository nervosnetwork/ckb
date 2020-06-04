use crate::component::{
    entry::{TxEntry, TxModifiedEntries},
    proposed::ProposedPool,
};
use ckb_fee_estimator::FeeRate;
use ckb_types::{core::Cycle, packed::ProposalShortId};
use std::cmp::max;
use std::collections::HashSet;

/// node will give up to package more txs after MAX_CONSECUTIVE_FAILED
const MAX_CONSECUTIVE_FAILED: usize = 500;

/// find txs to package into commitment
pub struct CommitTxsScanner<'a> {
    proposed_pool: &'a ProposedPool,
    entries: Vec<TxEntry>,
    // modified entries, after put a tx into block,
    // the scores of descendants txs should be updated,
    // these modified entries is stored in modified_entries.
    // in each loop,
    // we pick tx from modified_entries and pool to find the best tx to package
    modified_entries: TxModifiedEntries,
    // txs that packaged in block
    fetched_txs: HashSet<ProposalShortId>,
}

impl<'a> CommitTxsScanner<'a> {
    pub fn new(proposed_pool: &'a ProposedPool) -> CommitTxsScanner<'a> {
        CommitTxsScanner {
            proposed_pool,
            entries: Vec::new(),
            modified_entries: TxModifiedEntries::default(),
            fetched_txs: HashSet::default(),
        }
    }
    /// find txs to commit, return TxEntry vector, total_size and total_cycles.
    pub fn txs_to_commit(
        mut self,
        size_limit: usize,
        cycles_limit: Cycle,
        min_fee_rate: FeeRate,
    ) -> (Vec<TxEntry>, usize, Cycle) {
        let mut size: usize = 0;
        let mut cycles: Cycle = 0;
        self.proposed_pool.with_sorted_by_score_iter(|iter| {
            let mut candidate_pool_tx = None;
            let mut candidate_modified_tx = None;
            loop {
                // 1. choose best tx from pool and modified entries
                // 2. compare the two txs, package the better one
                // 3. update modified entries
                Self::next_candidate_tx(
                    &mut candidate_pool_tx,
                    iter,
                    size_limit,
                    cycles_limit,
                    size,
                    cycles,
                    |tx| {
                        let tx_id = tx.transaction.proposal_short_id();
                        !self.fetched_txs.contains(&tx_id)
                            && !self.modified_entries.contains_key(&tx_id)
                            && tx.ancestors_fee >= min_fee_rate.fee(tx.ancestors_size)
                    },
                );
                self.modified_entries.with_sorted_by_score_iter(|iter| {
                    Self::next_candidate_tx(
                        &mut candidate_modified_tx,
                        iter,
                        size_limit,
                        cycles_limit,
                        size,
                        cycles,
                        |tx| tx.ancestors_fee >= min_fee_rate.fee(tx.ancestors_size),
                    );
                });
                // take tx with higher scores
                let tx_entry = match max(&mut candidate_pool_tx, &mut candidate_modified_tx).take()
                {
                    Some(entry) => entry,
                    None => {
                        // can't find any satisfied tx
                        break;
                    }
                };
                debug_assert!(!self
                    .fetched_txs
                    .contains(&tx_entry.transaction.proposal_short_id()));
                // prepare to package tx with ancestors
                let mut ancestors = self
                    .proposed_pool
                    .get_ancestors(&tx_entry.transaction.proposal_short_id())
                    .into_iter()
                    .filter_map(|short_id| {
                        if self.fetched_txs.contains(&short_id) {
                            None
                        } else {
                            self.modified_entries.get(&short_id).or_else(|| {
                                let entry = self
                                    .proposed_pool
                                    .get(&short_id)
                                    .expect("pool should be consistent");
                                Some(entry)
                            })
                        }
                    })
                    .cloned()
                    .collect::<HashSet<TxEntry>>();
                ancestors.insert(tx_entry.to_owned());
                debug_assert_eq!(
                    tx_entry.ancestors_cycles,
                    ancestors.iter().map(|entry| entry.cycles).sum::<u64>(),
                    "proposed tx pool ancestors cycles inconsistent"
                );
                debug_assert_eq!(
                    tx_entry.ancestors_size,
                    ancestors.iter().map(|entry| entry.size).sum::<usize>(),
                    "proposed tx pool ancestors size inconsistent"
                );
                debug_assert_eq!(
                    tx_entry.ancestors_count,
                    ancestors.len(),
                    "proposed tx pool ancestors count inconsistent"
                );
                // update all descendants and insert into modified
                self.update_modified_entries(&ancestors);
                // sort acestors by ancestors_count,
                // if A is an ancestor of B, B.ancestors_count must large than A
                let mut ancestors = ancestors.into_iter().collect::<Vec<_>>();
                ancestors.sort_unstable_by_key(|entry| entry.ancestors_count);
                // insert ancestors
                for entry in ancestors {
                    let short_id = entry.transaction.proposal_short_id();
                    // try remove from modified
                    self.modified_entries.remove(&short_id);
                    let is_inserted = self.fetched_txs.insert(short_id);
                    debug_assert!(is_inserted, "package duplicate txs");
                    cycles = cycles.saturating_add(entry.cycles);
                    size = size.saturating_add(entry.size);
                    self.entries.push(entry);
                }
            }
        });
        (self.entries, size, cycles)
    }

    /// update weight for all descendants of packaged txs
    fn update_modified_entries(&mut self, new_fetched_txs: &HashSet<TxEntry>) {
        for ptx in new_fetched_txs {
            let ptx_id = ptx.transaction.proposal_short_id();
            if self.fetched_txs.contains(&ptx_id) {
                continue;
            }
            let descendants = self.proposed_pool.get_descendants(&ptx_id);
            for id in descendants {
                let mut tx = self.modified_entries.remove(&id).unwrap_or_else(|| {
                    self.proposed_pool
                        .get(&id)
                        .map(ToOwned::to_owned)
                        .expect("pool consistent")
                });
                tx.sub_entry_weight(&ptx);
                self.modified_entries.insert(tx);
            }
        }
    }

    /// find next fetchable candidate tx from iterator then place it into entry
    /// the tx should satisfy the size and cycles limits and pass the is_satisfied
    fn next_candidate_tx<F: Fn(&TxEntry) -> bool>(
        entry: &mut Option<TxEntry>,
        iter: &mut dyn Iterator<Item = &TxEntry>,
        size_limit: usize,
        cycles_limit: Cycle,
        size: usize,
        cycles: Cycle,
        is_satisfied: F,
    ) {
        let is_satisfy_limit = |entry: &TxEntry| -> bool {
            let next_cycles = cycles.saturating_add(entry.ancestors_cycles);
            let next_size = size.saturating_add(entry.ancestors_size);
            next_cycles <= cycles_limit && next_size <= size_limit
        };
        // return entry if it's exists and satisfy the requirements
        if let Some(tx_entry) = entry {
            if is_satisfied(&tx_entry) && is_satisfy_limit(&tx_entry) {
                return;
            }
        }
        let mut consecutive_failed = 0;
        for tx_entry in iter {
            if !is_satisfied(&tx_entry) {
                continue;
            }

            if !is_satisfy_limit(&tx_entry) {
                consecutive_failed += 1;
                // give up if failed too many times
                if consecutive_failed > MAX_CONSECUTIVE_FAILED {
                    break;
                }
                continue;
            }

            // find new tx entry
            entry.replace(tx_entry.to_owned());
            return;
        }
        // set entry to None if iter is end or consecutive failed
        entry.take();
    }
}
