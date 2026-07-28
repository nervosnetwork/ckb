//! Behavioral and recomputing-invariant tests for the concrete pre-pool kernel.
//!
//! These tests intentionally use only the public transition surface. The
//! test-only `audit` independently rebuilds every derived index and charge
//! from primary entries after each transition.

use crate::component::entry::resolved_transaction_charge_bytes;
use crate::component::pool_map::Status;
use crate::component::pre_pool::*;
use crate::resolved_tx::ResolvedTx;
use crate::test_support::build_tx;
use crate::tx_source::TxSource;
use ckb_network::PeerIndex;
use ckb_types::core::{Capacity, TransactionView, cell::ResolvedTransaction};
use ckb_types::packed::{Byte32, OutPoint};
use ckb_types::prelude::*;
use ckb_verification::cache::Completed;
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use std::time::Instant;

fn limits() -> PrePoolLimits {
    PrePoolLimits {
        total: Residency::new(128, 32_000_000),
        remote: Residency::new(96, 24_000_000),
        per_peer: Residency::new(32, 8_000_000),
        conflict_history: Residency::new(16, 4_000_000),
        max_dependencies_per_entry: 64,
        max_dependents_per_parent: 64,
        max_inputs_per_ready: 64,
        max_candidates_per_input: 16,
        max_active_work: 8,
        max_active_work_per_peer: 1,
        entry_overhead: 128,
        dependency_overhead: 32,
        verify_fee_rate_ordering: false,
    }
}

fn reconcile_dependencies(
    kernel: &mut PrePoolKernel,
    unavailable_parents: HashSet<Byte32>,
    available: impl IntoIterator<Item = DependencyKey>,
) -> Result<(), PrePoolError> {
    kernel
        .prepare_dependency_reconciliation(&unavailable_parents, available)?
        .apply();
    Ok(())
}

fn transaction(tag: u8) -> TransactionView {
    let parent = Byte32::new([tag; 32]);
    build_tx(vec![(&parent, 0)], 1)
}

fn with_cached_hash(tx: TransactionView, hash: Byte32) -> TransactionView {
    ckb_types::packed::TransactionView::new_builder()
        .data(tx.data())
        .hash(hash)
        .witness_hash(tx.witness_hash())
        .build()
        .unpack()
}

fn source_peer(source: PrePoolSource) -> PeerIndex {
    match source {
        PrePoolSource::Remote(remote) => remote.peer,
        PrePoolSource::Proposal | PrePoolSource::Recovery => {
            panic!("expected a remote owner")
        }
    }
}

fn admit(
    kernel: &mut PrePoolKernel,
    tx: TransactionView,
    source: TxSource,
    lane: ResolveLane,
    expires_at: Option<u64>,
) -> Result<EntryVersion, PrePoolError> {
    let owner = match source {
        TxSource::Remote { cycles, peer } => PrePoolSource::Remote(RemoteSource::new(peer, cycles)),
        TxSource::Proposal => PrePoolSource::Proposal,
        TxSource::Local => panic!("test admission source is non-local"),
    };
    let raw = PipelineRawTx::new(tx.clone(), source, 1);
    kernel.admit(
        raw,
        lane,
        owner,
        expires_at,
        conflict_dependency_keys(&tx, std::iter::empty()),
    )
}

fn resolved(tx: TransactionView, source: TxSource, fee: u64) -> ResolvedTx {
    let rtx = Arc::new(ResolvedTransaction::dummy_resolve(tx.clone()));
    let tx_size = tx.data().serialized_size_in_block();
    let resident_size = resolved_transaction_charge_bytes(tx_size, &rtx);
    ResolvedTx {
        rtx,
        status: Status::Pending,
        fee: Capacity::shannons(fee),
        tx_size,
        resident_size,
        pre_resolve_tip: Byte32::default(),
        source,
        epoch: 1,
    }
}

fn stage_ready(
    kernel: &mut PrePoolKernel,
    tx: TransactionView,
    source: TxSource,
    expires_at: Option<u64>,
    fee: u64,
) {
    let lane = ResolveLane::Ingress;
    admit(kernel, tx.clone(), source, lane, expires_at).unwrap();
    let raw = kernel.checkout_resolve(lane).unwrap().unwrap();
    let resolved = resolved(tx.clone(), source, fee);
    let resident_size = resolved.resident_size;
    kernel
        .complete_raw(&raw, resolved, resident_size, VerifySchedule::default())
        .unwrap();
    let verify = kernel
        .checkout_verify(WorkCapability::Any)
        .unwrap()
        .unwrap();
    let candidate = (*verify.payload).clone().into_pool_candidate();
    let charge = candidate
        .resident_size
        .checked_add(std::mem::size_of::<PipelineVerifiedTx>())
        .unwrap();
    kernel
        .complete_verify(
            &verify,
            PipelineVerifiedTx {
                candidate,
                completed: Completed {
                    cycles: 0,
                    fee: Capacity::shannons(fee),
                },
                verify_cache_hit: false,
                started_at: Instant::now(),
            },
            charge,
        )
        .unwrap();
}

fn commit_ready(kernel: &mut PrePoolKernel) -> CommitSettlement {
    let mut session = kernel.begin_next_commit().unwrap().unwrap();
    let plan = session
        .plan_ready(&HashSet::new(), Vec::<DependencyKey>::new(), Vec::new())
        .unwrap();
    let settlement = plan.settlement().clone();
    plan.apply();
    settlement
}

#[test]
fn concrete_kernel_transitions_preserve_recomputed_projections() {
    let mut kernel = PrePoolKernel::new(limits());
    let tx = transaction(1);
    admit(
        &mut kernel,
        tx.clone(),
        TxSource::Remote {
            cycles: 0,
            peer: 1.into(),
        },
        ResolveLane::Ingress,
        Some(100),
    )
    .unwrap();
    kernel.audit().unwrap();
    let raw = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    kernel.audit().unwrap();
    let resolved = resolved(
        tx.clone(),
        TxSource::Remote {
            cycles: 0,
            peer: 1.into(),
        },
        2_000,
    );
    let charge = resolved.resident_size;
    kernel
        .complete_raw(&raw, resolved, charge, VerifySchedule::new(2_000, false))
        .unwrap();
    kernel.audit().unwrap();
    let verify = kernel
        .checkout_verify(WorkCapability::Any)
        .unwrap()
        .unwrap();
    kernel.audit().unwrap();
    let candidate = (*verify.payload).clone().into_pool_candidate();
    let candidate_charge = candidate.resident_size + std::mem::size_of::<PipelineVerifiedTx>();
    kernel
        .complete_verify(
            &verify,
            PipelineVerifiedTx {
                candidate,
                completed: Completed {
                    cycles: 1,
                    fee: Capacity::shannons(2_000),
                },
                verify_cache_hit: false,
                started_at: Instant::now(),
            },
            candidate_charge,
        )
        .unwrap();
    kernel.audit().unwrap();
    let mut session = kernel.begin_next_commit().unwrap().unwrap();
    let plan = session
        .plan_failed(ConflictDisposition::Terminalize)
        .unwrap();
    plan.apply();
    assert_eq!(kernel.len(), 0);
    assert_eq!(kernel.total_usage(), Residency::default());
    kernel.audit().unwrap();
}

#[test]
fn verify_completion_derives_indexes_from_the_same_payload() {
    let mut kernel = PrePoolKernel::new(limits());
    let source = TxSource::Remote {
        cycles: 0,
        peer: 1.into(),
    };
    let tx = transaction(1);
    admit(
        &mut kernel,
        tx.clone(),
        source,
        ResolveLane::Ingress,
        Some(100),
    )
    .unwrap();
    let raw = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    let resolved_tx = resolved(tx, source, 2_000);
    let resolved_charge = resolved_tx.resident_size;
    kernel
        .complete_raw(
            &raw,
            resolved_tx,
            resolved_charge,
            VerifySchedule::default(),
        )
        .unwrap();
    let verify = kernel
        .checkout_verify(WorkCapability::Any)
        .unwrap()
        .unwrap();

    let other = resolved(transaction(2), source, 2_000).into_pool_candidate();
    let charge = other.resident_size + std::mem::size_of::<PipelineVerifiedTx>();
    let error = kernel
        .complete_verify(
            &verify,
            PipelineVerifiedTx {
                candidate: other,
                completed: Completed {
                    cycles: 0,
                    fee: Capacity::shannons(2_000),
                },
                verify_cache_hit: false,
                started_at: Instant::now(),
            },
            charge,
        )
        .unwrap_err();

    assert!(matches!(
        error,
        PrePoolError::Fault(PrePoolFault::PrimaryKeyMismatch { .. })
    ));
    assert_eq!(
        kernel.view(&verify.hash).unwrap().location,
        PrePoolLocation::VerifyLeased
    );
    kernel.audit().unwrap();
}

#[test]
fn stale_lease_cannot_mutate_a_removed_and_readmitted_hash() {
    let mut kernel = PrePoolKernel::new(limits());
    let tx = transaction(2);
    let source = TxSource::Remote {
        cycles: 0,
        peer: 2.into(),
    };
    admit(
        &mut kernel,
        tx.clone(),
        source,
        ResolveLane::Ingress,
        Some(100),
    )
    .unwrap();
    let stale = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    kernel.force_terminalize(&tx.hash()).unwrap();
    admit(
        &mut kernel,
        tx.clone(),
        source,
        ResolveLane::Ingress,
        Some(100),
    )
    .unwrap();
    assert!(matches!(
        kernel.requeue_resolve(&stale),
        Err(PrePoolError::Stale { .. })
    ));
    assert_eq!(
        kernel.view(&tx.hash()).unwrap().location,
        PrePoolLocation::ResolveQueued
    );
    kernel.audit().unwrap();
}

#[test]
fn admission_budget_failure_leaves_primary_and_views_unchanged() {
    let mut constrained = limits();
    constrained.total.entries = 1;
    constrained.remote.entries = 1;
    constrained.per_peer.entries = 1;
    let mut kernel = PrePoolKernel::new(constrained);
    let source = TxSource::Remote {
        cycles: 0,
        peer: 3.into(),
    };
    let first = transaction(3);
    admit(
        &mut kernel,
        first.clone(),
        source,
        ResolveLane::Ingress,
        Some(100),
    )
    .unwrap();
    let usage = kernel.total_usage();
    assert!(matches!(
        admit(
            &mut kernel,
            transaction(4),
            source,
            ResolveLane::Ingress,
            Some(100)
        ),
        Err(PrePoolError::Public(PrePoolPublicError::Backpressure(
            PrePoolBackpressure::TotalBudgetExceeded
                | PrePoolBackpressure::RemoteBudgetExceeded
                | PrePoolBackpressure::PeerBudgetExceeded(_)
        )))
    ));
    assert_eq!(kernel.len(), 1);
    assert_eq!(kernel.total_usage(), usage);
    assert!(kernel.contains_hash(&first.hash()));
    kernel.audit().unwrap();
}

#[test]
fn short_id_collision_is_backpressure_not_aliasing() {
    let mut kernel = PrePoolKernel::new(limits());
    let mut left_hash = [0x77; 32];
    let mut right_hash = left_hash;
    left_hash[31] = 1;
    right_hash[31] = 2;
    let left = with_cached_hash(transaction(5), Byte32::new(left_hash));
    let right = with_cached_hash(transaction(6), Byte32::new(right_hash));
    assert_eq!(left.proposal_short_id(), right.proposal_short_id());
    admit(
        &mut kernel,
        left.clone(),
        TxSource::Proposal,
        ResolveLane::Ingress,
        None,
    )
    .unwrap();
    assert!(matches!(
        admit(
            &mut kernel,
            right,
            TxSource::Proposal,
            ResolveLane::Ingress,
            None
        ),
        Err(PrePoolError::Public(PrePoolPublicError::Backpressure(
            PrePoolBackpressure::ShortIdCollision { .. }
        )))
    ));
    assert_eq!(
        kernel.hash_by_short_id(&left.proposal_short_id()),
        Some(&left.hash())
    );
    kernel.audit().unwrap();
}

#[test]
fn owner_fairness_and_active_caps_do_not_scan_a_capped_prefix() {
    let mut kernel = PrePoolKernel::new(limits());
    for (tag, peer) in [(7, 7), (8, 7), (9, 8)] {
        admit(
            &mut kernel,
            transaction(tag),
            TxSource::Remote {
                cycles: 0,
                peer: PeerIndex::from(peer),
            },
            ResolveLane::Ingress,
            Some(100),
        )
        .unwrap();
    }
    let first = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    let first_peer = source_peer(kernel.source_by_hash(&first.hash).unwrap());
    let second = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    let second_peer = source_peer(kernel.source_by_hash(&second.hash).unwrap());
    assert_ne!(
        first_peer, second_peer,
        "a capped owner cannot hide another runnable head"
    );
    assert_eq!(kernel.active_work(), 2);
    kernel.audit().unwrap();
}

#[test]
fn repeated_dependency_epochs_are_level_triggered_and_bounded() {
    let mut kernel = PrePoolKernel::new(limits());
    let tx = transaction(10);
    admit(
        &mut kernel,
        tx.clone(),
        TxSource::Remote {
            cycles: 0,
            peer: 10.into(),
        },
        ResolveLane::Ingress,
        Some(100),
    )
    .unwrap();
    let lease = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    let key = DependencyKey::Cell(OutPoint::new(Byte32::new([0xaa; 32]), 0));
    kernel
        .wait_resolve(&lease, BTreeSet::from([key.clone()]))
        .unwrap();
    reconcile_dependencies(&mut kernel, HashSet::new(), [key.clone()]).unwrap();
    reconcile_dependencies(&mut kernel, HashSet::new(), [key]).unwrap();
    for _ in 0..8 {
        if !kernel.wait_wake_pending() {
            break;
        }
        kernel.drain_wait_wakes(1).unwrap();
        kernel.audit().unwrap();
    }
    assert!(!kernel.wait_wake_pending());
    assert_eq!(kernel.dependency_epoch_len(), 0);
    assert_eq!(
        kernel.view(&tx.hash()).unwrap().location,
        PrePoolLocation::ResolveQueued
    );
    kernel.audit().unwrap();
}

#[test]
fn availability_without_a_wait_owner_retains_no_epoch_history() {
    let mut kernel = PrePoolKernel::new(limits());
    reconcile_dependencies(
        &mut kernel,
        HashSet::new(),
        (0u16..10_000).map(|tag| {
            let mut bytes = [0u8; 32];
            bytes[..2].copy_from_slice(&tag.to_le_bytes());
            DependencyKey::Cell(OutPoint::new(Byte32::new(bytes), 0))
        }),
    )
    .unwrap();
    assert_eq!(kernel.dependency_epoch_len(), 0);
    assert!(!kernel.wait_wake_pending());
    kernel.audit().unwrap();
}

#[test]
fn same_commit_wakes_prior_conflict_but_not_its_new_victim() {
    let mut kernel = PrePoolKernel::new(limits());
    let shared = Byte32::new([0xac; 32]);
    let key = DependencyKey::Cell(OutPoint::new(shared.clone(), 0));
    let prior = build_tx(vec![(&shared, 0)], 1);
    let victim = build_tx(vec![(&shared, 0)], 2);

    kernel
        .retain_conflict(
            PipelineRawTx::new(prior.clone(), TxSource::Proposal, 1),
            PrePoolSource::Proposal,
            BTreeSet::from([key.clone()]),
            None,
        )
        .unwrap();
    let history = ConflictRetention::new(
        PipelineRawTx::new(victim.clone(), TxSource::Proposal, 1),
        PrePoolSource::Proposal,
        BTreeSet::from([key.clone()]),
        None,
    );

    kernel
        .plan_external_commit(&HashSet::new(), &HashSet::new(), [key], vec![history])
        .unwrap()
        .apply();

    while kernel.wait_wake_pending() {
        kernel.drain_wait_wakes(1).unwrap();
    }
    assert_eq!(
        kernel.view(&prior.hash()).unwrap().location,
        PrePoolLocation::ResolveQueued,
        "a conflict retained before the commit must observe the released input"
    );
    assert_eq!(
        kernel.view(&victim.hash()).unwrap().location,
        PrePoolLocation::Wait(WaitReason::Conflict),
        "the victim retained by this commit must observe the post-commit level"
    );
    kernel.audit().unwrap();
}

#[test]
fn parent_loss_invalidates_an_active_lease_into_exact_wait() {
    let mut kernel = PrePoolKernel::new(limits());
    let parent = Byte32::new([0xbb; 32]);
    let tx = build_tx(vec![(&parent, 0)], 1);
    admit(
        &mut kernel,
        tx.clone(),
        TxSource::Remote {
            cycles: 0,
            peer: 11.into(),
        },
        ResolveLane::Ingress,
        Some(100),
    )
    .unwrap();
    let stale = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    reconcile_dependencies(&mut kernel, HashSet::from([parent]), []).unwrap();
    assert_eq!(kernel.active_work(), 0);
    assert_eq!(
        kernel.view(&tx.hash()).unwrap().location,
        PrePoolLocation::Wait(WaitReason::Missing)
    );
    assert!(matches!(
        kernel.requeue_resolve(&stale),
        Err(PrePoolError::Stale { .. })
    ));
    kernel.audit().unwrap();
}

#[test]
fn parent_loss_also_invalidates_recovery_after_it_has_resolved() {
    let mut kernel = PrePoolKernel::new(limits());
    let parent = Byte32::new([0xbd; 32]);
    let tx = build_tx(vec![(&parent, 0)], 1);
    assert_eq!(
        kernel.retain_recovery_batch(vec![tx.clone()], 1).unwrap(),
        1
    );
    let lease = kernel
        .checkout_resolve(ResolveLane::Ordered)
        .unwrap()
        .unwrap();
    let value = resolved(tx.clone(), TxSource::Local, 1_000);
    let charge = value.resident_size;
    kernel
        .complete_raw(&lease, value, charge, VerifySchedule::default())
        .unwrap();

    reconcile_dependencies(&mut kernel, HashSet::from([parent]), []).unwrap();

    assert_eq!(
        kernel.view(&tx.hash()).unwrap().location,
        PrePoolLocation::Wait(WaitReason::Missing)
    );
    kernel.audit().unwrap();
}

#[test]
fn definitive_parent_terminalization_wakes_trusted_dependents() {
    let mut kernel = PrePoolKernel::new(limits());
    let parent = transaction(117);
    let child = build_tx(vec![(&parent.hash(), 0)], 1);
    assert_eq!(
        kernel
            .retain_recovery_batch(vec![parent.clone(), child.clone()], 1)
            .unwrap(),
        2
    );

    let parent_lease = kernel
        .checkout_resolve(ResolveLane::Ordered)
        .unwrap()
        .unwrap();
    assert_eq!(parent_lease.hash, parent.hash());
    kernel.terminalize_resolve(&parent_lease).unwrap();

    assert_eq!(
        kernel.view(&child.hash()).unwrap().location,
        PrePoolLocation::Wait(WaitReason::Missing),
        "parent removal and child invalidation are one cohort Apply"
    );
    assert!(
        kernel.wait_wake_pending(),
        "definitive loss must publish a level change for bounded maintenance"
    );
    assert_eq!(kernel.drain_wait_wakes(1).unwrap(), 1);
    assert_eq!(
        kernel.view(&child.hash()).unwrap().location,
        PrePoolLocation::ResolveQueued,
        "trusted work must re-evaluate terminal policy instead of parking forever"
    );
    kernel.audit().unwrap();
}

#[test]
fn parent_loss_uses_the_continuous_wait_reservation_at_a_full_budget() {
    let parent = Byte32::new([0xbc; 32]);
    let tx = build_tx(vec![(&parent, 0)], 1);
    let raw = PipelineRawTx::new(
        tx.clone(),
        TxSource::Remote {
            cycles: 0,
            peer: 111.into(),
        },
        1,
    );
    // common projections: short ID (2), peer (2), parent (2), deadline (1);
    // state reservation: one exact wait key reserves waiter, epoch, dirty map
    // and cursor/order storage = 4.
    let exact_bytes = raw.charge_bytes() + 128 + 12 * 32;
    let mut constrained = limits();
    constrained.total = Residency::new(1, exact_bytes);
    constrained.remote = constrained.total;
    constrained.per_peer = constrained.total;
    let mut kernel = PrePoolKernel::new(constrained);
    admit(
        &mut kernel,
        tx.clone(),
        TxSource::Remote {
            cycles: 0,
            peer: 111.into(),
        },
        ResolveLane::Ingress,
        Some(100),
    )
    .unwrap();
    assert_eq!(kernel.total_usage().bytes, exact_bytes);
    let lease = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    assert_eq!(kernel.total_usage().bytes, exact_bytes);
    reconcile_dependencies(&mut kernel, HashSet::from([parent]), []).unwrap();
    assert_eq!(
        kernel.view(&tx.hash()).unwrap().location,
        PrePoolLocation::Wait(WaitReason::Missing)
    );
    assert_eq!(kernel.total_usage().bytes, exact_bytes);
    assert!(matches!(
        kernel.requeue_resolve(&lease),
        Err(PrePoolError::Stale { .. })
    ));
    kernel.audit().unwrap();
}

#[test]
fn parent_projection_deduplicates_cell_and_header_edges() {
    let parent = Byte32::new([0xbd; 32]);
    let tx = build_tx(vec![(&parent, 0), (&parent, 1)], 1)
        .as_advanced_builder()
        .header_dep(parent.clone())
        .build();
    let raw = PipelineRawTx::new(tx.clone(), TxSource::Proposal, 1);
    // Common projections: short ID (2), three dependency keys, and one
    // unique parent represented in the parent map and child set (2).
    // ResolveQueued reserves the larger exact-wait shape: 3 * 4.
    let exact_bytes = raw.charge_bytes() + 128 + (2 + 3 + 2 + 12) * 32;
    let mut kernel = PrePoolKernel::new(limits());

    admit(
        &mut kernel,
        tx,
        TxSource::Proposal,
        ResolveLane::Ordered,
        None,
    )
    .unwrap();

    assert_eq!(kernel.total_usage().bytes, exact_bytes);
    kernel.audit().unwrap();
}

#[test]
fn successive_expanded_parent_losses_keep_exact_causal_keys_in_wait() {
    let mut kernel = PrePoolKernel::new(limits());
    let tx = transaction(112);
    let source = TxSource::Remote {
        cycles: 0,
        peer: 112.into(),
    };
    admit(
        &mut kernel,
        tx.clone(),
        source,
        ResolveLane::Ingress,
        Some(100),
    )
    .unwrap();
    let lease = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    let expanded_a = OutPoint::new(Byte32::new([0xda; 32]), 0);
    let expanded_b = OutPoint::new(Byte32::new([0xdb; 32]), 1);
    let value = resolved(tx.clone(), source, 1_000);
    let charge = value.resident_size;
    kernel
        .complete_resolve(
            &lease,
            value,
            charge,
            VerifySchedule::default(),
            BTreeSet::from([
                DependencyKey::Cell(expanded_a.clone()),
                DependencyKey::Cell(expanded_b.clone()),
            ]),
        )
        .unwrap();

    reconcile_dependencies(&mut kernel, HashSet::from([expanded_a.tx_hash()]), []).unwrap();
    reconcile_dependencies(&mut kernel, HashSet::from([expanded_b.tx_hash()]), []).unwrap();
    let view = kernel.view(&tx.hash()).unwrap();
    assert_eq!(view.location, PrePoolLocation::Wait(WaitReason::Missing));
    assert!(view.dependencies.contains(&expanded_a.tx_hash()));
    assert!(view.dependencies.contains(&expanded_b.tx_hash()));
    kernel.audit().unwrap();
}

#[test]
fn oversized_missing_dependency_set_is_rejected_without_mutating_the_lease() {
    let mut constrained = limits();
    constrained.max_dependencies_per_entry = 2;
    let mut kernel = PrePoolKernel::new(constrained);
    let tx = transaction(147);
    admit(
        &mut kernel,
        tx.clone(),
        TxSource::Proposal,
        ResolveLane::Ingress,
        None,
    )
    .unwrap();
    let lease = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    let dependencies = (0..3)
        .map(|index| DependencyKey::Cell(OutPoint::new(Byte32::new([index; 32]), u32::from(index))))
        .collect();

    assert!(matches!(
        kernel.wait_resolve(&lease, dependencies),
        Err(PrePoolError::Public(PrePoolPublicError::Backpressure(
            PrePoolBackpressure::DependencyLimitExceeded
        )))
    ));
    assert_eq!(
        kernel.view(&tx.hash()).unwrap().location,
        PrePoolLocation::ResolveLeased
    );
    kernel.audit().unwrap();
}

#[test]
fn proposal_promotion_transfers_active_accounting_without_invalidating_same_witness() {
    let mut kernel = PrePoolKernel::new(limits());
    let tx = transaction(12);
    let peer = PeerIndex::from(12);
    admit(
        &mut kernel,
        tx.clone(),
        TxSource::Remote { cycles: 0, peer },
        ResolveLane::Ingress,
        Some(100),
    )
    .unwrap();
    let lease = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    let version = lease.version;
    kernel.promote_source(&tx.hash()).unwrap();
    assert_eq!(kernel.peer_active_work(peer), 0);
    assert_eq!(kernel.active_work(), 1);
    assert_eq!(
        kernel.view(&tx.hash()).unwrap().source,
        PrePoolSource::Proposal
    );
    assert_eq!(kernel.view(&tx.hash()).unwrap().version, version);
    let value = resolved(tx, TxSource::Proposal, 0);
    let charge = value.resident_size;
    kernel
        .complete_raw(&lease, value, charge, VerifySchedule::default())
        .unwrap();
    kernel.audit().unwrap();
}

#[test]
fn banned_peer_revocation_plan_uses_immutable_ingress_attribution() {
    let mut kernel = PrePoolKernel::new(limits());
    let peer = PeerIndex::from(121);
    let promoted = transaction(121);
    let remote = transaction(122);
    for tx in [&promoted, &remote] {
        admit(
            &mut kernel,
            tx.clone(),
            TxSource::Remote { cycles: 0, peer },
            ResolveLane::Ingress,
            Some(100),
        )
        .unwrap();
    }

    // Model the capacity-wait race: the outer ingress snapshot was taken while
    // both entries were remote, then one scheduling source was promoted before
    // the authoritative revocation plan was compiled.
    let stale_slice = vec![promoted.hash(), remote.hash()];
    kernel.promote_source(&promoted.hash()).unwrap();
    let plan = kernel
        .plan_peer_revocation(peer, &stale_slice)
        .unwrap()
        .unwrap();
    assert_eq!(plan.records().len(), 2);
    let removed = plan.apply();
    assert_eq!(removed.len(), 2);
    assert!(kernel.view(&promoted.hash()).is_none());
    assert!(kernel.view(&remote.hash()).is_none());
    kernel.audit().unwrap();
}

#[test]
fn remote_peer_order_cannot_hijack_an_existing_conflict_owner() {
    let mut kernel = PrePoolKernel::new(limits());
    let tx = transaction(13);
    let variant = tx
        .as_advanced_builder()
        .witness(ckb_types::bytes::Bytes::from_static(b"variant").pack())
        .build();
    assert_eq!(tx.hash(), variant.hash());
    assert_ne!(tx.witness_hash(), variant.witness_hash());
    let key = DependencyKey::Cell(tx.input_pts_iter().next().unwrap());
    kernel
        .retain_conflict(
            PipelineRawTx::new(
                tx.clone(),
                TxSource::Remote {
                    cycles: 0,
                    peer: 1.into(),
                },
                1,
            ),
            PrePoolSource::Remote(RemoteSource::new(1.into(), 0)),
            BTreeSet::from([key.clone()]),
            Some(100),
        )
        .unwrap();
    kernel
        .retain_conflict(
            PipelineRawTx::new(
                variant,
                TxSource::Remote {
                    cycles: 0,
                    peer: 99.into(),
                },
                1,
            ),
            PrePoolSource::Remote(RemoteSource::new(99.into(), 0)),
            BTreeSet::from([key]),
            Some(100),
        )
        .unwrap();
    assert_eq!(
        kernel.view(&tx.hash()).unwrap().source,
        PrePoolSource::Remote(RemoteSource::new(1.into(), 0))
    );
    assert_eq!(
        kernel.raw_by_hash(&tx.hash()).unwrap().tx.witness_hash(),
        tx.witness_hash()
    );
    kernel.audit().unwrap();
}

#[test]
fn trusted_conflict_resubmission_refreshes_the_exact_witness_owner() {
    let mut kernel = PrePoolKernel::new(limits());
    let first = transaction(143)
        .as_advanced_builder()
        .witness(ckb_types::bytes::Bytes::from_static(b"first").pack())
        .build();
    let second = first
        .as_advanced_builder()
        .set_witnesses(vec![ckb_types::bytes::Bytes::from_static(b"second").pack()])
        .build();
    assert_eq!(first.hash(), second.hash());
    assert_ne!(first.witness_hash(), second.witness_hash());
    let key = DependencyKey::Cell(first.input_pts_iter().next().unwrap());
    kernel
        .retain_conflict(
            PipelineRawTx::new(first.clone(), TxSource::Proposal, 1),
            PrePoolSource::Proposal,
            BTreeSet::from([key.clone()]),
            None,
        )
        .unwrap();
    kernel
        .retain_conflict(
            PipelineRawTx::new(second.clone(), TxSource::Proposal, 2),
            PrePoolSource::Proposal,
            BTreeSet::from([key]),
            None,
        )
        .unwrap();
    assert_eq!(
        kernel.raw_by_hash(&first.hash()).unwrap().tx.witness_hash(),
        second.witness_hash()
    );
    kernel.audit().unwrap();
}

#[test]
fn full_conflict_history_terminalizes_rejected_owner_without_panicking() {
    let mut constrained = limits();
    constrained.conflict_history = Residency::default();
    let mut kernel = PrePoolKernel::new(constrained);
    let tx = transaction(14);
    admit(
        &mut kernel,
        tx.clone(),
        TxSource::Remote {
            cycles: 0,
            peer: 14.into(),
        },
        ResolveLane::Ingress,
        Some(100),
    )
    .unwrap();
    let lease = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    kernel
        .park_conflict_or_terminalize(&lease.hash, lease.version, PrePoolLocation::ResolveLeased)
        .unwrap();
    assert!(!kernel.contains_hash(&tx.hash()));
    assert_eq!(kernel.conflict_usage(), Residency::default());
    kernel.audit().unwrap();
}

#[test]
fn multi_input_conflict_union_uses_the_product_bound() {
    let mut constrained = limits();
    constrained.max_inputs_per_ready = 3;
    constrained.max_candidates_per_input = 2;
    let mut kernel = PrePoolKernel::new(constrained);
    let parents = [
        Byte32::new([0xe1; 32]),
        Byte32::new([0xe2; 32]),
        Byte32::new([0xe3; 32]),
    ];
    let losers = parents
        .iter()
        .enumerate()
        .map(|(index, parent)| {
            let tx = build_tx(vec![(parent, 0)], index + 1);
            stage_ready(&mut kernel, tx.clone(), TxSource::Proposal, None, 1_000);
            tx
        })
        .collect::<Vec<_>>();
    let winner = build_tx(parents.iter().map(|parent| (parent, 0)).collect(), 10);
    stage_ready(
        &mut kernel,
        winner.clone(),
        TxSource::Proposal,
        None,
        100_000,
    );

    let settlement = commit_ready(&mut kernel);

    assert_eq!(settlement.winner.hash, winner.hash());
    assert_eq!(settlement.superseded.len(), 3);
    for loser in losers {
        assert_eq!(
            kernel.view(&loser.hash()).unwrap().location,
            PrePoolLocation::Wait(WaitReason::Conflict)
        );
    }
    kernel.audit().unwrap();
}

#[test]
fn multi_input_conflict_union_respects_the_global_commit_bound() {
    let mut constrained = limits();
    let over_bound = crate::constants::MAX_POOL_MUTATION_CANDIDATES + 1;
    constrained.max_dependencies_per_entry = over_bound;
    constrained.max_inputs_per_ready = over_bound;
    constrained.max_candidates_per_input = 2;
    constrained.total = Residency::new(over_bound + 1, usize::MAX);
    constrained.remote = constrained.total;
    let mut kernel = PrePoolKernel::new(constrained);
    let parents = (0..over_bound)
        .map(|index| {
            let mut raw = [0u8; 32];
            raw[..std::mem::size_of::<usize>()].copy_from_slice(&index.to_le_bytes());
            Byte32::new(raw)
        })
        .collect::<Vec<_>>();
    for parent in &parents {
        let tx = build_tx(vec![(parent, 0)], 1);
        stage_ready(&mut kernel, tx, TxSource::Proposal, None, 1_000);
    }
    let winner = build_tx(parents.iter().map(|parent| (parent, 0)).collect(), 1);
    let winner_hash = winner.hash();
    stage_ready(&mut kernel, winner, TxSource::Proposal, None, 10_000_000);

    let mut session = kernel.begin_next_commit().unwrap().unwrap();
    let Err(error) = session.plan_ready(&HashSet::new(), std::iter::empty(), Vec::new()) else {
        panic!("an over-bound conflict cohort must not produce an apply capability");
    };
    assert_eq!(
        error,
        PrePoolError::Public(PrePoolPublicError::Backpressure(
            PrePoolBackpressure::CommitConflictCohortLimitExceeded,
        ))
    );
    drop(session);
    assert_eq!(
        kernel.view(&winner_hash).unwrap().location,
        PrePoolLocation::Ready,
        "a rejected read-only Plan must not mutate the selected owner"
    );
    kernel.audit().unwrap();
}

#[test]
fn commit_session_selects_the_current_highest_rank() {
    let mut kernel = PrePoolKernel::new(limits());
    let earlier = transaction(0xd1);
    stage_ready(
        &mut kernel,
        earlier.clone(),
        TxSource::Proposal,
        None,
        1_000,
    );
    let later = transaction(0xd2);
    stage_ready(
        &mut kernel,
        later.clone(),
        TxSource::Proposal,
        None,
        100_000,
    );
    let settlement = commit_ready(&mut kernel);
    assert_eq!(settlement.winner.hash, later.hash());
    assert_eq!(
        kernel.view(&earlier.hash()).unwrap().location,
        PrePoolLocation::Ready
    );
    kernel.audit().unwrap();
}

#[test]
fn equal_priority_ready_candidates_commit_earlier_arrival_first() {
    let mut kernel = PrePoolKernel::new(limits());
    let earlier = transaction(0xd3);
    stage_ready(
        &mut kernel,
        earlier.clone(),
        TxSource::Proposal,
        None,
        1_000,
    );
    let later = transaction(0xd4);
    stage_ready(&mut kernel, later, TxSource::Proposal, None, 1_000);

    let session = kernel.begin_next_commit().unwrap().unwrap();
    assert_eq!(session.payload().candidate.tx.hash(), earlier.hash());
    drop(session);
    kernel.audit().unwrap();
}

#[test]
fn commit_handoff_terminalizes_superseded_entries_when_history_is_full() {
    let mut constrained = limits();
    constrained.conflict_history = Residency::default();
    let mut kernel = PrePoolKernel::new(constrained);
    let parent = Byte32::new([0xe4; 32]);
    let loser = build_tx(vec![(&parent, 0)], 1);
    stage_ready(&mut kernel, loser.clone(), TxSource::Proposal, None, 1_000);
    let winner = build_tx(vec![(&parent, 0)], 2);
    stage_ready(
        &mut kernel,
        winner.clone(),
        TxSource::Proposal,
        None,
        100_000,
    );

    let settlement = commit_ready(&mut kernel);

    assert_eq!(settlement.winner.hash, winner.hash());
    assert_eq!(settlement.superseded.len(), 1);
    assert_eq!(settlement.superseded[0].hash, loser.hash());
    assert!(!kernel.contains_hash(&winner.hash()));
    assert!(!kernel.contains_hash(&loser.hash()));
    assert_eq!(kernel.conflict_usage(), Residency::default());
    kernel.audit().unwrap();
}

#[test]
fn terminalized_superseded_parent_wakes_its_trusted_child() {
    let mut constrained = limits();
    constrained.conflict_history = Residency::default();
    let mut kernel = PrePoolKernel::new(constrained);
    let shared_input = Byte32::new([0xe7; 32]);
    let loser = build_tx(vec![(&shared_input, 0)], 1);
    stage_ready(&mut kernel, loser.clone(), TxSource::Proposal, None, 1_000);

    let child = build_tx(vec![(&loser.hash(), 0)], 2);
    admit(
        &mut kernel,
        child.clone(),
        TxSource::Proposal,
        ResolveLane::Ingress,
        None,
    )
    .unwrap();
    let child_lease = kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    let lost_output = DependencyKey::Cell(child.input_pts_iter().next().unwrap());
    kernel
        .wait_resolve(&child_lease, BTreeSet::from([lost_output]))
        .unwrap();

    let winner = build_tx(vec![(&shared_input, 0)], 3);
    stage_ready(&mut kernel, winner, TxSource::Proposal, None, 100_000);
    commit_ready(&mut kernel);

    assert!(!kernel.contains_hash(&loser.hash()));
    assert!(
        kernel.wait_wake_pending(),
        "definitive loser death publishes its exact output-level change"
    );
    kernel.drain_wait_wakes(1).unwrap();
    assert_eq!(
        kernel.view(&child.hash()).unwrap().location,
        PrePoolLocation::ResolveQueued,
        "the trusted child must re-resolve and reach terminal policy instead of stranding"
    );
    kernel.audit().unwrap();
}

#[test]
fn optional_commit_history_cannot_veto_the_ready_winner() {
    let mut kernel = PrePoolKernel::new(limits());
    let existing = transaction(0xe5);
    kernel
        .retain_conflict(
            PipelineRawTx::new(existing.clone(), TxSource::Proposal, 1),
            PrePoolSource::Proposal,
            BTreeSet::from([DependencyKey::Cell(
                existing.input_pts_iter().next().unwrap(),
            )]),
            None,
        )
        .unwrap();

    let winner = transaction(0xe6);
    stage_ready(
        &mut kernel,
        winner.clone(),
        TxSource::Proposal,
        None,
        100_000,
    );
    let history = ConflictRetention::new(
        PipelineRawTx::new(existing.clone(), TxSource::Proposal, 1),
        PrePoolSource::Proposal,
        BTreeSet::from([DependencyKey::Cell(
            existing.input_pts_iter().next().unwrap(),
        )]),
        None,
    );

    let mut session = kernel.begin_next_commit().unwrap().unwrap();
    let plan = session
        .plan_ready(&HashSet::new(), Vec::<DependencyKey>::new(), vec![history])
        .expect("duplicate optional history must fall back to the winner-only cohort");
    plan.apply();
    drop(session);

    assert!(!kernel.contains_hash(&winner.hash()));
    assert!(kernel.contains_hash(&existing.hash()));
    kernel.audit().unwrap();
}

#[test]
fn remote_conflict_keeps_remote_reservation_and_wakes_without_capacity_retry() {
    let mut constrained = limits();
    constrained.remote.entries = 1;
    constrained.per_peer.entries = 1;
    let mut kernel = PrePoolKernel::new(constrained);
    let peer = PeerIndex::from(140);
    let source = TxSource::Remote { cycles: 0, peer };
    let tx = transaction(140);
    let key = DependencyKey::Cell(tx.input_pts_iter().next().unwrap());
    kernel
        .retain_conflict(
            PipelineRawTx::new(tx.clone(), source, 1),
            PrePoolSource::Remote(RemoteSource::new(peer, 0)),
            BTreeSet::from([key.clone()]),
            Some(100),
        )
        .unwrap();

    let remote_before = kernel.remote_usage();
    assert_eq!(remote_before.entries, 1);
    assert_eq!(kernel.peer_usage(peer), remote_before);
    assert_eq!(kernel.conflict_usage().entries, 1);
    assert!(matches!(
        admit(
            &mut kernel,
            transaction(141),
            TxSource::Remote {
                cycles: 0,
                peer: PeerIndex::from(141),
            },
            ResolveLane::Ingress,
            Some(100),
        ),
        Err(PrePoolError::Public(PrePoolPublicError::Backpressure(
            PrePoolBackpressure::RemoteBudgetExceeded
        )))
    ));

    reconcile_dependencies(&mut kernel, HashSet::new(), [key]).unwrap();
    assert_eq!(kernel.drain_wait_wakes(8).unwrap(), 1);
    assert!(!kernel.wait_wake_pending());
    assert_eq!(
        kernel.view(&tx.hash()).unwrap().location,
        PrePoolLocation::ResolveQueued
    );
    assert_eq!(kernel.remote_usage(), remote_before);
    assert_eq!(kernel.conflict_usage(), Residency::default());
    kernel.audit().unwrap();
}

#[test]
fn remote_conflict_history_keeps_its_bounded_residency_deadline() {
    let mut kernel = PrePoolKernel::new(limits());
    let peer = PeerIndex::from(142);
    let source = TxSource::Remote { cycles: 0, peer };
    let tx = transaction(142);
    let key = DependencyKey::Cell(tx.input_pts_iter().next().unwrap());
    kernel
        .retain_conflict(
            PipelineRawTx::new(tx.clone(), source, 1),
            PrePoolSource::Remote(RemoteSource::new(peer, 0)),
            BTreeSet::from([key]),
            Some(5),
        )
        .unwrap();
    assert!(kernel.plan_expiry(4, 1).unwrap().is_none());
    let expired = kernel.plan_expiry(5, 1).unwrap().unwrap().apply();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].hash, tx.hash());
    assert_eq!(kernel.total_usage(), Residency::default());
    assert_eq!(kernel.remote_usage(), Residency::default());
    assert_eq!(kernel.conflict_usage(), Residency::default());
    kernel.audit().unwrap();
}

#[test]
fn expiry_batch_is_bounded_without_a_ready_prefix_scan() {
    let mut kernel = PrePoolKernel::new(limits());
    let ready_tx = transaction(15);
    stage_ready(
        &mut kernel,
        ready_tx.clone(),
        TxSource::Remote {
            cycles: 0,
            peer: 15.into(),
        },
        Some(5),
        1_000,
    );
    let queued_tx = transaction(16);
    admit(
        &mut kernel,
        queued_tx.clone(),
        TxSource::Remote {
            cycles: 0,
            peer: 16.into(),
        },
        ResolveLane::Ingress,
        Some(5),
    )
    .unwrap();
    let expired = kernel.plan_expiry(5, 2).unwrap().unwrap().apply();
    assert_eq!(expired.len(), 2);
    assert!(expired.iter().any(|record| record.hash == ready_tx.hash()));
    assert!(expired.iter().any(|record| record.hash == queued_tx.hash()));
    assert!(!kernel.contains_hash(&ready_tx.hash()));
    assert!(!kernel.contains_hash(&queued_tx.hash()));
    kernel.audit().unwrap();
}

#[test]
fn recovery_batch_is_atomic_parent_first_and_uses_ordered_resolve() {
    let mut kernel = PrePoolKernel::new(limits());
    let parent = transaction(201);
    let child = build_tx(vec![(&parent.hash(), 0)], 1);
    let retained = kernel
        .retain_recovery_batch(vec![parent.clone(), child.clone()], 7)
        .unwrap();
    assert_eq!(retained, 2);
    assert_eq!(
        kernel.view(&parent.hash()).unwrap().location,
        PrePoolLocation::ResolveQueued
    );
    assert_eq!(
        kernel.view(&child.hash()).unwrap().location,
        PrePoolLocation::ResolveQueued
    );
    assert_eq!(
        kernel
            .recovery_snapshot()
            .into_iter()
            .map(|tx| tx.hash())
            .collect::<Vec<_>>(),
        vec![parent.hash(), child.hash()]
    );

    let parent_lease = kernel
        .checkout_resolve(ResolveLane::Ordered)
        .unwrap()
        .unwrap();
    assert_eq!(parent_lease.hash, parent.hash());
    assert!(
        kernel
            .recovery_snapshot()
            .iter()
            .any(|tx| tx.hash() == parent.hash()),
        "an active borrower must not create a persistence gap"
    );
    kernel.terminalize_resolve(&parent_lease).unwrap();
    assert_eq!(
        kernel.view(&child.hash()).unwrap().location,
        PrePoolLocation::Wait(WaitReason::Missing)
    );
    assert_eq!(kernel.drain_wait_wakes(1).unwrap(), 1);
    let child_lease = kernel
        .checkout_resolve(ResolveLane::Ordered)
        .unwrap()
        .unwrap();
    assert_eq!(child_lease.hash, child.hash());
    kernel.terminalize_resolve(&child_lease).unwrap();
    assert!(kernel.recovery_snapshot().is_empty());
    kernel.audit().unwrap();
}

#[test]
fn recovery_leases_share_the_trusted_active_work_budget() {
    let mut kernel = PrePoolKernel::new(limits());
    let tx = transaction(146);
    kernel.retain_recovery_batch(vec![tx.clone()], 1).unwrap();

    let lease = kernel
        .checkout_resolve(ResolveLane::Ordered)
        .unwrap()
        .unwrap();
    assert_eq!(lease.hash, tx.hash());
    assert_eq!(kernel.active_work(), 1);

    kernel.requeue_resolve(&lease).unwrap();
    assert_eq!(kernel.active_work(), 0);
    kernel.audit().unwrap();
}

#[test]
fn over_budget_recovery_plan_is_mutation_free() {
    let mut limits = limits();
    limits.total.entries = 1;
    let mut kernel = PrePoolKernel::new(limits);
    let first = transaction(202);
    let second = transaction(203);
    assert_eq!(
        kernel.retain_recovery_batch(vec![first.clone(), second], 9),
        Err(PrePoolError::Public(PrePoolPublicError::Backpressure(
            PrePoolBackpressure::TotalBudgetExceeded
        )))
    );
    assert!(!kernel.contains_hash(&first.hash()));
    assert_eq!(kernel.len(), 0);
    kernel.audit().unwrap();
}

#[test]
fn empty_generation_recovery_retains_closure_safe_prefix() {
    let mut limits = limits();
    limits.total.entries = 2;
    let mut kernel = PrePoolKernel::new(limits);
    let parent = transaction(204);
    let child = build_tx(vec![(&parent.hash(), 0)], 1);
    let grandchild = build_tx(vec![(&child.hash(), 0)], 2);

    let retained = kernel
        .retain_recovery_prefix_after_clear(
            vec![parent.clone(), child.clone(), grandchild.clone()],
            10,
        )
        .unwrap();

    assert_eq!(retained, 2);
    assert!(kernel.contains_hash(&parent.hash()));
    assert!(kernel.contains_hash(&child.hash()));
    assert!(!kernel.contains_hash(&grandchild.hash()));
    assert!(
        kernel
            .recovery_snapshot()
            .iter()
            .map(TransactionView::hash)
            .eq([parent.hash(), child.hash()])
    );
    kernel.audit().unwrap();
}

#[test]
fn randomized_public_transitions_always_match_full_rebuild() {
    let mut kernel = PrePoolKernel::new(limits());
    let mut rng = StdRng::seed_from_u64(0x0050_5245_504f_4f4c);
    let txs = (32u8..64).map(transaction).collect::<Vec<_>>();
    for step in 0..1_000 {
        let tx = txs[rng.gen_range(0..txs.len())].clone();
        let action = rng.gen_range(0..6);
        match action {
            0 => {
                let peer = PeerIndex::from(rng.gen_range(1usize..=8));
                let _ = admit(
                    &mut kernel,
                    tx,
                    TxSource::Remote { cycles: 0, peer },
                    ResolveLane::Ingress,
                    Some(100),
                );
            }
            1 => {
                if let Ok(Some(lease)) = kernel.checkout_resolve(ResolveLane::Ingress) {
                    let _ = kernel.requeue_resolve(&lease);
                }
            }
            2 => {
                let _ = kernel.promote_source(&tx.hash());
            }
            3 => {
                let _ = kernel.force_terminalize(&tx.hash());
            }
            4 => {
                let key = DependencyKey::Cell(OutPoint::new(tx.hash(), 0));
                reconcile_dependencies(&mut kernel, HashSet::new(), [key]).unwrap();
                let _ = kernel.drain_wait_wakes(4);
            }
            _ => {
                if let Ok(Some(lease)) = kernel.checkout_resolve(ResolveLane::Ingress) {
                    let key = DependencyKey::Cell(OutPoint::new(Byte32::new([0xcc; 32]), 0));
                    let _ = kernel.wait_resolve(&lease, BTreeSet::from([key]));
                }
            }
        }
        if let Err(error) = kernel.audit() {
            panic!("random transition step {step}, action {action}: {error}");
        }
    }
}
