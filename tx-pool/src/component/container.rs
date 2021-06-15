//! The primary module containing the implementations of the transaction pool
//! and its top-level members.

use crate::{component::entry::TxEntry, error::Reject};
use ckb_types::{
    core::{Capacity, TransactionView},
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
    pub vbytes: u64,
    pub id: ProposalShortId,
    pub ancestors_fee: Capacity,
    pub ancestors_vbytes: u64,
    pub ancestors_size: usize,
}

impl AncestorsScoreSortKey {
    /// compare tx fee rate with ancestors fee rate and return the min one
    fn min_fee_and_vbytes(&self) -> (Capacity, u64) {
        // avoid division a_fee/a_vbytes > b_fee/b_vbytes
        let tx_weight = u128::from(self.fee.as_u64()) * u128::from(self.ancestors_vbytes);
        let ancestors_weight = u128::from(self.ancestors_fee.as_u64()) * u128::from(self.vbytes);

        if tx_weight < ancestors_weight {
            (self.fee, self.vbytes)
        } else {
            (self.ancestors_fee, self.ancestors_vbytes)
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
        // avoid division a_fee/a_vbytes > b_fee/b_vbytes
        let (fee, vbytes) = self.min_fee_and_vbytes();
        let (other_fee, other_vbytes) = other.min_fee_and_vbytes();
        let self_weight = u128::from(fee.as_u64()) * u128::from(other_vbytes);
        let other_weight = u128::from(other_fee.as_u64()) * u128::from(vbytes);
        if self_weight == other_weight {
            // if fee rate weight is same, then compare with ancestor vbytes
            if self.ancestors_vbytes == other.ancestors_vbytes {
                self.id.raw_data().cmp(&other.id.raw_data())
            } else {
                self.ancestors_vbytes.cmp(&other.ancestors_vbytes)
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

fn calc_relation_ids(
    stage: Cow<HashSet<ProposalShortId>>,
    map: &TxLinksMap,
    relation: Relation,
) -> HashSet<ProposalShortId> {
    let mut stage = stage.into_owned();
    let mut relation_ids = HashSet::with_capacity(stage.len());

    while let Some(id) = stage.iter().next().cloned() {
        relation_ids.insert(id.clone());
        stage.remove(&id);

        //recursively
        for id in map
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

        calc_relation_ids(Cow::Owned(direct), &self, relation)
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

    fn clear(&mut self) {
        self.inner.clear();
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SortedTxMap {
    entries: HashMap<ProposalShortId, TxEntry>,
    sorted_index: BTreeSet<AncestorsScoreSortKey>,
    deps: HashMap<OutPoint, HashSet<ProposalShortId>>,
    /// A map track transaction ancestors and descendants
    links: TxLinksMap,
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
            let id = ProposalShortId::from_tx_hash(&parent_hash);
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

            // insert dep-ref map
            self.deps
                .entry(dep_pt)
                .or_insert_with(HashSet::new)
                .insert(short_id.clone());
        }

        let ancestors = calc_relation_ids(Cow::Borrowed(&parents), &self.links, Relation::Parents);

        // update parents references
        for ancestor_id in &ancestors {
            let ancestor = self.entries.get(ancestor_id).expect("pool consistent");
            entry.add_entry_weight(&ancestor);
        }

        if entry.ancestors_count > self.max_ancestors_count {
            return Err(Reject::ExceededMaximumAncestorsCount);
        }

        for parent in &parents {
            self.links.add_child(&parent, short_id.clone());
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

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.entries.contains_key(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.entries.get(id)
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
        self.entries.remove(&id).map(|entry| {
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
                if let Some(desc_entry) = self.entries.get_mut(&desc_id) {
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

    /// Clear and return all transactions.
    pub(crate) fn drain(&mut self) -> Vec<TransactionView> {
        self.sorted_index.clear();
        self.deps.clear();
        self.links.clear();
        self.entries
            .drain()
            .map(|(_, entry)| entry.transaction().to_owned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_types::{
        bytes::Bytes,
        core::TransactionBuilder,
        packed::{CellInput, OutPoint},
        prelude::*,
    };
    use std::mem::size_of;

    const DEFAULT_MAX_ANCESTORS_SIZE: usize = 25;

    #[test]
    fn test_min_fee_and_vbytes() {
        let result = vec![
            (0, 0, 0, 0),
            (1, 0, 1, 0),
            (500, 10, 1000, 30),
            (10, 500, 30, 1000),
            (500, 10, 1000, 20),
            (std::u64::MAX, 0, std::u64::MAX, 0),
            (std::u64::MAX, 100, std::u64::MAX, 2000),
            (std::u64::MAX, std::u64::MAX, std::u64::MAX, std::u64::MAX),
        ]
        .into_iter()
        .map(|(fee, vbytes, ancestors_fee, ancestors_vbytes)| {
            let key = AncestorsScoreSortKey {
                fee: Capacity::shannons(fee),
                vbytes,
                id: ProposalShortId::new([0u8; 10]),
                ancestors_fee: Capacity::shannons(ancestors_fee),
                ancestors_vbytes,
                ancestors_size: 0,
            };
            key.min_fee_and_vbytes()
        })
        .collect::<Vec<_>>();
        assert_eq!(
            result,
            vec![
                (Capacity::shannons(0), 0),
                (Capacity::shannons(1), 0),
                (Capacity::shannons(1000), 30),
                (Capacity::shannons(10), 500),
                (Capacity::shannons(1000), 20),
                (Capacity::shannons(std::u64::MAX), 0),
                (Capacity::shannons(std::u64::MAX), 2000),
                (Capacity::shannons(std::u64::MAX), std::u64::MAX),
            ]
        );
    }

    #[test]
    fn test_ancestors_sorted_key_order() {
        let mut keys = vec![
            (0, 0, 0, 0),
            (1, 0, 1, 0),
            (500, 10, 1000, 30),
            (10, 500, 30, 1000),
            (500, 10, 1000, 30),
            (10, 500, 30, 1000),
            (500, 10, 1000, 20),
            (std::u64::MAX, 0, std::u64::MAX, 0),
            (std::u64::MAX, 100, std::u64::MAX, 2000),
            (std::u64::MAX, std::u64::MAX, std::u64::MAX, std::u64::MAX),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (fee, vbytes, ancestors_fee, ancestors_vbytes))| {
            let mut id = [0u8; 10];
            id[..size_of::<u32>()].copy_from_slice(&(i as u32).to_be_bytes());
            AncestorsScoreSortKey {
                fee: Capacity::shannons(fee),
                vbytes,
                id: ProposalShortId::new(id),
                ancestors_fee: Capacity::shannons(ancestors_fee),
                ancestors_vbytes,
                ancestors_size: 0,
            }
        })
        .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(
            keys.into_iter().map(|k| k.id).collect::<Vec<_>>(),
            [0, 3, 5, 9, 2, 4, 6, 8, 1, 7]
                .iter()
                .map(|&i| {
                    let mut id = [0u8; 10];
                    id[..size_of::<u32>()].copy_from_slice(&(i as u32).to_be_bytes());
                    ProposalShortId::new(id)
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_remove_entry() {
        let mut map = SortedTxMap::new(DEFAULT_MAX_ANCESTORS_SIZE);
        let tx1 = TxEntry::dummy_resolve(
            TransactionBuilder::default().build(),
            100,
            Capacity::shannons(100),
            100,
        );
        let tx2 = TxEntry::dummy_resolve(
            TransactionBuilder::default()
                .input(
                    CellInput::new_builder()
                        .previous_output(
                            OutPoint::new_builder()
                                .tx_hash(tx1.transaction().hash())
                                .index(0u32.pack())
                                .build(),
                        )
                        .build(),
                )
                .witness(Bytes::new().pack())
                .build(),
            200,
            Capacity::shannons(200),
            200,
        );
        let tx3 = TxEntry::dummy_resolve(
            TransactionBuilder::default()
                .input(
                    CellInput::new_builder()
                        .previous_output(
                            OutPoint::new_builder()
                                .tx_hash(tx2.transaction().hash())
                                .index(0u32.pack())
                                .build(),
                        )
                        .build(),
                )
                .witness(Bytes::new().pack())
                .build(),
            200,
            Capacity::shannons(200),
            200,
        );
        let tx1_id = tx1.proposal_short_id();
        let tx2_id = tx2.proposal_short_id();
        let tx3_id = tx3.proposal_short_id();
        map.add_entry(tx1).unwrap();
        map.add_entry(tx2).unwrap();
        map.add_entry(tx3).unwrap();
        let descendants_set = map.calc_descendants(&tx1_id);
        assert!(descendants_set.contains(&tx2_id));
        assert!(descendants_set.contains(&tx3_id));

        let tx3_entry = map.get(&tx3_id);
        assert!(tx3_entry.is_some());
        let tx3_entry = tx3_entry.unwrap();
        assert_eq!(tx3_entry.ancestors_count, 3);

        map.remove_entry(&tx1_id);
        assert!(!map.contains_key(&tx1_id));
        assert!(map.contains_key(&tx2_id));
        assert!(map.contains_key(&tx3_id));

        let tx3_entry = map.get(&tx3_id).unwrap();
        assert_eq!(tx3_entry.ancestors_count, 2);
        assert_eq!(
            map.calc_ancestors(&tx3_id),
            vec![tx2_id].into_iter().collect()
        );
    }

    #[test]
    fn test_remove_entry_and_descendants() {
        let mut map = SortedTxMap::new(DEFAULT_MAX_ANCESTORS_SIZE);
        let tx1 = TxEntry::dummy_resolve(
            TransactionBuilder::default().build(),
            100,
            Capacity::shannons(100),
            100,
        );
        let tx2 = TxEntry::dummy_resolve(
            TransactionBuilder::default()
                .input(
                    CellInput::new_builder()
                        .previous_output(
                            OutPoint::new_builder()
                                .tx_hash(tx1.transaction().hash())
                                .index(0u32.pack())
                                .build(),
                        )
                        .build(),
                )
                .witness(Bytes::new().pack())
                .build(),
            200,
            Capacity::shannons(200),
            200,
        );
        let tx3 = TxEntry::dummy_resolve(
            TransactionBuilder::default()
                .input(
                    CellInput::new_builder()
                        .previous_output(
                            OutPoint::new_builder()
                                .tx_hash(tx2.transaction().hash())
                                .index(0u32.pack())
                                .build(),
                        )
                        .build(),
                )
                .witness(Bytes::new().pack())
                .build(),
            200,
            Capacity::shannons(200),
            200,
        );
        let tx1_id = tx1.proposal_short_id();
        let tx2_id = tx2.proposal_short_id();
        let tx3_id = tx3.proposal_short_id();
        map.add_entry(tx1).unwrap();
        map.add_entry(tx2).unwrap();
        map.add_entry(tx3).unwrap();
        let descendants_set = map.calc_descendants(&tx1_id);
        assert!(descendants_set.contains(&tx2_id));
        assert!(descendants_set.contains(&tx3_id));
        map.remove_entry_and_descendants(&tx2_id);
        assert!(!map.contains_key(&tx2_id));
        assert!(!map.contains_key(&tx3_id));
        let descendants_set = map.calc_descendants(&tx1_id);
        assert!(!descendants_set.contains(&tx2_id));
        assert!(!descendants_set.contains(&tx3_id));
    }
}
