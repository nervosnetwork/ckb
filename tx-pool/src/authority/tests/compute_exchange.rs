use super::foundation::{
    admit_remote, apply_plan, limits, owner_version, resolved_payload_with_facts, take_resolve_work,
};
use crate::{
    authority::{
        exchange::{
            AuthorityComputeExecutionPermit, ComputeVerifierSlot, ComputeWorkerGrant,
            ComputeWorkerSlot,
        },
        plan::{
            ComputeExchangeCompletion, ComputePeerExclusion, PlanError,
            SharedComputeExchangeOutcome, TxPoolAuthority,
        },
        resources::{
            AcceptedResources, ComputeLimits, ResidencyPolicy, ResourceLimits, ResourceVector,
        },
        runtime::{AuthorityComputeAftermath, AuthorityFinishedCompute},
        shard::{ComputeExchangeProbePhase, ConcurrentRemovalProbe},
        state::{
            OwnedTx, PreAcceptedPhase, QueuedWork, VerifyCapability, VerifyCycleClass, WorkPermit,
        },
        work::CheckedOutWork,
    },
    error::Reject,
};
use ckb_types::core::Capacity;
use std::{num::NonZeroUsize, sync::Arc};
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

fn limits_with_one_peer_active_work() -> ResourceLimits {
    const COMPUTE_BYTES: usize = 4 * 1024;
    let compute = ComputeLimits::new(COMPUTE_BYTES, COMPUTE_BYTES, 16);
    let aggregate = ResourceVector::new(8, 64 * 1024, 64, 3)
        .with_compute_capacity(3 * COMPUTE_BYTES, 48)
        .expect("aggregate compute capacity fits");
    let per_peer = ResourceVector::new(4, 32 * 1024, 32, 1)
        .with_compute_capacity(COMPUTE_BYTES, 16)
        .expect("one peer compute capability fits");
    ResourceLimits::with_residency_policy(
        aggregate,
        aggregate,
        per_peer,
        AcceptedResources::new(8, 64 * 1024, 64 * 1024, u64::MAX),
        compute,
        ResidencyPolicy::production(
            NonZeroUsize::new(128).expect("entry metadata is non-zero"),
            NonZeroUsize::new(64).expect("edge metadata is non-zero"),
        ),
    )
    .expect("the peer-active-work fixture has a valid limit hierarchy")
}

#[test]
fn uak_compute_exchange_checks_out_one_canonical_multi_slot_wave_with_one_stamp() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let first = admit_remote(&mut authority, 80_001, 1);
    let second = admit_remote(&mut authority, 80_002, 2);
    let before = authority.clocks();

    let committed = authority
        .apply_compute_exchange(
            vec![
                grant(any_verifier(0)),
                grant(ComputeWorkerSlot::ordered_resolve()),
            ],
            &[],
        )
        .unwrap_or_else(|failure| {
            let (error, grants) = failure.into_parts();
            drop(grants);
            panic!("one available worker wave plans: {error:?}");
        });

    assert!(committed.retirement.is_some());
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
fn uak_compute_exchange_runs_verifier_primaries_before_resolve_fallbacks() {
    let mut authority = TxPoolAuthority::for_foundation(limits_with_one_peer_active_work());
    let peer = 80usize;
    let verify = admit_remote(&mut authority, 80_080, peer);
    let checkout = authority
        .checkout_for_foundation(
            &verify,
            owner_version(&authority, &verify),
            WorkPermit::ResolveOnly,
        )
        .expect("the Verify fixture resolves");
    let (_, resolve) = take_resolve_work(checkout);
    let payload = resolved_payload_with_facts(
        resolve.transaction(),
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1),
    );
    apply_plan(
        authority
            .apply_settlement(
                resolve
                    .yield_verify_as(payload, VerifyCycleClass::Large)
                    .expect("the large verification payload belongs to its owner"),
            )
            .expect("the large Verify phase commits"),
    );
    let queued_resolve = admit_remote(&mut authority, 80_081, peer);
    let small: ComputeWorkerSlot =
        ComputeVerifierSlot::new(0, VerifyCapability::SmallCycleOnly).into();
    let any = any_verifier(1);

    let committed = authority
        .apply_compute_exchange(vec![grant(small), grant(any)], &[])
        .unwrap_or_else(|failure| {
            let (error, grants) = failure.into_parts();
            drop(grants);
            panic!("the mixed verifier wave plans: {error:?}");
        });
    assert_eq!(committed.assignments.len(), 1);
    assert_eq!(committed.unused_grants.len(), 1);
    let assignment = committed
        .assignments
        .into_iter()
        .next()
        .expect("the Any verifier receives the existing Verify head");
    let (slot, execution, work) = assignment.into_parts();
    assert_eq!(slot, any);
    assert_eq!(assignment_hash(&work), verify);
    drop(execution);
    drop(work);
    assert!(matches!(
        authority.entry(&queued_resolve),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
}

#[test]
fn uak_compute_exchange_changed_owner_cut_returns_the_grant_for_stale_retry() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 80_009, 12);
    let original = owner_version(&authority, &hash);
    let slot = ComputeWorkerSlot::ordered_resolve();
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    authority
        .entries_for_reference()
        .set_compute_exchange_probe(ComputeExchangeProbePhase::AfterSchedulerWave, Some(probe));

    let (outcome, rebound) = std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            let inputs = authority
                .validate_compute_exchange_inputs(vec![grant(slot)])
                .expect("the unique grant validates");
            authority
                .prepare_shared_compute_exchange(inputs, &[])
                .expect("the shared exchange prepares")
                .apply()
        });
        entered
            .recv()
            .expect("the deterministic scheduler-wave probe is reached");
        let rebound = authority.rebind_owner_version_during_shared_plan_for_foundation(&hash);
        release
            .send(())
            .expect("the deterministic scheduler-wave probe resumes");
        (
            handle
                .join()
                .expect("the shared exchange thread does not panic"),
            rebound,
        )
    });
    authority
        .entries_for_reference()
        .set_compute_exchange_probe(ComputeExchangeProbePhase::AfterSchedulerWave, None);

    let recovered = match outcome {
        SharedComputeExchangeOutcome::RetryProbe(recovered) => recovered,
        SharedComputeExchangeOutcome::Committed { .. }
        | SharedComputeExchangeOutcome::Fault { .. } => {
            panic!("a changed selected owner cut must retry")
        }
    };
    assert_eq!(recovered.unused_grants.len(), 1);
    assert_eq!(recovered.unused_grants[0].slot(), slot);
    assert_ne!(rebound, original);
    assert_eq!(owner_version(&authority, &hash), rebound);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
}

#[test]
fn uak_compute_exchange_rejects_duplicate_grants_without_mutating_authority() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let before = authority.normalized_snapshot();
    let slot = ComputeWorkerSlot::ordered_resolve();

    let failure = authority
        .apply_compute_exchange(vec![grant(slot), grant(slot)], &[])
        .err()
        .expect("duplicate stable slot identity is rejected");
    let (error, returned) = failure.into_parts();
    assert_eq!(
        error,
        PlanError::Fault(crate::authority::plan::AuthorityFault::SchedulerProjection)
    );
    let returned = returned.collect::<Vec<_>>();
    assert_eq!(returned.len(), 2);
    assert!(returned.iter().all(|grant| grant.slot() == slot));
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_current_peer_exclusion_skips_its_peer_but_assigns_an_independent_peer() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let peer = 21usize;
    let same_peer = admit_remote(&mut authority, 80_054, peer);
    let unrelated = admit_remote(&mut authority, 80_055, 22);
    let culprit = admit_remote(&mut authority, 80_056, peer);
    let checkout = authority
        .checkout_for_foundation(
            &culprit,
            owner_version(&authority, &culprit),
            WorkPermit::ResolveOnly,
        )
        .expect("the malformed culprit checks out");
    let (_, culprit_work) = take_resolve_work(checkout);
    let completion = ComputeExchangeCompletion::from_finished(
        any_verifier(0),
        AuthorityFinishedCompute::from_parts(
            culprit_work.rejected(Reject::Malformed(
                "fixture".to_owned(),
                "blocked malformed peer payload".to_owned(),
            )),
            AuthorityComputeAftermath::completed_without_cache(),
        ),
    );
    let exclusion = ComputePeerExclusion::from_completion(&completion, peer.into());

    let committed = authority
        .apply_compute_exchange(
            vec![grant(ComputeWorkerSlot::ordered_resolve())],
            &[exclusion],
        )
        .unwrap_or_else(|failure| {
            let (error, grants) = failure.into_parts();
            drop(grants);
            panic!("the independent peer remains schedulable: {error:?}");
        });
    assert_eq!(committed.assignments.len(), 1);
    assert!(committed.unused_grants.is_empty());
    let (_, execution, work) = committed
        .assignments
        .into_iter()
        .next()
        .expect("the independent owner receives the grant")
        .into_parts();
    drop(execution);
    assert_eq!(assignment_hash(&work), unrelated);
    assert!(matches!(
        authority.entry(&same_peer),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(matches!(
        authority.entry(&culprit),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    drop(completion);
}

#[test]
fn uak_compute_exchange_plan_failure_returns_every_grant() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 80_090, 30);
    let held = authority
        .hold_positive_compute_reservation_for_foundation()
        .expect("the sibling plan holds the bounded compute reservation");
    let slot = ComputeWorkerSlot::ordered_resolve();
    let inputs = authority
        .validate_compute_exchange_inputs(vec![grant(slot)])
        .expect("the unique grant validates");

    let failure = match authority.prepare_shared_compute_exchange(inputs, &[]) {
        Err(failure) => failure,
        Ok(_) => panic!("the competing reservation must block planning"),
    };
    let (error, returned) = failure.into_parts();
    assert!(matches!(error, PlanError::ResourceContended(_)));
    let returned = returned.collect::<Vec<_>>();
    assert_eq!(returned.len(), 1);
    assert_eq!(returned[0].slot(), slot);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    held.release();
}
