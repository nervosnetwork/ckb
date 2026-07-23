//! Bounded historical cache for verified transactions rejected because an
//! accepted pool transaction currently consumes one of their inputs.
//!
//! This is not an executable pipeline owner. Recovery admits the raw
//! transaction into the coordinator and then removes the cache entry while
//! holding the same `TxPool` write lock. Keeping the cache inside `TxPool`
//! makes input release, candidate discovery, and ownership transfer one
//! lock-domain transaction.

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

#[derive(Debug, Clone)]
pub(crate) struct ConflictRecoveryCandidate {
    pub(crate) tx: TransactionView,
    pub(crate) source: TxSource,
}

#[derive(Debug, Default)]
pub(crate) struct ConflictCache {
    by_id: HashMap<ProposalShortId, ConflictEntry>,
    by_outpoint: HashMap<OutPoint, HashSet<ProposalShortId>>,
    insertion_order: VecDeque<(u64, ProposalShortId)>,
    /// Level-triggered transfer work. The cache remains the sole owner until
    /// a candidate is synchronously admitted to the coordinator; queue
    /// tickets carry the cache generation so a stale ticket can never act on
    /// a removed-and-readmitted transaction with the same full hash.
    recovery_queue: VecDeque<(u64, ckb_types::packed::Byte32)>,
    recovery_scheduled: HashMap<ckb_types::packed::Byte32, u64>,
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

    #[cfg(test)]
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
        self.recovery_scheduled.remove(&entry.tx.hash());
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
        self.compact_tickets_if_needed();
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

    pub(crate) fn schedule_recoverable_by_inputs(
        &mut self,
        inputs: impl Iterator<Item = OutPoint>,
        mut all_inputs_free: impl FnMut(&TransactionView) -> bool,
    ) -> usize {
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        for input in inputs {
            if let Some(ids) = self.by_outpoint.get(&input) {
                for id in ids {
                    if seen.insert(id.clone())
                        && let Some(entry) = self.by_id.get(id)
                        && all_inputs_free(&entry.tx)
                    {
                        candidates.push((entry.tx.hash(), entry.generation));
                    }
                }
            }
        }
        let mut added = 0;
        for (hash, generation) in candidates {
            if !self.recovery_scheduled.contains_key(&hash) {
                self.recovery_scheduled.insert(hash.clone(), generation);
                self.recovery_queue.push_back((generation, hash));
                added += 1;
            }
        }
        added
    }

    pub(crate) fn schedule_hashes(
        &mut self,
        hashes: impl Iterator<Item = ckb_types::packed::Byte32>,
        mut eligible: impl FnMut(&TransactionView) -> bool,
    ) -> usize {
        let mut added = 0;
        for hash in hashes {
            let id = ProposalShortId::from_tx_hash(&hash);
            let Some(entry) = self
                .by_id
                .get(&id)
                .filter(|entry| entry.tx.hash() == hash && eligible(&entry.tx))
            else {
                continue;
            };
            let generation = entry.generation;
            if !self.recovery_scheduled.contains_key(&hash) {
                self.recovery_scheduled.insert(hash.clone(), generation);
                self.recovery_queue.push_back((generation, hash));
                added += 1;
            }
        }
        added
    }

    pub(crate) fn pop_recovery_candidate(&mut self) -> Option<ConflictRecoveryCandidate> {
        while let Some((generation, hash)) = self.recovery_queue.pop_front() {
            if self.recovery_scheduled.get(&hash) != Some(&generation) {
                continue;
            }
            let id = ProposalShortId::from_tx_hash(&hash);
            let Some(entry) = self
                .by_id
                .get(&id)
                .filter(|entry| entry.tx.hash() == hash && entry.generation == generation)
            else {
                self.recovery_scheduled.remove(&hash);
                continue;
            };
            self.recovery_scheduled.remove(&hash);
            return Some(ConflictRecoveryCandidate {
                tx: entry.tx.clone(),
                source: entry.source,
            });
        }
        None
    }

    pub(crate) fn reschedule_recovery(&mut self, hash: &ckb_types::packed::Byte32) -> bool {
        let id = ProposalShortId::from_tx_hash(hash);
        let Some(entry) = self.by_id.get(&id).filter(|entry| entry.tx.hash() == *hash) else {
            return false;
        };
        let generation = entry.generation;
        if self.recovery_scheduled.contains_key(hash) {
            return false;
        }
        self.recovery_scheduled.insert(hash.clone(), generation);
        self.recovery_queue.push_back((generation, hash.clone()));
        true
    }

    pub(crate) fn recovery_len(&self) -> usize {
        self.recovery_scheduled.len()
    }

    /// Cancel executable transfer work without deleting historical conflict
    /// records. Used by the pipeline epoch barrier: old-epoch maintenance must
    /// not resurrect a pre-pool owner after clear.
    pub(crate) fn clear_recovery_schedule(&mut self) {
        self.recovery_queue.clear();
        self.recovery_scheduled.clear();
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = &ConflictEntry> {
        self.by_id.values()
    }

    pub(crate) fn clear(&mut self) {
        self.by_id.clear();
        self.by_outpoint.clear();
        self.insertion_order.clear();
        self.clear_recovery_schedule();
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
        let scheduled_order = self
            .recovery_queue
            .iter()
            .filter_map(|(generation, hash)| {
                (self.recovery_scheduled.get(hash) == Some(generation)).then(|| hash.clone())
            })
            .collect::<Vec<_>>();
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

        self.recovery_queue.clear();
        self.recovery_scheduled.clear();
        for hash in scheduled_order {
            let id = ProposalShortId::from_tx_hash(&hash);
            let Some(entry) = self.by_id.get(&id).filter(|entry| entry.tx.hash() == hash) else {
                continue;
            };
            if self
                .recovery_scheduled
                .insert(hash.clone(), entry.generation)
                .is_none()
            {
                self.recovery_queue.push_back((entry.generation, hash));
            }
        }
    }

    /// Both FIFO lists use lazy stale tickets. Rebuild after enough churn so
    /// repeated remove/reinsert traffic cannot grow metadata without bound.
    fn compact_tickets_if_needed(&mut self) {
        let insertion_bound = self
            .by_id
            .len()
            .saturating_mul(2)
            .saturating_add(SHRINK_THRESHOLD);
        if self.insertion_order.len() > insertion_bound {
            let mut live = self
                .by_id
                .iter()
                .map(|(id, entry)| (entry.generation, id.clone()))
                .collect::<Vec<_>>();
            live.sort_unstable_by_key(|(generation, _)| *generation);
            self.insertion_order = live.into();
        }

        let recovery_bound = self
            .recovery_scheduled
            .len()
            .saturating_mul(2)
            .saturating_add(SHRINK_THRESHOLD);
        if self.recovery_queue.len() > recovery_bound {
            self.recovery_queue
                .retain(|(generation, hash)| self.recovery_scheduled.get(hash) == Some(generation));
        }
    }

    fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.by_id, SHRINK_THRESHOLD);
        shrink_to_fit!(self.by_outpoint, SHRINK_THRESHOLD);
    }

    #[cfg(test)]
    fn audit(&self) -> Result<(), &'static str> {
        if self.by_id.len() > MAX_ENTRIES || self.total_tx_size > MAX_TX_SIZE {
            return Err("conflict cache exceeds its payload budget");
        }
        let actual_size = self.by_id.values().fold(0usize, |total, entry| {
            total.saturating_add(entry.tx.data().serialized_size_in_block())
        });
        if actual_size != self.total_tx_size {
            return Err("conflict cache byte accounting mismatch");
        }
        for (id, entry) in &self.by_id {
            if entry.tx.proposal_short_id() != *id
                || entry.inputs.iter().any(|input| {
                    !self
                        .by_outpoint
                        .get(input)
                        .is_some_and(|ids| ids.contains(id))
                })
            {
                return Err("conflict cache entry/index mismatch");
            }
            if self
                .insertion_order
                .iter()
                .filter(|(generation, queued)| *generation == entry.generation && queued == id)
                .count()
                != 1
            {
                return Err("conflict cache live insertion ticket mismatch");
            }
        }
        for (input, ids) in &self.by_outpoint {
            for id in ids {
                if !self
                    .by_id
                    .get(id)
                    .is_some_and(|entry| entry.inputs.contains(input))
                {
                    return Err("conflict cache reverse input index mismatch");
                }
            }
        }
        for (hash, generation) in &self.recovery_scheduled {
            let id = ProposalShortId::from_tx_hash(hash);
            if !self
                .by_id
                .get(&id)
                .is_some_and(|entry| entry.tx.hash() == *hash && entry.generation == *generation)
                || self
                    .recovery_queue
                    .iter()
                    .filter(|(queued_generation, queued_hash)| {
                        queued_generation == generation && queued_hash == hash
                    })
                    .count()
                    != 1
            {
                return Err("conflict cache live recovery ticket mismatch");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ConflictCache, SHRINK_THRESHOLD};
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
        cache.audit().unwrap();
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
        assert_eq!(
            cache.schedule_recoverable_by_inputs(std::iter::once(input.clone()), |_| true),
            1
        );
        assert_eq!(cache.recovery_len(), 1);
        assert!(cache.remove(&id).is_some());
        assert_eq!(cache.recovery_len(), 0);
        assert!(cache.pop_recovery_candidate().is_none());
        assert!(
            cache
                .recoverable_by_inputs(std::iter::once(input), |_| true)
                .is_empty()
        );
        cache.audit().unwrap();
    }

    #[test]
    fn stale_recovery_ticket_cannot_reorder_a_readmitted_hash() {
        let mut cache = ConflictCache::new();
        let first = tx(4);
        let first_id = first.proposal_short_id();
        let first_input = first.input_pts_iter().next().unwrap();
        assert!(cache.insert(first.clone(), TxSource::Local).0);
        assert_eq!(
            cache.schedule_recoverable_by_inputs(std::iter::once(first_input.clone()), |_| true),
            1
        );
        assert!(cache.remove(&first_id).is_some());

        let second = tx(5);
        let second_input = second.input_pts_iter().next().unwrap();
        assert!(cache.insert(second.clone(), TxSource::Local).0);
        assert_eq!(
            cache.schedule_recoverable_by_inputs(std::iter::once(second_input), |_| true),
            1
        );
        assert!(cache.insert(first.clone(), TxSource::Local).0);
        assert_eq!(
            cache.schedule_recoverable_by_inputs(std::iter::once(first_input), |_| true),
            1
        );

        assert_eq!(
            cache.pop_recovery_candidate().unwrap().tx.hash(),
            second.hash(),
            "the stale first-generation ticket must not jump the readmitted tx ahead"
        );
        assert_eq!(
            cache.pop_recovery_candidate().unwrap().tx.hash(),
            first.hash()
        );
        assert!(cache.pop_recovery_candidate().is_none());
        cache.audit().unwrap();
    }

    #[test]
    fn adversarial_remove_reinsert_churn_keeps_lazy_ticket_storage_bounded() {
        let mut cache = ConflictCache::new();
        let stable = tx(6);
        assert!(cache.insert(stable, TxSource::Local).0);

        let churned = tx(7);
        let churned_id = churned.proposal_short_id();
        let churned_input = churned.input_pts_iter().next().unwrap();
        for _ in 0..SHRINK_THRESHOLD.saturating_mul(4) {
            assert!(cache.insert(churned.clone(), TxSource::Local).0);
            assert_eq!(
                cache
                    .schedule_recoverable_by_inputs(std::iter::once(churned_input.clone()), |_| {
                        true
                    }),
                1
            );
            assert!(cache.remove(&churned_id).is_some());
        }

        let bound = cache
            .by_id
            .len()
            .saturating_mul(2)
            .saturating_add(SHRINK_THRESHOLD);
        assert!(cache.insertion_order.len() <= bound);
        assert!(cache.recovery_queue.len() <= SHRINK_THRESHOLD);
        assert!(cache.recovery_scheduled.is_empty());
        cache.audit().unwrap();
    }

    #[test]
    fn generation_rebase_preserves_scheduled_recovery_order() {
        let mut cache = ConflictCache::new();
        let first = tx(8);
        let second = tx(9);
        let first_input = first.input_pts_iter().next().unwrap();
        let second_input = second.input_pts_iter().next().unwrap();
        assert!(cache.insert(first.clone(), TxSource::Local).0);
        assert!(cache.insert(second.clone(), TxSource::Local).0);
        assert_eq!(
            cache.schedule_recoverable_by_inputs([first_input, second_input].into_iter(), |_| true),
            2
        );

        cache.next_generation = u64::MAX;
        assert!(cache.insert(tx(10), TxSource::Local).0);
        assert_eq!(cache.recovery_len(), 2);
        assert_eq!(
            cache.pop_recovery_candidate().unwrap().tx.hash(),
            first.hash()
        );
        assert_eq!(
            cache.pop_recovery_candidate().unwrap().tx.hash(),
            second.hash()
        );
        cache.audit().unwrap();
    }
}
