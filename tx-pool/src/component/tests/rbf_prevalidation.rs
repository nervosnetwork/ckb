//! Regression test: an RBF replacement whose commit is *certain* to fail
//! must be rejected before its conflicts are removed.
//!
//! `prepare_rbf_replacement` used to remove the conflicted (victim) cluster
//! first and only discover commit failures afterwards, so every failed
//! attempt evicted the victim cluster and then restored it. A replacement
//! crafted to always fail (here: more than `max_ancestors_count` in-pool
//! ancestors via a cell dep on a long chain) could repeat that
//! remove-and-restore churn indefinitely at no cost — a failed replacement
//! pays no fee. `PoolMap::validate_ancestor_capacity` now rejects such
//! entries before anything is removed.

use crate::{
    component::{entry::TxEntry, pool_map::Status},
    error::Reject,
};
use ckb_types::{
    core::Capacity,
    packed::{Byte32, ProposalShortId},
};

use super::pipeline::service_with_rbf;
use super::util::{MOCK_CYCLES, MOCK_SIZE, build_tx, build_tx_with_dep, build_tx_with_since};

/// The pool accepts a chain link `a_i` (spending `a_{i-1}:0`) while
/// `i + 1 <= max_ancestors_count`, so the deepest addable chain is exactly
/// `max_ancestors_count` transactions long.
const CHAIN_LEN: u32 = 125;

#[tokio::test]
async fn accepted_callback_runs_only_after_pool_write_lock_is_released() {
    use super::harness::{WorkerSet, harness};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let mut service = harness(1).workers(WorkerSet::None).build().service;
    let callback_ran = Arc::new(AtomicBool::new(false));
    let pool = Arc::clone(&service.pool.tx_pool);
    let callback_ran_clone = Arc::clone(&callback_ran);
    let mut callbacks = crate::callback::Callbacks::new();
    callbacks.register_pending(Box::new(move |_entry| {
        assert!(
            pool.try_read().is_ok(),
            "pending callback must not run under tx_pool.write()"
        );
        callback_ran_clone.store(true, Ordering::SeqCst);
    }));
    service.relay.callbacks = Arc::new(callbacks);

    let tx = build_tx(vec![(&Byte32::new([99; 32]), 0)], 1);
    let entry = TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1), MOCK_SIZE);
    let id = entry.proposal_short_id();
    let effect_permit = service
        .reserve_effects(service.max_submit_effect_bytes())
        .await
        .unwrap();
    let outcome = {
        let mut guard = service.pool.tx_pool.write().await;
        let snapshot = guard.cloned_snapshot();
        let mut outcome = service.try_submit_entry(
            &mut guard,
            Arc::clone(&snapshot),
            snapshot.tip_hash(),
            entry,
            Status::Pending,
            id.clone(),
        );
        assert!(
            !callback_ran.load(Ordering::SeqCst),
            "try_submit_entry must only collect callback side effects"
        );
        service.journal_submit_effects(&mut outcome, effect_permit, Vec::new());
        outcome
    };
    outcome.result.unwrap();
    service.relay.effects.wait_idle().await;
    assert!(callback_ran.load(Ordering::SeqCst));
}

#[tokio::test]
async fn rbf_replacement_certain_to_fail_commit_cannot_churn_pool() {
    let (service, _relay, _cancel, _store, _out_points) = service_with_rbf(2);

    // The victim cluster: a tx and its child, both in the pool.
    let victim = build_tx(vec![(&Byte32::zero(), 1)], 1);
    let victim_child = build_tx(vec![(&victim.hash(), 0)], 1);

    // The attack entry's in-pool ancestry: the deepest chain the pool
    // accepts. A cell dep on the chain tip then pushes the attack entry
    // over the ancestor limit even after the victim cluster is removed.
    let mut chain_tip = Byte32::zero();
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        for _ in 0..CHAIN_LEN {
            let link = build_tx(vec![(&chain_tip, 0)], 1);
            chain_tip = link.hash();
            tx_pool
                .pool_map
                .add_entry(
                    TxEntry::dummy_resolve(link, MOCK_CYCLES, Capacity::zero(), MOCK_SIZE),
                    Status::Pending,
                )
                .unwrap();
        }
        for tx in [&victim, &victim_child] {
            tx_pool
                .pool_map
                .add_entry(
                    TxEntry::dummy_resolve(tx.clone(), MOCK_CYCLES, Capacity::zero(), MOCK_SIZE),
                    Status::Pending,
                )
                .unwrap();
        }
    }

    // Conflicts with the victim by spending the same input; passes the RBF
    // rules (inputs are all conflict inputs; fee is far above the victim
    // cluster's) but can never be committed: its in-pool ancestors are the
    // whole chain (125) plus itself, exceeding the limit of 125.
    let attack = build_tx_with_dep(vec![(&Byte32::zero(), 1)], vec![(&chain_tip, 0)], 1);
    let attack_entry = TxEntry::dummy_resolve(
        attack,
        MOCK_CYCLES,
        Capacity::shannons(1_000_000_000),
        MOCK_SIZE,
    );
    let attack_id = attack_entry.proposal_short_id();

    let crate::process::submit::rbf_commit::SubmitEntryOutcome {
        result,
        replaced: _,
        rolled_back,
        reject_events,
        accept_event: _,
    } = {
        let mut tx_pool = service.pool.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();
        let pre_resolve_tip = snapshot.tip_hash();
        service.try_submit_entry(
            &mut tx_pool,
            snapshot,
            pre_resolve_tip,
            attack_entry,
            Status::Pending,
            attack_id,
        )
    };

    // The replacement is rejected *before* any removal: nothing was
    // evicted, nothing needs recovering, and the victim cluster never left
    // the pool.
    assert!(matches!(result, Err(Reject::ExceededMaximumAncestorsCount)));
    assert!(rolled_back.is_empty());
    assert!(reject_events.is_empty());
    let tx_pool = service.pool.tx_pool.read().await;
    let resident = |tx: &ckb_types::core::TransactionView| {
        tx_pool
            .pool_map
            .get_by_id(&tx.proposal_short_id())
            .is_some_and(|entry| entry.inner.transaction().hash() == tx.hash())
    };
    assert!(resident(&victim), "victim must remain in the pool");
    assert!(
        resident(&victim_child),
        "victim's descendant must remain in the pool"
    );
}

/// `conflict_closure` must abort as soon as the union exceeds the given
/// limit, making rule #5's candidate cap the hard bound on traversal cost
/// regardless of pool population.
#[test]
fn conflict_closure_aborts_at_candidate_limit() {
    use crate::component::pool_map::{ConflictClosure, PoolMap};
    use std::collections::HashSet;

    let mut pool = PoolMap::new(super::util::DEFAULT_MAX_ANCESTORS_COUNT);
    // A chain one link longer than the candidate limit (depth stays within
    // the pool's ancestor limit).
    let chain_len = 101u32;
    let mut tip = Byte32::zero();
    let mut chain = Vec::new();
    for _ in 0..chain_len {
        let link = build_tx(vec![(&tip, 0)], 1);
        tip = link.hash();
        chain.push(link.clone());
        pool.add_entry(
            TxEntry::dummy_resolve(link, MOCK_CYCLES, Capacity::zero(), MOCK_SIZE),
            Status::Pending,
        )
        .unwrap();
    }
    let root = chain[0].proposal_short_id();
    let roots = HashSet::from([root.clone()]);

    match pool.conflict_closure(&roots, 100) {
        ConflictClosure::Exceeded { count_lower_bound } => {
            assert_eq!(count_lower_bound, 101);
        }
        ConflictClosure::Complete { .. } => panic!("closure must abort above the limit"),
    }

    // The same structure completes within a higher limit, ordered children
    // before parents (chain tip first, root last).
    match pool.conflict_closure(&roots, 125) {
        ConflictClosure::Complete {
            removal,
            removal_set,
        } => {
            assert_eq!(removal.len(), chain_len as usize);
            assert_eq!(removal_set.len(), chain_len as usize);
            assert_eq!(
                removal.first(),
                Some(&chain[chain.len() - 1].proposal_short_id())
            );
            assert_eq!(removal.last(), Some(&root));
        }
        ConflictClosure::Exceeded { .. } => panic!("closure must complete within the limit"),
    }
}

/// Entries evicted by `add_entry`'s cell-ref escape hatch must be recovered
/// when the commit that caused them later fails: a failed commit must not
/// evict in-pool transactions it can no longer invalidate.
#[tokio::test]
async fn escape_hatch_evictions_are_recovered_on_commit_failure() {
    use super::harness::{WorkerSet, harness};

    // Pool totals: 124 links * 100 + E * 100 = 12_500, so
    // max_tx_pool_size = 12_499 forces `limit_size` to evict after N is
    // inserted. N pays zero fee while every other entry pays 1_000, making
    // N the lowest evict key and thus the self-evicted one.
    let service = harness(2)
        .max_tx_pool_size(12_499)
        .workers(WorkerSet::None)
        .build()
        .service;

    // Funding output X that N spends as an input while E cell-deps on it,
    // which makes E a cell-ref parent of N.
    let x_hash = Byte32::new([9u8; 32]);

    // A 124-link chain (the deepest the pool accepts). N's ancestry is the
    // chain (124) plus E (1) = 125, count 126 > max_ancestors_count, so
    // add_entry's escape hatch evicts E to fit (126 - 1 <= 125).
    let mut chain_tip = Byte32::zero();
    let e = build_tx_with_dep(vec![(&Byte32::zero(), 2)], vec![(&x_hash, 0)], 1);
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        for _ in 0..124u32 {
            let link = build_tx(vec![(&chain_tip, 0)], 1);
            chain_tip = link.hash();
            tx_pool
                .pool_map
                .add_entry(
                    TxEntry::dummy_resolve(link, MOCK_CYCLES, Capacity::shannons(1_000), 100),
                    Status::Pending,
                )
                .unwrap();
        }
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(e.clone(), MOCK_CYCLES, Capacity::shannons(1_000), 100),
                Status::Pending,
            )
            .unwrap();
        assert_eq!(tx_pool.pool_map.stats.total_tx_size.get(), 12_500);
    }

    // N: spends X, cell-deps on the chain tip, pays no fee.
    let n = build_tx_with_dep(vec![(&x_hash, 0)], vec![(&chain_tip, 0)], 1);
    let n_id = n.proposal_short_id();
    let n_entry = TxEntry::dummy_resolve(n.clone(), MOCK_CYCLES, Capacity::zero(), 100);

    let crate::process::submit::rbf_commit::SubmitEntryOutcome {
        result,
        replaced: _,
        rolled_back,
        reject_events,
        accept_event: _,
    } = {
        let mut tx_pool = service.pool.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();
        let pre_resolve_tip = snapshot.tip_hash();
        service.try_submit_entry(
            &mut tx_pool,
            snapshot,
            pre_resolve_tip,
            n_entry,
            Status::Pending,
            n_id,
        )
    };

    // The escape hatch evicted E to admit N, then N self-evicted under the
    // pool limit: the commit failed, and E must come back.
    assert!(
        result.is_err(),
        "the zero-fee entry must self-evict under the pool limit"
    );
    assert!(
        rolled_back.iter().any(|(tx, _)| tx.hash() == e.hash()),
        "the escape-hatch eviction of E must be recovered after the commit failure"
    );
    assert!(
        !reject_events
            .iter()
            .any(|(entry, _)| entry.transaction().hash() == e.hash()),
        "E's Invalidated reject event must be suppressed once it is recovered"
    );
}

/// A size-limit failure can evict unrelated low-fee entries before it reaches
/// and rejects the just-inserted candidate. Every one of those removals is
/// part of the failed commit transaction: restore it under the same write
/// guard and preserve its exact proposal-window status.
#[tokio::test]
async fn failed_commit_restores_all_size_evictions_with_original_status_in_lock() {
    use super::harness::{WorkerSet, harness};
    use std::{collections::HashSet, sync::Arc};

    let service = harness(2)
        .rbf(true)
        .max_tx_pool_size(250)
        .workers(WorkerSet::None)
        .build()
        .service;

    let victim = build_tx(vec![(&Byte32::new([71; 32]), 0)], 1);
    let unrelated = build_tx(vec![(&Byte32::new([72; 32]), 0)], 1);
    let replacement = build_tx(vec![(&Byte32::new([71; 32]), 0)], 2);
    let victim_id = victim.proposal_short_id();
    let unrelated_id = unrelated.proposal_short_id();
    let replacement_id = replacement.proposal_short_id();

    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(victim.clone(), MOCK_CYCLES, Capacity::shannons(1_000), 100),
                Status::Proposed,
            )
            .unwrap();
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(unrelated.clone(), MOCK_CYCLES, Capacity::zero(), 100),
                Status::Gap,
            )
            .unwrap();
    }

    // Once the victim is removed, the 300-byte replacement makes the pool
    // 400 bytes. Eviction removes the zero-fee unrelated entry first, then
    // the still-oversized replacement itself. The failed transaction must
    // put both prior entries back before this guard can be released.
    let replacement_entry = TxEntry::dummy_resolve(
        replacement,
        MOCK_CYCLES,
        Capacity::shannons(1_000_000_000),
        300,
    );
    let outcome = {
        let mut tx_pool = service.pool.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();
        let outcome = service.try_submit_entry(
            &mut tx_pool,
            Arc::clone(&snapshot),
            snapshot.tip_hash(),
            replacement_entry,
            Status::Proposed,
            replacement_id.clone(),
        );

        assert!(outcome.result.is_err());
        assert_eq!(
            tx_pool
                .pool_map
                .get_by_id(&victim_id)
                .map(|entry| entry.status),
            Some(Status::Proposed)
        );
        assert_eq!(
            tx_pool
                .pool_map
                .get_by_id(&unrelated_id)
                .map(|entry| entry.status),
            Some(Status::Gap)
        );
        assert!(tx_pool.pool_map.get_by_id(&replacement_id).is_none());
        assert_eq!(tx_pool.pool_map.stats.total_tx_size.get(), 200);
        outcome
    };

    let restored: HashSet<_> = outcome
        .rolled_back
        .iter()
        .map(|(tx, status)| (tx.proposal_short_id(), *status))
        .collect();
    assert_eq!(
        restored,
        HashSet::from([
            (victim_id.clone(), Status::Proposed),
            (unrelated_id.clone(), Status::Gap),
        ])
    );
    assert!(
        outcome.reject_events.iter().all(|(entry, _)| {
            let id = entry.proposal_short_id();
            id != victim_id && id != unrelated_id
        }),
        "restored entries must not emit terminal reject callbacks"
    );
    assert!(matches!(
        outcome.reject_events.as_slice(),
        [(entry, Reject::Full(_))] if entry.proposal_short_id() == replacement_id
    ));
}

/// The escape-hatch eviction journal must also cover the case where
/// `add_entry` itself fails *after* the escape eviction (here: the dep
/// pre-validation rejects the entry because another in-pool tx consumes
/// one of its deps). The returned `Err` carries no evict set, so without
/// the journal the eviction would be lost.
#[tokio::test]
async fn escape_hatch_evictions_are_recovered_when_dep_check_fails() {
    use super::harness::{WorkerSet, harness};

    let service = harness(2).workers(WorkerSet::None).build().service;

    // Funding outputs: N spends X as an input (E cell-deps on it, making E
    // a cell-ref parent of N) and deps on D', which C consumes as an input.
    let x_hash = Byte32::new([9u8; 32]);
    let d_hash = Byte32::new([7u8; 32]);

    let c = build_tx(vec![(&d_hash, 0)], 1);
    let e = build_tx_with_dep(vec![(&Byte32::zero(), 2)], vec![(&x_hash, 0)], 1);

    let mut chain_tip = Byte32::zero();
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        for _ in 0..124u32 {
            let link = build_tx(vec![(&chain_tip, 0)], 1);
            chain_tip = link.hash();
            tx_pool
                .pool_map
                .add_entry(
                    TxEntry::dummy_resolve(link, MOCK_CYCLES, Capacity::shannons(1_000), 100),
                    Status::Pending,
                )
                .unwrap();
        }
        for tx in [c.clone(), e.clone()] {
            tx_pool
                .pool_map
                .add_entry(
                    TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1_000), 100),
                    Status::Pending,
                )
                .unwrap();
        }
    }

    // N: input X, deps = [chain_tip, D']. Ancestry = chain(124) + E(1) =
    // 125 (count 126 > 125), so the escape hatch evicts E first; the dep
    // pre-validation then rejects N because C already consumes D'.
    let n = build_tx_with_dep(vec![(&x_hash, 0)], vec![(&chain_tip, 0), (&d_hash, 0)], 1);
    let n_id = n.proposal_short_id();
    let n_entry = TxEntry::dummy_resolve(n.clone(), MOCK_CYCLES, Capacity::zero(), 100);

    let crate::process::submit::rbf_commit::SubmitEntryOutcome {
        result,
        replaced: _,
        rolled_back,
        reject_events: _,
        accept_event: _,
    } = {
        let mut tx_pool = service.pool.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();
        let pre_resolve_tip = snapshot.tip_hash();
        service.try_submit_entry(
            &mut tx_pool,
            snapshot,
            pre_resolve_tip,
            n_entry,
            Status::Pending,
            n_id.clone(),
        )
    };

    assert!(
        matches!(result, Err(Reject::Resolve(_))),
        "the dep pre-validation must reject after C consumed D'"
    );
    assert!(
        rolled_back.iter().any(|(tx, _)| tx.hash() == e.hash()),
        "the escape-hatch eviction of E must be recovered even though add_entry itself failed"
    );

    // The rejected entry must not leave ghost links behind: no links node
    // for N, and no parent→child reference to N anywhere in the graph.
    // (Ancestor links are committed only after every fallible validation.)
    let tx_pool = service.pool.tx_pool.read().await;
    assert!(
        !tx_pool.pool_map.links.contains_key(&n_id),
        "rejected entry must not have a links node"
    );
    let chain_tip_id = ProposalShortId::from_tx_hash(&chain_tip);
    assert!(
        !tx_pool
            .pool_map
            .links
            .get_children(&chain_tip_id)
            .is_some_and(|children| children.contains(&n_id)),
        "rejected entry must not be referenced as a child of its parents"
    );
}

/// A tip-change revalidation failure must not strand the removed cascade.
///
/// `prepare_rbf_replacement` removes the conflict cluster *before* the
/// fallible tip revalidation. The removed set used to reach the caller only
/// on success, so this failure recovered just the direct conflict (found
/// through the replacement's own inputs), stranded descendants in the
/// conflicts cache, and leaked their "replaced" reject events.
#[tokio::test]
async fn failed_tip_revalidation_recovers_whole_removed_cascade() {
    use std::collections::HashSet;

    let (service, _relay, _cancel, _store, _out_points) = service_with_rbf(2);

    // In-pool victim cluster: a parent and its child.
    let parent = build_tx(vec![(&Byte32::zero(), 5)], 1);
    let child = build_tx(vec![(&parent.hash(), 0)], 1);
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        for tx in [parent.clone(), child.clone()] {
            tx_pool
                .pool_map
                .add_entry(
                    TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::zero(), MOCK_SIZE),
                    Status::Pending,
                )
                .unwrap();
        }
    }

    // The replacement spends the parent's input with a far-future absolute
    // `since`: it passes the RBF rules (its fee is far above the zero-fee
    // cluster and its only input is the conflict input) but fails the
    // time-relative revalidation once the tip has "changed".
    let attack = build_tx_with_since(vec![(&Byte32::zero(), 5, 1_000_000_000_000)], 1);
    let attack_entry = TxEntry::dummy_resolve(
        attack,
        MOCK_CYCLES,
        Capacity::shannons(1_000_000_000),
        MOCK_SIZE,
    );
    let attack_id = attack_entry.proposal_short_id();

    let crate::process::submit::rbf_commit::SubmitEntryOutcome {
        result,
        replaced: _,
        rolled_back,
        reject_events,
        accept_event: _,
    } = {
        let mut tx_pool = service.pool.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();
        // A stale pre-resolve tip forces the tip-change revalidation branch.
        let stale_tip = Byte32::new([1u8; 32]);
        service.try_submit_entry(
            &mut tx_pool,
            snapshot,
            stale_tip,
            attack_entry,
            Status::Pending,
            attack_id,
        )
    };

    assert!(result.is_err(), "far-future since must fail revalidation");
    let recovered_hashes: HashSet<_> = rolled_back.iter().map(|(tx, _)| tx.hash()).collect();
    assert!(
        recovered_hashes.contains(&parent.hash()),
        "the direct conflict must be recovered"
    );
    assert!(
        recovered_hashes.contains(&child.hash()),
        "the cascade-removed descendant must be recovered too"
    );
    assert!(
        !reject_events.iter().any(|(entry, _)| {
            let h = entry.transaction().hash();
            h == parent.hash() || h == child.hash()
        }),
        "no spurious 'replaced' reject events may leak for recovered txs"
    );

    // The recovered txs were taken out of the conflict cache: ownership
    // moved to the recovery set.
    let tx_pool = service.pool.tx_pool.read().await;
    for tx in [&parent, &child] {
        assert!(
            !tx_pool.conflict_cache.contains(&tx.proposal_short_id()),
            "recovered tx must not stay in the conflict cache"
        );
    }
}

/// The waiting-room conflict index is symmetric by construction: entries
/// leave `by_outpoint` exactly when their transaction leaves, and the
/// recovery query only returns candidates whose inputs are *all* free.
#[tokio::test]
async fn conflict_recovery_index_stays_consistent() {
    use super::harness::{WorkerSet, harness};
    use crate::component::entry::TxEntry;
    use crate::component::pool_map::Status;
    use crate::tx_source::TxSource;
    use ckb_types::packed::OutPoint;

    let service = harness(1).workers(WorkerSet::None).build().service;
    let x_hash = Byte32::new([13u8; 32]);
    let y_hash = Byte32::new([14u8; 32]);
    let mut tx_pool = service.pool.tx_pool.write().await;

    // A candidate with two blocked inputs, and a single-input candidate.
    let candidate = build_tx(vec![(&x_hash, 0), (&y_hash, 0)], 1);
    tx_pool.record_conflict(candidate.clone(), TxSource::Local);
    let other = build_tx(vec![(&x_hash, 1)], 1);
    tx_pool.record_conflict(other.clone(), TxSource::Local);

    // While Y is consumed by an in-pool tx, the two-input candidate must
    // NOT be recoverable (recovering it would just reject it again, and
    // could cycle two conflicting txs through the cache forever).
    let blocker = build_tx(vec![(&y_hash, 0)], 1);
    let blocker_id = blocker.proposal_short_id();
    tx_pool
        .pool_map
        .add_entry(
            TxEntry::dummy_resolve(blocker, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
            Status::Pending,
        )
        .unwrap();

    let recovered = tx_pool.get_conflicted_txs_from_inputs(
        vec![
            OutPoint::new(x_hash.clone(), 0),
            OutPoint::new(x_hash.clone(), 1),
            OutPoint::new(y_hash.clone(), 0),
        ]
        .into_iter(),
    );
    assert_eq!(recovered.len(), 1, "only the fully-free candidate recovers");
    assert_eq!(recovered[0].0.hash(), other.hash());

    // Once the blocker leaves the pool, the two-input candidate recovers.
    tx_pool.pool_map.remove_entry(&blocker_id);
    let recovered =
        tx_pool.get_conflicted_txs_from_inputs(vec![OutPoint::new(y_hash.clone(), 0)].into_iter());
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].0.hash(), candidate.hash());

    // remove_conflict drops the index entry symmetrically.
    tx_pool.remove_conflict(&candidate.proposal_short_id());
    tx_pool.remove_conflict(&other.proposal_short_id());
    let recovered = tx_pool.get_conflicted_txs_from_inputs(
        vec![OutPoint::new(x_hash, 0), OutPoint::new(y_hash, 0)].into_iter(),
    );
    assert!(recovered.is_empty());
}

/// On a successful commit the conflict cache remains the candidate's sole
/// owner until bounded maintenance admits it to the coordinator. The handoff
/// then removes the historical copy under the same pool lock, so combined
/// readers cannot observe dual or zero ownership.
#[tokio::test]
async fn successful_commit_transfers_recovered_tx_from_cache_to_coordinator_once() {
    use super::harness::{WorkerSet, harness};
    use crate::tx_source::TxSource;

    let service = harness(2)
        .rbf(true)
        .workers(WorkerSet::None)
        .build()
        .service;

    // The victim spends A and B; the conflict-cached C spends B (blocked by
    // the victim while it is in the pool).
    let a_hash = Byte32::new([41u8; 32]);
    let b_hash = Byte32::new([42u8; 32]);
    let victim = build_tx(vec![(&a_hash, 0), (&b_hash, 0)], 1);
    let recovered_tx = build_tx(vec![(&b_hash, 0)], 1);
    let recovered_id = recovered_tx.proposal_short_id();
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(
                    victim.clone(),
                    MOCK_CYCLES,
                    Capacity::shannons(1),
                    MOCK_SIZE,
                ),
                Status::Pending,
            )
            .unwrap();
        tx_pool.record_conflict(recovered_tx.clone(), TxSource::Local);
    }

    // The replacement spends only A: it replaces the victim, freeing B,
    // which makes the conflict-cached C recoverable.
    let replacement = build_tx(vec![(&a_hash, 0)], 1);
    let replacement_entry = TxEntry::dummy_resolve(
        replacement,
        MOCK_CYCLES,
        Capacity::shannons(1_000_000_000),
        MOCK_SIZE,
    );
    let replacement_id = replacement_entry.proposal_short_id();

    let crate::process::submit::rbf_commit::SubmitEntryOutcome {
        result,
        replaced: _,
        rolled_back,
        reject_events: _events,
        accept_event: _,
    } = {
        let mut tx_pool = service.pool.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();
        let pre_resolve_tip = snapshot.tip_hash();
        service.try_submit_entry(
            &mut tx_pool,
            snapshot,
            pre_resolve_tip,
            replacement_entry,
            Status::Pending,
            replacement_id,
        )
    };

    assert!(result.is_ok(), "replacement must commit: {:?}", result);
    assert!(rolled_back.is_empty());
    {
        let tx_pool = service.pool.tx_pool.read().await;
        assert!(
            tx_pool.conflict_cache.contains(&recovered_id),
            "the cache owns the candidate before maintenance handoff"
        );
        assert_eq!(tx_pool.conflict_recovery_len(), 1);
    }
    assert!(
        !service
            .pipeline
            .runtime
            .mutate(|coordinator| coordinator.contains_hash(&recovered_tx.hash())),
        "the coordinator must not own the candidate before handoff"
    );

    let progress = service.recover_conflict_cache_slice(1).await;
    assert!(!progress.capacity_blocked);
    assert!(
        service
            .pipeline
            .runtime
            .mutate(|coordinator| coordinator.contains_hash(&recovered_tx.hash())),
        "maintenance must transfer the candidate to the coordinator"
    );
    let tx_pool = service.pool.tx_pool.read().await;
    assert!(
        !tx_pool.conflict_cache.contains(&recovered_id),
        "cache ownership must end at coordinator admission"
    );
    assert_eq!(tx_pool.conflict_recovery_len(), 0);
}

/// A successful replacement must not re-enqueue the descendants it
/// removed: they are dead as a cluster (their ancestry is destroyed),
/// not merely blocked. They stay in the conflict cache as rejected
/// candidates (the audit/RPC view); only third-party txs blocked by the
/// removed cluster may be recovered. Re-enqueueing them would flip their
/// recorded `RBFRejected` status to a bogus `Pending` (pipeline) and
/// eventually to a misleading `Resolve Unknown`.
#[tokio::test]
async fn successful_replacement_does_not_recover_removed_descendants() {
    use super::harness::{WorkerSet, harness};

    let service = harness(2)
        .rbf(true)
        .workers(WorkerSet::None)
        .build()
        .service;

    // Chain T1 -> T2 in the pool.
    let t1 = build_tx(vec![(&Byte32::new([61u8; 32]), 0)], 1);
    let t2 = build_tx(vec![(&t1.hash(), 0)], 1);
    let t1_id = t1.proposal_short_id();
    let t2_id = t2.proposal_short_id();
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        for (tx, status) in [
            (t1.clone(), Status::Proposed),
            (t2.clone(), Status::Pending),
        ] {
            tx_pool
                .pool_map
                .add_entry(
                    TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1), MOCK_SIZE),
                    status,
                )
                .unwrap();
        }
    }

    // R replaces T1 (same input, different outputs -> different hash,
    // much higher fee).
    let replacement = build_tx(vec![(&Byte32::new([61u8; 32]), 0)], 2);
    let replacement_entry = TxEntry::dummy_resolve(
        replacement,
        MOCK_CYCLES,
        Capacity::shannons(1_000_000_000),
        MOCK_SIZE,
    );
    let replacement_id = replacement_entry.proposal_short_id();

    let (outcome, assembler_statuses) = {
        let mut tx_pool = service.pool.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();
        let pre_resolve_tip = snapshot.tip_hash();
        let coordinated = service.try_submit_entry_coordinated(
            &mut tx_pool,
            snapshot,
            pre_resolve_tip,
            replacement_entry,
            Status::Pending,
            replacement_id,
        );
        let statuses = coordinated.block_assembler_statuses();
        (coordinated.outcome, statuses)
    };
    assert_eq!(
        assembler_statuses,
        std::collections::HashSet::from([Status::Pending, Status::Proposed]),
        "template refresh must include both the new Pending entry and the removed Proposed root"
    );
    let crate::process::submit::rbf_commit::SubmitEntryOutcome {
        result,
        replaced: _,
        rolled_back,
        reject_events: _events,
        accept_event: _,
    } = outcome;

    assert!(result.is_ok(), "replacement must commit: {:?}", result);
    assert!(rolled_back.is_empty());
    // The whole removed cluster stays in the conflict cache as rejected
    // candidates (the audit/RPC view, see `RbfReplaceProposedSuccess`).
    let tx_pool = service.pool.tx_pool.read().await;
    assert!(
        tx_pool.conflict_cache.contains(&t1_id),
        "the replaced root must stay in the conflict cache"
    );
    assert!(
        tx_pool.conflict_cache.contains(&t2_id),
        "the removed descendant must stay in the conflict cache"
    );
}
