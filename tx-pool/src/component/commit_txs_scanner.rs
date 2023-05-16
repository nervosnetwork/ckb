use crate::component::{container::AncestorsScoreSortKey, entry::TxEntry, proposed::ProposedPool};
use ckb_types::{core::Cycle, packed::ProposalShortId};
use ckb_util::LinkedHashMap;
use std::collections::{BTreeSet, HashMap, HashSet};
use crate::component::pool_map::MultiIndexPoolEntryMap;

// A template data struct used to store modified entries when package txs
#[derive(Default)]
pub struct TxModifiedEntries {
    entries: HashMap<ProposalShortId, TxEntry>,
    sorted_index: BTreeSet<AncestorsScoreSortKey>,
}

impl TxModifiedEntries {
    pub fn next_best_entry(&self) -> Option<&TxEntry> {
        self.sorted_index
            .iter()
            .max()
            .map(|key| self.entries.get(&key.id).expect("consistent"))
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.entries.get(id)
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.entries.contains_key(id)
    }

    pub fn insert(&mut self, entry: TxEntry) {
        let key = AncestorsScoreSortKey::from(&entry);
        let short_id = entry.proposal_short_id();
        self.entries.insert(short_id, entry);
        self.sorted_index.insert(key);
    }

    pub fn remove(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        self.entries.remove(id).map(|entry| {
            self.sorted_index.remove(&(&entry).into());
            entry
        })
    }
}

// Limit the number of attempts to add transactions to the block when it is
// close to full; this is just a simple heuristic to finish quickly if the
// mempool has a lot of entries.
const MAX_CONSECUTIVE_FAILURES: usize = 500;

/// find txs to package into commitment
pub struct CommitTxsScanner<'a> {
    proposed_pool: &'a ProposedPool,
    pool_entries: &'a MultiIndexPoolEntryMap,
    entries: Vec<TxEntry>,
    // modified_entries will store sorted packages after they are modified
    // because some of their txs are already in the block
    modified_entries: TxModifiedEntries,
    // txs that packaged in block
    fetched_txs: HashSet<ProposalShortId>,
    // Keep track of entries that failed inclusion, to avoid duplicate work
    failed_txs: HashSet<ProposalShortId>,
}

impl<'a> CommitTxsScanner<'a> {
    pub fn new(proposed_pool: &'a ProposedPool, pool_entries: &'a MultiIndexPoolEntryMap) -> CommitTxsScanner<'a> {
        CommitTxsScanner {
            proposed_pool,
            entries: Vec::new(),
            pool_entries: pool_entries,
            modified_entries: TxModifiedEntries::default(),
            fetched_txs: HashSet::default(),
            failed_txs: HashSet::default(),
        }
    }

    /// find txs to commit, return TxEntry vector, total_size and total_cycles.
    pub fn txs_to_commit(
        mut self,
        size_limit: usize,
        cycles_limit: Cycle,
    ) -> (Vec<TxEntry>, usize, Cycle) {
        let mut size: usize = 0;
        let mut cycles: Cycle = 0;
        let mut consecutive_failed = 0;

        let mut iter = self.pool_entries.score_sorted_iter().peekable();
        loop {
            let mut using_modified = false;

            if let Some(entry) = iter.peek() {
                if self.skip_proposed_entry(&entry.proposal_short_id()) {
                    iter.next();
                    continue;
                }
            }

            // First try to find a new transaction in `proposed_pool` to evaluate.
            let tx_entry: TxEntry = match (iter.peek(), self.modified_entries.next_best_entry()) {
                (Some(entry), Some(best_modified)) => {
                    if &best_modified > entry {
                        using_modified = true;
                        best_modified.clone()
                    } else {
                        // worse than `proposed_pool`
                        iter.next().cloned().expect("peek guard")
                    }
                }
                (Some(_), None) => {
                    // Either no entry in `modified_entries`
                    iter.next().cloned().expect("peek guarded")
                }
                (None, Some(best_modified)) => {
                    // We're out of entries in `proposed`; use the entry from `modified_entries`
                    using_modified = true;
                    best_modified.clone()
                }
                (None, None) => {
                    break;
                }
            };

            let short_id = tx_entry.proposal_short_id();
            let next_size = size.saturating_add(tx_entry.ancestors_size);
            let next_cycles = cycles.saturating_add(tx_entry.ancestors_cycles);

            if next_cycles > cycles_limit || next_size > size_limit {
                consecutive_failed += 1;
                if using_modified {
                    self.modified_entries.remove(&short_id);
                    self.failed_txs.insert(short_id.clone());
                }
                if consecutive_failed > MAX_CONSECUTIVE_FAILURES {
                    break;
                }
                continue;
            }

            let only_unconfirmed = |short_id| {
                if self.fetched_txs.contains(short_id) {
                    None
                } else {
                    let entry = self.retrieve_entry(short_id);
                    debug_assert!(entry.is_some(), "pool should be consistent");
                    entry
                }
            };

            // prepare to package tx with ancestors
            let ancestors_ids = self.proposed_pool.calc_ancestors(&short_id);
            let mut ancestors = ancestors_ids
                .iter()
                .filter_map(only_unconfirmed)
                .cloned()
                .collect::<Vec<TxEntry>>();

            // sort ancestors by ancestors_count,
            // if A is an ancestor of B, B.ancestors_count must large than A
            ancestors.sort_unstable_by_key(|entry| entry.ancestors_count);
            ancestors.push(tx_entry.to_owned());

            let ancestors: LinkedHashMap<ProposalShortId, TxEntry> = ancestors
                .into_iter()
                .map(|entry| (entry.proposal_short_id(), entry))
                .collect();

            for (short_id, entry) in &ancestors {
                let is_inserted = self.fetched_txs.insert(short_id.clone());
                debug_assert!(is_inserted, "package duplicate txs");
                cycles = cycles.saturating_add(entry.cycles);
                size = size.saturating_add(entry.size);
                self.entries.push(entry.to_owned());
                // try remove from modified
                self.modified_entries.remove(short_id);
            }

            self.update_modified_entries(&ancestors);
        }
        (self.entries, size, cycles)
    }

    fn retrieve_entry(&self, short_id: &ProposalShortId) -> Option<&TxEntry> {
        self.modified_entries
            .get(short_id)
            .or_else(|| self.proposed_pool.get(short_id))
    }

    // Skip entries in `proposed` that are already in a block or are present
    // in `modified_entries` (which implies that the mapTx ancestor state is
    // stale due to ancestor inclusion in the block)
    // Also skip transactions that we've already failed to add.
    fn skip_proposed_entry(&self, short_id: &ProposalShortId) -> bool {
        self.fetched_txs.contains(short_id)
            || self.modified_entries.contains_key(short_id)
            || self.failed_txs.contains(short_id)
    }

    /// Add descendants of given transactions to `modified_entries` with ancestor
    /// state updated assuming given transactions are inBlock.
    fn update_modified_entries(&mut self, already_added: &LinkedHashMap<ProposalShortId, TxEntry>) {
        for (id, entry) in already_added {
            let descendants = self.proposed_pool.calc_descendants(id);
            for desc_id in descendants
                .iter()
                .filter(|id| !already_added.contains_key(id))
            {
                // Note: since https://github.com/nervosnetwork/ckb/pull/3706
                // calc_descendants() may not consistent
                if let Some(mut desc) = self
                    .modified_entries
                    .remove(desc_id)
                    .or_else(|| self.proposed_pool.get(desc_id).cloned())
                {
                    desc.sub_entry_weight(entry);
                    self.modified_entries.insert(desc);
                }
            }
        }
    }
}
