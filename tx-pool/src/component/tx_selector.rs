extern crate slab;
use crate::component::pool_map::PoolMap;
use crate::component::{
    entry::{TxEntry, WeightDelta},
    sort_key::AncestorsScoreSortKey,
};
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

/// Limit on consecutive failed attempts to add a transaction to a block template.
///
/// When the template is close to its size/cycles limit, most remaining transactions
/// will fail to fit. This heuristic stops the selection loop early after a large
/// number of consecutive failures, avoiding wasted work when the mempool contains
/// many entries that are too big or too low-fee to be included. 4000 is an
/// empirically chosen threshold that keeps block packing quality high while
/// bounding the worst-case selection time.
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
/// Membership budget for [`TxSelector::descendants_cache`]: the total
/// number of (ancestor → descendant) pairs retained across one selection
/// round. 200k pairs (a few MB of ids plus set overhead) is far above what
/// realistic CPFP workloads need to stay fully cached, and caps the
/// transient memory a crafted wide-deep graph can make a block-template
/// selection hold while `tx_pool.read()` is held.
const DESCENDANTS_CACHE_MEMBER_BUDGET: usize = 200_000;

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
    // Cache for calc_descendants results, bounded in total membership by
    // `budget`.  PoolMap is immutably borrowed for the lifetime of
    // TxSelector, so cached results remain valid throughout the selection
    // round.  The cache eliminates redundant BFS traversals when multiple
    // committed txs share descendants (CPFP chains); beyond the budget,
    // descendants are recomputed and dropped after use, capping peak memory
    // instead of letting a wide-deep graph accumulate quadratic set state.
    descendants_cache: std::collections::HashMap<ProposalShortId, HashSet<ProposalShortId>>,
    // Total membership currently held in `descendants_cache`.
    descendants_cache_members: usize,
    // Membership budget for `descendants_cache` (see
    // DESCENDANTS_CACHE_MEMBER_BUDGET).
    descendants_cache_budget: usize,
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
            descendants_cache_members: 0,
            descendants_cache_budget: DESCENDANTS_CACHE_MEMBER_BUDGET,
        }
    }

    #[cfg(test)]
    fn set_descendants_cache_budget_for_test(&mut self, budget: usize) {
        self.descendants_cache_budget = budget;
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
                // A proposed tx whose ancestors are not all proposed can
                // never be packaged; if this keeps firing for the same tx
                // it signals a links/entries inconsistency (ghost links),
                // which must not fail silently.
                ckb_logger::debug!(
                    "tx_selector: skipping {}: not all ancestors are proposed",
                    short_id
                );
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

    /// Descendants of `id`, served from the membership-bounded cache when
    /// possible. Falls back to a transient (uncached) BFS result once the
    /// budget is exhausted: correct either way, the cache is purely a CPU
    /// optimization for CPFP-shaped graphs.
    fn descendants_of(
        &mut self,
        id: &ProposalShortId,
    ) -> std::borrow::Cow<'_, HashSet<ProposalShortId>> {
        if self.descendants_cache.contains_key(id) {
            return std::borrow::Cow::Borrowed(&self.descendants_cache[id]);
        }
        let desc = self.pool_map.calc_descendants(id);
        if self.descendants_cache_members.saturating_add(desc.len())
            <= self.descendants_cache_budget
        {
            self.descendants_cache_members += desc.len();
            let set = self.descendants_cache.entry(id.clone()).or_insert(desc);
            return std::borrow::Cow::Borrowed(set);
        }
        std::borrow::Cow::Owned(desc)
    }

    /// Add descendants of given transactions to `modified_entries` with ancestor
    /// state updated assuming given transactions are inBlock.
    ///
    /// When multiple committed transactions share descendants (CPFP chains),
    /// the old code would remove → adjust → re-insert the same descendant
    /// once per committed ancestor.  This version collects all adjustments
    /// first, then applies them in a single remove → batch-sub → insert per
    /// unique descendant.  Descendant sets are folded into the adjustments
    /// immediately (bounded by the cache budget, see `descendants_of`), and
    /// each adjustment is one aggregate [`WeightDelta`] per unique descendant
    /// instead of a list of entries, so peak memory is O(unique descendants
    /// of the current package), not O(Σ |descendants|).
    fn update_modified_entries(
        &mut self,
        committed: &[(ProposalShortId, TxEntry)],
        committed_ids: &HashSet<ProposalShortId>,
    ) {
        use std::collections::HashMap;

        // Phase 1: collect all (descendant_id → aggregate ancestor weight to
        // subtract).
        let pool_map = self.pool_map;
        let mut adjustments: HashMap<ProposalShortId, WeightDelta> = HashMap::new();
        for (id, entry) in committed {
            let descendants = self.descendants_of(id);
            for desc_id in descendants
                .iter()
                .filter(|id| !committed_ids.contains(*id) && pool_map.has_proposed(id))
            {
                adjustments
                    .entry(desc_id.clone())
                    .or_default()
                    .add_entry(entry);
            }
        }
        if adjustments.is_empty() {
            return;
        }

        // Phase 2: apply all adjustments in a single remove → batch-sub → insert
        // per unique descendant.
        for (desc_id, delta) in adjustments {
            // Note: since https://github.com/nervosnetwork/ckb/pull/3706
            // calc_descendants() may not consistent
            if let Some(mut desc) = self
                .modified_entries
                .remove(&desc_id)
                .or_else(|| self.pool_map.get(&desc_id).cloned())
            {
                desc.sub_ancestors_weight(delta);
                self.modified_entries.insert_entry(desc);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::pool_map::Status;
    use crate::component::tests::util::{MOCK_CYCLES, build_tx};
    use ckb_types::core::{Capacity, TransactionView};
    use ckb_types::packed::Byte32;

    const SIZE: usize = 100;

    fn add_proposed(pool_map: &mut PoolMap, tx: &TransactionView, fee: u64) {
        pool_map
            .add_entry(
                TxEntry::dummy_resolve(tx.clone(), MOCK_CYCLES, Capacity::shannons(fee), SIZE),
                Status::Proposed,
            )
            .unwrap();
    }

    /// A CPFP-shaped graph: roots A and B, child C spending both, grandchild
    /// D spending C. Committed A and B share descendants C and D.
    fn shared_descendant_graph() -> (PoolMap, [TransactionView; 4]) {
        let mut pool_map = PoolMap::new(125);
        let a = build_tx(vec![(&Byte32::new([1u8; 32]), 0)], 1);
        let b = build_tx(vec![(&Byte32::new([2u8; 32]), 0)], 1);
        let c = build_tx(vec![(&a.hash(), 0), (&b.hash(), 0)], 1);
        let d = build_tx(vec![(&c.hash(), 0)], 1);
        add_proposed(&mut pool_map, &a, 1_000);
        add_proposed(&mut pool_map, &b, 2_000);
        add_proposed(&mut pool_map, &c, 3_000);
        add_proposed(&mut pool_map, &d, 4_000);
        (pool_map, [a, b, c, d])
    }

    /// Aggregate batch subtraction must produce exactly the same adjusted
    /// entries as before, with and without the descendants cache, and match
    /// the hand-computed expectation.
    #[test]
    fn aggregate_adjustments_match_across_cache_modes() {
        let (pool_map, [a, b, c, d]) = shared_descendant_graph();
        let committed: Vec<(ProposalShortId, TxEntry)> = [&a, &b]
            .iter()
            .map(|tx| {
                let id = tx.proposal_short_id();
                (id.clone(), pool_map.get(&id).cloned().unwrap())
            })
            .collect();
        let committed_ids: HashSet<ProposalShortId> =
            committed.iter().map(|(id, _)| id.clone()).collect();

        let mut results = Vec::new();
        for budget in [usize::MAX, 0] {
            let mut selector = TxSelector::new(&pool_map);
            selector.set_descendants_cache_budget_for_test(budget);
            selector.update_modified_entries(&committed, &committed_ids);
            let c_adj = selector
                .modified_entries
                .get(&c.proposal_short_id())
                .cloned()
                .expect("C is adjusted");
            let d_adj = selector
                .modified_entries
                .get(&d.proposal_short_id())
                .cloned()
                .expect("D is adjusted");
            results.push((c_adj, d_adj));
        }
        let (c_transient, d_transient) = results.pop().unwrap();
        let (c_cached, d_cached) = results.pop().unwrap();
        assert_eq!(c_cached, c_transient);
        assert_eq!(d_cached, d_transient);

        // C's ancestors were exactly {A, B}: subtracting both leaves only
        // C's own weight.
        assert_eq!(c_cached.ancestors_count, 1);
        assert_eq!(c_cached.ancestors_size, SIZE);
        assert_eq!(c_cached.ancestors_fee, Capacity::shannons(3_000));
        // D's ancestors were {A, B, C}: subtracting A and B leaves D + C.
        assert_eq!(d_cached.ancestors_count, 2);
        assert_eq!(d_cached.ancestors_size, 2 * SIZE);
        assert_eq!(d_cached.ancestors_fee, Capacity::shannons(7_000));
    }

    /// The cache must never hold more memberships than its budget allows,
    /// no matter how wide the descendant graph is; over-budget lookups fall
    /// back to transient (uncached) results.
    #[test]
    fn descendants_cache_members_stay_within_budget() {
        let (pool_map, [a, b, c, d]) = shared_descendant_graph();
        let mut selector = TxSelector::new(&pool_map);
        selector.set_descendants_cache_budget_for_test(2);

        for tx in [&a, &b, &c, &d] {
            let _ = selector.descendants_of(&tx.proposal_short_id());
        }
        assert!(selector.descendants_cache_members <= 2);
        // A's descendant set ({C, D}) fit exactly; B's identical set pushed
        // the total over budget and was served transiently.
        assert!(
            selector
                .descendants_cache
                .contains_key(&a.proposal_short_id())
        );
        assert!(
            !selector
                .descendants_cache
                .contains_key(&b.proposal_short_id())
        );
    }
}
