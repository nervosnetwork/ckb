extern crate slab;
use crate::component::pool_map::PoolMap;
use crate::component::{entry::TxEntry, sort_key::AncestorsScoreSortKey};
use ckb_logger::debug;
use ckb_types::{core::Cycle, packed::ProposalShortId};
use multi_index_map::MultiIndexMap;
use std::collections::HashSet;

// A template data struct used to store modified entries when package txs
#[derive(MultiIndexMap, Clone)]
pub struct ModifiedTx {
    #[multi_index(hashed_unique)]
    pub id: ProposalShortId,
    #[multi_index(ordered_non_unique)]
    pub score: AncestorsScoreSortKey,
    pub inner: TxEntry,
}

impl MultiIndexModifiedTxMap {
    pub fn next_best_entry(&self) -> Option<&TxEntry> {
        // ordered_non_unique iterator is DoubleEnded, so `next_back()` returns
        // the max-score entry in O(1) instead of walking the whole index.
        self.iter_by_score().next_back().map(|x| &x.inner)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.get_by_id(id).map(|x| &x.inner)
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.get_by_id(id).is_some()
    }

    pub fn insert_entry(&mut self, entry: TxEntry) {
        let score = AncestorsScoreSortKey::from(&entry);
        self.insert(ModifiedTx {
            id: entry.proposal_short_id(),
            score,
            inner: entry,
        });
    }

    pub fn remove(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        self.remove_by_id(id).map(|x| x.inner)
    }
}

// Limit the number of attempts to add transactions to the block when it is
// close to full; this is just a simple heuristic to finish quickly if the
// mempool has a lot of entries.
const MAX_CONSECUTIVE_FAILURES: usize = 4000;

/// Selects transactions for inclusion in a block-template using **package-aware** fee-rate sorting.
///
/// ### Package definition
/// A package is a connected group of ≤ MAX_ANCESTORS_COUNT（1_000）transactions
/// The mempool is linearly ordered into non-overlapping packages using a greedy clustering
/// algorithm that maximizes total fee for a given size and cycles.
///
/// ### Why packages instead of individual transactions?
/// - A high-fee child transaction is worthless without its low-fee parent(s) (CPFP).
/// - A low-fee parent with many high-fee children should be prioritized as a unit (package).
/// - Sorting individual txs breaks incentive compatibility and leads to suboptimal templates.
///
/// ### Sorting rule
/// Packages are sorted by **package fee rate** = total fee / total weight of the entire package.
///
/// ### Selection process
// This is accomplished by walking the descendants of selected
// transactions and storing a temporary modified state in `modified_entries``.
// Each time through the loop, we compare the best transaction in
// `modified_entries` with the next transaction in the tx-pool to decide what
// transaction package to work on next.
pub struct TxSelector<'a> {
    pool_map: &'a PoolMap,
    entries: Vec<TxEntry>,
    // modified_entries will store sorted packages after they are modified
    // because some of their txs are already in the block
    modified_entries: MultiIndexModifiedTxMap,
    // txs that packaged in block
    fetched_txs: HashSet<ProposalShortId>,
    // Keep track of entries that failed inclusion, to avoid duplicate work
    failed_txs: HashSet<ProposalShortId>,
    // Cache for calc_descendants results.  PoolMap is immutably borrowed for
    // the lifetime of TxSelector, so cached results remain valid throughout
    // the selection round.  This eliminates redundant BFS traversals when
    // multiple committed txs share descendants (CPFP chains).
    descendants_cache: std::collections::HashMap<ProposalShortId, HashSet<ProposalShortId>>,
}

impl<'a> TxSelector<'a> {
    pub fn new(pool_map: &'a PoolMap) -> TxSelector<'a> {
        TxSelector {
            entries: Vec::new(),
            pool_map,
            modified_entries: MultiIndexModifiedTxMap::default(),
            fetched_txs: HashSet::default(),
            failed_txs: HashSet::default(),
            descendants_cache: std::collections::HashMap::new(),
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

        let mut iter = self
            .pool_map
            .sorted_proposed_iter()
            .filter(|entry| {
                entry.ancestors_size <= size_limit && entry.ancestors_cycles <= cycles_limit
            })
            .peekable();
        loop {
            let mut using_modified = false;

            if let Some(entry) = iter.peek()
                && self.skip_proposed_entry(&entry.proposal_short_id())
            {
                iter.next();
                continue;
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
                }
                self.failed_txs.insert(short_id);
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
            let ancestors_ids = self.pool_map.calc_ancestors(&short_id);
            if ancestors_ids
                .iter()
                .any(|id| !self.pool_map.has_proposed(id))
            {
                if using_modified {
                    self.modified_entries.remove(&short_id);
                }
                self.failed_txs.insert(short_id);
                consecutive_failed += 1;
                if consecutive_failed > MAX_CONSECUTIVE_FAILURES {
                    break;
                }
                continue;
            }

            let mut ancestors: Vec<(ProposalShortId, TxEntry)> = ancestors_ids
                .iter()
                .filter_map(only_unconfirmed)
                .map(|entry| (entry.proposal_short_id(), entry.clone()))
                .collect();

            // sort ancestors by ancestors_count,
            // if A is an ancestor of B, B.ancestors_count must large than A
            ancestors.sort_unstable_by_key(|(_, entry)| entry.ancestors_count);
            let tx_short_id = tx_entry.proposal_short_id();
            ancestors.push((tx_short_id, tx_entry));

            let committed_ids: HashSet<ProposalShortId> =
                ancestors.iter().map(|(id, _)| id.clone()).collect();

            self.update_modified_entries(&ancestors, &committed_ids);

            for (short_id, entry) in ancestors {
                let is_new = self.fetched_txs.insert(short_id.clone());
                if !is_new {
                    debug!("package duplicate txs {}", short_id);
                    continue;
                }
                cycles = cycles.saturating_add(entry.cycles);
                size = size.saturating_add(entry.size);
                self.entries.push(entry);
                // try remove from modified
                self.modified_entries.remove(&short_id);
            }

            consecutive_failed = 0;
        }
        (self.entries, size, cycles)
    }

    fn retrieve_entry(&self, short_id: &ProposalShortId) -> Option<&TxEntry> {
        self.modified_entries
            .get(short_id)
            .or_else(|| self.pool_map.get_proposed(short_id))
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

    /// Memoized wrapper around `pool_map.calc_descendants`.
    ///
    /// PoolMap is immutably borrowed for the lifetime of TxSelector, so
    /// cached results remain valid throughout the selection round.
    fn cache_descendants(&mut self, id: &ProposalShortId) {
        if !self.descendants_cache.contains_key(id) {
            let desc = self.pool_map.calc_descendants(id);
            self.descendants_cache.insert(id.clone(), desc);
        }
    }

    /// Add descendants of given transactions to `modified_entries` with ancestor
    /// state updated assuming given transactions are inBlock.
    ///
    /// When multiple committed transactions share descendants (CPFP chains),
    /// the old code would remove → adjust → re-insert the same descendant
    /// once per committed ancestor.  This version collects all adjustments
    /// first, then applies them in a single remove → batch-sub → insert per
    /// unique descendant.  Descendant sets are memoized to avoid redundant
    /// BFS traversals across committed txs in the same package.
    fn update_modified_entries(
        &mut self,
        committed: &[(ProposalShortId, TxEntry)],
        committed_ids: &HashSet<ProposalShortId>,
    ) {
        use std::collections::HashMap;

        // Phase 1a: populate descendants cache for all committed txs.
        // Track whether any committed tx actually has descendants — leaf
        // packages (the common non-CPFP case) can skip the rest entirely.
        let mut has_descendants = false;
        for (id, _) in committed {
            self.cache_descendants(id);
            if !self.descendants_cache[id].is_empty() {
                has_descendants = true;
            }
        }
        if !has_descendants {
            return;
        }

        // Phase 1b: collect all (descendant_id → list of ancestor entries to subtract).
        // Uses cached descendant sets — no BFS here.
        let mut adjustments: HashMap<ProposalShortId, Vec<&TxEntry>> = HashMap::new();
        for (id, entry) in committed {
            let descendants = &self.descendants_cache[id];
            for desc_id in descendants
                .iter()
                .filter(|id| !committed_ids.contains(*id) && self.pool_map.has_proposed(id))
            {
                adjustments.entry(desc_id.clone()).or_default().push(entry);
            }
        }

        // Phase 2: apply all adjustments in a single remove → batch-sub → insert
        // per unique descendant.
        for (desc_id, entries_to_sub) in adjustments {
            // Note: since https://github.com/nervosnetwork/ckb/pull/3706
            // calc_descendants() may not consistent
            if let Some(mut desc) = self
                .modified_entries
                .remove(&desc_id)
                .or_else(|| self.pool_map.get(&desc_id).cloned())
            {
                for entry in entries_to_sub {
                    desc.sub_ancestor_weight(entry);
                }
                self.modified_entries.insert_entry(desc);
            }
        }
    }
}
