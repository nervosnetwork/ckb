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
    let outcome = {
        let mut guard = service.pool.tx_pool.write().await;
        let snapshot = guard.cloned_snapshot();
        let outcome = service.try_submit_entry(
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
        outcome
    };
    service
        .dispatch_submit_aftermath(&id, outcome)
        .await
        .unwrap();
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
        recovered,
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
    assert!(recovered.is_empty());
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
        recovered: _,
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
        recovered: _,
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
        recovered,
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
    let recovered_hashes: HashSet<_> = rolled_back
        .iter()
        .map(|(tx, _)| tx.hash())
        .chain(recovered.iter().map(|(tx, _)| tx.hash()))
        .collect();
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

    // The recovered txs were taken out of the waiting room: ownership
    // moved to the recovery set.
    let tx_pool = service.pool.tx_pool.read().await;
    for tx in [&parent, &child] {
        assert!(
            tx_pool.waiting_room.get(&tx.proposal_short_id()).is_none(),
            "recovered tx must not stay in the waiting room"
        );
    }
}

/// Hold-and-restore: a displaced candidate must not be rejected when its
/// (unverified) displacer leaves the pipeline — it is restored to the
/// verify queue with no recent-reject side effects. Only a *committed*
/// displacer makes the displacement real (finalize rejects the loser).
#[tokio::test]
async fn displaced_candidate_is_restored_unless_displacer_commits() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_queue::PipelineQueue;
    use crate::resolved_tx::ResolvedTx;
    use crate::tx_source::TxSource;
    use ckb_types::core::cell::ResolvedTransaction;
    use ckb_types::packed::OutPoint;
    use std::sync::Arc;

    let service = harness(2)
        .rbf(true)
        .workers(WorkerSet::None)
        .build()
        .service;

    // In-pool original spending X: the conflict target for every candidate.
    let x_hash = Byte32::new([11u8; 32]);
    let original = build_tx(vec![(&x_hash, 0)], 1);
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(
                    original.clone(),
                    MOCK_CYCLES,
                    Capacity::shannons(1),
                    MOCK_SIZE,
                ),
                Status::Pending,
            )
            .unwrap();
    }
    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();

    let mk_candidate = |outputs_len: usize, fee: u64, peer: u64| {
        let tx = build_tx(vec![(&x_hash, 0)], outputs_len);
        let source = TxSource::Remote {
            cycles: 0,
            peer: (peer as usize).into(),
        };
        let resolved = ResolvedTx {
            tx: tx.clone(),
            rtx: Arc::new(ResolvedTransaction::dummy_resolve(tx.clone())),
            status: Status::Pending,
            fee: Capacity::shannons(fee),
            tx_size: tx.data().serialized_size_in_block(),
            pre_resolve_tip: Default::default(),
            snapshot: Arc::clone(&snapshot),
            source,
            resident_permit: None,
        };
        (tx, source, resolved)
    };

    let (tx_a, source_a, resolved_a) = mk_candidate(1, 1_000, 1);
    let (tx_b, source_b, resolved_b) = mk_candidate(2, 10_000, 2);
    let (tx_c, source_c, resolved_c) = mk_candidate(3, 100_000, 3);
    let id_a = tx_a.proposal_short_id();
    let id_b = tx_b.proposal_short_id();
    let id_c = tx_c.proposal_short_id();

    // A registers and enters the verify queue.
    assert!(
        service
            .register_rbf_candidate(
                tx_a,
                source_a,
                &resolved_a,
                resolved_a.fee,
                resolved_a.tx_size
            )
            .await
            .unwrap()
    );
    assert!(
        service
            .pipeline
            .queues
            .verify_queue
            .read()
            .await
            .contains_key(&id_a)
    );

    // B (higher fee rate) registers: A leaves the verify queue but is held
    // by B's registration, not rejected.
    assert!(
        service
            .register_rbf_candidate(
                tx_b,
                source_b,
                &resolved_b,
                resolved_b.fee,
                resolved_b.tx_size
            )
            .await
            .unwrap()
    );
    {
        let verify = service.pipeline.queues.verify_queue.read().await;
        assert!(!verify.contains_key(&id_a), "A must be displaced");
        assert!(verify.contains_key(&id_b));
        assert_eq!(
            verify.resident_usage(),
            (resolved_a.tx_size + resolved_b.tx_size, 2),
            "RaceLost must remain charged to the resolved residency budget"
        );
    }

    // While A is held by B's registration, pipeline queries must still see
    // it as in flight (not as Unknown): it may be restored at any moment.
    {
        let room = service.pipeline.waiting_room.read().await;
        assert!(room.find_held(&id_a).is_some());
    }
    let location = service.find_tx_in_pipeline(&id_a).await;
    assert!(
        matches!(
            location,
            Some(crate::service::PipelineTxLocation::Verifying { .. })
        ),
        "held candidate must be reported as in the verify stage"
    );

    // B is popped by a verify worker and fails verification: aborting its
    // registration must restore A (and B stays gone).
    {
        let mut verify = service.pipeline.queues.verify_queue.write().await;
        let popped = verify.pop_front(false).expect("B is queued");
        assert_eq!(popped.tx.proposal_short_id(), id_b);
    }
    service.abort_rbf_candidate(&id_b).await;
    {
        let verify = service.pipeline.queues.verify_queue.read().await;
        assert!(
            verify.contains_key(&id_a),
            "displaced candidate must be restored when the displacer aborts"
        );
        assert!(!verify.contains_key(&id_b));
    }

    // C (even higher fee rate) displaces A again; committing C makes A's
    // rejection real, so A is *not* restored this time.
    assert!(
        service
            .register_rbf_candidate(
                tx_c,
                source_c,
                &resolved_c,
                resolved_c.fee,
                resolved_c.tx_size
            )
            .await
            .unwrap()
    );
    service.finalize_rbf_candidate(&id_c).await;
    {
        let verify = service.pipeline.queues.verify_queue.read().await;
        assert!(
            !verify.contains_key(&id_a),
            "finalized displacement must not restore the loser"
        );
        assert!(verify.contains_key(&id_c));
    }

    // Finalizing removed C's registration: nothing blocks candidates for X
    // anymore, no matter how high their fee rate.
    let rbf = service.pipeline.queues.rbf_candidates.read().await;
    assert!(!rbf.is_superseded(
        &id_a,
        ckb_types::core::FeeRate::from_u64(u64::MAX),
        &[OutPoint::new(x_hash, 0)],
    ));
}

/// Superseded at submit: the candidate is *held* by the winner's
/// registration instead of being rejected — restored if the winner aborts,
/// really rejected only if the winner commits. This closes the residual
/// censorship window for candidates that were already active
/// (mid-verification) when a stronger candidate appeared.
#[tokio::test]
async fn superseded_at_submit_is_held_then_restored_on_winner_abort() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_queue::PipelineQueue;
    use crate::resolved_tx::ResolvedTx;
    use crate::tx_source::TxSource;
    use ckb_types::core::cell::ResolvedTransaction;
    use ckb_types::packed::OutPoint;
    use std::sync::Arc;

    let service = harness(2)
        .rbf(true)
        .workers(WorkerSet::None)
        .build()
        .service;

    // In-pool original spending X: the conflict target for every candidate.
    let x_hash = Byte32::new([17u8; 32]);
    let original = build_tx(vec![(&x_hash, 0)], 1);
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(
                    original.clone(),
                    MOCK_CYCLES,
                    Capacity::shannons(1),
                    MOCK_SIZE,
                ),
                Status::Pending,
            )
            .unwrap();
    }
    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();

    let mk_candidate = |outputs_len: usize, fee: u64, peer: u64| {
        let tx = build_tx(vec![(&x_hash, 0)], outputs_len);
        let source = TxSource::Remote {
            cycles: 0,
            peer: (peer as usize).into(),
        };
        let resolved = ResolvedTx {
            tx: tx.clone(),
            rtx: Arc::new(ResolvedTransaction::dummy_resolve(tx.clone())),
            status: Status::Pending,
            fee: Capacity::shannons(fee),
            tx_size: tx.data().serialized_size_in_block(),
            pre_resolve_tip: Default::default(),
            snapshot: Arc::clone(&snapshot),
            source,
            resident_permit: None,
        };
        (tx, source, resolved)
    };

    let (tx_d, source_d, resolved_d) = mk_candidate(1, 1_000, 1);
    let (tx_w, source_w, resolved_w) = mk_candidate(2, 10_000, 2);
    let id_d = tx_d.proposal_short_id();
    let id_w = tx_w.proposal_short_id();

    // D registers and enters the verify queue, then a worker pops it (it is
    // now active, mid-verification).
    assert!(
        service
            .register_rbf_candidate(
                tx_d,
                source_d,
                &resolved_d,
                resolved_d.fee,
                resolved_d.tx_size
            )
            .await
            .unwrap()
    );
    {
        let mut verify = service.pipeline.queues.verify_queue.write().await;
        assert_eq!(
            verify.pop_front(false).map(|r| r.tx.proposal_short_id()),
            Some(id_d.clone())
        );
    }

    // W registers with a higher fee rate: D is active, so it cannot be held
    // at register time (its registration is removed, but its job is not).
    assert!(
        service
            .register_rbf_candidate(
                tx_w,
                source_w,
                &resolved_w,
                resolved_w.fee,
                resolved_w.tx_size
            )
            .await
            .unwrap()
    );

    // D reaches submit while W is still in flight: superseded → held by
    // W's registration, not rejected.
    let result = service.submit_entry(resolved_d, MOCK_CYCLES).await.unwrap();
    assert!(matches!(
        result,
        crate::process::submit::SubmitEntryResult::Superseded
    ));
    {
        let room = service.pipeline.waiting_room.read().await;
        assert!(
            room.find_held(&id_d).is_some(),
            "D must be held by W's registration"
        );
    }

    // W aborts (e.g. fails verification): D is restored — queued and
    // re-registered, not rejected.
    service.abort_rbf_candidate(&id_w).await;
    {
        let verify = service.pipeline.queues.verify_queue.read().await;
        assert!(
            verify.contains_key(&id_d),
            "D must be restored to the verify queue after W aborts"
        );
        let rbf = service.pipeline.queues.rbf_candidates.read().await;
        assert!(
            rbf.is_superseded(
                &id_w,
                ckb_types::core::FeeRate::from_u64(1),
                &[OutPoint::new(x_hash, 0)],
            ),
            "D must be re-registered for the conflict input after restore"
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

/// Double-park guard: a superseded-at-submit candidate that *also* flows
/// through `after_process` (the RPC/direct path flattens the hold into
/// `RBFRejected`) must not be parked a second time in the pool-side waiting
/// room — its fate follows the winner's registration, and a second parked
/// copy would race the hold-and-restore machinery.
#[tokio::test]
async fn superseded_candidate_is_not_double_parked_by_after_process() {
    use super::harness::{WorkerSet, harness};
    use crate::resolved_tx::ResolvedTx;
    use crate::tx_source::TxSource;
    use ckb_types::core::cell::ResolvedTransaction;
    use std::sync::Arc;

    let service = harness(2)
        .rbf(true)
        .workers(WorkerSet::None)
        .build()
        .service;

    // In-pool original spending X: the conflict target for both candidates.
    let x_hash = Byte32::new([23u8; 32]);
    let original = build_tx(vec![(&x_hash, 0)], 1);
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(
                    original.clone(),
                    MOCK_CYCLES,
                    Capacity::shannons(1),
                    MOCK_SIZE,
                ),
                Status::Pending,
            )
            .unwrap();
    }
    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();

    let mk_candidate = |outputs_len: usize, fee: u64, peer: u64| {
        let tx = build_tx(vec![(&x_hash, 0)], outputs_len);
        let source = TxSource::Remote {
            cycles: 0,
            peer: (peer as usize).into(),
        };
        let resolved = ResolvedTx {
            tx: tx.clone(),
            rtx: Arc::new(ResolvedTransaction::dummy_resolve(tx.clone())),
            status: Status::Pending,
            fee: Capacity::shannons(fee),
            tx_size: tx.data().serialized_size_in_block(),
            pre_resolve_tip: Default::default(),
            snapshot: Arc::clone(&snapshot),
            source,
            resident_permit: None,
        };
        (tx, source, resolved)
    };

    let (tx_d, source_d, resolved_d) = mk_candidate(1, 1_000, 1);
    let (tx_w, source_w, resolved_w) = mk_candidate(2, 10_000, 2);
    let id_d = tx_d.proposal_short_id();

    // D registers and is popped (active); W registers stronger. D reaches
    // submit while W is in flight and is held as W's `RaceLost`.
    assert!(
        service
            .register_rbf_candidate(
                tx_d.clone(),
                source_d,
                &resolved_d,
                resolved_d.fee,
                resolved_d.tx_size
            )
            .await
            .unwrap()
    );
    {
        let mut verify = service.pipeline.queues.verify_queue.write().await;
        assert!(verify.pop_front(false).is_some());
    }
    assert!(
        service
            .register_rbf_candidate(
                tx_w,
                source_w,
                &resolved_w,
                resolved_w.fee,
                resolved_w.tx_size
            )
            .await
            .unwrap()
    );
    let result = service.submit_entry(resolved_d, MOCK_CYCLES).await.unwrap();
    assert!(matches!(
        result,
        crate::process::submit::SubmitEntryResult::Superseded
    ));
    {
        let room = service.pipeline.waiting_room.read().await;
        assert!(room.find_held(&id_d).is_some(), "D must be held by W");
    }

    // The RPC/direct path flattens the hold into `RBFRejected` and runs
    // after_process — with D's conflict (the in-pool original) still live.
    let ret = Err(Reject::RBFRejected("superseded".to_string()));
    service.after_process(tx_d.clone(), source_d, &ret).await;

    // Pre-fix, D was also parked pool-side (InputsBlocked): two parked
    // copies of the same tx raced the hold-and-restore machinery.
    let tx_pool = service.pool.tx_pool.read().await;
    assert!(
        tx_pool.waiting_room.get(&id_d).is_none(),
        "a held candidate must not be double-parked into the pool-side waiting room"
    );
}

/// A winner that commits *without replacing anything* (its conflicts
/// vanished between registration and submit) must abort — restoring the
/// candidates it displaced — not finalize-reject them.
#[tokio::test]
async fn winner_committing_without_replacement_restores_displaced() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_queue::PipelineQueue;
    use crate::resolved_tx::ResolvedTx;
    use crate::tx_source::TxSource;
    use ckb_types::core::cell::ResolvedTransaction;
    use ckb_types::core::{TransactionBuilder, TransactionView};
    use ckb_types::packed::{CellInput, CellOutput};
    use ckb_types::prelude::*;
    use std::sync::Arc;

    // A real genesis funding cell: the winner's submit re-resolves against
    // the chain (`check_rtx`/`time_relative_verify` run on tip change), so
    // the conflict input must actually exist.
    let h = harness(1).rbf(true).workers(WorkerSet::None).build();
    let service = h.service;
    let funding = h.out_points[0].clone();

    let mk_tx = |outputs_len: usize| -> TransactionView {
        TransactionBuilder::default()
            .input(CellInput::new(funding.clone(), 0))
            .outputs((0..outputs_len).map(|i| {
                CellOutput::new_builder()
                    .capacity(Capacity::bytes(i + 1).unwrap())
                    .build()
            }))
            .outputs_data((0..outputs_len).map(|_| ckb_types::packed::Bytes::default()))
            .build()
    };

    // In-pool original spending the funding cell: the conflict target.
    let original = mk_tx(1);
    let original_id = original.proposal_short_id();
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(
                    original.clone(),
                    MOCK_CYCLES,
                    Capacity::shannons(1),
                    MOCK_SIZE,
                ),
                Status::Pending,
            )
            .unwrap();
    }
    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();

    let mk_candidate = |tx: TransactionView, fee: u64, peer: u64| {
        let source = TxSource::Remote {
            cycles: 0,
            peer: (peer as usize).into(),
        };
        let resolved = ResolvedTx {
            tx: tx.clone(),
            rtx: Arc::new(ResolvedTransaction::dummy_resolve(tx.clone())),
            status: Status::Pending,
            fee: Capacity::shannons(fee),
            tx_size: tx.data().serialized_size_in_block(),
            pre_resolve_tip: Default::default(),
            snapshot: Arc::clone(&snapshot),
            source,
            resident_permit: None,
        };
        (tx, source, resolved)
    };

    let (tx_a, source_a, resolved_a) = mk_candidate(mk_tx(2), 1_000, 1);
    let (tx_b, source_b, resolved_b) = mk_candidate(mk_tx(3), 10_000, 2);
    let id_a = tx_a.proposal_short_id();
    let id_b = tx_b.proposal_short_id();

    // A registers; B (stronger) registers and displaces A into the hold.
    assert!(
        service
            .register_rbf_candidate(
                tx_a,
                source_a,
                &resolved_a,
                resolved_a.fee,
                resolved_a.tx_size
            )
            .await
            .unwrap()
    );
    assert!(
        service
            .register_rbf_candidate(
                tx_b,
                source_b,
                &resolved_b,
                resolved_b.fee,
                resolved_b.tx_size
            )
            .await
            .unwrap()
    );
    {
        let room = service.pipeline.waiting_room.read().await;
        assert!(room.find_held(&id_a).is_some(), "A must be held by B");
    }

    // The conflict vanishes before B commits (third-party removal).
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool.pool_map.remove_entry(&original_id);
    }

    // B commits with nothing to replace: A must be restored (abort), not
    // really rejected (finalize).
    let result = service.submit_entry(resolved_b, MOCK_CYCLES).await.unwrap();
    assert!(matches!(
        result,
        crate::process::submit::SubmitEntryResult::Committed
    ));
    {
        let verify = service.pipeline.queues.verify_queue.read().await;
        assert!(
            verify.contains_key(&id_a),
            "displaced candidate must be restored when the winner replaced nothing"
        );
    }
    // A's registration resumed for the conflict input (B now consumes it).
    let rbf = service.pipeline.queues.rbf_candidates.read().await;
    assert!(
        rbf.is_superseded(&id_b, ckb_types::core::FeeRate::from_u64(1), &[funding],),
        "A must be re-registered for the conflict input after the abort"
    );
}

/// On a *successful* commit the recovered txs stay in the conflict cache
/// while they are re-enqueued: the cache is their durable home until they
/// reach a terminal state (re-committed or terminally rejected). Pulling
/// them out at commit time would make the whole removed cluster vanish
/// from the conflicts view the moment the replacement lands (the
/// `RbfReplaceProposedSuccess` integration spec asserts exactly that
/// visibility).
#[tokio::test]
async fn successful_commit_keeps_recovered_txs_in_conflict_cache() {
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
        recovered,
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
    assert!(
        recovered
            .iter()
            .any(|(tx, _)| tx.hash() == recovered_tx.hash()),
        "the conflict-cached tx must be recovered once its input is freed"
    );
    let tx_pool = service.pool.tx_pool.read().await;
    assert!(
        tx_pool.waiting_room.get(&recovered_id).is_some(),
        "a recovered tx stays in the conflict cache until its own terminal state"
    );
}

/// A winner whose transaction commits *on-chain* (block attachment) makes
/// its displacement real: the candidates it held are really rejected
/// (finalize — relayed, not recorded), not restored.
#[tokio::test]
async fn attached_winner_finalizes_held_candidates() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_queue::PipelineQueue;
    use crate::resolved_tx::ResolvedTx;
    use crate::tx_source::TxSource;
    use ckb_types::core::cell::ResolvedTransaction;
    use ckb_types::packed::OutPoint;
    use std::sync::Arc;

    let h = harness(2).rbf(true).workers(WorkerSet::None).build();
    let service = h.service;
    let relay_rx = h.relay_rx;

    // In-pool original spending X: the conflict target for both candidates.
    let x_hash = Byte32::new([43u8; 32]);
    let original = build_tx(vec![(&x_hash, 0)], 1);
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(
                    original.clone(),
                    MOCK_CYCLES,
                    Capacity::shannons(1),
                    MOCK_SIZE,
                ),
                Status::Pending,
            )
            .unwrap();
    }
    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();

    let mk_candidate = |outputs_len: usize, fee: u64, peer: u64| {
        let tx = build_tx(vec![(&x_hash, 0)], outputs_len);
        let source = TxSource::Remote {
            cycles: 0,
            peer: (peer as usize).into(),
        };
        let resolved = ResolvedTx {
            tx: tx.clone(),
            rtx: Arc::new(ResolvedTransaction::dummy_resolve(tx.clone())),
            status: Status::Pending,
            fee: Capacity::shannons(fee),
            tx_size: tx.data().serialized_size_in_block(),
            pre_resolve_tip: Default::default(),
            snapshot: Arc::clone(&snapshot),
            source,
            resident_permit: None,
        };
        (tx, source, resolved)
    };

    let (tx_l, source_l, resolved_l) = mk_candidate(1, 1_000, 1);
    let (tx_a, source_a, resolved_a) = mk_candidate(2, 10_000, 2);
    let id_l = tx_l.proposal_short_id();
    let id_a = tx_a.proposal_short_id();

    // L registers; A (stronger) registers and displaces L into the hold.
    assert!(
        service
            .register_rbf_candidate(
                tx_l.clone(),
                source_l,
                &resolved_l,
                resolved_l.fee,
                resolved_l.tx_size
            )
            .await
            .unwrap()
    );
    assert!(
        service
            .register_rbf_candidate(
                tx_a,
                source_a,
                &resolved_a,
                resolved_a.fee,
                resolved_a.tx_size
            )
            .await
            .unwrap()
    );
    {
        let room = service.pipeline.waiting_room.read().await;
        assert!(room.find_held(&id_l).is_some(), "L must be held by A");
    }

    // A commits on-chain (block attachment).
    service
        .remove_attached_from_pipeline(std::slice::from_ref(&id_a))
        .await;

    // A left the verify queue; L was really rejected — not restored.
    {
        let verify = service.pipeline.queues.verify_queue.read().await;
        assert!(!verify.contains_key(&id_a), "attached winner must leave");
        assert!(
            !verify.contains_key(&id_l),
            "a finalized loser must not be restored"
        );
    }
    // A's registration is gone: nothing blocks candidates for X anymore.
    let rbf = service.pipeline.queues.rbf_candidates.read().await;
    assert!(
        !rbf.is_superseded(
            &id_l,
            ckb_types::core::FeeRate::from_u64(u64::MAX),
            &[OutPoint::new(x_hash, 0)],
        ),
        "the attached winner's registration must be removed"
    );
    drop(rbf);

    // The loser's rejection is real and relayed (RBFRejected is exempt from
    // recent_reject recording, so the relayer notification is the only
    // terminal signal).
    let hash_l = tx_l.hash();
    let notified = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(crate::service::TxVerificationResult::Reject { tx_hash }) =
                relay_rx.try_recv()
                && tx_hash == hash_l
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        notified.is_ok(),
        "a finalized loser must be relayed as rejected"
    );
}

/// An expired `RaceLost` entry must not loop or be censored by an unverified
/// winner: expiry revokes a still-live speculative registration and restores
/// its losers. If the winner is already gone, the loser is restored directly.
#[tokio::test]
async fn expired_race_lost_revokes_stalled_winner_and_restores_loser() {
    use super::harness::{WorkerSet, harness};
    use crate::component::pipeline_queue::PipelineQueue;
    use crate::resolved_tx::ResolvedTx;
    use crate::tx_source::TxSource;
    use ckb_types::core::cell::ResolvedTransaction;
    use std::sync::Arc;

    let h = harness(2).rbf(true).workers(WorkerSet::None).build();
    let service = h.service;
    let _relay_rx = h.relay_rx;

    let x_hash = Byte32::new([53u8; 32]);
    let original = build_tx(vec![(&x_hash, 0)], 1);
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(
                    original.clone(),
                    MOCK_CYCLES,
                    Capacity::shannons(1),
                    MOCK_SIZE,
                ),
                Status::Pending,
            )
            .unwrap();
    }
    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();

    let mk_candidate = |outputs_len: usize, fee: u64, peer: u64| {
        let tx = build_tx(vec![(&x_hash, 0)], outputs_len);
        let source = TxSource::Remote {
            cycles: 0,
            peer: (peer as usize).into(),
        };
        let resolved = ResolvedTx {
            tx: tx.clone(),
            rtx: Arc::new(ResolvedTransaction::dummy_resolve(tx.clone())),
            status: Status::Pending,
            fee: Capacity::shannons(fee),
            tx_size: tx.data().serialized_size_in_block(),
            pre_resolve_tip: Default::default(),
            snapshot: Arc::clone(&snapshot),
            source,
            resident_permit: None,
        };
        (tx, source, resolved)
    };

    // Scenario 1: the deadline passes while the winner is still in flight —
    // revoke its speculative registration and restore the loser.
    let (tx_l, source_l, resolved_l) = mk_candidate(1, 1_000, 1);
    let (tx_a, source_a, resolved_a) = mk_candidate(2, 10_000, 2);
    let id_l = tx_l.proposal_short_id();
    let id_a = tx_a.proposal_short_id();

    assert!(
        service
            .register_rbf_candidate(
                tx_l.clone(),
                source_l,
                &resolved_l,
                resolved_l.fee,
                resolved_l.tx_size
            )
            .await
            .unwrap()
    );
    assert!(
        service
            .register_rbf_candidate(
                tx_a,
                source_a,
                &resolved_a,
                resolved_a.fee,
                resolved_a.tx_size
            )
            .await
            .unwrap()
    );
    {
        let room = service.pipeline.waiting_room.read().await;
        assert!(room.find_held(&id_l).is_some(), "L must be held by A");
    }
    {
        let mut room = service.pipeline.waiting_room.write().await;
        room.expire_entry_for_test(&id_l);
    }
    // Drive the expiry scan (any wait() runs it) and route the eviction.
    let dummy = build_tx(vec![(&Byte32::new([54u8; 32]), 0)], 1);
    service
        .handle_missing_input_orphan(
            dummy,
            TxSource::Local,
            std::collections::HashSet::from([Byte32::new([54u8; 32])]),
        )
        .await;

    {
        let verify = service.pipeline.queues.verify_queue.read().await;
        assert!(
            verify.contains_key(&id_l),
            "an unverified stalled winner must not censor the loser"
        );
    }
    assert!(
        !service
            .pipeline
            .queues
            .rbf_candidates
            .read()
            .await
            .contains_candidate(&id_a),
        "expiry must revoke the stalled winner's speculative registration"
    );

    // Scenario 2: the winner is gone before the deadline — the loser is
    // restored to the verify queue.
    assert!(matches!(
        service.remove_tx(tx_l.hash()).await,
        crate::service::RemoveTxOutcome::Removed
    ));
    let (tx_l2, source_l2, resolved_l2) = mk_candidate(3, 1_000, 1);
    let (tx_b, source_b, resolved_b) = mk_candidate(4, 10_000, 2);
    let id_l2 = tx_l2.proposal_short_id();
    let id_b = tx_b.proposal_short_id();

    assert!(
        service
            .register_rbf_candidate(
                tx_l2,
                source_l2,
                &resolved_l2,
                resolved_l2.fee,
                resolved_l2.tx_size
            )
            .await
            .unwrap()
    );
    assert!(
        service
            .register_rbf_candidate(
                tx_b,
                source_b,
                &resolved_b,
                resolved_b.fee,
                resolved_b.tx_size
            )
            .await
            .unwrap()
    );
    {
        let mut room = service.pipeline.waiting_room.write().await;
        room.expire_entry_for_test(&id_l2);
    }
    // The winner's registration disappears without waking the loser.
    {
        let mut rbf = service.pipeline.queues.rbf_candidates.write().await;
        rbf.remove(&id_b);
    }
    let dummy2 = build_tx(vec![(&Byte32::new([55u8; 32]), 0)], 1);
    service
        .handle_missing_input_orphan(
            dummy2,
            TxSource::Local,
            std::collections::HashSet::from([Byte32::new([55u8; 32])]),
        )
        .await;

    {
        let verify = service.pipeline.queues.verify_queue.read().await;
        assert!(
            verify.contains_key(&id_l2),
            "must be restored when the winner is gone before the deadline"
        );
    }
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
        for tx in [t1.clone(), t2.clone()] {
            tx_pool
                .pool_map
                .add_entry(
                    TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1), MOCK_SIZE),
                    Status::Pending,
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

    let crate::process::submit::rbf_commit::SubmitEntryOutcome {
        result,
        replaced: _,
        rolled_back,
        recovered,
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
    assert!(
        !recovered
            .iter()
            .any(|(tx, _)| tx.proposal_short_id() == t2_id),
        "a removed descendant must not be re-enqueued on a successful replacement"
    );
    // The whole removed cluster stays in the conflict cache as rejected
    // candidates (the audit/RPC view, see `RbfReplaceProposedSuccess`).
    let tx_pool = service.pool.tx_pool.read().await;
    assert!(
        tx_pool.waiting_room.get(&t1_id).is_some(),
        "the replaced root must stay in the conflict cache"
    );
    assert!(
        tx_pool.waiting_room.get(&t2_id).is_some(),
        "the removed descendant must stay in the conflict cache"
    );
}
