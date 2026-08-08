use super::foundation::{admit_remote, limits, owner_version, take_resolve_work};
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
        state::{ApplySequence, OwnedTx, PreAcceptedPhase, VerifyCapability, WorkPermit},
        work::{CheckedOutWork, ComputeSettlement, SettlementNext, SettlementToken},
    },
    error::Reject,
};
use ckb_network::PeerIndex;
use ckb_types::packed::Byte32;
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
