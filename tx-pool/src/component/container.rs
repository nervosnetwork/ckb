//! The primary module containing the implementations of the transaction pool
//! and its top-level members.

use crate::{component::entry::TxEntry, error::Reject};
use ckb_types::{
    core::Capacity,
    packed::{OutPoint, ProposalShortId},
};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::hash_map::Entry as HashMapEntry;
use std::collections::{BTreeSet, HashMap, HashSet};

/// A struct to use as a sorted key
#[derive(Eq, PartialEq, Clone, Debug)]
pub struct AncestorsScoreSortKey {
    pub fee: Capacity,
    pub weight: u64,
    pub id: ProposalShortId,
    pub ancestors_fee: Capacity,
    pub ancestors_weight: u64,
    pub ancestors_size: usize,
}

impl AncestorsScoreSortKey {
    /// compare tx fee rate with ancestors fee rate and return the min one
    pub(crate) fn min_fee_and_weight(&self) -> (Capacity, u64) {
        // avoid division a_fee/a_weight > b_fee/b_weight
        let tx_weight = u128::from(self.fee.as_u64()) * u128::from(self.ancestors_weight);
        let ancestors_weight = u128::from(self.ancestors_fee.as_u64()) * u128::from(self.weight);

        if tx_weight < ancestors_weight {
            (self.fee, self.weight)
        } else {
            (self.ancestors_fee, self.ancestors_weight)
        }
    }
}

impl PartialOrd for AncestorsScoreSortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AncestorsScoreSortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        // avoid division a_fee/a_weight > b_fee/b_weight
        let (fee, weight) = self.min_fee_and_weight();
        let (other_fee, other_weight) = other.min_fee_and_weight();
        let self_weight = u128::from(fee.as_u64()) * u128::from(other_weight);
        let other_weight = u128::from(other_fee.as_u64()) * u128::from(weight);
        if self_weight == other_weight {
            // if fee rate weight is same, then compare with ancestor weight
            if self.ancestors_weight == other.ancestors_weight {
                self.id.raw_data().cmp(&other.id.raw_data())
            } else {
                self.ancestors_weight.cmp(&other.ancestors_weight)
            }
        } else {
            self_weight.cmp(&other_weight)
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct TxLinks {
    pub parents: HashSet<ProposalShortId>,
    pub children: HashSet<ProposalShortId>,
}

#[derive(Clone, Copy)]
enum Relation {
    Parents,
    Children,
}

impl TxLinks {
    fn get_direct_ids(&self, relation: Relation) -> &HashSet<ProposalShortId> {
        match relation {
            Relation::Parents => &self.parents,
            Relation::Children => &self.children,
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct TxLinksMap {
    pub(crate) inner: HashMap<ProposalShortId, TxLinks>,
}

impl TxLinksMap {
    fn new() -> Self {
        TxLinksMap {
            inner: Default::default(),
        }
    }

    fn calc_relative_ids(
        &self,
        short_id: &ProposalShortId,
        relation: Relation,
    ) -> HashSet<ProposalShortId> {
        let direct = self
            .inner
            .get(short_id)
            .map(|link| link.get_direct_ids(relation))
            .cloned()
            .unwrap_or_default();

        self.calc_relation_ids(Cow::Owned(direct), relation)
    }

    fn calc_relation_ids(
        &self,
        stage: Cow<HashSet<ProposalShortId>>,
        relation: Relation,
    ) -> HashSet<ProposalShortId> {
        let mut stage = stage.into_owned();
        let mut relation_ids = HashSet::with_capacity(stage.len());

        while let Some(id) = stage.iter().next().cloned() {
            relation_ids.insert(id.clone());
            stage.remove(&id);

            //recursively
            for id in self
                .inner
                .get(&id)
                .map(|link| link.get_direct_ids(relation))
                .cloned()
                .unwrap_or_default()
            {
                if !relation_ids.contains(&id) {
                    stage.insert(id);
                }
            }
        }
        relation_ids
    }

    pub fn calc_ancestors(&self, short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.calc_relative_ids(short_id, Relation::Parents)
    }

    pub fn calc_descendants(&self, short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.calc_relative_ids(short_id, Relation::Children)
    }

    pub fn get_children(&self, short_id: &ProposalShortId) -> Option<&HashSet<ProposalShortId>> {
        self.inner.get(short_id).map(|link| &link.children)
    }

    pub fn get_parents(&self, short_id: &ProposalShortId) -> Option<&HashSet<ProposalShortId>> {
        self.inner.get(short_id).map(|link| &link.parents)
    }

    pub fn remove(&mut self, short_id: &ProposalShortId) -> Option<TxLinks> {
        self.inner.remove(short_id)
    }

    fn remove_child(
        &mut self,
        short_id: &ProposalShortId,
        child: &ProposalShortId,
    ) -> Option<bool> {
        self.inner
            .get_mut(short_id)
            .map(|links| links.children.remove(child))
    }

    fn remove_parent(
        &mut self,
        short_id: &ProposalShortId,
        parent: &ProposalShortId,
    ) -> Option<bool> {
        self.inner
            .get_mut(short_id)
            .map(|links| links.parents.remove(parent))
    }

    fn add_child(&mut self, short_id: &ProposalShortId, child: ProposalShortId) -> Option<bool> {
        self.inner
            .get_mut(short_id)
            .map(|links| links.children.insert(child))
    }

    fn add_parent(&mut self, short_id: &ProposalShortId, parent: ProposalShortId) -> Option<bool> {
        self.inner
            .get_mut(short_id)
            .map(|links| links.parents.insert(parent))
    }

    fn clear(&mut self) {
        self.inner.clear();
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SortedTxMap {
    entries: HashMap<ProposalShortId, TxEntry>,
    pub(crate) sorted_index: BTreeSet<AncestorsScoreSortKey>,
    deps: HashMap<OutPoint, HashSet<ProposalShortId>>,
    /// A map track transaction ancestors and descendants
    pub(crate) links: TxLinksMap,
    max_ancestors_count: usize,
}

impl SortedTxMap {
    pub fn new(max_ancestors_count: usize) -> Self {
        SortedTxMap {
            entries: Default::default(),
            sorted_index: Default::default(),
            links: TxLinksMap::new(),
            deps: Default::default(),
            max_ancestors_count,
        }
    }

    pub fn size(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ProposalShortId, &TxEntry)> {
        self.entries.iter()
    }

    // Usually when a new transaction is added to the pool, it has no in-pool
    // children (because any such children would be an orphan).  So in add_entry(), we:
    // - update a new entry's parents set to include all in-pool parents
    // - update the new entry's parents to include the new tx as a child
    // - update all ancestors of the transaction to include the new tx's size/fee
    pub fn add_entry(&mut self, mut entry: TxEntry) -> Result<bool, Reject> {
        let short_id = entry.proposal_short_id();

        if self.contains_key(&short_id) {
            return Ok(false);
        };

        // find in pool parents
        let mut parents: HashSet<ProposalShortId> = HashSet::with_capacity(
            entry.transaction().inputs().len() + entry.transaction().cell_deps().len(),
        );

        for input in entry.transaction().inputs() {
            let input_pt = input.previous_output();
            if let Some(deps) = self.deps.get(&input_pt) {
                parents.extend(deps.iter().cloned());
            }

            let parent_hash = &input_pt.tx_hash();
            let id = ProposalShortId::from_tx_hash(parent_hash);
            if self.links.inner.contains_key(&id) {
                parents.insert(id);
            }
        }
        for cell_dep in entry.transaction().cell_deps() {
            let dep_pt = cell_dep.out_point();
            let id = ProposalShortId::from_tx_hash(&dep_pt.tx_hash());
            if self.links.inner.contains_key(&id) {
                parents.insert(id);
            }
        }

        let ancestors = self
            .links
            .calc_relation_ids(Cow::Borrowed(&parents), Relation::Parents);

        // update parents references
        for ancestor_id in &ancestors {
            let ancestor = self.entries.get(ancestor_id).expect("pool consistent");
            entry.add_entry_weight(ancestor);
        }

        if entry.ancestors_count > self.max_ancestors_count {
            return Err(Reject::ExceededMaximumAncestorsCount);
        }

        for cell_dep in entry.transaction().cell_deps() {
            let dep_pt = cell_dep.out_point();
            // insert dep-ref map
            self.deps
                .entry(dep_pt)
                .or_insert_with(HashSet::new)
                .insert(short_id.clone());
        }

        for parent in &parents {
            self.links.add_child(parent, short_id.clone());
        }

        // insert links
        let links = TxLinks {
            parents,
            children: Default::default(),
        };
        self.links.inner.insert(short_id.clone(), links);
        self.sorted_index.insert(entry.as_sorted_key());
        self.entries.insert(short_id, entry);
        Ok(true)
    }

    // update_descendants_from_detached is used to update
    // the descendants for a single transaction that has been added to the
    // pool but may have child transactions in the pool, eg during a
    // chain reorg.
    pub fn update_descendants_from_detached(
        &mut self,
        id: &ProposalShortId,
        children: HashSet<ProposalShortId>,
    ) {
        if let Some(entry) = self.entries.get(id).cloned() {
            for child in &children {
                self.links.add_parent(child, id.clone());
            }
            if let Some(links) = self.links.inner.get_mut(id) {
                links.children.extend(children);
            }

            let descendants = self.calc_descendants(id);
            for desc_id in &descendants {
                if let Some(desc_entry) = self.entries.get_mut(desc_id) {
                    let deleted = self.sorted_index.remove(&desc_entry.as_sorted_key());
                    debug_assert!(deleted, "pool inconsistent");
                    desc_entry.add_entry_weight(&entry);
                    self.sorted_index.insert(desc_entry.as_sorted_key());
                }
            }
        }
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.entries.contains_key(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.entries.get(id)
    }

    #[cfg(test)]
    pub(crate) fn deps(&self) -> &HashMap<OutPoint, HashSet<ProposalShortId>> {
        &self.deps
    }

    fn update_deps_for_remove(&mut self, entry: &TxEntry) {
        for cell_dep in entry.transaction().cell_deps() {
            let dep_pt = cell_dep.out_point();
            if let HashMapEntry::Occupied(mut o) = self.deps.entry(dep_pt) {
                let set = o.get_mut();
                if set.remove(&entry.proposal_short_id()) && set.is_empty() {
                    o.remove_entry();
                }
            }
        }
    }

    fn update_children_for_remove(&mut self, id: &ProposalShortId) {
        if let Some(children) = self.get_children(id).cloned() {
            for child in children {
                self.links.remove_parent(&child, id);
            }
        }
    }

    fn update_parents_for_remove(&mut self, id: &ProposalShortId) {
        if let Some(parents) = self.get_parents(id).cloned() {
            for parent in parents {
                self.links.remove_child(&parent, id);
            }
        }
    }

    fn remove_unchecked(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        self.entries.remove(id).map(|entry| {
            self.sorted_index.remove(&entry.as_sorted_key());
            self.update_deps_for_remove(&entry);
            entry
        })
    }

    pub fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        let mut removed_ids = vec![id.to_owned()];
        let mut removed = vec![];
        let descendants = self.calc_descendants(id);
        removed_ids.extend(descendants);

        // update links state for remove
        for id in &removed_ids {
            self.update_parents_for_remove(id);
            self.update_children_for_remove(id);
        }

        for id in removed_ids {
            if let Some(entry) = self.remove_unchecked(&id) {
                self.links.remove(&id);
                removed.push(entry);
            }
        }
        removed
    }

    // notice:
    // we are sure that all in-pool ancestor have already been processed.
    // otherwise `links` will differ from the set of parents we'd calculate by searching
    pub fn remove_entry(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        let descendants = self.calc_descendants(id);
        self.remove_unchecked(id).map(|entry| {
            // We're not recursively removing a tx and all its descendants
            // So we need update statistics state
            for desc_id in &descendants {
                if let Some(desc_entry) = self.entries.get_mut(desc_id) {
                    let deleted = self.sorted_index.remove(&desc_entry.as_sorted_key());
                    debug_assert!(deleted, "pool inconsistent");
                    desc_entry.sub_entry_weight(&entry);
                    self.sorted_index.insert(desc_entry.as_sorted_key());
                }
            }
            self.update_parents_for_remove(id);
            self.update_children_for_remove(id);
            self.links.remove(id);
            entry
        })
    }

    /// calculate all ancestors from pool
    pub fn calc_ancestors(&self, short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.links.calc_ancestors(short_id)
    }

    /// calculate all descendants from pool
    pub fn calc_descendants(&self, short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.links.calc_descendants(short_id)
    }

    /// find children from pool
    pub fn get_children(&self, short_id: &ProposalShortId) -> Option<&HashSet<ProposalShortId>> {
        self.links.get_children(short_id)
    }

    /// find parents from pool
    pub fn get_parents(&self, short_id: &ProposalShortId) -> Option<&HashSet<ProposalShortId>> {
        self.links.get_parents(short_id)
    }

    /// sorted by ancestor score from higher to lower
    pub fn score_sorted_iter(&self) -> impl Iterator<Item = &TxEntry> {
        self.sorted_index
            .iter()
            .rev()
            .map(move |key| self.entries.get(&key.id).expect("consistent"))
    }

    pub(crate) fn clear(&mut self) {
        self.sorted_index.clear();
        self.deps.clear();
        self.links.clear();
        self.entries.clear();
    }
}
