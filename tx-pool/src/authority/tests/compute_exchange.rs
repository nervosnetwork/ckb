use super::foundation::{
    admit_remote, apply_plan, checkout_remote_for_verify_with_claim, limits, owner_version,
    take_resolve_work, tx,
};
use crate::{
    authority::{
        effect::{
            CommittedAcceptance, CommittedEffect, EffectBatchBound, EffectBatchBounds,
            EffectCapacity, EffectLimits, EffectPolicy,
        },
        exchange::{
            AuthorityComputeExecutionPermit, ComputeVerifierSlot, ComputeWorkerGrant,
            ComputeWorkerSlot,
        },
        plan::{
            CommittedComputeExchange, ComputeExchangeCompletion, ComputeExchangeDeferred,
            PlanError, TxPoolAuthority, test_support::ComputeExchangeRecovery,
        },
        resources::{AcceptedResources, ComputeLimits, ResourceLimits, ResourceVector},
        state::{
            ApplySequence, EntryVersion, OwnedTx, PreAcceptedPhase, RawTxHash, ValidatedAdmission,
            VerifyCapability, WorkPermit,
        },
        work::{CheckedOutWork, ComputeSettlement, SettlementNext, SettlementToken},
    },
    error::Reject,
};
use ckb_network::PeerIndex;
use ckb_types::{bytes::Bytes, packed::Byte32, prelude::Pack};
use std::sync::Arc;
use tokio::sync::{Notify, Semaphore};

fn any_verifier(worker_id: usize) -> ComputeWorkerSlot {
    ComputeVerifierSlot::new(worker_id, VerifyCapability::Any).into()
}

fn grant(slot: ComputeWorkerSlot) -> ComputeWorkerGrant {
    let permit = Arc::new(Semaphore::new(1))
        .try_acquire_owned()
        .expect("fixture owns its one execution permit");
    ComputeWorkerGrant::new(
        slot,
        AuthorityComputeExecutionPermit::new(permit, Arc::new(Notify::new())),
    )
}

fn assignment_hash(work: &CheckedOutWork) -> crate::authority::state::RawTxHash {
    let transaction = match work {
        CheckedOutWork::Resolve(work) => work.transaction(),
        CheckedOutWork::ContinuousResolve(work) => work.transaction(),
        CheckedOutWork::Verify(work) => work.transaction(),
    };
    crate::authority::state::TxIdentity::from_transaction(transaction).raw
}

fn exchange_settlements(authority: &mut TxPoolAuthority) -> [ComputeSettlement; 2] {
    let first = admit_remote(authority, 80_071, 13);
    let second = admit_remote(authority, 80_072, 14);
    let _third = admit_remote(authority, 80_073, 15);
    let _fourth = admit_remote(authority, 80_074, 16);
    let first_checkout = authority
        .plan_checkout_for_foundation(
            &first,
            owner_version(authority, &first),
            WorkPermit::ResolveOnly,
        )
        .expect("the first reference owner checks out")
        .apply();
    let (_, first_work) = take_resolve_work(first_checkout);
    let second_checkout = authority
        .plan_checkout_for_foundation(
            &second,
            owner_version(authority, &second),
            WorkPermit::ResolveOnly,
        )
        .expect("the second reference owner checks out")
        .apply();
    let (_, second_work) = take_resolve_work(second_checkout);
    [
        first_work.internal_failure(),
        second_work.internal_failure(),
    ]
}

fn one_batch_effect_limits() -> EffectLimits {
    const EFFECT_BYTES: usize = 1024 * 1024;
    EffectLimits::partitioned(
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectCapacity::new(0, 0),
        EffectCapacity::new(0, 0),
        EffectBatchBounds::new(
            EffectBatchBound::new(1, EFFECT_BYTES),
            EffectBatchBound::new(1, EFFECT_BYTES),
            EffectBatchBound::new(1, EFFECT_BYTES),
        ),
    )
    .expect("one total effect slot admits every fixture batch")
}

fn fill_total_effect_capacity(authority: &mut TxPoolAuthority) {
    let publication = authority
        .effect_publication_for_foundation(
            EffectPolicy::CriticalDetail,
            vec![CommittedEffect::Accepted(CommittedAcceptance::Duplicate {
                tx_hash: crate::authority::state::RawTxHash(Byte32::new([81; 32])),
                requesting_peer: None,
            })],
        )
        .expect("one critical fixture publication fits its batch bound");
    drop(
        authority
            .plan_effect_publication_for_foundation(&publication)
            .expect("the sole total effect slot is initially empty")
            .apply(),
    );
}

fn limits_with_active_work(active_work: usize) -> ResourceLimits {
    let bytes = active_work * 8 * 1024;
    let edges = active_work * 16;
    ResourceLimits::new(
        ResourceVector::new(active_work, bytes, edges, active_work),
        ResourceVector::new(active_work, bytes, edges, active_work),
        ResourceVector::new(active_work, bytes, edges, active_work),
        AcceptedResources::new(active_work, bytes, bytes, u64::MAX),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .expect("the fixture active-work partition is internally consistent")
}

fn stale_completion(worker: usize) -> ComputeExchangeCompletion {
    ComputeExchangeCompletion::new(
        any_verifier(worker),
        ComputeSettlement {
            token: SettlementToken {
                hash: RawTxHash(Byte32::new([worker as u8; 32])),
                version: EntryVersion(worker as u128 + 1),
            },
            next: SettlementNext::Retry,
        },
    )
}

#[test]
fn uak_compute_exchange_uses_the_configured_active_work_bound() {
    let active_work = crate::constants::MAX_POOL_MUTATION_CANDIDATES + 1;
    let mut authority = TxPoolAuthority::for_foundation(limits_with_active_work(active_work));
    let grants = (0..active_work)
        .map(|worker| grant(any_verifier(worker)))
        .collect::<Vec<_>>();

    let committed = authority
        .apply_compute_exchange(Vec::new(), grants)
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!(
                "the configured worker topology, not the membership bound, limits a wave: {error:?}"
            );
        });
    assert!(committed.retirement.is_none());
    assert!(committed.assignments.is_empty());
    assert_eq!(committed.unused_grants.len(), active_work);
    drop(committed);

    let completions = (0..active_work).map(stale_completion).collect::<Vec<_>>();
    let committed = authority
        .apply_compute_exchange(completions, Vec::new())
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("the configured completion partition is also valid: {error:?}");
        });
    assert_eq!(committed.obsolete.len(), active_work);
    assert!(committed.assignments.is_empty());
}

#[test]
fn uak_compute_exchange_rejects_a_capability_partition_larger_than_the_worker_topology() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let before = authority.normalized_snapshot();
    let active_work = authority.resources().limits().active_work_limit();
    let grants = (0..=active_work)
        .map(|worker| grant(any_verifier(worker)))
        .collect::<Vec<_>>();

    let failure = authority
        .apply_compute_exchange(Vec::new(), grants)
        .err()
        .expect("a capability partition cannot exceed the configured worker topology");
    let (error, recoveries) = failure.into_parts();
    assert_eq!(
        error,
        PlanError::Fault(crate::authority::plan::AuthorityFault::SchedulerProjection)
    );
    assert_eq!(
        recoveries
            .filter(|recovery| matches!(recovery, ComputeExchangeRecovery::Grant(_)))
            .count(),
        active_work + 1
    );
    assert_eq!(authority.normalized_snapshot(), before);

    let completions = (0..=active_work).map(stale_completion).collect::<Vec<_>>();
    let failure = authority
        .apply_compute_exchange(completions, Vec::new())
        .err()
        .expect("a completion partition cannot exceed the configured worker topology");
    let (error, recoveries) = failure.into_parts();
    assert_eq!(
        error,
        PlanError::Fault(crate::authority::plan::AuthorityFault::SchedulerProjection)
    );
    assert_eq!(
        recoveries
            .filter(|recovery| matches!(recovery, ComputeExchangeRecovery::Settlement(_)))
            .count(),
        active_work + 1
    );
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_initial_compute_exchange_checks_out_one_available_worker_wave_with_one_stamp() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let first = admit_remote(&mut authority, 80_001, 1);
    let second = admit_remote(&mut authority, 80_002, 2);
    let before = authority.clocks();

    let committed = authority
        .apply_compute_exchange(
            Vec::new(),
            vec![
                grant(any_verifier(0)),
                grant(ComputeWorkerSlot::ordered_resolve()),
            ],
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("one immediately available worker wave plans: {error:?}");
        });
    assert!(committed.retirement.is_some());
    assert!(committed.settled.is_empty());
    assert!(committed.obsolete.is_empty());
    assert!(committed.deferred.is_empty());
    assert!(committed.unused_grants.is_empty());
    assert_eq!(committed.assignments.len(), 2);
    assert_eq!(
        authority.clocks().next_sequence.0,
        before.next_sequence.0 + 1
    );
    assert_eq!(authority.clocks().next_version.0, before.next_version.0 + 2);
    assert_eq!(authority.resources().preaccepted().active_work, 2);
    for hash in [&first, &second] {
        assert!(matches!(
            authority.entry(hash),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Computing(_))
        ));
    }

    let mut assigned = committed
        .assignments
        .into_iter()
        .map(|assignment| {
            let (_, execution, work) = assignment.into_parts();
            drop(execution);
            (assignment_hash(&work), work)
        })
        .collect::<Vec<_>>();
    assigned.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let mut expected = [first, second];
    expected.sort_unstable();
    assert_eq!(
        assigned.iter().map(|(hash, _)| hash).collect::<Vec<_>>(),
        expected.iter().collect::<Vec<_>>()
    );
}

#[test]
fn uak_compute_exchange_selects_from_the_virtual_post_settlement_frontier() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let first = admit_remote(&mut authority, 80_011, 3);
    let second = admit_remote(&mut authority, 80_012, 3);
    let third = admit_remote(&mut authority, 80_013, 5);
    let checkout = authority
        .plan_checkout_for_foundation(
            &first,
            owner_version(&authority, &first),
            WorkPermit::ResolveOnly,
        )
        .expect("first owner checks out")
        .apply();
    let (_, resolve) = take_resolve_work(checkout);
    let before = authority.clocks();

    let committed = authority
        .apply_compute_exchange(
            vec![ComputeExchangeCompletion::new(
                ComputeWorkerSlot::ordered_resolve(),
                resolve.internal_failure(),
            )],
            vec![
                grant(ComputeWorkerSlot::ordered_resolve()),
                grant(any_verifier(0)),
            ],
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("settlement and refill compose: {error:?}");
        });
    assert_eq!(
        committed.settled,
        vec![ComputeWorkerSlot::ordered_resolve()]
    );
    assert!(committed.deferred.is_empty());
    assert!(committed.unused_grants.is_empty());
    assert_eq!(committed.assignments.len(), 2);
    assert_eq!(
        authority.clocks().next_sequence.0,
        before.next_sequence.0 + 1
    );
    assert_eq!(authority.clocks().next_version.0, before.next_version.0 + 3);
    let assigned = committed
        .assignments
        .into_iter()
        .map(|assignment| {
            let (_, execution, work) = assignment.into_parts();
            drop(execution);
            assignment_hash(&work)
        })
        .collect::<Vec<_>>();
    assert!(assigned.contains(&first));
    assert!(assigned.contains(&third));
    assert!(!assigned.contains(&second));
}

#[test]
fn uak_compute_exchange_defers_effectful_work_without_blocking_an_idle_slot() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let first = admit_remote(&mut authority, 80_021, 6);
    let second = admit_remote(&mut authority, 80_022, 7);
    let checkout = authority
        .plan_checkout_for_foundation(
            &first,
            owner_version(&authority, &first),
            WorkPermit::ResolveOnly,
        )
        .expect("first owner checks out")
        .apply();
    let (_, resolve) = take_resolve_work(checkout);

    let committed = authority
        .apply_compute_exchange(
            vec![ComputeExchangeCompletion::new(
                ComputeWorkerSlot::ordered_resolve(),
                resolve.rejected(Reject::Invalidated("exchange rejection".to_owned())),
            )],
            vec![grant(any_verifier(0))],
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("effectful deferral preserves unrelated progress: {error:?}");
        });
    assert!(committed.settled.is_empty());
    assert_eq!(committed.deferred.len(), 1);
    assert_eq!(committed.assignments.len(), 1);
    let (_, execution, work) = committed
        .assignments
        .into_iter()
        .next()
        .expect("the independent queued owner is assigned")
        .into_parts();
    drop(execution);
    assert_eq!(assignment_hash(&work), second);
    assert!(matches!(
        authority.entry(&first),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
}

#[test]
fn uak_compute_exchange_duplicate_slot_failure_returns_every_idle_capability() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let before = authority.normalized_snapshot();
    let failure = authority
        .apply_compute_exchange(
            Vec::new(),
            vec![
                grant(ComputeWorkerSlot::ordered_resolve()),
                grant(ComputeWorkerSlot::ordered_resolve()),
            ],
        )
        .err()
        .expect("duplicate stable slot identity is rejected");
    let (error, recoveries) = failure.into_parts();
    assert_eq!(
        error,
        PlanError::Fault(crate::authority::plan::AuthorityFault::SchedulerProjection)
    );
    assert_eq!(
        recoveries
            .filter(|recovery| matches!(recovery, ComputeExchangeRecovery::Grant(_)))
            .count(),
        2
    );
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_compute_exchange_duplicate_completion_identity_returns_both_capabilities() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 80_030, 25);
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::ResolveOnly,
        )
        .expect("the retained owner checks out")
        .apply();
    let (_, resolve) = take_resolve_work(checkout);
    let original = resolve.internal_failure();
    let duplicate = ComputeSettlement {
        token: SettlementToken {
            hash: original.token.hash.clone(),
            version: original.token.version,
        },
        next: SettlementNext::Retry,
    };
    let before = authority.normalized_snapshot();

    let failure = authority
        .apply_compute_exchange(
            vec![
                ComputeExchangeCompletion::new(ComputeWorkerSlot::ordered_resolve(), original),
                ComputeExchangeCompletion::new(any_verifier(0), duplicate),
            ],
            Vec::new(),
        )
        .err()
        .expect("one linear settlement identity cannot occupy two worker slots");
    let (error, recoveries) = failure.into_parts();
    assert_eq!(
        error,
        PlanError::Fault(crate::authority::plan::AuthorityFault::SchedulerProjection)
    );
    assert_eq!(
        recoveries
            .filter(|recovery| matches!(recovery, ComputeExchangeRecovery::Settlement(_)))
            .count(),
        2
    );
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_compute_exchange_without_a_fair_grant_can_settle_but_never_checkout() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 80_031, 8);
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::ResolveOnly,
        )
        .expect("the retained owner checks out")
        .apply();
    let (_, resolve) = take_resolve_work(checkout);

    let committed = authority
        .apply_compute_exchange(
            vec![ComputeExchangeCompletion::new(
                ComputeWorkerSlot::ordered_resolve(),
                resolve.internal_failure(),
            )],
            Vec::new(),
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("settlement without checkout authority plans: {error:?}");
        });
    assert_eq!(
        committed.settled,
        vec![ComputeWorkerSlot::ordered_resolve()]
    );
    assert!(committed.assignments.is_empty());
    assert!(committed.unused_grants.is_empty());
    assert_eq!(authority.resources().preaccepted().active_work, 0);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(_))
    ));
}

#[test]
fn uak_compute_exchange_rejects_an_old_completion_while_replacement_is_computing() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let raw = tx(80_032);
    let remote = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"remote-active").pack()])
        .build();
    let trusted = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"trusted-active").pack()])
        .build();
    let admission = ValidatedAdmission::remote(remote, PeerIndex::from(33usize))
        .expect("the remote witness variant is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("the remote witness enters ownership"),
    );
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::ResolveOnly,
        )
        .expect("the remote witness checks out")
        .apply();
    let (_, old_work) = take_resolve_work(checkout);
    let old_version = owner_version(&authority, &hash);

    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(trusted)
                    .expect("the trusted witness replacement is valid"),
            )
            .expect("the trusted witness replaces the active remote owner"),
    );
    assert_ne!(owner_version(&authority, &hash), old_version);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(_))
    ));

    let replacement = authority
        .apply_compute_exchange(Vec::new(), vec![grant(any_verifier(0))])
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("the replacement checks out while the old worker is still live: {error:?}");
        });
    let (_, execution, replacement_work) = replacement
        .assignments
        .into_iter()
        .next()
        .expect("the replacement owns the available second worker slot")
        .into_parts();
    drop(execution);
    assert_eq!(assignment_hash(&replacement_work), hash);
    let replacement_version = owner_version(&authority, &hash);
    assert_ne!(replacement_version, old_version);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    let before_old_completion = authority.normalized_snapshot();

    let old_slot = ComputeWorkerSlot::ordered_resolve();
    let stale = authority
        .apply_compute_exchange(
            vec![ComputeExchangeCompletion::new(
                old_slot,
                old_work.internal_failure(),
            )],
            Vec::new(),
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("the old completion is an ordinary obsolete capability: {error:?}");
        });
    assert!(stale.retirement.is_none());
    assert!(stale.settled.is_empty());
    assert_eq!(stale.obsolete, vec![old_slot]);
    assert!(stale.deferred.is_empty());
    assert!(stale.assignments.is_empty());
    assert_eq!(authority.normalized_snapshot(), before_old_completion);
    assert_eq!(owner_version(&authority, &hash), replacement_version);

    drop(
        authority
            .apply_settlement(replacement_work.cancelled())
            .expect("the exact replacement capability remains current"),
    );
}

#[test]
fn uak_effectful_completion_keeps_its_slot_and_returns_a_matching_grant_unused() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 80_041, 9);
    let slot = ComputeWorkerSlot::ordered_resolve();
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::ResolveOnly,
        )
        .expect("the retained owner checks out")
        .apply();
    let (_, resolve) = take_resolve_work(checkout);

    let committed = authority
        .apply_compute_exchange(
            vec![ComputeExchangeCompletion::new(
                slot,
                resolve.rejected(Reject::Invalidated("effectful boundary".to_owned())),
            )],
            vec![grant(slot)],
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("effectful deferral is a committed no-op: {error:?}");
        });
    assert!(committed.settled.is_empty());
    assert_eq!(committed.deferred.len(), 1);
    assert!(committed.assignments.is_empty());
    assert_eq!(committed.unused_grants.len(), 1);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
}

#[test]
fn uak_malformed_remote_completion_gives_peer_revocation_exclusive_precedence() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let peer = 10usize;
    let unrelated = admit_remote(&mut authority, 80_051, 11);
    let culprit = admit_remote(&mut authority, 80_052, peer);
    let cohort = admit_remote(&mut authority, 80_053, peer);

    let unrelated_checkout = authority
        .plan_checkout_for_foundation(
            &unrelated,
            owner_version(&authority, &unrelated),
            WorkPermit::ResolveOnly,
        )
        .expect("the older unrelated owner checks out")
        .apply();
    let (_, unrelated_work) = take_resolve_work(unrelated_checkout);
    let culprit_checkout = authority
        .plan_checkout_for_foundation(
            &culprit,
            owner_version(&authority, &culprit),
            WorkPermit::ResolveOnly,
        )
        .expect("the peer-attributed culprit checks out")
        .apply();
    let (_, culprit_work) = take_resolve_work(culprit_checkout);

    let committed = authority
        .apply_compute_exchange(
            vec![
                ComputeExchangeCompletion::new(
                    ComputeWorkerSlot::ordered_resolve(),
                    unrelated_work.internal_failure(),
                ),
                ComputeExchangeCompletion::new(
                    any_verifier(0),
                    culprit_work.rejected(Reject::Malformed(
                        "fixture".to_owned(),
                        "malformed peer payload".to_owned(),
                    )),
                ),
            ],
            vec![grant(any_verifier(1))],
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("peer revocation has exclusive precedence: {error:?}");
        });

    assert_eq!(committed.settled, vec![any_verifier(0)]);
    assert_eq!(committed.deferred.len(), 1);
    assert!(committed.assignments.is_empty());
    assert_eq!(committed.unused_grants.len(), 1);
    assert!(authority.entry(&culprit).is_none());
    assert!(authority.entry(&cohort).is_none());
    assert!(matches!(
        authority.entry(&unrelated),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    assert!(authority.peer_is_banned_for_reference(PeerIndex::from(peer)));
}

#[test]
fn uak_malformed_verify_completion_revokes_its_remote_peer_cohort() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let peer = PeerIndex::from(31usize);
    let transaction = tx(80_057);
    let (culprit, verify) =
        checkout_remote_for_verify_with_claim(&mut authority, &transaction, peer, 0);
    let cohort = admit_remote(&mut authority, 80_060, 31);

    let committed = authority
        .apply_compute_exchange(
            vec![ComputeExchangeCompletion::new(
                any_verifier(0),
                verify.rejected(Reject::Malformed(
                    "fixture".to_owned(),
                    "malformed verification payload".to_owned(),
                )),
            )],
            Vec::new(),
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("malformed verification revokes its attributed peer: {error:?}");
        });

    assert_eq!(committed.settled, vec![any_verifier(0)]);
    assert!(committed.deferred.is_empty());
    assert!(authority.entry(&culprit).is_none());
    assert!(authority.entry(&cohort).is_none());
    assert!(authority.peer_is_banned_for_reference(peer));
}

#[test]
fn uak_nonmalformed_verify_completion_never_revokes_its_remote_peer() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let peer = PeerIndex::from(32usize);
    let transaction = tx(80_062);
    let (hash, verify) =
        checkout_remote_for_verify_with_claim(&mut authority, &transaction, peer, 0);

    let committed = authority
        .apply_compute_exchange(
            vec![ComputeExchangeCompletion::new(
                any_verifier(0),
                verify.rejected(Reject::Invalidated(
                    "nonmalformed verification rejection".to_owned(),
                )),
            )],
            Vec::new(),
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("ordinary verification rejection remains exact settlement work: {error:?}");
        });

    assert!(committed.settled.is_empty());
    assert_eq!(committed.deferred.len(), 1);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    assert!(!authority.peer_is_banned_for_reference(peer));
}

#[test]
fn uak_blocked_peer_revocation_freezes_that_peer_but_preserves_unrelated_progress() {
    let mut authority =
        TxPoolAuthority::for_foundation_with_effect_limits(limits(), one_batch_effect_limits())
            .expect("the bounded fixture reserves its one effect slot");
    fill_total_effect_capacity(&mut authority);

    let peer = 21usize;
    let same_peer = admit_remote(&mut authority, 80_054, peer);
    let unrelated = admit_remote(&mut authority, 80_055, 22);
    let culprit = admit_remote(&mut authority, 80_056, peer);

    let same_peer_checkout = authority
        .plan_checkout_for_foundation(
            &same_peer,
            owner_version(&authority, &same_peer),
            WorkPermit::ResolveOnly,
        )
        .expect("the same-peer predecessor checks out")
        .apply();
    let (_, same_peer_work) = take_resolve_work(same_peer_checkout);
    let unrelated_checkout = authority
        .plan_checkout_for_foundation(
            &unrelated,
            owner_version(&authority, &unrelated),
            WorkPermit::ResolveOnly,
        )
        .expect("the unrelated owner checks out")
        .apply();
    let (_, unrelated_work) = take_resolve_work(unrelated_checkout);
    let culprit_checkout = authority
        .plan_checkout_for_foundation(
            &culprit,
            owner_version(&authority, &culprit),
            WorkPermit::ResolveOnly,
        )
        .expect("the malformed culprit checks out")
        .apply();
    let (_, culprit_work) = take_resolve_work(culprit_checkout);

    let committed = authority
        .apply_compute_exchange(
            vec![
                ComputeExchangeCompletion::new(
                    any_verifier(1),
                    culprit_work.rejected(Reject::Malformed(
                        "fixture".to_owned(),
                        "blocked malformed peer payload".to_owned(),
                    )),
                ),
                ComputeExchangeCompletion::new(
                    ComputeWorkerSlot::ordered_resolve(),
                    same_peer_work.internal_failure(),
                ),
                ComputeExchangeCompletion::new(any_verifier(0), unrelated_work.internal_failure()),
            ],
            vec![grant(any_verifier(2))],
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("effect pressure is an ordinary bounded deferral: {error:?}");
        });

    assert_eq!(committed.settled, vec![any_verifier(0)]);
    assert_eq!(committed.deferred.len(), 2);
    assert_eq!(committed.assignments.len(), 1);
    assert!(committed.unused_grants.is_empty());
    let (_, execution, work) = committed
        .assignments
        .into_iter()
        .next()
        .expect("the unrelated owner reuses the available execution slot")
        .into_parts();
    drop(execution);
    assert_eq!(assignment_hash(&work), unrelated);
    assert!(matches!(
        authority.entry(&same_peer),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    assert!(matches!(
        authority.entry(&culprit),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    assert!(!authority.peer_is_banned_for_reference(PeerIndex::from(peer)));
}

#[test]
fn uak_blocked_multi_peer_revocations_exclude_every_deferred_peer_from_refill() {
    let mut authority =
        TxPoolAuthority::for_foundation_with_effect_limits(limits(), one_batch_effect_limits())
            .expect("the bounded fixture reserves its one effect slot");
    fill_total_effect_capacity(&mut authority);

    let first_peer = 34usize;
    let second_peer = 35usize;
    let first_culprit = admit_remote(&mut authority, 80_064, first_peer);
    let second_culprit = admit_remote(&mut authority, 80_065, second_peer);
    let second_peer_cohort = admit_remote(&mut authority, 80_066, second_peer);
    let first_checkout = authority
        .plan_checkout_for_foundation(
            &first_culprit,
            owner_version(&authority, &first_culprit),
            WorkPermit::ResolveOnly,
        )
        .expect("the first malformed owner checks out")
        .apply();
    let (_, first_work) = take_resolve_work(first_checkout);
    let second_checkout = authority
        .plan_checkout_for_foundation(
            &second_culprit,
            owner_version(&authority, &second_culprit),
            WorkPermit::ResolveOnly,
        )
        .expect("the second malformed owner checks out")
        .apply();
    let (_, second_work) = take_resolve_work(second_checkout);

    let committed = authority
        .apply_compute_exchange(
            vec![
                ComputeExchangeCompletion::new(
                    ComputeWorkerSlot::ordered_resolve(),
                    first_work.rejected(Reject::Malformed(
                        "fixture".to_owned(),
                        "first blocked peer".to_owned(),
                    )),
                ),
                ComputeExchangeCompletion::new(
                    any_verifier(0),
                    second_work.rejected(Reject::Malformed(
                        "fixture".to_owned(),
                        "second blocked peer".to_owned(),
                    )),
                ),
            ],
            vec![grant(any_verifier(1))],
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("both effect-blocked peer revocations defer exactly: {error:?}");
        });

    assert!(committed.settled.is_empty());
    assert_eq!(committed.deferred.len(), 2);
    assert!(committed.assignments.is_empty());
    assert_eq!(committed.unused_grants.len(), 1);
    assert!(matches!(
        authority.entry(&second_peer_cohort),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(_))
    ));
    assert!(matches!(
        authority.entry(&first_culprit),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    assert!(matches!(
        authority.entry(&second_culprit),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    assert!(!authority.peer_is_banned_for_reference(PeerIndex::from(first_peer)));
    assert!(!authority.peer_is_banned_for_reference(PeerIndex::from(second_peer)));
}

#[test]
fn uak_effectful_completion_deferral_uses_monotonic_capability_rank() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let first = admit_remote(&mut authority, 80_058, 23);
    let second = admit_remote(&mut authority, 80_059, 24);
    let first_checkout = authority
        .plan_checkout_for_foundation(
            &first,
            owner_version(&authority, &first),
            WorkPermit::ResolveOnly,
        )
        .expect("the older capability checks out")
        .apply();
    let (_, first_work) = take_resolve_work(first_checkout);
    let second_checkout = authority
        .plan_checkout_for_foundation(
            &second,
            owner_version(&authority, &second),
            WorkPermit::ResolveOnly,
        )
        .expect("the newer capability checks out")
        .apply();
    let (_, second_work) = take_resolve_work(second_checkout);
    let older = ComputeExchangeCompletion::new(
        ComputeWorkerSlot::ordered_resolve(),
        first_work.rejected(Reject::Invalidated("older effect".to_owned())),
    );
    let newer = ComputeExchangeCompletion::new(
        any_verifier(0),
        second_work.rejected(Reject::Invalidated("newer effect".to_owned())),
    );
    let expected = [older.version(), newer.version()];
    assert!(expected[0] < expected[1]);

    let committed = authority
        .apply_compute_exchange(vec![newer, older], Vec::new())
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("effectful completion rank is deterministic: {error:?}");
        });
    assert!(committed.settled.is_empty());
    assert!(committed.assignments.is_empty());
    assert_eq!(
        committed
            .deferred
            .iter()
            .map(ComputeExchangeDeferred::version)
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn uak_exchange_rejection_returns_every_completion_and_grant_capability() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 80_061, 12);
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::ResolveOnly,
        )
        .expect("the retained owner checks out")
        .apply();
    let (_, resolve) = take_resolve_work(checkout);
    let slot = ComputeWorkerSlot::ordered_resolve();
    let before = authority.normalized_snapshot();
    let failure = authority
        .apply_compute_exchange(
            vec![ComputeExchangeCompletion::new(
                slot,
                resolve.internal_failure(),
            )],
            vec![grant(slot), grant(slot)],
        )
        .err()
        .expect("duplicate grant identity rejects the complete exchange");
    let (error, recoveries) = failure.into_parts();
    assert_eq!(
        error,
        PlanError::Fault(crate::authority::plan::AuthorityFault::SchedulerProjection)
    );
    let mut settlements = 0usize;
    let mut grants = 0usize;
    for recovery in recoveries {
        match recovery {
            ComputeExchangeRecovery::Settlement(completion) => {
                settlements += 1;
                drop(completion);
            }
            ComputeExchangeRecovery::Obsolete(slot) => {
                panic!("a current completion cannot become obsolete: {slot:?}");
            }
            ComputeExchangeRecovery::Grant(grant) => {
                grants += 1;
                drop(grant);
            }
        }
    }
    assert_eq!(settlements, 1);
    assert_eq!(grants, 2);
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_compute_exchange_refines_the_named_no_interleave_settle_refill_fold() {
    let mut aggregate = TxPoolAuthority::for_foundation(limits());
    let [aggregate_first, aggregate_second] = exchange_settlements(&mut aggregate);
    let batch_sequence = aggregate.clocks().next_sequence;
    let committed = aggregate
        .apply_compute_exchange(
            vec![
                ComputeExchangeCompletion::new(
                    ComputeWorkerSlot::ordered_resolve(),
                    aggregate_first,
                ),
                ComputeExchangeCompletion::new(any_verifier(0), aggregate_second),
            ],
            vec![
                grant(ComputeWorkerSlot::ordered_resolve()),
                grant(any_verifier(0)),
            ],
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("the aggregate settle/refill fold plans: {error:?}");
        });
    assert_eq!(committed.settled.len(), 2);
    assert_eq!(committed.assignments.len(), 2);
    for assignment in committed.assignments {
        let (_, execution, work) = assignment.into_parts();
        drop(execution);
        drop(work);
    }

    let mut reference = TxPoolAuthority::for_foundation(limits());
    let [reference_first, reference_second] = exchange_settlements(&mut reference);
    drop(
        reference
            .apply_settlement(reference_first)
            .expect("the first canonical settlement applies"),
    );
    drop(
        reference
            .apply_settlement(reference_second)
            .expect("the second canonical settlement applies"),
    );
    let (ordered, _) = reference
        .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::ResolveOnly)
        .expect("the ordered reference checkout plans");
    let ordered = ordered
        .expect("the ordered reference slot is assigned")
        .apply();
    drop(ordered);
    let (verify, _) = reference
        .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::VerifyOnly(
            VerifyCapability::Any,
        ))
        .expect("the verifier primary lane is readable");
    assert!(verify.is_none());
    let (fallback, _) = reference
        .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::ResolveThenVerify(
            VerifyCapability::Any,
        ))
        .expect("the verifier fallback plans");
    let fallback = fallback
        .expect("the verifier reference slot is assigned")
        .apply();
    drop(fallback);

    let canonical_next_sequence = ApplySequence(batch_sequence.0 + 4);
    assert_eq!(
        aggregate.clocks().next_sequence,
        ApplySequence(batch_sequence.0 + 1)
    );
    assert_eq!(reference.clocks().next_sequence, canonical_next_sequence);
    assert!(
        aggregate
            .normalized_snapshot()
            .equivalent_modulo_atomic_batch_stamp(
                &reference.normalized_snapshot(),
                batch_sequence,
                canonical_next_sequence,
            )
    );
}

#[test]
fn uak_compute_exchange_is_invariant_to_completion_and_grant_arrival_order() {
    let mut forward = TxPoolAuthority::for_foundation(limits());
    let [forward_first, forward_second] = exchange_settlements(&mut forward);
    let forward_committed = forward
        .apply_compute_exchange(
            vec![
                ComputeExchangeCompletion::new(ComputeWorkerSlot::ordered_resolve(), forward_first),
                ComputeExchangeCompletion::new(any_verifier(0), forward_second),
            ],
            vec![
                grant(ComputeWorkerSlot::ordered_resolve()),
                grant(any_verifier(0)),
            ],
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("the forward exchange plans: {error:?}");
        });

    let mut reverse = TxPoolAuthority::for_foundation(limits());
    let [reverse_first, reverse_second] = exchange_settlements(&mut reverse);
    let reverse_committed = reverse
        .apply_compute_exchange(
            vec![
                ComputeExchangeCompletion::new(any_verifier(0), reverse_second),
                ComputeExchangeCompletion::new(ComputeWorkerSlot::ordered_resolve(), reverse_first),
            ],
            vec![
                grant(any_verifier(0)),
                grant(ComputeWorkerSlot::ordered_resolve()),
            ],
        )
        .unwrap_or_else(|failure| {
            let (error, recoveries) = failure.into_parts();
            drop(recoveries);
            panic!("the reverse exchange plans: {error:?}");
        });

    let assignment_hashes = |committed: CommittedComputeExchange| {
        let mut hashes = committed
            .assignments
            .into_iter()
            .map(|assignment| {
                let (_, execution, work) = assignment.into_parts();
                drop(execution);
                assignment_hash(&work)
            })
            .collect::<Vec<_>>();
        hashes.sort_unstable();
        hashes
    };
    assert_eq!(
        assignment_hashes(forward_committed),
        assignment_hashes(reverse_committed)
    );
    assert_eq!(forward.normalized_snapshot(), reverse.normalized_snapshot());
}
