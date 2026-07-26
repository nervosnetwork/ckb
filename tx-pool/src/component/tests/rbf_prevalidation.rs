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
    component::{
        entry::TxEntry,
        pool_map::{PoolMap, Status},
    },
    error::Reject,
};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, DepType, TransactionBuilder, cell::CellMeta},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint, ProposalShortId},
    prelude::{Builder, Entity, Pack},
};
use std::sync::Arc;

use super::pipeline::service_with_rbf;
use super::util::{MOCK_CYCLES, MOCK_SIZE, build_tx, build_tx_with_dep};

/// The pool accepts a chain link `a_i` (spending `a_{i-1}:0`) while
/// `i + 1 <= max_ancestors_count`, so the deepest addable chain is exactly
/// `max_ancestors_count` transactions long.
const CHAIN_LEN: u32 = 125;

/// Build the exact Pending ancestor shape used by the RBF capacity tests.
/// Fee and size remain explicit at every call site because they are part of
/// the attack construction, rather than incidental fixture defaults.
fn add_pending_chain(pool: &mut PoolMap, len: u32, fee: Capacity, size: usize) -> (Byte32, Byte32) {
    let mut tip = Byte32::zero();
    let mut root = None;
    for _ in 0..len {
        let link = build_tx(vec![(&tip, 0)], 1);
        tip = link.hash();
        root.get_or_insert_with(|| tip.clone());
        pool.add_entry(
            TxEntry::dummy_resolve(link, MOCK_CYCLES, fee, size),
            Status::Pending,
        )
        .unwrap();
    }
    (root.expect("test chain must be non-empty"), tip)
}

#[tokio::test]
async fn rbf_rejects_dep_group_member_from_replacement_victim() {
    use super::harness::{WorkerSet, harness};
    use ckb_types::core::cell::ResolvedTransaction;

    let service = harness(1)
        .rbf(true)
        .workers(WorkerSet::None)
        .build()
        .service;
    let victim = build_tx(vec![(&Byte32::new([0x61; 32]), 0)], 1);
    let victim_entry = TxEntry::dummy_resolve(
        victim.clone(),
        MOCK_CYCLES,
        Capacity::shannons(1),
        MOCK_SIZE,
    );
    // The raw dep points only at an unrelated dep-group cell. Its verified
    // expansion, however, consumes the victim's output. Checking raw deps
    // alone would remove the victim and admit a transaction whose resolved
    // dependency no longer exists in the accepted graph.
    let dep_group = OutPoint::new(Byte32::new([0x62; 32]), 0);
    let replacement = TransactionBuilder::default()
        .input(CellInput::new(victim.input_pts_iter().next().unwrap(), 0))
        .cell_dep(
            CellDep::new_builder()
                .out_point(dep_group.clone())
                .dep_type(DepType::DepGroup)
                .build(),
        )
        .output(CellOutput::new_builder().build())
        .output_data(Bytes::new().pack())
        .build();
    let cell_meta = |out_point: OutPoint| CellMeta {
        cell_output: CellOutput::new_builder().build(),
        out_point,
        transaction_info: None,
        data_bytes: 0,
        mem_cell_data: None,
        mem_cell_data_hash: None,
    };
    let replacement_entry = TxEntry::new(
        Arc::new(ResolvedTransaction {
            transaction: replacement,
            resolved_cell_deps: vec![cell_meta(OutPoint::new(victim.hash(), 0))],
            resolved_inputs: Vec::new(),
            resolved_dep_groups: vec![cell_meta(dep_group)],
        }),
        MOCK_CYCLES,
        Capacity::shannons(1_000_000),
        MOCK_SIZE,
    );

    let mut tx_pool = service.pool.tx_pool.write().await;
    tx_pool
        .pool_map
        .add_entry(victim_entry, Status::Pending)
        .unwrap();
    let conflicted = tx_pool.get_pool_entry(&victim.proposal_short_id()).unwrap();
    assert!(matches!(
        tx_pool.check_rbf_no_conflict_cell_deps(&[conflicted], &replacement_entry),
        Err(Reject::RBFRejected(_))
    ));
}

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

    let tx = TransactionBuilder::default().build();
    let entry = TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1), MOCK_SIZE);
    let id = entry.proposal_short_id();
    let effect_bound = service.max_submit_effect_bytes();
    let outcome = {
        let mut guard = service.pool.tx_pool.write().await;
        let snapshot = guard.cloned_snapshot();
        service
            .relay
            .effects
            .try_apply_bounded(
                effect_bound,
                crate::service::effects::EffectClass::Trusted,
                || {
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
                    let batch = service.prepare_submit_effects(&mut outcome, Vec::new());
                    (outcome, batch)
                },
            )
            .unwrap()
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
    let chain_tip;
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        let (_, tip) = add_pending_chain(
            &mut tx_pool.pool_map,
            CHAIN_LEN,
            Capacity::zero(),
            MOCK_SIZE,
        );
        chain_tip = tip;
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
        reject_events,
        accept_event: _,
        ..
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
    assert!(result.is_err(), "the invalid replacement must be rejected");
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
    use crate::component::pool_map::ConflictClosure;
    use std::collections::HashSet;

    let mut pool = PoolMap::new(super::util::DEFAULT_MAX_ANCESTORS_COUNT);
    // A chain one link longer than the candidate limit (depth stays within
    // the pool's ancestor limit).
    let chain_len = 101u32;
    let (root_hash, tip_hash) =
        add_pending_chain(&mut pool, chain_len, Capacity::zero(), MOCK_SIZE);
    let root = ProposalShortId::from_tx_hash(&root_hash);
    let tip = ProposalShortId::from_tx_hash(&tip_hash);
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
            assert_eq!(removal.first(), Some(&tip));
            assert_eq!(removal.last(), Some(&root));
        }
        ConflictClosure::Exceeded { .. } => panic!("closure must complete within the limit"),
    }
}

/// A candidate selected for virtual self-eviction is rejected without
/// touching unrelated cell-dep readers.
#[tokio::test]
async fn self_eviction_plan_leaves_cell_dep_readers_untouched() {
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

    // Funding output X that N spends as an input while E cell-deps on it.
    let x_hash = Byte32::new([9u8; 32]);

    // A 124-link chain (the deepest the pool accepts). E is intentionally
    // unrelated to that causal chain despite reading an input consumed by N.
    let chain_tip;
    let e = build_tx_with_dep(vec![(&Byte32::zero(), 2)], vec![(&x_hash, 0)], 1);
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        let (_, tip) =
            add_pending_chain(&mut tx_pool.pool_map, 124, Capacity::shannons(1_000), 100);
        chain_tip = tip;
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(e.clone(), MOCK_CYCLES, Capacity::shannons(1_000), 100),
                Status::Pending,
            )
            .unwrap();
        assert_eq!(tx_pool.pool_map.stats.total_tx_size, 12_500);
    }

    // N: spends X, cell-deps on the chain tip, pays no fee.
    let n = build_tx_with_dep(vec![(&x_hash, 0)], vec![(&chain_tip, 0)], 1);
    let n_id = n.proposal_short_id();
    let n_entry = TxEntry::dummy_resolve(n.clone(), MOCK_CYCLES, Capacity::zero(), 100);

    let crate::process::submit::rbf_commit::SubmitEntryOutcome {
        result,
        reject_events,
        accept_event: _,
        ..
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

    // Plan sees N lose the capacity policy and rejects before Apply.
    assert!(
        result.is_err(),
        "the zero-fee entry must self-evict under the pool limit"
    );
    assert!(
        service
            .pool
            .tx_pool
            .read()
            .await
            .pool_map
            .contains_key(&e.proposal_short_id()),
        "the reader never leaves the accepted pool"
    );
    assert!(
        !reject_events
            .iter()
            .any(|(entry, _)| entry.transaction().hash() == e.hash()),
        "E's Invalidated reject event must be suppressed once it is recovered"
    );
}

/// A size-limit plan that eventually selects the candidate leaves every prior
/// member and proposal-window status unchanged.
#[tokio::test]
async fn failed_size_plan_is_mutation_free_with_original_statuses() {
    use super::harness::{WorkerSet, harness};
    use std::sync::Arc;

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
    // reject before changing either prior entry.
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
        assert_eq!(tx_pool.pool_map.stats.total_tx_size, 200);
        outcome
    };

    assert!(outcome.reject_events.is_empty());
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
    let (service, _relay, _cancel, _store, out_points) = service_with_rbf(2);

    // Chain T1 -> T2 in the pool.
    let funding = out_points[0].clone();
    let t1 = TransactionBuilder::default()
        .input(CellInput::new(funding.clone(), 0))
        .output(CellOutput::new_builder().build())
        .output_data(Bytes::new().pack())
        .build();
    let t2 = build_tx(vec![(&t1.hash(), 0)], 1);
    let t1_hash = t1.hash();
    let t2_hash = t2.hash();
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
    let replacement = TransactionBuilder::default()
        .input(CellInput::new(funding, 0))
        .output(CellOutput::new_builder().build())
        .output(CellOutput::new_builder().build())
        .output_data(Bytes::new().pack())
        .output_data(Bytes::new().pack())
        .build();
    let replacement_entry = TxEntry::dummy_resolve(
        replacement,
        MOCK_CYCLES,
        Capacity::shannons(1_000_000_000),
        MOCK_SIZE,
    );
    let (outcome, assembler_statuses) = {
        let mut tx_pool = service.pool.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();
        let pre_resolve_tip = snapshot.tip_hash();
        let (outcome, _) = service.pipeline.kernel.mutate(|kernel| {
            service.try_submit_entry_with_handoff(
                &mut tx_pool,
                snapshot,
                pre_resolve_tip,
                replacement_entry.clone(),
                |tx_pool, plan| {
                    service.settle_kernel_for_pool_plan(kernel, tx_pool, &replacement_entry, plan)
                },
            )
        });
        let statuses = outcome.block_assembler_statuses();
        (outcome, statuses)
    };
    assert_eq!(
        assembler_statuses,
        std::collections::HashSet::from([Status::Pending, Status::Proposed]),
        "template refresh must include both the new Pending entry and the removed Proposed root"
    );
    let crate::process::submit::rbf_commit::SubmitEntryOutcome {
        result,
        reject_events: _events,
        accept_event: _,
        ..
    } = outcome;

    assert!(result.is_ok(), "replacement must commit: {:?}", result);
    // The whole removed cluster stays in the conflict cache as rejected
    // candidates (the audit/RPC view, see `RbfReplaceProposedSuccess`).
    let _tx_pool = service.pool.tx_pool.read().await;
    let conflicts = service
        .pipeline
        .kernel
        .read(crate::component::pre_pool::PrePoolKernel::conflict_hashes);
    assert!(
        conflicts.contains(&t1_hash),
        "the replaced root must stay in conflict history"
    );
    assert!(
        conflicts.contains(&t2_hash),
        "the removed descendant must stay in conflict history"
    );
    assert!(matches!(
        service.find_tx_in_coordinator_hash(&t1_hash),
        Some(crate::service::PipelineTxLocation::ConflictHistory)
    ));
    assert!(matches!(
        service.find_tx_in_coordinator_hash(&t2_hash),
        Some(crate::service::PipelineTxLocation::ConflictHistory)
    ));
}
