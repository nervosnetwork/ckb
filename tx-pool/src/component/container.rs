//! The primary module containing the implementations of the transaction pool
//! and its top-level members.

use crate::component::entry::TxEntry;
use crate::component::relative::RelativeTraversal;
use ckb_types::{core::Capacity, packed::ProposalShortId};
use std::cmp::Ordering;
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
pub struct TxLink {
    pub parents: HashSet<ProposalShortId>,
    pub children: HashSet<ProposalShortId>,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct SortedTxMap {
    entries: HashMap<ProposalShortId, (TxEntry, TxLink)>,
    sorted_index: BTreeSet<AncestorsScoreSortKey>,
}

impl SortedTxMap {
    pub fn size(&self) -> usize {
        self.entries.len()
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.entries.contains_key(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.entries.get(id).map(|(entry, _)| entry)
    }

    fn get_mut(&mut self, id: &ProposalShortId) -> Option<&mut TxEntry> {
        self.entries.get_mut(id).map(|(entry, _)| entry)
    }

    pub fn get_link(&self, id: &ProposalShortId) -> Option<&TxLink> {
        self.entries.get(id).map(|(_, link)| link)
    }

    fn get_link_mut(&mut self, id: &ProposalShortId) -> Option<&mut TxLink> {
        self.entries.get_mut(id).map(|(_, link)| link)
    }

    fn insert(&mut self, entry: TxEntry, link: TxLink) -> Option<TxEntry> {
        let id = entry.transaction.proposal_short_id();
        self.sorted_index
            .insert(AncestorsScoreSortKey::from(&entry));
        self.entries
            .insert(id, (entry, link))
            .map(|(old_entry, _)| old_entry)
    }

    fn remove(&mut self, id: &ProposalShortId) -> Option<(TxEntry, TxLink)> {
        self.entries.remove(id).map(|(entry, link)| {
            self.sorted_index.remove(&entry.as_sorted_key());
            (entry, link)
        })
    }

    /// update entry ancestor prefix fields
    fn update_ancestors_stat_for_entry(
        &self,
        entry: &mut TxEntry,
        parents: &HashSet<ProposalShortId>,
    ) {
        for id in parents {
            let parent_entry = self.get(&id).expect("pool consistent");
            entry.add_ancestors_weight(parent_entry);
        }
    }

    pub fn add_entry(&mut self, mut entry: TxEntry) -> Option<TxEntry> {
        let short_id = entry.transaction.proposal_short_id();

        // find in pool parents
        let mut parents: HashSet<ProposalShortId> = HashSet::with_capacity(
            entry.transaction.inputs().len() + entry.transaction.cell_deps().len(),
        );
        for input in entry.transaction.inputs() {
            let parent_hash = &input.previous_output().tx_hash();
            let id = ProposalShortId::from_tx_hash(&(parent_hash));
            if self.contains_key(&id) {
                parents.insert(id);
            }
        }
        for cell_dep in entry.transaction.cell_deps() {
            let id = ProposalShortId::from_tx_hash(&(cell_dep.out_point().tx_hash()));
            if self.contains_key(&id) {
                parents.insert(id);
            }
        }
        // update ancestor_fields
        self.update_ancestors_stat_for_entry(&mut entry, &parents);
        // update parents references
        for parent_id in &parents {
            self.get_link_mut(parent_id)
                .expect("exists")
                .children
                .insert(short_id.clone());
        }

        let link = TxLink {
            parents,
            children: Default::default(),
        };
        self.insert(entry, link)
    }

    pub fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        let mut removal = Vec::new();

        if let Some(entry) = self.get(id) {
            removal.push(entry.to_owned());
        } else {
            return removal;
        }
        let descendants = self.get_descendants(id);
        removal.extend(descendants.iter().map(|desc_id| {
            self.get(desc_id)
                .expect("found inconsistency when remove_entry_and_descendants")
                .to_owned()
        }));

        removal.iter().for_each(|e| {
            self.remove_entry(&e.transaction.proposal_short_id());
        });
        removal
    }

    pub fn remove_entry(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        if !self.contains_key(id) {
            return None;
        }

        let descendants = self.get_descendants(id);
        let (entry, link) = self.remove(id).expect("checked above");

        // Update its descendants' entries
        for desc_id in descendants.iter() {
            let (old_sorted_key, new_sorted_key) = {
                let desc_entry = self
                    .get_mut(&desc_id)
                    .expect("found inconsistency when remove_entry");
                let old_sorted_key = desc_entry.as_sorted_key();

                desc_entry.sub_entry_weight(&entry);
                let new_sorted_key = desc_entry.as_sorted_key();
                (old_sorted_key, new_sorted_key)
            };

            self.sorted_index.insert(new_sorted_key);
            self.sorted_index.remove(&old_sorted_key);
        }

        // Remove the links between entry and parents/children
        for p_id in link.parents.iter() {
            self.get_link_mut(p_id)
                .map(|p_link| p_link.children.remove(id));
        }
        for c_id in link.children.iter() {
            self.get_link_mut(c_id)
                .map(|c_link| c_link.parents.remove(id));
        }

        Some(entry)
    }

    /// return sorted keys
    pub fn sorted_keys(&self) -> impl Iterator<Item = &AncestorsScoreSortKey> {
        self.sorted_index.iter().rev()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

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
}
