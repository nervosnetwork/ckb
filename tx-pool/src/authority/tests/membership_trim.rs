//! Independent full-graph eviction oracle cross-checking `eviction::select_victims`
//! and `AuthorityState::eviction_bounds`.

use super::*;
use crate::authority::{
    eviction::{self, EvictionRank, own_fee_rate},
    model::{Phase, Status, hash_key},
    tests::common::{self, accept, delete, replace, spend, store},
};
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::Capacity,
    packed::{Byte32, OutPoint},
    prelude::Pack,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};

type Members = BTreeMap<Byte32, Arc<Entry>>;

fn to_byte32_set(set: &HashSet<[u8; 32]>) -> BTreeSet<Byte32> {
    set.iter().map(Byte32::from).collect()
}

fn to_key_set(set: &BTreeSet<Byte32>) -> HashSet<[u8; 32]> {
    set.iter().map(hash_key).collect()
}

fn ranked_member(store: &Store, nonce: u32, fee: u64, parents: &[Byte32]) -> Arc<Entry> {
    let deps: Vec<_> = parents
        .iter()
        .map(|hash| OutPoint::new(hash.clone(), 0))
        .collect();
    let hash = accept(store, spend(nonce, &[], &deps), fee, 1, Status::Pending);
    let owner = store.point(&hash).1.unwrap();
    let mut value = owner.accepted().unwrap().clone();
    // Normalize virtual weight to isolate rank changes from encoding length.
    value.size = 100;
    replace(store, owner, Phase::Accepted(value))
}

/// Independent brute-force full-graph eviction oracle that ignores `eviction_bounds`
/// completely and recomputes every rank from scratch each round.
fn reference_trim(
    virtual_owners: &Members,
    snapshot: &Snapshot,
    candidate: &Entry,
    candidate_ancestors: &BTreeSet<Byte32>,
    late_descendants: &BTreeSet<Byte32>,
    initial_removed: &BTreeSet<Byte32>,
    mut excess: Amount,
) -> Result<BTreeSet<Byte32>, Error> {
    let mut removed = initial_removed.clone();
    let mut remaining: Members = virtual_owners
        .iter()
        .filter(|(hash, _)| !removed.contains(*hash))
        .map(|(hash, owner)| (hash.clone(), Arc::clone(owner)))
        .collect();
    remaining.insert(candidate.hash(), Arc::new(candidate.clone()));
    let candidate_rate = own_fee_rate(candidate.accepted().unwrap());

    while excess != Amount::default() {
        let mut children: BTreeMap<Byte32, Vec<Byte32>> = BTreeMap::new();
        for (hash, entry) in &remaining {
            for parent in &entry.accepted().unwrap().parents {
                if remaining.contains_key(parent) {
                    children
                        .entry(parent.clone())
                        .or_default()
                        .push(hash.clone());
                }
            }
        }
        let mut best_rank: Option<EvictionRank> = None;
        let mut best_family: Option<Vec<Byte32>> = None;
        for (hash, entry) in &remaining {
            let mut family = Vec::new();
            let mut seen = BTreeSet::new();
            let mut stack = vec![hash.clone()];
            while let Some(cur) = stack.pop() {
                if seen.insert(cur.clone()) {
                    family.push(cur.clone());
                    if let Some(kids) = children.get(&cur) {
                        stack.extend(kids.iter().cloned());
                    }
                }
            }
            let protected = candidate_ancestors.contains(hash);
            if !protected {
                let touched: BTreeSet<_> = removed
                    .iter()
                    .chain(late_descendants.iter())
                    .chain(family.iter())
                    .cloned()
                    .collect();
                if touched.len() > MAX_POOL_MUTATION_CANDIDATES {
                    continue;
                }
            }
            let total = family.iter().fold(Aggregate::default(), |acc, member| {
                acc.add(Aggregate::one(remaining[member].accepted().unwrap()))
                    .unwrap()
            });
            let rank = EvictionRank::for_entry(entry, total, snapshot)?;
            if best_rank.is_none_or(|best| rank < best) {
                best_rank = Some(rank);
                best_family = if protected { None } else { Some(family) };
            }
        }
        let Some(family) = best_family else {
            return Err(Reject::Full(format!(
                "the fee_rate for this transaction is: {candidate_rate}"
            ))
            .into());
        };
        for hash in family {
            let entry = remaining.remove(&hash).unwrap();
            excess = excess.positive_difference(entry.charge());
            removed.insert(hash);
        }
    }
    Ok(removed)
}

fn candidate_ancestors_in(store: &Store, candidate: &Entry) -> BTreeSet<Byte32> {
    let state = store.state_read();
    let mut seen = BTreeSet::from([candidate.hash()]);
    let mut stack: Vec<_> = candidate
        .accepted()
        .unwrap()
        .parents
        .iter()
        .cloned()
        .collect();
    while let Some(cur) = stack.pop() {
        if seen.insert(cur.clone())
            && let Some(owner) = state.owner(&hash_key(&cur))
            && let Some(acc) = owner.accepted()
        {
            stack.extend(acc.parents.iter().cloned());
        }
    }
    seen
}

#[test]
fn select_victims_evicts_shared_family_and_skips_proposed_root() {
    let pool = store();
    let root = accept(&pool, common::output_tx(10630), 0, 1, Status::Pending);
    let point = OutPoint::new(root.clone(), 0);
    let left = accept(
        &pool,
        spend(10631, &[], std::slice::from_ref(&point)),
        100,
        1,
        Status::Pending,
    );
    let right = accept(&pool, spend(10632, &[], &[point]), 100, 1, Status::Pending);
    let join = accept(
        &pool,
        spend(
            10633,
            &[],
            &[
                OutPoint::new(left.clone(), 0),
                OutPoint::new(right.clone(), 0),
            ],
        ),
        100,
        1,
        Status::Pending,
    );
    let _independent = accept(&pool, common::tx(10634), 10_000, 1, Status::Pending);
    let candidate = ranked_member(&store(), 10635, 100_000, &[]);
    let candidate_ancestors = HashSet::from([candidate.hash_bytes()]);
    let snapshot = pool.snapshot().1;

    let empty_late = HashMap::new();
    let mut removed = HashSet::new();
    eviction::select_victims(
        &pool.state_read(),
        &candidate,
        &candidate_ancestors,
        &empty_late,
        &HashSet::new(),
        &mut removed,
        Amount {
            items: 4,
            ..Amount::default()
        },
    )
    .unwrap();
    assert_eq!(
        to_byte32_set(&removed),
        BTreeSet::from([root.clone(), left, right, join])
    );

    // Promote root to Proposed: eviction must skip root and pick its pending
    // descendants plus the higher-fee pending singleton instead.
    let old = pool.point(&root).1.unwrap();
    let mut accepted = old.accepted().unwrap().clone();
    accepted.forced_status = Some(Status::Proposed);
    replace(&pool, old, Phase::Accepted(accepted));

    let mut removed = HashSet::new();
    eviction::select_victims(
        &pool.state_read(),
        &candidate,
        &candidate_ancestors,
        &empty_late,
        &HashSet::new(),
        &mut removed,
        Amount {
            items: 4,
            ..Amount::default()
        },
    )
    .unwrap();
    let virtual_owners: Members = pool
        .capture_accepted()
        .owners
        .into_iter()
        .map(|owner| (owner.hash(), owner))
        .collect();
    let expected = reference_trim(
        &virtual_owners,
        &snapshot,
        &candidate,
        &BTreeSet::from([candidate.hash()]),
        &BTreeSet::new(),
        &BTreeSet::new(),
        Amount {
            items: 4,
            ..Amount::default()
        },
    )
    .unwrap();
    assert_eq!(to_byte32_set(&removed), expected);
    assert_eq!(removed.len(), 4);
    assert!(!removed.contains(&hash_key(&root)));
}

#[test]
fn select_victims_matches_reference_oracle_for_leaf_with_shared_ancestor() {
    let pool = store();
    let root = accept(&pool, common::output_tx(10640), 0, 1, Status::Pending);
    let root_output = OutPoint::new(root.clone(), 0);
    let mut parents = Vec::new();
    for nonce in [10641, 10642] {
        let hash = accept(
            &pool,
            spend(nonce, &[], std::slice::from_ref(&root_output)),
            100,
            1,
            Status::Pending,
        );
        parents.push(OutPoint::new(hash, 0));
    }
    let first = accept(&pool, common::tx(10643), 1000, 1, Status::Pending);
    accept(&pool, common::tx(10644), 2000, 1, Status::Pending);
    let sibling = accept(
        &pool,
        spend(10646, &[], &parents),
        100_000,
        1,
        Status::Pending,
    );
    let raw = common::entry(&pool, spend(10645, &[], &parents), Source::Local);
    let incumbents: Members = pool
        .capture_accepted()
        .owners
        .into_iter()
        .map(|owner| (owner.hash(), owner))
        .collect();
    let snapshot = pool.snapshot().1;
    let mut decisions = (0, 0);
    for fee in (0..=100_000).step_by(2500) {
        let verified = common::verification_fixture(&pool, &raw, fee, 1, Status::Pending);
        let value = Accepted {
            transaction: Arc::clone(&verified.resolved().transaction),
            scripts: verified.scripts(),
            fee: verified.resolved().fee,
            size: verified.serialized_size(),
            timestamp: verified.timestamp(),
            parents: parents.iter().map(|p| p.tx_hash()).collect(),
            context_sensitive: false,
            forced_status: Some(Status::Pending),
        };
        let candidate = raw.with_phase(Phase::Accepted(value));
        let ancestors = candidate_ancestors_in(&pool, &candidate);
        assert_eq!(ancestors.len(), 4);
        assert!(ancestors.contains(&root));
        let ancestors_keys = to_key_set(&ancestors);

        for replaced in [
            BTreeSet::new(),
            BTreeSet::from([first.clone()]),
            BTreeSet::from([sibling.clone()]),
        ] {
            for count in [1, 2] {
                let excess = Amount {
                    items: count,
                    ..Amount::default()
                };
                let empty_late = HashMap::new();
                let mut actual_removed = to_key_set(&replaced);
                let actual = eviction::select_victims(
                    &pool.state_read(),
                    &candidate,
                    &ancestors_keys,
                    &empty_late,
                    &HashSet::new(),
                    &mut actual_removed,
                    excess,
                );
                let expected = reference_trim(
                    &incumbents,
                    &snapshot,
                    &candidate,
                    &ancestors,
                    &BTreeSet::new(),
                    &replaced,
                    excess,
                );
                match (actual, expected) {
                    (Ok(()), Ok(expected_victims)) => {
                        let actual_set = to_byte32_set(&actual_removed);
                        assert_eq!(actual_set, expected_victims, "fee={fee} count={count}");
                        assert!(actual_set.is_disjoint(&ancestors));
                        decisions.0 += 1;
                    }
                    (
                        Err(Error::Rejected(Reject::Full(a))),
                        Err(Error::Rejected(Reject::Full(b))),
                    ) => {
                        assert_eq!(a, b);
                        decisions.1 += 1;
                    }
                    (actual, expected) => {
                        panic!("fee={fee} count={count}: {actual:?}, {expected:?}")
                    }
                }
            }
        }
    }
    assert!(decisions.0 > 0 && decisions.1 > 0, "{decisions:?}");
}

#[test]
fn replacement_victims_share_the_capacity_mutation_limit() {
    let pool = store();
    let mut replaced = BTreeSet::new();
    for nonce in 0..MAX_POOL_MUTATION_CANDIDATES - 1 {
        replaced.insert(accept(
            &pool,
            common::tx(11_000 + u32::try_from(nonce).unwrap()),
            10_000,
            1,
            Status::Pending,
        ));
    }
    for (nonce, fee) in [(11_200, 1), (11_201, 2)] {
        accept(&pool, common::tx(nonce), fee, 1, Status::Pending);
    }
    let candidate = ranked_member(&store(), 11_202, 100_000, &[]);
    let empty_late = HashMap::new();
    let mut removed = to_key_set(&replaced);
    let result = eviction::select_victims(
        &pool.state_read(),
        &candidate,
        &HashSet::from([candidate.hash_bytes()]),
        &empty_late,
        &HashSet::new(),
        &mut removed,
        Amount {
            items: 2,
            ..Amount::default()
        },
    );
    assert!(matches!(result, Err(Error::Rejected(Reject::Full(_)))));
}

#[test]
fn eviction_bounds_follow_owner_edits_in_authority_state() {
    let pool = store();
    let low = ranked_member(&pool, 12_100, 10, &[]);
    let high = ranked_member(&pool, 12_101, 100, &[]);
    assert_eq!(
        pool.state_read()
            .eviction_bounds
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![
            EvictionRank::lower_bound(&low).unwrap(),
            EvictionRank::lower_bound(&high).unwrap(),
        ]
    );
    let mut repriced = low.accepted().unwrap().clone();
    repriced.fee = Capacity::shannons(200);
    let repriced = replace(&pool, Arc::clone(&low), Phase::Accepted(repriced));
    assert_eq!(
        pool.state_read()
            .eviction_bounds
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![
            EvictionRank::lower_bound(&high).unwrap(),
            EvictionRank::lower_bound(&repriced).unwrap(),
        ]
    );
    delete(&pool, high).unwrap();
    assert_eq!(
        pool.state_read()
            .eviction_bounds
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![EvictionRank::lower_bound(&repriced).unwrap()]
    );
}

fn rank_fixture() -> (Arc<Store>, [Byte32; 5], Arc<Entry>) {
    let pool = store();
    let a = ranked_member(&pool, 10600, 0, &[]);
    let b = ranked_member(&pool, 10601, 1, &[a.hash()]);
    let c = ranked_member(&pool, 10602, 9, &[b.hash()]);
    let d = ranked_member(&pool, 10603, 100, &[a.hash()]);
    let e = ranked_member(&pool, 10604, 45, &[]);
    let candidate = ranked_member(&store(), 10605, 1000, &[]);
    (
        pool,
        [a.hash(), b.hash(), c.hash(), d.hash(), e.hash()],
        candidate,
    )
}

#[test]
fn trim_batch_updates_shared_ancestor_before_the_next_round() {
    let (pool, [_a, b, c, _d, e], candidate) = rank_fixture();
    // First remove b+c: their average rate is 5, below a's 27.5.
    // Then a's remaining average rises to 50, so e (45) must be selected next.
    let empty_late = HashMap::new();
    let mut removed = HashSet::new();
    eviction::select_victims(
        &pool.state_read(),
        &candidate,
        &HashSet::from([candidate.hash_bytes()]),
        &empty_late,
        &HashSet::new(),
        &mut removed,
        Amount {
            items: 3,
            ..Amount::default()
        },
    )
    .unwrap();
    assert_eq!(to_byte32_set(&removed), BTreeSet::from([b, c, e]));
}

#[test]
fn trimming_continues_until_every_excess_coordinate_is_released() {
    let (pool, [_, b, c, _, e], candidate) = rank_fixture();
    // The first b+c closure covers the item excess, but its serialized bytes
    // leave more to release. The next eligible owner is e.
    let bc_serialized = pool.point(&b).1.unwrap().charge().serialized
        + pool.point(&c).1.unwrap().charge().serialized;
    let empty_late = HashMap::new();
    let mut removed = HashSet::new();
    eviction::select_victims(
        &pool.state_read(),
        &candidate,
        &HashSet::from([candidate.hash_bytes()]),
        &empty_late,
        &HashSet::new(),
        &mut removed,
        Amount {
            items: 1,
            serialized: bc_serialized + 1,
            ..Amount::default()
        },
    )
    .unwrap();
    assert_eq!(to_byte32_set(&removed), BTreeSet::from([b, c, e]));
}

#[test]
fn trim_protected_rejection_leaves_live_state_untouched() {
    let (pool, _hashes, candidate) = rank_fixture();
    let empty_late = HashMap::new();
    let mut removed = HashSet::new();
    let result = eviction::select_victims(
        &pool.state_read(),
        &candidate,
        &HashSet::from([candidate.hash_bytes()]),
        &empty_late,
        &HashSet::new(),
        &mut removed,
        Amount {
            items: 6,
            ..Amount::default()
        },
    );
    assert!(matches!(result, Err(Error::Rejected(Reject::Full(_)))));
    assert_eq!(pool.capture_accepted().owners.len(), 5);
}

#[test]
fn trim_singleton_round_still_updates_its_surviving_ancestor() {
    let pool = store();
    let a = ranked_member(&pool, 10610, 0, &[]);
    let b = ranked_member(&pool, 10611, 1, &[a.hash()]);
    let _d = ranked_member(&pool, 10612, 100, &[a.hash()]);
    let e = ranked_member(&pool, 10613, 45, &[]);
    let candidate = ranked_member(&store(), 10614, 1000, &[]);
    let empty_late = HashMap::new();
    let mut removed = HashSet::new();
    eviction::select_victims(
        &pool.state_read(),
        &candidate,
        &HashSet::from([candidate.hash_bytes()]),
        &empty_late,
        &HashSet::new(),
        &mut removed,
        Amount {
            items: 2,
            ..Amount::default()
        },
    )
    .unwrap();
    assert_eq!(
        to_byte32_set(&removed),
        BTreeSet::from([b.hash(), e.hash()])
    );
}

#[test]
fn skipped_overlimit_root_is_never_reselected_after_descendant_eviction() {
    let pool = store();
    // Fill MAX_POOL_MUTATION_CANDIDATES - 1 into initial `removed` so only 1
    // additional victim fits. `b` has a 2-node family (b+c) and must be skipped.
    // Evicting leaf `c` reduces `b`'s remaining fee, but `b` cannot be evicted
    // either because `removed` now holds MAX_POOL_MUTATION_CANDIDATES entries.
    let a = ranked_member(&pool, 10700, 50, &[]);
    let b = ranked_member(&pool, 10701, 1, &[a.hash()]);
    let c = ranked_member(&pool, 10702, 2, &[b.hash()]);
    let candidate = ranked_member(&store(), 10703, 1000, &[]);
    let mut removed: HashSet<[u8; 32]> = (0..MAX_POOL_MUTATION_CANDIDATES - 1)
        .map(|i| {
            let mut key = [0xee; 32];
            key[..8].copy_from_slice(&(i as u64).to_le_bytes());
            key
        })
        .collect();
    let empty_late = HashMap::new();
    eviction::select_victims(
        &pool.state_read(),
        &candidate,
        &HashSet::from([candidate.hash_bytes()]),
        &empty_late,
        &HashSet::new(),
        &mut removed,
        Amount {
            items: 1,
            ..Amount::default()
        },
    )
    .unwrap();
    assert!(removed.contains(&c.hash_bytes()));
    assert!(!removed.contains(&b.hash_bytes()));
    assert!(!removed.contains(&a.hash_bytes()));
}

#[test]
fn late_parent_eviction_accounts_for_late_descendants_and_matches_reference() {
    let pool = store();
    let low_singleton = ranked_member(&pool, 10800, 5, &[]);
    let candidate = ranked_member(&store(), 10801, 1, &[]);
    // Existing pool entry that reads candidate's output (late child) with high fee:
    // its high fee lifts candidate's effective eviction rank above `low_singleton`.
    let late_child = ranked_member(&pool, 10802, 50_000, &[candidate.hash()]);
    let late_descendants = HashSet::from([late_child.hash_bytes()]);

    let empty_late = HashMap::new();
    let mut removed = HashSet::new();
    eviction::select_victims(
        &pool.state_read(),
        &candidate,
        &HashSet::from([candidate.hash_bytes()]),
        &empty_late,
        &late_descendants,
        &mut removed,
        Amount {
            items: 1,
            ..Amount::default()
        },
    )
    .unwrap();
    assert_eq!(
        to_byte32_set(&removed),
        BTreeSet::from([low_singleton.hash()])
    );
}

#[test]
fn late_parent_evicting_late_child_refunds_projected_charge_without_over_evicting_unrelated_tx() {
    // Regression test: when a late parent adds a parent edge (+160 bytes) to an
    // existing accepted child, `evaluate_admission` counts the child's projected
    // charge in `excess`. Evicting that child must refund the same projected
    // charge so an unrelated transaction that fits inside the exact remaining
    // byte budget is never over-evicted.
    let configuration = common::config();
    let snapshot = common::chain_snapshot();

    // Give the late parent enough output data so `{late_child, unrelated}` fits
    // within the exact budget `parent_charge.bytes + unrelated_charge.bytes`.
    let late_parent_tx = common::output_tx(10900)
        .as_advanced_builder()
        .set_outputs_data(vec![ckb_types::bytes::Bytes::from(vec![0u8; 2048]).pack()])
        .build();
    let late_child_tx = spend(10901, &[], &[OutPoint::new(late_parent_tx.hash(), 0)]);
    let unrelated_tx = common::tx(10902);

    // Measure exact accepted charges on a scratch store before setting the tight budget.
    let scratch = store();
    let parent_probe = accept(&scratch, late_parent_tx.clone(), 10_000, 1, Status::Pending);
    let unrelated_probe = accept(&scratch, unrelated_tx.clone(), 500, 1, Status::Pending);
    let child_probe = accept(&scratch, late_child_tx.clone(), 10, 1, Status::Pending);
    let parent_charge = scratch.point(&parent_probe).1.unwrap().charge();
    let unrelated_charge = scratch.point(&unrelated_probe).1.unwrap().charge();
    let child_with_parent_charge = scratch.point(&child_probe).1.unwrap().charge();

    let scratch_no_parent = store();
    let child_solo_probe = accept(
        &scratch_no_parent,
        late_child_tx.clone(),
        10,
        1,
        Status::Pending,
    );
    let child_old_charge = scratch_no_parent
        .point(&child_solo_probe)
        .1
        .unwrap()
        .charge();
    assert!(
        child_with_parent_charge.bytes > child_old_charge.bytes,
        "late parent edge must increase projected child charge"
    );

    let exact_budget = parent_charge.bytes + unrelated_charge.bytes;
    assert!(
        child_old_charge.bytes + unrelated_charge.bytes <= exact_budget,
        "pre-admission pool must fit late child and unrelated tx"
    );
    let mut limits =
        crate::authority::budget::Limits::new(&configuration, snapshot.consensus()).unwrap();
    limits.accepted.bytes = exact_budget;
    let pool = Store::with_limits(snapshot, &configuration, limits).unwrap();

    // Admit late_child (lowest rank) and unrelated (higher rank) before late_parent.
    let child_hash = accept(&pool, late_child_tx, 10, 1, Status::Pending);
    let unrelated_hash = accept(&pool, unrelated_tx, 500, 1, Status::Pending);
    assert_eq!(pool.capture_accepted().owners.len(), 2);

    // Admit late_parent (highest rank): only late_child should be evicted,
    // leaving both late_parent and unrelated at the exact byte ceiling.
    let parent_candidate = common::entry(&pool, late_parent_tx, Source::Local);
    let parent_hash = parent_candidate.hash();
    let (_, reject) = common::commit_admission(
        &pool,
        &parent_candidate,
        10_000,
        1,
        Status::Pending,
        &configuration,
    )
    .unwrap();
    assert!(reject.is_none());

    let survivors: BTreeSet<Byte32> = pool
        .capture_accepted()
        .owners
        .into_iter()
        .map(|entry| entry.hash())
        .collect();
    assert_eq!(
        survivors,
        BTreeSet::from([parent_hash, unrelated_hash]),
        "late child must be the sole eviction victim; unrelated tx must survive"
    );
    assert!(
        pool.point(&child_hash)
            .1
            .is_none_or(|e| e.accepted().is_none())
    );
    assert_eq!(pool.accepted_usage().bytes, exact_budget);
    assert!(!pool.is_faulted());
}

#[test]
fn protected_ancestor_eviction_matches_reference_with_diamonds_and_fanout() {
    let pool = store();
    // Build a diamond + fanout under protected ancestor `root`:
    //   root -> left, root -> right, left + right -> join, plus extra leaves
    //   under `root` and unrelated victims.
    let root = ranked_member(&pool, 11000, 10, &[]);
    let left = ranked_member(&pool, 11001, 20, &[root.hash()]);
    let right = ranked_member(&pool, 11002, 25, &[root.hash()]);
    let join = ranked_member(&pool, 11003, 5, &[left.hash(), right.hash()]);
    let mut leaves = Vec::new();
    for i in 0..32u32 {
        leaves.push(ranked_member(
            &pool,
            11010 + i,
            15 + u64::from(i),
            &[root.hash()],
        ));
    }
    let victim_low = ranked_member(&pool, 11100, 3, &[]);
    let victim_mid = ranked_member(&pool, 11101, 8, &[]);

    // Candidate is a high-fee child of `root`: `root` is a protected ancestor
    // whose virtual descendant aggregate includes all 36 live descendants plus
    // `candidate` (and shrinks as `join` / leaves are evicted).
    let candidate = ranked_member(&store(), 11200, 50_000, &[root.hash()]);
    let protected = HashSet::from([root.hash_bytes()]);
    let late_descendants = HashSet::new();
    let empty_late = HashMap::new();
    let excess = Amount {
        items: 4,
        ..Amount::default()
    };

    let state = pool.state_read();
    // Verify incremental descendant aggregate matches exact diamond count (1+3+32 = 36).
    let root_agg = state.descendant_aggregate(&root.hash_bytes()).unwrap();
    assert_eq!(root_agg.count, 36);
    assert_eq!(
        state
            .descendant_aggregate(&left.hash_bytes())
            .unwrap()
            .count,
        2
    );
    assert_eq!(
        state
            .descendant_aggregate(&right.hash_bytes())
            .unwrap()
            .count,
        2
    );
    assert_eq!(
        state
            .descendant_aggregate(&join.hash_bytes())
            .unwrap()
            .count,
        1
    );

    let mut actual_removed = HashSet::new();
    eviction::select_victims(
        &state,
        &candidate,
        &protected,
        &empty_late,
        &late_descendants,
        &mut actual_removed,
        excess,
    )
    .unwrap();
    let virtual_owners: Members = state
        .owners
        .values()
        .map(|entry| (entry.hash(), Arc::clone(entry)))
        .collect();
    let expected_removed = reference_trim(
        &virtual_owners,
        &state.snapshot,
        &candidate,
        &BTreeSet::from([root.hash()]),
        &BTreeSet::new(),
        &BTreeSet::new(),
        excess,
    )
    .unwrap();
    assert_eq!(to_byte32_set(&actual_removed), expected_removed);
    assert!(actual_removed.contains(&victim_low.hash_bytes()));
    assert!(actual_removed.contains(&victim_mid.hash_bytes()));
    assert!(actual_removed.contains(&join.hash_bytes()));
    assert!(!actual_removed.contains(&root.hash_bytes()));
    let _ = leaves;
}
