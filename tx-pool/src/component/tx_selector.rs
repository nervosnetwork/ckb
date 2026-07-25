extern crate slab;
use crate::component::pool_map::PoolMap;
use crate::component::{
    entry::{TxEntry, WeightDelta},
    sort_key::AncestorsScoreSortKey,
};
use ckb_logger::debug;
use ckb_types::{core::Cycle, packed::ProposalShortId};
use multi_index_map::MultiIndexMap;
use std::collections::{BTreeSet, HashMap, HashSet};

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

/// Maximum number of expanded cell-dependency occurrences inspected while
/// imposing conditional reader-before-spender order on one template. The
/// accepted pool already charges these occurrences as resident memory; this
/// independent cap bounds block-template transient CPU and memory.
const SELECTED_DEP_ORDERING_BUDGET: usize = 200_000;

/// Maximum number of exact SCC-shedding passes. One weakest member is removed
/// from every cyclic component per pass. A deliberately dense component can
/// require many feedback vertices, so after this bound the selector keeps
/// only the strongest representative of each remaining cyclic component.
/// That preserves a deterministic valid template while bounding hostile
/// conditional-cycle work.
const MAX_CONDITIONAL_CYCLE_ROUNDS: usize = 64;

/// Compute exact strongly connected components without recursive stack use.
/// The graph is already bounded by [`SELECTED_DEP_ORDERING_BUDGET`].
fn strongly_connected_components(
    nodes: &HashSet<ProposalShortId>,
    children: &HashMap<ProposalShortId, HashSet<ProposalShortId>>,
) -> Vec<Vec<ProposalShortId>> {
    let mut visited = HashSet::with_capacity(nodes.len());
    let mut finish = Vec::with_capacity(nodes.len());
    for start in nodes {
        if visited.contains(start) {
            continue;
        }
        let mut stack = vec![(start.clone(), false)];
        while let Some((id, expanded)) = stack.pop() {
            if expanded {
                finish.push(id);
                continue;
            }
            if !visited.insert(id.clone()) {
                continue;
            }
            stack.push((id.clone(), true));
            if let Some(next) = children.get(&id) {
                for child in next {
                    if nodes.contains(child) && !visited.contains(child) {
                        stack.push((child.clone(), false));
                    }
                }
            }
        }
    }

    let mut parents = nodes
        .iter()
        .cloned()
        .map(|id| (id, Vec::new()))
        .collect::<HashMap<_, _>>();
    for (parent, next) in children {
        if !nodes.contains(parent) {
            continue;
        }
        for child in next {
            if nodes.contains(child) {
                parents
                    .get_mut(child)
                    .expect("selected child has a reverse edge bucket")
                    .push(parent.clone());
            }
        }
    }

    visited.clear();
    let mut components = Vec::new();
    for start in finish.into_iter().rev() {
        if !visited.insert(start.clone()) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![start];
        while let Some(id) = stack.pop() {
            component.push(id.clone());
            for parent in &parents[&id] {
                if visited.insert(parent.clone()) {
                    stack.push(parent.clone());
                }
            }
        }
        components.push(component);
    }
    components
}

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
        let entries = self.order_selected_entries();
        size = entries
            .iter()
            .fold(0usize, |total, entry| total.saturating_add(entry.size));
        cycles = entries
            .iter()
            .fold(0u64, |total, entry| total.saturating_add(entry.cycles));
        (entries, size, cycles)
    }

    /// Add conditional `dep reader -> spender` edges only to the bounded set
    /// already selected by the causal CPFP policy, then return one
    /// deterministic topological order. Conditional cycles are not accepted
    /// state corruption: drop the weakest cycle member and its selected
    /// causal descendants, then retry.
    fn order_selected_entries(&mut self) -> Vec<TxEntry> {
        let selected = std::mem::take(&mut self.entries);
        if selected.len() < 2 {
            return selected;
        }

        let selected = self.retain_selected_with_dep_budget(selected, SELECTED_DEP_ORDERING_BUDGET);
        if selected.len() < 2 {
            return selected;
        }
        let mut selected = selected
            .into_iter()
            .enumerate()
            .map(|(rank, entry)| (entry.proposal_short_id(), (rank, entry)))
            .collect::<HashMap<_, _>>();

        let mut cycle_round = 0usize;
        loop {
            let mut children = selected
                .keys()
                .cloned()
                .map(|id| (id, HashSet::new()))
                .collect::<HashMap<_, _>>();
            let mut causal_children = selected
                .keys()
                .cloned()
                .map(|id| (id, HashSet::new()))
                .collect::<HashMap<_, _>>();
            let mut indegree = selected
                .keys()
                .cloned()
                .map(|id| (id, 0usize))
                .collect::<HashMap<_, _>>();

            let mut add_edge = |parent: &ProposalShortId, child: &ProposalShortId| {
                if parent == child
                    || !selected.contains_key(parent)
                    || !selected.contains_key(child)
                {
                    return;
                }
                if children
                    .get_mut(parent)
                    .expect("selected parent has an edge bucket")
                    .insert(child.clone())
                {
                    *indegree
                        .get_mut(child)
                        .expect("selected child has an indegree") += 1;
                }
            };

            // Persistent links contain causal producer edges only.
            for id in selected.keys() {
                if let Some(parents) = self.pool_map.links.get_parents(id) {
                    for parent in parents {
                        if selected.contains_key(parent) {
                            causal_children
                                .get_mut(parent)
                                .expect("selected causal parent has an edge bucket")
                                .insert(id.clone());
                        }
                        add_edge(parent, id);
                    }
                }
            }

            // There is at most one accepted spender for an outpoint.
            let spenders = selected
                .iter()
                .flat_map(|(id, (_, entry))| {
                    entry
                        .transaction()
                        .input_pts_iter()
                        .map(move |out_point| (crate::util::compact_packed(&out_point), id.clone()))
                })
                .collect::<HashMap<_, _>>();
            for (reader, (_, entry)) in &selected {
                let mut seen = HashSet::new();
                for dep in entry.related_dep_out_points() {
                    let dep = crate::util::compact_packed(dep);
                    if seen.insert(dep.clone())
                        && let Some(spender) = spenders.get(&dep)
                    {
                        add_edge(reader, spender);
                    }
                }
            }

            let mut ready = indegree
                .iter()
                .filter(|(_, degree)| **degree == 0)
                .map(|(id, _)| {
                    let rank = selected.get(id).expect("selected rank exists").0;
                    (rank, id.clone())
                })
                .collect::<BTreeSet<_>>();
            let mut order = Vec::with_capacity(selected.len());
            while let Some((rank, id)) = ready.pop_first() {
                order.push(id.clone());
                for child in &children[&id] {
                    let degree = indegree
                        .get_mut(child)
                        .expect("conditional child remains selected");
                    *degree -= 1;
                    if *degree == 0 {
                        ready.insert((selected[child].0, child.clone()));
                    }
                }
                debug_assert_eq!(selected[&id].0, rank);
            }

            if order.len() == selected.len() {
                return order
                    .into_iter()
                    .map(|id| selected.remove(&id).expect("ordered entry exists").1)
                    .collect();
            }

            // Kahn's residual also contains acyclic nodes downstream of a
            // cycle. Selecting the weakest residual node lets an attacker
            // discard unrelated low-fee transactions before touching the
            // actual cycle. Exact SCCs isolate only cycle participants.
            let nodes = selected.keys().cloned().collect::<HashSet<_>>();
            let cyclic_components = strongly_connected_components(&nodes, &children)
                .into_iter()
                .filter(|component| component.len() > 1)
                .collect::<Vec<_>>();
            assert!(
                !cyclic_components.is_empty(),
                "incomplete topological order must contain a cyclic component"
            );
            cycle_round += 1;
            let bounded_fallback = cycle_round > MAX_CONDITIONAL_CYCLE_ROUNDS;
            let mut roots = HashSet::new();
            for component in cyclic_components {
                let by_strength = |left: &&ProposalShortId, right: &&ProposalShortId| {
                    let left_entry = self
                        .pool_map
                        .get_by_id(left)
                        .expect("selected entry remains accepted");
                    let right_entry = self
                        .pool_map
                        .get_by_id(right)
                        .expect("selected entry remains accepted");
                    left_entry
                        .evict_key
                        .cmp(&right_entry.evict_key)
                        .then_with(|| left.cmp(right))
                };
                if bounded_fallback {
                    let strongest = component
                        .iter()
                        .max_by(by_strength)
                        .expect("cyclic component is non-empty")
                        .clone();
                    roots.extend(component.into_iter().filter(|id| id != &strongest));
                } else {
                    roots.insert(
                        component
                            .iter()
                            .min_by(by_strength)
                            .expect("cyclic component is non-empty")
                            .clone(),
                    );
                }
            }

            // A selected causal child cannot remain after its producer is
            // shed. Traverse all roots together so work is O(selected graph),
            // not one complete descendant walk per cyclic component.
            let mut dropped = roots.clone();
            let mut stack = roots.into_iter().collect::<Vec<_>>();
            while let Some(id) = stack.pop() {
                for child in &causal_children[&id] {
                    if dropped.insert(child.clone()) {
                        stack.push(child.clone());
                    }
                }
            }
            selected.retain(|id, _| !dropped.contains(id));
            if selected.len() < 2 {
                return selected.into_values().map(|(_, entry)| entry).collect();
            }
        }
    }

    /// Retain a causally closed selected subset while bounding the number of
    /// expanded dependency occurrences inspected for conditional ordering.
    ///
    /// Truncating at the first over-budget entry lets a high-score transaction
    /// with a huge dep group censor every independent transaction behind it.
    /// Instead, shed that entry and propagate the causal omission as the
    /// already parent-first selection order is visited. After the occurrence
    /// budget is consumed, a constant-time empty-dep probe still admits
    /// independent transactions that need no conditional edge work.
    fn retain_selected_with_dep_budget(
        &self,
        selected: Vec<TxEntry>,
        budget: usize,
    ) -> Vec<TxEntry> {
        let mut remaining = budget;
        let mut dropped = HashSet::new();
        let mut retained = Vec::with_capacity(selected.len());
        for entry in selected {
            let id = entry.proposal_short_id();
            let causal_parent_dropped = self
                .pool_map
                .links
                .get_parents(&id)
                .is_some_and(|parents| parents.iter().any(|parent| dropped.contains(parent)));
            if causal_parent_dropped {
                dropped.insert(id);
                continue;
            }

            if remaining == 0 {
                if entry.related_dep_out_points().next().is_some() {
                    dropped.insert(id);
                } else {
                    retained.push(entry);
                }
                continue;
            }
            let inspected = entry
                .related_dep_out_points()
                .take(remaining.saturating_add(1))
                .count();
            if inspected > remaining {
                remaining = 0;
                dropped.insert(id);
            } else {
                remaining -= inspected;
                retained.push(entry);
            }
        }
        retained
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
#[path = "tests/tx_selector.rs"]
mod tests;
