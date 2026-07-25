use crate::component::{
    TxEntry,
    pool_map::{ConflictClosure, PoolMap, RemovalCause, Status},
};
use crate::error::Reject;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder},
    packed::{Byte32, CellInput, CellOutput, OutPoint},
    prelude::*,
};
use std::collections::HashSet;

fn entry(seed: u64, parent: Option<&TxEntry>, fee: u64, size: usize) -> TxEntry {
    let input = parent.map_or_else(
        || OutPoint::new(Byte32::new([(seed as u8).wrapping_add(1); 32]), 0),
        |parent| OutPoint::new(parent.transaction().hash(), 0),
    );
    let tx = TransactionBuilder::default()
        .input(CellInput::new(input, 0))
        .output(CellOutput::new_builder().build())
        .output_data(Bytes::new().pack())
        .build();
    TxEntry::dummy_resolve(tx, seed.saturating_mul(17), Capacity::shannons(fee), size)
}

fn copy_pool(pool: &PoolMap) -> PoolMap {
    let mut entries = pool
        .iter()
        .map(|entry| {
            let raw = TxEntry::new_with_timestamp(
                entry.inner.rtx.clone(),
                entry.inner.cycles,
                entry.inner.fee,
                entry.inner.size,
                entry.inner.timestamp,
            );
            (entry.inner.ancestors_count, raw, entry.status)
        })
        .collect::<Vec<_>>();
    entries.sort_unstable_by_key(|(ancestors, _, _)| *ancestors);

    let mut copy = PoolMap::new(pool.max_ancestors_count);
    for (_, entry, status) in entries {
        copy.add_entry(entry, status).unwrap();
    }
    copy
}

fn state(pool: &PoolMap) -> Vec<(Byte32, Status, TxEntry)> {
    let mut state = pool
        .iter()
        .map(|entry| (entry.hash.clone(), entry.status, entry.inner.clone()))
        .collect::<Vec<_>>();
    state.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    state
}

fn mutable_reference(
    pool: &PoolMap,
    candidate: TxEntry,
    status: Status,
    mandatory: &[ckb_types::packed::ProposalShortId],
    size_limit: usize,
    resident_limit: usize,
) -> Result<PoolMap, Reject> {
    let mut pool = copy_pool(pool);
    for id in mandatory {
        pool.remove_entry_with_status(id)
            .expect("reference mandatory victim exists");
    }
    let candidate_id = candidate.proposal_short_id();
    let inserted = pool.add_entry(candidate.clone(), status)?;
    assert!(inserted);
    while pool.stats.total_tx_size > size_limit
        || pool.stats.total_tx_resident_size > resident_limit
    {
        let root = pool
            .next_evict_entry(Status::Pending)
            .or_else(|| pool.next_evict_entry(Status::Gap))
            .or_else(|| pool.next_evict_entry(Status::Proposed))
            .expect("over-budget reference has an eviction candidate");
        let removed = pool.remove_entry_and_descendants_with_status(&root);
        if removed
            .iter()
            .any(|removed| removed.entry.proposal_short_id() == candidate_id)
        {
            return Err(Reject::Full(format!(
                "the fee_rate for this transaction is: {}",
                candidate.fee_rate()
            )));
        }
    }
    Ok(pool)
}

/// The sparse immutable planner must reproduce the old stepwise CPFP/status
/// policy, including re-ranking after each descendant closure is removed.
#[test]
fn sparse_plan_matches_stepwise_reference_across_small_graphs() {
    for seed in 0u64..96 {
        let mut base = PoolMap::new(16);
        let mut entries = Vec::new();
        for chain in 0..4u64 {
            let mut parent = None;
            for depth in 0..=seed.wrapping_add(chain) % 3 {
                let value = seed
                    .wrapping_mul(97)
                    .wrapping_add(chain * 19)
                    .wrapping_add(depth * 7);
                let next = entry(
                    value,
                    parent.as_ref(),
                    1 + value % 10_000,
                    40 + (value as usize % 120),
                );
                let status = match value % 3 {
                    0 => Status::Pending,
                    1 => Status::Gap,
                    _ => Status::Proposed,
                };
                base.add_entry(next.clone(), status).unwrap();
                parent = Some(next.clone());
                entries.push(next);
            }
        }
        let candidate = entry(
            10_000 + seed,
            None,
            1 + seed.wrapping_mul(1_001) % 20_000,
            60 + (seed as usize % 180),
        );
        let status = match seed % 3 {
            0 => Status::Pending,
            1 => Status::Gap,
            _ => Status::Proposed,
        };
        let mandatory = if seed % 5 == 0 {
            let root = entries[(seed as usize) % entries.len()].proposal_short_id();
            match base.conflict_closure(&HashSet::from([root]), 100) {
                ConflictClosure::Complete { removal, .. } => removal,
                ConflictClosure::Exceeded { .. } => unreachable!(),
            }
        } else {
            Vec::new()
        };
        let combined_size = base.stats.total_tx_size.saturating_add(candidate.size);
        let combined_resident = base
            .stats
            .total_tx_resident_size
            .saturating_add(candidate.resident_size());
        let size_limit = combined_size.saturating_sub(25 + seed as usize % 280);
        let resident_limit = if seed % 2 == 0 {
            usize::MAX
        } else {
            combined_resident.saturating_sub(25 + seed as usize % 500)
        };

        let before = state(&base);
        let reference = mutable_reference(
            &base,
            candidate.clone(),
            status,
            &mandatory,
            size_limit,
            resident_limit,
        );
        let planned = base.plan_mutation(candidate, status, &mandatory, size_limit, resident_limit);
        match (reference, planned) {
            (Ok(reference), Ok(plan)) => {
                let causes = plan
                    .removals
                    .iter()
                    .map(|removal| removal.cause)
                    .collect::<Vec<_>>();
                assert_eq!(
                    causes
                        .iter()
                        .filter(|cause| **cause == RemovalCause::Replacement)
                        .count(),
                    mandatory.len()
                );
                let mut actual = copy_pool(&base);
                actual.apply_mutation(plan);
                assert_eq!(state(&actual), state(&reference), "seed {seed}");
                actual.audit().unwrap();
            }
            (Err(_), Err(_)) => {
                assert_eq!(state(&base), before, "rejected seed {seed} mutated base");
                base.audit().unwrap();
            }
            (reference, planned) => panic!(
                "planner/reference divergence at seed {seed}: reference={:?}, planned={:?}",
                reference.as_ref().err(),
                planned.as_ref().err()
            ),
        }
    }
}
