//! The primary module containing the implementations of the transaction pool
//! and its top-level members.

use crate::{component::entry::TxEntry, error::Reject};
use ckb_types::{core::Capacity, packed::ProposalShortId};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

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
pub struct TxLink {
    pub parents: HashSet<ProposalShortId>,
    pub children: HashSet<ProposalShortId>,
}

#[derive(Clone, Copy)]
enum Relation {
    Parents,
    Children,
}

impl TxLink {
    fn get_direct_ids(&self, r: Relation) -> &HashSet<ProposalShortId> {
        match r {
            Relation::Parents => &self.parents,
            Relation::Children => &self.children,
        }
    }

    fn get_relative_ids(
        links: &HashMap<ProposalShortId, TxLink>,
        tx_short_id: &ProposalShortId,
        relation: Relation,
    ) -> HashSet<ProposalShortId> {
        let mut family_txs = links
            .get(tx_short_id)
            .map(|link| link.get_direct_ids(relation).clone())
            .unwrap_or_default();
        let mut relative_txs = HashSet::with_capacity(family_txs.len());
        while !family_txs.is_empty() {
            let id = family_txs
                .iter()
                .next()
                .map(ToOwned::to_owned)
                .expect("exists");
            relative_txs.insert(id.clone());
            family_txs.remove(&id);

            // check parents recursively
            for id in links
                .get(&id)
                .map(|link| link.get_direct_ids(relation).clone())
                .unwrap_or_default()
            {
                if !relative_txs.contains(&id) {
                    family_txs.insert(id);
                }
            }
        }
        relative_txs
    }

    pub fn get_ancestors(
        links: &HashMap<ProposalShortId, TxLink>,
        tx_short_id: &ProposalShortId,
    ) -> HashSet<ProposalShortId> {
        TxLink::get_relative_ids(links, tx_short_id, Relation::Parents)
    }

    pub fn get_descendants(
        links: &HashMap<ProposalShortId, TxLink>,
        tx_short_id: &ProposalShortId,
    ) -> HashSet<ProposalShortId> {
        TxLink::get_relative_ids(links, tx_short_id, Relation::Children)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SortedTxMap {
    entries: HashMap<ProposalShortId, TxEntry>,
    sorted_index: BTreeSet<AncestorsScoreSortKey>,
    /// A map track transaction ancestors and descendants
    links: HashMap<ProposalShortId, TxLink>,
    max_ancestors_count: usize,
}

impl SortedTxMap {
    pub fn new(max_ancestors_count: usize) -> Self {
        SortedTxMap {
            entries: Default::default(),
            sorted_index: Default::default(),
            links: Default::default(),
            max_ancestors_count,
        }
    }

    pub fn size(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ProposalShortId, &TxEntry)> {
        self.entries.iter()
    }

    /// update entry ancestor prefix fields
    fn update_ancestors_stat_for_entry(
        &self,
        entry: &mut TxEntry,
        parents: &HashSet<ProposalShortId>,
    ) {
        for id in parents {
            let parent_entry = self.entries.get(&id).expect("pool consistent");
            entry.add_ancestors_weight(&parent_entry);
        }
    }

    pub fn add_entry(&mut self, mut entry: TxEntry) -> Result<Option<TxEntry>, Reject> {
        let short_id = entry.transaction.proposal_short_id();

        // find in pool parents
        let mut parents: HashSet<ProposalShortId> = HashSet::with_capacity(
            entry.transaction.inputs().len() + entry.transaction.cell_deps().len(),
        );
        for input in entry.transaction.inputs() {
            let parent_hash = &input.previous_output().tx_hash();
            let id = ProposalShortId::from_tx_hash(&(parent_hash));
            if self.links.contains_key(&id) {
                parents.insert(id);
            }
        }
        for cell_dep in entry.transaction.cell_deps() {
            let id = ProposalShortId::from_tx_hash(&(cell_dep.out_point().tx_hash()));
            if self.links.contains_key(&id) {
                parents.insert(id);
            }
        }
        // update ancestor_fields
        self.update_ancestors_stat_for_entry(&mut entry, &parents);

        if entry.ancestors_count > self.max_ancestors_count {
            return Err(Reject::ExceededMaximumAncestorsCount);
        }

        // check duplicate tx
        let removed_entry = if self.contains_key(&short_id) {
            self.remove_entry(&short_id)
        } else {
            None
        };

        // update parents references
        for parent_id in &parents {
            self.links
                .get_mut(parent_id)
                .expect("exists")
                .children
                .insert(short_id.clone());
        }
        // insert links
        self.links.insert(
            short_id.clone(),
            TxLink {
                parents,
                children: Default::default(),
            },
        );
        self.sorted_index
            .insert(AncestorsScoreSortKey::from(&entry));
        self.entries.insert(short_id, entry);
        Ok(removed_entry)
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.entries.contains_key(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.entries.get(id)
    }

    pub fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        let mut queue = VecDeque::new();
        let mut removed = Vec::new();
        let tx_link = self.links.get(&id).map(ToOwned::to_owned);
        queue.push_back(id.clone());
        while let Some(id) = queue.pop_front() {
            if let Some(entry) = self.entries.remove(&id) {
                let deleted = self
                    .sorted_index
                    .remove(&AncestorsScoreSortKey::from(&entry));
                debug_assert!(deleted, "pending pool inconsistent");
                if let Some(link) = self.links.remove(&id) {
                    queue.extend(link.children);
                }
                removed.push(entry);
            }
        }
        // update parents links
        if let Some(link) = tx_link {
            for p_id in link.parents {
                self.links
                    .get_mut(&p_id)
                    .map(|link| link.children.remove(&id));
            }
        }
        removed
    }

    pub fn remove_entry(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        self.entries.remove(&id).map(|entry| {
            let deleted = self
                .sorted_index
                .remove(&AncestorsScoreSortKey::from(&entry));
            debug_assert!(deleted, "pending pool inconsistent");
            // update descendants entries
            for desc_id in self.get_descendants(&id) {
                if let Some(key) = self
                    .entries
                    .get(&desc_id)
                    .map(|entry| entry.as_sorted_key())
                {
                    self.sorted_index.remove(&key);
                }
                if let Some(desc_entry) = self.entries.get_mut(&desc_id) {
                    // remove entry
                    desc_entry.sub_entry_weight(&entry);
                }
                if let Some(key) = self
                    .entries
                    .get(&desc_id)
                    .map(|entry| entry.as_sorted_key())
                {
                    self.sorted_index.insert(key);
                }
            }
            // update links
            if let Some(link) = self.links.remove(&id) {
                for p_id in link.parents {
                    self.links
                        .get_mut(&p_id)
                        .map(|link| link.children.remove(&id));
                }
                for c_id in link.children {
                    self.links
                        .get_mut(&c_id)
                        .map(|link| link.parents.remove(&id));
                }
            }
            entry
        })
    }

    /// find all ancestors from pool
    pub fn get_ancestors(&self, tx_short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        TxLink::get_ancestors(&self.links, tx_short_id)
    }

    /// find all descendants from pool
    pub fn get_descendants(&self, tx_short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        TxLink::get_descendants(&self.links, tx_short_id)
    }

    /// return keys sorted by tx fee rate
    pub fn keys_sorted_by_fee(&self) -> impl Iterator<Item = &AncestorsScoreSortKey> {
        self.sorted_index.iter().rev()
    }

    /// return keys sorted by tx fee rate and transaction relation
    pub fn keys_sorted_by_fee_and_relation(&self) -> Vec<&AncestorsScoreSortKey> {
        let mut keys: Vec<_> = self.keys_sorted_by_fee().collect();
        keys.sort_by_key(|k| {
            self.entries
                .get(&k.id)
                .expect("entries should consistent with sorted_index")
                .ancestors_count
        });
        keys
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
    fn test_sorted_tx_map_with_conflict_tx_hash() {
        let mut map = SortedTxMap::new(DEFAULT_MAX_ANCESTORS_SIZE);
        let tx1 = TxEntry::new(
            TransactionBuilder::default().build(),
            100,
            Capacity::shannons(100),
            100,
            Default::default(),
        );
        let tx2 = TxEntry::new(
            TransactionBuilder::default()
                .witness(Bytes::new().pack())
                .build(),
            200,
            Capacity::shannons(200),
            200,
            Default::default(),
        );
        assert_eq!(tx1.transaction.hash(), tx2.transaction.hash());
        assert_ne!(
            tx1.transaction.witness_hash(),
            tx2.transaction.witness_hash()
        );
        let ret = map.add_entry(tx1.clone()).unwrap();
        assert!(ret.is_none());
        // tx2 should replace tx1
        let ret = map.add_entry(tx2.clone()).unwrap().unwrap();
        assert_eq!(
            ret.transaction.witness_hash(),
            tx1.transaction.witness_hash()
        );
        // should return tx2
        let ret = map
            .remove_entry(&tx2.transaction.proposal_short_id())
            .unwrap();
        assert_eq!(
            ret.transaction.witness_hash(),
            tx2.transaction.witness_hash()
        );
        // check consistency
        for key in map.keys_sorted_by_fee() {
            map.get(&key.id).expect("should consistent");
        }
    }

    #[test]
    fn test_remove_entry_and_descendants() {
        let mut map = SortedTxMap::new(DEFAULT_MAX_ANCESTORS_SIZE);
        let tx1 = TxEntry::new(
            TransactionBuilder::default().build(),
            100,
            Capacity::shannons(100),
            100,
            Default::default(),
        );
        let tx2 = TxEntry::new(
            TransactionBuilder::default()
                .input(
                    CellInput::new_builder()
                        .previous_output(
                            OutPoint::new_builder()
                                .tx_hash(tx1.transaction.hash())
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
            Default::default(),
        );
        let tx3 = TxEntry::new(
            TransactionBuilder::default()
                .input(
                    CellInput::new_builder()
                        .previous_output(
                            OutPoint::new_builder()
                                .tx_hash(tx2.transaction.hash())
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
            Default::default(),
        );
        let tx1_id = tx1.transaction.proposal_short_id();
        let tx2_id = tx2.transaction.proposal_short_id();
        let tx3_id = tx3.transaction.proposal_short_id();
        map.add_entry(tx1).unwrap();
        map.add_entry(tx2).unwrap();
        map.add_entry(tx3).unwrap();
        let descendants_map = map.get_descendants(&tx1_id);
        assert!(descendants_map.contains(&tx2_id));
        assert!(descendants_map.contains(&tx3_id));
        map.remove_entry_and_descendants(&tx2_id);
        assert!(!map.contains_key(&tx2_id));
        assert!(!map.contains_key(&tx3_id));
        let descendants_map = map.get_descendants(&tx1_id);
        assert!(!descendants_map.contains(&tx2_id));
        assert!(!descendants_map.contains(&tx3_id));
    }
}
