//! The primary module containing the implementations of the transaction pool
//! and its top-level members.

use crate::component::entry::TxEntry;
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
}

impl AncestorsScoreSortKey {
    /// compare tx fee rate with ancestors fee rate and return the min one
    fn min_fee_and_vbytes(&self) -> (Capacity, u64) {
        // avoid division a_fee/a_vbytes > b_fee/b_vbytes
        let tx_weight = self.fee.as_u64() * self.ancestors_vbytes;
        let ancestors_weight = self.ancestors_fee.as_u64() * self.vbytes;

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
        let self_weight = fee.as_u64() * other_vbytes;
        let other_weight = other_fee.as_u64() * vbytes;
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

#[derive(Default, Debug, Clone)]
pub(crate) struct SortedTxMap {
    entries: HashMap<ProposalShortId, TxEntry>,
    sorted_index: BTreeSet<AncestorsScoreSortKey>,
    /// A map track transaction ancestors and descendants
    links: HashMap<ProposalShortId, TxLink>,
}

impl SortedTxMap {
    pub fn size(&self) -> usize {
        self.entries.len()
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

    pub fn add_entry(&mut self, mut entry: TxEntry) -> Option<TxEntry> {
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
        self.entries.insert(short_id, entry)
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

    /// return sorted keys
    pub fn sorted_keys(&self) -> impl Iterator<Item = &AncestorsScoreSortKey> {
        self.sorted_index.iter().rev()
    }
}
