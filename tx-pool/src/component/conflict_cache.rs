//! Bounded historical cache for verified transactions rejected because an
//! accepted pool transaction currently consumes one of their inputs.
//!
//! This is not an executable pipeline owner: recovery removes an entry and
//! re-admits its raw transaction into the coordinator. Keeping the cache
//! inside `TxPool` makes input release and candidate discovery one lock-domain
//! transaction.

use crate::constants::SHRINK_THRESHOLD;
use crate::tx_source::TxSource;
use ckb_types::core::TransactionView;
use ckb_types::packed::{OutPoint, ProposalShortId};
use ckb_util::shrink_to_fit;
use std::collections::{HashMap, HashSet, VecDeque};

const MAX_ENTRIES: usize = 10_000;
const MAX_TX_SIZE: usize = 50_000_000;

#[derive(Debug, Clone)]
pub(crate) struct ConflictEntry {
    pub(crate) tx: TransactionView,
    pub(crate) source: TxSource,
    inputs: HashSet<OutPoint>,
    generation: u64,
}

#[derive(Debug, Default)]
pub(crate) struct ConflictCache {
    by_id: HashMap<ProposalShortId, ConflictEntry>,
    by_outpoint: HashMap<OutPoint, HashSet<ProposalShortId>>,
    insertion_order: VecDeque<(u64, ProposalShortId)>,
    total_tx_size: usize,
    next_generation: u64,
}

impl ConflictCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    pub(crate) fn contains(&self, id: &ProposalShortId) -> bool {
        self.by_id.contains_key(id)
    }

    pub(crate) fn insert(
        &mut self,
        tx: TransactionView,
        source: TxSource,
    ) -> (bool, Vec<ConflictEntry>) {
        let id = tx.proposal_short_id();
        if self.by_id.contains_key(&id) {
            return (false, Vec::new());
        }
        let inserted_id = id.clone();
        let generation = self.allocate_generation();
        let inputs: HashSet<_> = tx.input_pts_iter().collect();
        for input in &inputs {
            self.by_outpoint
                .entry(input.clone())
                .or_default()
                .insert(id.clone());
        }
        self.total_tx_size = self
            .total_tx_size
            .saturating_add(tx.data().serialized_size_in_block());
        self.insertion_order.push_back((generation, id.clone()));
        self.by_id.insert(
            id,
            ConflictEntry {
                tx,
                source,
                inputs,
                generation,
            },
        );

        let mut evicted = Vec::new();
        while self.by_id.len() > MAX_ENTRIES || self.total_tx_size > MAX_TX_SIZE {
            let Some((generation, oldest)) = self.insertion_order.pop_front() else {
                break;
            };
            if self
                .by_id
                .get(&oldest)
                .is_some_and(|entry| entry.generation == generation)
                && let Some(entry) = self.remove(&oldest)
            {
                evicted.push(entry);
            }
        }
        (self.by_id.contains_key(&inserted_id), evicted)
    }

    pub(crate) fn remove(&mut self, id: &ProposalShortId) -> Option<ConflictEntry> {
        let entry = self.by_id.remove(id)?;
        self.total_tx_size = self
            .total_tx_size
            .saturating_sub(entry.tx.data().serialized_size_in_block());
        for input in &entry.inputs {
            if let Some(ids) = self.by_outpoint.get_mut(input) {
                ids.remove(id);
                if ids.is_empty() {
                    self.by_outpoint.remove(input);
                }
            }
        }
        self.shrink_to_fit();
        Some(entry)
    }

    pub(crate) fn remove_hash(&mut self, hash: &ckb_types::packed::Byte32) -> bool {
        let id = ProposalShortId::from_tx_hash(hash);
        if !self
            .by_id
            .get(&id)
            .is_some_and(|entry| entry.tx.hash() == *hash)
        {
            return false;
        }
        self.remove(&id).is_some()
    }

    pub(crate) fn recoverable_by_inputs(
        &self,
        inputs: impl Iterator<Item = OutPoint>,
        mut all_inputs_free: impl FnMut(&TransactionView) -> bool,
    ) -> Vec<(TransactionView, TxSource)> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for input in inputs {
            if let Some(ids) = self.by_outpoint.get(&input) {
                for id in ids {
                    if seen.insert(id.clone())
                        && let Some(entry) = self.by_id.get(id)
                        && all_inputs_free(&entry.tx)
                    {
                        result.push((entry.tx.clone(), entry.source));
                    }
                }
            }
        }
        result
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = &ConflictEntry> {
        self.by_id.values()
    }

    pub(crate) fn clear(&mut self) {
        self.by_id.clear();
        self.by_outpoint.clear();
        self.insertion_order.clear();
        self.total_tx_size = 0;
        self.next_generation = 0;
    }

    fn allocate_generation(&mut self) -> u64 {
        if self.next_generation == u64::MAX {
            self.rebase_generations();
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        generation
    }

    fn rebase_generations(&mut self) {
        let mut compact = VecDeque::with_capacity(self.by_id.len());
        let mut generation = 0u64;
        while let Some((old_generation, id)) = self.insertion_order.pop_front() {
            let Some(entry) = self.by_id.get_mut(&id) else {
                continue;
            };
            if entry.generation != old_generation {
                continue;
            }
            entry.generation = generation;
            compact.push_back((generation, id));
            generation += 1;
        }
        self.insertion_order = compact;
        self.next_generation = generation;
    }

    fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.by_id, SHRINK_THRESHOLD);
        shrink_to_fit!(self.by_outpoint, SHRINK_THRESHOLD);
    }
}

#[cfg(test)]
mod tests {
    use super::ConflictCache;
    use crate::tx_source::TxSource;
    use ckb_types::core::{TransactionBuilder, TransactionView};
    use ckb_types::packed::{Byte32, CellInput, OutPoint};

    fn tx(seed: u8) -> TransactionView {
        TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(Byte32::new([seed; 32]), 0), 0))
            .build()
    }

    #[test]
    fn stale_eviction_ticket_cannot_remove_a_readmitted_hash() {
        let mut cache = ConflictCache::new();
        let first = tx(1);
        let id = first.proposal_short_id();
        assert!(cache.insert(first, TxSource::Local).0);
        assert!(cache.remove(&id).is_some());
        assert!(cache.insert(tx(2), TxSource::Local).0);
        assert!(cache.insert(tx(1), TxSource::Local).0);

        let (stale_generation, stale_id) = cache.insertion_order.pop_front().unwrap();
        assert_eq!(stale_id, id);
        assert_ne!(
            cache.by_id.get(&stale_id).unwrap().generation,
            stale_generation
        );
        assert!(cache.by_id.contains_key(&id));
    }

    #[test]
    fn recovery_index_requires_every_input_to_be_free_and_unindexes_remove() {
        let mut cache = ConflictCache::new();
        let candidate = tx(3);
        let id = candidate.proposal_short_id();
        let input = candidate.input_pts_iter().next().unwrap();
        assert!(cache.insert(candidate.clone(), TxSource::Local).0);
        assert!(
            cache
                .recoverable_by_inputs(std::iter::once(input.clone()), |_| false)
                .is_empty()
        );
        assert_eq!(
            cache
                .recoverable_by_inputs(std::iter::once(input.clone()), |_| true)
                .len(),
            1
        );
        assert!(cache.remove(&id).is_some());
        assert!(
            cache
                .recoverable_by_inputs(std::iter::once(input), |_| true)
                .is_empty()
        );
    }
}
