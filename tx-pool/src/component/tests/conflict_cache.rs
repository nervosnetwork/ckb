use super::{
    ConflictCache, ConflictReleaseEvent, MAX_ENTRIES, OUTPOINT_INDEX_RESIDENT_OVERHEAD,
    SHRINK_THRESHOLD, conflict_entry_resident_charge,
};
use crate::tx_source::TxSource;
use ckb_types::packed::{Byte32, CellInput, OutPoint};
use ckb_types::{
    bytes::Bytes,
    core::{TransactionBuilder, TransactionView},
    packed,
    prelude::*,
};
use std::collections::HashSet;
use std::sync::Arc;

impl ConflictCache {
    pub(crate) fn set_limits_for_test(&mut self, max_entries: usize, max_resident_size: usize) {
        assert!(self.by_hash.len() <= max_entries);
        assert!(self.total_resident_size <= max_resident_size);
        self.max_entries_override = Some(max_entries);
        self.max_resident_size_override = Some(max_resident_size);
    }

    pub(crate) fn contains_hash(&self, hash: &Byte32) -> bool {
        self.by_hash.contains_key(hash)
    }

    pub(crate) fn recoverable_by_inputs(
        &self,
        inputs: impl Iterator<Item = OutPoint>,
        mut ready: impl FnMut(&TransactionView, &[OutPoint]) -> bool,
    ) -> Vec<(TransactionView, TxSource)> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for input in inputs {
            if let Some(ids) = self.by_outpoint.get(&input) {
                for id in ids {
                    if seen.insert(id.clone())
                        && let Some(entry) = self.by_hash.get(id)
                        && ready(&entry.tx, &entry.recovery_outpoints)
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
        mut ready: impl FnMut(&TransactionView, &[OutPoint]) -> bool,
    ) -> usize {
        self.schedule_discovery_by_inputs(inputs, None);
        let mut scheduled = 0;
        while !self.discovery_pending.is_empty() {
            scheduled += self.discover_recoverable(usize::MAX, &mut ready).scheduled;
        }
        scheduled
    }

    pub(crate) fn schedule_hashes(
        &mut self,
        hashes: impl Iterator<Item = Byte32>,
        mut ready: impl FnMut(&TransactionView, &[OutPoint]) -> bool,
    ) -> usize {
        let mut added = 0;
        for hash in hashes {
            let Some(entry) = self
                .by_hash
                .get(&hash)
                .filter(|entry| ready(&entry.tx, &entry.recovery_outpoints))
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

    fn audit(&self) -> Result<(), &'static str> {
        if self.by_hash.len() > self.max_entries()
            || self.total_resident_size > self.max_resident_size()
        {
            return Err("conflict cache exceeds its resident budget");
        }
        let actual_size = self
            .by_hash
            .values()
            .try_fold(0usize, |total, entry| {
                total.checked_add(conflict_entry_resident_charge(
                    &entry.tx,
                    entry.recovery_outpoints.len(),
                ))
            })
            .ok_or("conflict cache byte accounting overflow")?;
        if actual_size != self.total_resident_size
            || self.by_hash.values().any(|entry| {
                entry.resident_charge
                    != conflict_entry_resident_charge(&entry.tx, entry.recovery_outpoints.len())
            })
        {
            return Err("conflict cache byte accounting mismatch");
        }
        for (hash, entry) in &self.by_hash {
            if entry.tx.hash() != *hash
                || entry.recovery_outpoints.iter().any(|out_point| {
                    !self
                        .by_outpoint
                        .get(out_point)
                        .is_some_and(|hashes| hashes.contains(hash))
                })
            {
                return Err("conflict cache entry/index mismatch");
            }
            if self
                .insertion_order
                .iter()
                .filter(|(generation, queued)| *generation == entry.generation && queued == hash)
                .count()
                != 1
            {
                return Err("conflict cache live insertion ticket mismatch");
            }
        }
        for (input, hashes) in &self.by_outpoint {
            for hash in hashes {
                if !self
                    .by_hash
                    .get(hash)
                    .is_some_and(|entry| entry.recovery_outpoints.contains(input))
                {
                    return Err("conflict cache reverse outpoint index mismatch");
                }
            }
        }
        for (hash, generation) in &self.recovery_scheduled {
            let entry_matches = self
                .by_hash
                .get(hash)
                .is_some_and(|entry| entry.generation == *generation);
            let ticket_count = self
                .recovery_queue
                .iter()
                .filter(|(queued_generation, queued_hash)| {
                    queued_generation == generation && queued_hash == hash
                })
                .count();
            if !entry_matches || ticket_count != 1 {
                return Err("conflict cache live recovery ticket mismatch");
            }
        }
        for (input, state) in &self.discovery_pending {
            let has_candidates = self
                .by_outpoint
                .get(input)
                .is_some_and(|ids| !ids.is_empty());
            let ticket_count = self
                .discovery_queue
                .iter()
                .filter(|(generation, queued)| *generation == state.generation && queued == input)
                .count();
            if !has_candidates || ticket_count != 1 {
                return Err("conflict cache live discovery ticket mismatch");
            }
        }
        Ok(())
    }
}

fn tx(seed: u8) -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(Byte32::new([seed; 32]), 0), 0))
        .build()
}

fn indexed_tx(seed: u32) -> TransactionView {
    let mut hash = [0u8; 32];
    hash[..4].copy_from_slice(&seed.to_le_bytes());
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(Byte32::new(hash), 0), 0))
        .build()
}

fn shared_input_tx(shared: &OutPoint, seed: u32) -> TransactionView {
    let mut hash = [0u8; 32];
    hash[..4].copy_from_slice(&seed.to_le_bytes());
    TransactionBuilder::default()
        .input(CellInput::new(shared.clone(), 0))
        .input(CellInput::new(OutPoint::new(Byte32::new(hash), 0), 0))
        .build()
}

fn with_cached_hash(tx: TransactionView, hash: Byte32) -> TransactionView {
    packed::TransactionView::new_builder()
        .data(tx.data())
        .hash(hash)
        .witness_hash(tx.witness_hash())
        .build()
        .unpack()
}

#[test]
fn complete_hash_identity_retains_short_id_collisions() {
    let mut first_hash = [0x11; 32];
    let mut second_hash = first_hash;
    first_hash[31] = 1;
    second_hash[31] = 2;
    let first = with_cached_hash(tx(90), Byte32::new(first_hash));
    let second = with_cached_hash(tx(91), Byte32::new(second_hash));
    assert_eq!(first.proposal_short_id(), second.proposal_short_id());
    assert_ne!(first.hash(), second.hash());

    let mut cache = ConflictCache::new();
    assert!(cache.insert(first.clone(), TxSource::Local).0);
    assert!(cache.insert(second.clone(), TxSource::Local).0);
    assert_eq!(cache.len(), 2);
    assert!(cache.contains_hash(&first.hash()));
    assert!(cache.contains_hash(&second.hash()));
    cache.audit().unwrap();
}

#[test]
fn resident_charge_accounts_for_each_materialized_outpoint_index() {
    let one = tx(1);
    let shared = one.input_pts_iter().next().unwrap();
    let two = shared_input_tx(&shared, 2);
    let payload_delta = two
        .data()
        .serialized_size_in_block()
        .checked_sub(one.data().serialized_size_in_block())
        .unwrap();
    assert_eq!(
        conflict_entry_resident_charge(&two, 2) - conflict_entry_resident_charge(&one, 1),
        payload_delta + OUTPOINT_INDEX_RESIDENT_OVERHEAD
    );
}

#[test]
fn accepted_entry_metadata_extends_duplicate_wake_edges() {
    let mut cache = ConflictCache::new();
    let candidate = tx(3);
    let input = candidate.input_pts_iter().next().unwrap();
    let expanded_dep = OutPoint::new(Byte32::new([0x44; 32]), 7);
    assert!(cache.insert(candidate.clone(), TxSource::Local).0);

    // The raw transaction does not carry expanded dep-group members.
    // Re-recording the same hash as an accepted RBF victim must extend,
    // never replace or ignore, its existing wake metadata.
    assert!(
        !cache
            .insert_with_outpoints_for_release(
                candidate.clone(),
                TxSource::Remote {
                    cycles: 0,
                    peer: 7.into(),
                },
                [input, expanded_dep.clone()],
                None,
            )
            .0
    );
    assert_eq!(
        cache.schedule_discovery_by_inputs(std::iter::once(expanded_dep.clone()), None),
        1
    );
    let progress = cache.discover_recoverable(1, |_, outpoints| outpoints.contains(&expanded_dep));
    assert_eq!(progress.scheduled, 1);
    let recovered = cache.pop_recovery_candidate().unwrap();
    assert_eq!(recovered.tx.hash(), candidate.hash());
    assert_eq!(recovered.source, TxSource::Local);
    cache.audit().unwrap();
}

#[test]
fn trusted_duplicate_promotes_source_and_witness_without_merging_collisions() {
    let mut cache = ConflictCache::new();
    let remote = tx(42)
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"remote").pack()])
        .build();
    let proposal = remote
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"proposal").pack()])
        .build();
    assert_eq!(remote.hash(), proposal.hash());
    assert_ne!(remote.witness_hash(), proposal.witness_hash());
    assert!(
        cache
            .insert(
                remote,
                TxSource::Remote {
                    cycles: 1,
                    peer: 9.into(),
                },
            )
            .0
    );

    // Promotion updates the existing historical owner rather than adding
    // a second record. `inserted == false` still means no new cache slot.
    assert!(!cache.insert(proposal.clone(), TxSource::Proposal).0);
    let entry = cache.entries().next().unwrap();
    assert_eq!(entry.source, TxSource::Proposal);
    assert_eq!(entry.tx.witness_hash(), proposal.witness_hash());

    // Equal trusted strength may refresh a witness-bearing payload; the
    // first proposal must not pin a later authoritative variant forever.
    let refreshed = proposal
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"refreshed-proposal").pack()])
        .build();
    assert!(!cache.insert(refreshed.clone(), TxSource::Proposal).0);
    let entry = cache.entries().next().unwrap();
    assert_eq!(entry.tx.witness_hash(), refreshed.witness_hash());

    // A weaker source cannot downgrade or replace the trusted witness.
    assert!(
        !cache
            .insert(
                proposal
                    .as_advanced_builder()
                    .set_witnesses(vec![Bytes::from_static(b"later-remote").pack()])
                    .build(),
                TxSource::Remote {
                    cycles: 2,
                    peer: 10.into(),
                },
            )
            .0
    );
    let entry = cache.entries().next().unwrap();
    assert_eq!(entry.source, TxSource::Proposal);
    assert_eq!(entry.tx.witness_hash(), refreshed.witness_hash());
    cache.audit().unwrap();
}

#[test]
fn trusted_promotion_reissues_recovery_and_fifo_generation() {
    let mut cache = ConflictCache::new();
    let remote = indexed_tx(0)
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"remote").pack()])
        .build();
    let proposal = remote
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"proposal").pack()])
        .build();
    let promoted_hash = proposal.hash();
    let input = proposal.input_pts_iter().next().unwrap();
    assert!(
        cache
            .insert(
                remote,
                TxSource::Remote {
                    cycles: 1,
                    peer: 9.into(),
                },
            )
            .0
    );
    assert_eq!(
        cache.schedule_recoverable_by_inputs(std::iter::once(input), |_, _| true),
        1
    );
    for seed in 1..MAX_ENTRIES as u32 {
        assert!(
            cache
                .insert(
                    indexed_tx(seed),
                    TxSource::Remote {
                        cycles: 1,
                        peer: 10.into(),
                    },
                )
                .0
        );
    }

    let (_, promotion_evictions) = cache.insert(proposal.clone(), TxSource::Proposal);
    assert!(promotion_evictions.is_empty());
    let recovered = cache.pop_recovery_candidate().unwrap();
    assert_eq!(recovered.tx.witness_hash(), proposal.witness_hash());
    assert_eq!(recovered.source, TxSource::Proposal);

    let (_, evicted) = cache.insert(
        indexed_tx(MAX_ENTRIES as u32),
        TxSource::Remote {
            cycles: 1,
            peer: 10.into(),
        },
    );
    assert_eq!(evicted.len(), 1);
    assert_ne!(evicted[0].tx.hash(), promoted_hash);
    assert!(
        cache
            .entries()
            .any(|entry| entry.tx.hash() == promoted_hash),
        "the refreshed trusted owner must not inherit the attacker's stale FIFO age"
    );
    cache.audit().unwrap();
}

#[test]
fn stale_eviction_ticket_cannot_remove_a_readmitted_hash() {
    let mut cache = ConflictCache::new();
    let first = tx(1);
    let hash = first.hash();
    assert!(cache.insert(first, TxSource::Local).0);
    assert!(cache.remove(&hash).is_some());
    assert!(cache.insert(tx(2), TxSource::Local).0);
    assert!(cache.insert(tx(1), TxSource::Local).0);

    let (stale_generation, stale_hash) = cache.insertion_order.pop_front().unwrap();
    assert_eq!(stale_hash, hash);
    assert_ne!(
        cache.by_hash.get(&stale_hash).unwrap().generation,
        stale_generation
    );
    assert!(cache.by_hash.contains_key(&hash));
    cache.audit().unwrap();
}

#[test]
fn recovery_index_requires_every_input_to_be_free_and_unindexes_remove() {
    let mut cache = ConflictCache::new();
    let candidate = tx(3);
    let hash = candidate.hash();
    let input = candidate.input_pts_iter().next().unwrap();
    assert!(cache.insert(candidate.clone(), TxSource::Local).0);
    assert!(
        cache
            .recoverable_by_inputs(std::iter::once(input.clone()), |_, _| false)
            .is_empty()
    );
    assert_eq!(
        cache
            .recoverable_by_inputs(std::iter::once(input.clone()), |_, _| true)
            .len(),
        1
    );
    assert_eq!(
        cache.schedule_recoverable_by_inputs(std::iter::once(input.clone()), |_, _| true),
        1
    );
    assert_eq!(cache.recovery_len(), 1);
    assert!(cache.remove(&hash).is_some());
    assert_eq!(cache.recovery_len(), 0);
    assert!(cache.pop_recovery_candidate().is_none());
    assert!(
        cache
            .recoverable_by_inputs(std::iter::once(input), |_, _| true)
            .is_empty()
    );
    cache.audit().unwrap();
}

#[test]
fn stale_recovery_ticket_cannot_reorder_a_readmitted_hash() {
    let mut cache = ConflictCache::new();
    let first = tx(4);
    let first_hash = first.hash();
    let first_input = first.input_pts_iter().next().unwrap();
    assert!(cache.insert(first.clone(), TxSource::Local).0);
    assert_eq!(
        cache.schedule_recoverable_by_inputs(std::iter::once(first_input.clone()), |_, _| true,),
        1
    );
    assert!(cache.remove(&first_hash).is_some());

    let second = tx(5);
    let second_input = second.input_pts_iter().next().unwrap();
    assert!(cache.insert(second.clone(), TxSource::Local).0);
    assert_eq!(
        cache.schedule_recoverable_by_inputs(std::iter::once(second_input), |_, _| true),
        1
    );
    assert!(cache.insert(first.clone(), TxSource::Local).0);
    assert_eq!(
        cache.schedule_recoverable_by_inputs(std::iter::once(first_input), |_, _| true),
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
    let churned_hash = churned.hash();
    let churned_input = churned.input_pts_iter().next().unwrap();
    for _ in 0..SHRINK_THRESHOLD.saturating_mul(4) {
        assert!(cache.insert(churned.clone(), TxSource::Local).0);
        assert_eq!(
            cache.schedule_recoverable_by_inputs(std::iter::once(churned_input.clone()), |_, _| {
                true
            },),
            1
        );
        assert!(cache.remove(&churned_hash).is_some());
    }

    let bound = cache
        .by_hash
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
        cache.schedule_recoverable_by_inputs([first_input, second_input].into_iter(), |_, _| {
            true
        },),
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

#[test]
fn freed_input_discovery_is_probe_bounded_and_level_triggered() {
    let mut cache = ConflictCache::new();
    let shared = OutPoint::new(Byte32::new([0xf0; 32]), 0);
    let mut expected = HashSet::new();
    for seed in 0..10 {
        let candidate = shared_input_tx(&shared, seed);
        expected.insert(candidate.hash());
        assert!(cache.insert(candidate, TxSource::Local).0);
    }

    assert_eq!(
        cache.schedule_discovery_by_inputs(std::iter::once(shared), None),
        1
    );
    let first = cache.discover_recoverable(3, |_, _| true);
    assert_eq!(first.examined, 3);
    assert_eq!(first.scheduled, 3);
    assert!(first.pending);
    assert_eq!(cache.recovery_len(), 3);

    let second = cache.discover_recoverable(2, |_, _| true);
    assert_eq!(second.examined, 2);
    assert_eq!(second.scheduled, 2);
    assert!(second.pending);
    assert_eq!(cache.recovery_len(), 5);

    let final_slice = cache.discover_recoverable(100, |_, _| true);
    assert_eq!(final_slice.examined, 5);
    assert_eq!(final_slice.scheduled, 5);
    assert!(!final_slice.pending);

    let mut recovered = HashSet::new();
    while let Some(candidate) = cache.pop_recovery_candidate() {
        recovered.insert(candidate.tx.hash());
    }
    assert_eq!(recovered, expected);
    cache.audit().unwrap();
}

#[test]
fn release_identity_excludes_only_records_from_the_same_pool_mutation() {
    let mut cache = ConflictCache::new();
    let shared = OutPoint::new(Byte32::new([0xf2; 32]), 0);
    let same_release = shared_input_tx(&shared, 1);
    let independent = shared_input_tx(&shared, 2);
    let release = ConflictReleaseEvent::new();
    assert!(
        cache
            .insert_for_release(
                same_release.clone(),
                TxSource::Local,
                Some(Arc::clone(&release)),
            )
            .0
    );
    assert!(cache.insert(independent.clone(), TxSource::Local).0);

    assert_eq!(
        cache.schedule_discovery_by_inputs(std::iter::once(shared.clone()), Some(release),),
        1
    );
    let progress = cache.discover_recoverable(usize::MAX, |_, _| true);
    assert_eq!(progress.scheduled, 1);
    assert_eq!(
        cache.pop_recovery_candidate().unwrap().tx.hash(),
        independent.hash()
    );
    assert!(cache.pop_recovery_candidate().is_none());

    cache.remove(&independent.hash());
    assert_eq!(
        cache.schedule_discovery_by_inputs(std::iter::once(shared), None),
        1
    );
    cache.discover_recoverable(usize::MAX, |_, _| true);
    assert_eq!(
        cache.pop_recovery_candidate().unwrap().tx.hash(),
        same_release.hash()
    );
    cache.audit().unwrap();
}

#[test]
fn repeated_hot_input_release_does_not_restart_discovery_cursor() {
    let mut cache = ConflictCache::new();
    let shared = OutPoint::new(Byte32::new([0xf1; 32]), 0);
    let mut expected = HashSet::new();
    for seed in 0..32 {
        let candidate = shared_input_tx(&shared, seed);
        expected.insert(candidate.hash());
        assert!(cache.insert(candidate, TxSource::Local).0);
    }

    assert_eq!(
        cache.schedule_discovery_by_inputs(std::iter::once(shared.clone()), None),
        1
    );
    for _ in 0..32 {
        let progress = cache.discover_recoverable(1, |_, _| true);
        assert_eq!(progress.examined, 1);
        // Simulate an accepted blocker repeatedly occupying and freeing
        // the same hot input before the current pass has completed.
        assert_eq!(
            cache.schedule_discovery_by_inputs(std::iter::once(shared.clone()), None),
            0,
            "an already-live outpoint owns exactly one discovery ticket"
        );
    }

    assert_eq!(
        cache.recovery_len(),
        expected.len(),
        "repeated release must not starve ids after the cursor head"
    );
    let mut recovered = HashSet::new();
    while let Some(candidate) = cache.pop_recovery_candidate() {
        recovered.insert(candidate.tx.hash());
    }
    assert_eq!(recovered, expected);
    cache.audit().unwrap();
}

#[test]
fn cancelled_discovery_churn_keeps_lazy_ticket_storage_bounded() {
    let mut cache = ConflictCache::new();
    let candidate = tx(77);
    let hash = candidate.hash();
    let input = candidate.input_pts_iter().next().unwrap();
    for _ in 0..SHRINK_THRESHOLD.saturating_mul(4) {
        assert!(cache.insert(candidate.clone(), TxSource::Local).0);
        assert_eq!(
            cache.schedule_discovery_by_inputs(std::iter::once(input.clone()), None),
            1
        );
        assert!(cache.remove(&hash).is_some());
    }
    assert!(cache.discovery_pending.is_empty());
    assert!(cache.discovery_queue.len() <= SHRINK_THRESHOLD);
    cache.audit().unwrap();
}
